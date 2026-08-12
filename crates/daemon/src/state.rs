use crate::caddy::CaddyManager;
use crate::netns::SlirpHandle;
use crate::ports::PortLeases;
use neals_common::ProjectRuntime;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Instant;
use tokio::process::Child;
use tokio::task::JoinHandle;
use tokio::time::{timeout, Duration};
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BoundTarget {
    Unix { socket_file: String },
    /// `host_port`: Caddy / host tools. `guest_port`: bind inside the netns.
    Tcp { host_port: u16, guest_port: u16 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoundRoute {
    pub service: String,
    pub target: BoundTarget,
    /// When true, Neals reverse-proxies via Caddy (`{service}.{project}.localhost`).
    pub proxy: bool,
}

impl BoundRoute {
    pub fn public_host(&self, project: &str) -> String {
        format!("{}.{project}.localhost", self.service)
    }

    pub fn status_label(&self, project: &str, proxy_port: u16) -> String {
        match &self.target {
            BoundTarget::Unix { .. } => {
                let host = self.public_host(project);
                if proxy_port == 80 {
                    format!("http://{host}/")
                } else {
                    format!("http://{host}:{proxy_port}/")
                }
            }
            BoundTarget::Tcp {
                host_port,
                guest_port,
            } if self.proxy => {
                let host = self.public_host(project);
                let url = if proxy_port == 80 {
                    format!("http://{host}/")
                } else {
                    format!("http://{host}:{proxy_port}/")
                };
                if host_port == guest_port {
                    format!("{url} → 127.0.0.1:{host_port}")
                } else {
                    format!("{url} → 127.0.0.1:{host_port} (guest :{guest_port})")
                }
            }
            BoundTarget::Tcp {
                host_port,
                guest_port,
            } => {
                if host_port == guest_port {
                    format!("{} → 127.0.0.1:{host_port}", self.service)
                } else {
                    format!(
                        "{} → 127.0.0.1:{host_port} (guest :{guest_port})",
                        self.service
                    )
                }
            }
        }
    }

    pub fn tcp_port(&self) -> Option<u16> {
        match self.target {
            BoundTarget::Tcp { host_port, .. } => Some(host_port),
            BoundTarget::Unix { .. } => None,
        }
    }

    pub fn guest_tcp_port(&self) -> Option<u16> {
        match self.target {
            BoundTarget::Tcp { guest_port, .. } => Some(guest_port),
            BoundTarget::Unix { .. } => None,
        }
    }
}

pub struct RunningProject {
    pub name: String,
    pub child: Child,
    pub pid: u32,
    /// Process inside the netns (for nsenter / setns).
    pub netns_pid: u32,
    pub started_at: Instant,
    pub bound: Vec<BoundRoute>,
    pub project_path: PathBuf,
    pub slirp: SlirpHandle,
    pub cancel: CancellationToken,
    pub _proxy_tasks: Vec<JoinHandle<()>>,
}

pub struct AppState {
    pub projects: HashMap<String, RunningProject>,
    pub caddy: CaddyManager,
    pub leases: PortLeases,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            projects: HashMap::new(),
            caddy: CaddyManager::disabled(),
            leases: PortLeases::default(),
        }
    }
}

impl AppState {
    pub fn status(&self) -> Vec<ProjectRuntime> {
        let proxy_port = {
            let addr = self.caddy.http_addr();
            if addr.is_empty() {
                2015
            } else {
                crate::caddy::http_port_from_addr(addr)
            }
        };
        self.projects
            .values()
            .map(|p| ProjectRuntime {
                name: p.name.clone(),
                pid: p.pid,
                netns_pid: p.netns_pid,
                uptime_secs: p.started_at.elapsed().as_secs(),
                routes: p
                    .bound
                    .iter()
                    .map(|r| r.status_label(&p.name, proxy_port))
                    .collect(),
            })
            .collect()
    }

    pub fn is_running(&self, name: &str) -> bool {
        self.projects.contains_key(name)
    }

    pub fn bound_snapshot(&self) -> Vec<(String, Vec<BoundRoute>)> {
        self.projects
            .values()
            .map(|p| (p.name.clone(), p.bound.clone()))
            .collect()
    }

    pub fn proxy_snapshot(&self) -> Vec<(String, Vec<BoundRoute>)> {
        self.projects
            .values()
            .filter_map(|p| {
                let proxied: Vec<_> = p.bound.iter().filter(|r| r.proxy).cloned().collect();
                if proxied.is_empty() {
                    None
                } else {
                    Some((p.name.clone(), proxied))
                }
            })
            .collect()
    }

    pub async fn finish_cleanup(&mut self, running: &RunningProject) {
        running.cancel.cancel();
        running.slirp.stop().await;
        for route in &running.bound {
            if let Some(port) = route.tcp_port() {
                self.leases.release(port);
            }
        }
        let _ = crate::caddy::cleanup_neals_dir(&running.project_path);
        let snapshot = self.proxy_snapshot();
        if let Err(err) = self.caddy.apply_routes(&snapshot).await {
            eprintln!("caddy apply after cleanup `{}`: {err:#}", running.name);
        }
    }

    pub async fn stop(&mut self, name: &str) -> Result<(), String> {
        let Some(mut running) = self.projects.remove(name) else {
            return Err(format!("project `{name}` is not running"));
        };
        running.cancel.cancel();
        stop_process_group(running.pid, &mut running.child).await;
        self.finish_cleanup(&running).await;
        Ok(())
    }

    pub async fn reap_exited(&mut self) {
        let mut dead = Vec::new();
        for (name, project) in self.projects.iter_mut() {
            match project.child.try_wait() {
                Ok(Some(status)) => {
                    eprintln!("project `{name}` exited unexpectedly ({status}); cleaning up");
                    dead.push(name.clone());
                }
                Ok(None) => {}
                Err(err) => {
                    eprintln!("try_wait `{name}`: {err}; cleaning up");
                    dead.push(name.clone());
                }
            }
        }
        for name in dead {
            if let Some(running) = self.projects.remove(&name) {
                self.finish_cleanup(&running).await;
            }
        }
    }

    pub async fn stop_all(&mut self) {
        let names: Vec<String> = self.projects.keys().cloned().collect();
        for name in names {
            let _ = self.stop(&name).await;
        }
        self.caddy.shutdown().await;
    }
}

async fn stop_process_group(pid: u32, child: &mut Child) {
    let _ = tokio::process::Command::new("kill")
        .args(["-TERM", &format!("-{pid}")])
        .status()
        .await;
    if timeout(Duration::from_secs(2), child.wait()).await.is_ok() {
        return;
    }
    let _ = child.start_kill();
    let _ = tokio::process::Command::new("kill")
        .args(["-KILL", &format!("-{pid}")])
        .status()
        .await;
    let _ = timeout(Duration::from_secs(2), child.wait()).await;
}
