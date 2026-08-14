use crate::caddy::CaddyManager;
use crate::netns::SlirpHandle;
use crate::ports::PortLeases;
use neals_common::ProjectRuntime;
use nix::errno::Errno;
use nix::sys::signal::{killpg, Signal};
use nix::unistd::Pid;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Instant;
use tokio::process::Child;
use tokio::time::{timeout, Duration};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BoundTarget {
    Unix { socket_file: String },
    // host_port = host/Caddy; guest_port = bind inside netns
    Tcp { host_port: u16, guest_port: u16 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoundRoute {
    pub service: String,
    pub target: BoundTarget,
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
    pub netns_pid: u32,
    pub started_at: Instant,
    pub bound: Vec<BoundRoute>,
    pub project_path: PathBuf,
    pub slirp: SlirpHandle,
    // Killed on drop, which frees the host ports they listen on.
    pub _proxy_helpers: Vec<Child>,
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

/// Signal the whole group led by `pid`: bwrap is its group leader (`process_group(0)` at
/// spawn), so this reaches every devenv child too.
///
/// Must not shell out to `kill`: procps swallows a leading `-pid` as an option and sends
/// nothing, which used to leave the entire project tree running after `neals down`.
fn signal_group(pid: u32, signal: Signal) {
    // A pgid of 0 means nealsd's own group and 1 means init's; never derive either from a pid.
    let pgid = match i32::try_from(pid) {
        Ok(pgid) if pgid > 1 => pgid,
        _ => {
            eprintln!("refusing to signal process group {pid}");
            return;
        }
    };
    match killpg(Pid::from_raw(pgid), signal) {
        Ok(()) | Err(Errno::ESRCH) => {}
        Err(err) => eprintln!("killpg({pgid}, {signal:?}): {err}"),
    }
}

pub(crate) async fn stop_process_group(pid: u32, child: &mut Child) {
    signal_group(pid, Signal::SIGTERM);
    if timeout(Duration::from_secs(2), child.wait()).await.is_ok() {
        return;
    }
    signal_group(pid, Signal::SIGKILL);
    // Covers the case where the group kill missed bwrap itself.
    let _ = child.start_kill();
    let _ = timeout(Duration::from_secs(2), child.wait()).await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncBufReadExt, BufReader};

    fn alive(pid: u32) -> bool {
        std::path::Path::new(&format!("/proc/{pid}")).exists()
    }

    /// Guards the bug that made `neals down` a no-op: shelling out to `kill -TERM -<pid>` let
    /// procps eat the negative pid as an option, so the project tree kept running.
    #[tokio::test]
    async fn stop_process_group_reaches_grandchildren() {
        let mut cmd = tokio::process::Command::new("sh");
        cmd.arg("-c")
            .arg("sleep 300 & echo $!; wait")
            .stdout(std::process::Stdio::piped());
        cmd.process_group(0);

        let mut child = cmd.spawn().expect("spawn sh");
        let pid = child.id().expect("sh pid");
        let mut lines = BufReader::new(child.stdout.take().expect("stdout")).lines();
        let grandchild: u32 = lines
            .next_line()
            .await
            .expect("read pid")
            .expect("pid line")
            .trim()
            .parse()
            .expect("parse pid");
        assert!(alive(grandchild), "grandchild should start alive");

        stop_process_group(pid, &mut child).await;

        for _ in 0..40 {
            if !alive(grandchild) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        signal_group(grandchild, Signal::SIGKILL);
        panic!("grandchild {grandchild} survived stop_process_group");
    }
}
