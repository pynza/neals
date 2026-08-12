use crate::caddy::{ensure_neals_symlinks, project_runtime_dir};
use crate::netns::{self, bwrap_command, require_tools, start_proxies, wait_netns_pid};
use crate::state::{AppState, BoundRoute, BoundTarget, RunningProject};
use neals_common::{
    ensure_dir, env_port_var, read_neals_services, state_dir, Registry, Request, Response,
    ServiceDecl, ServiceKind,
};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

pub async fn handle_request(request: Request, state: &Arc<Mutex<AppState>>) -> Response {
    match request {
        Request::Ping => Response::Pong,
        Request::Status => {
            let state = state.lock().await;
            Response::Status {
                projects: state.status(),
            }
        }
        Request::Up { project } => match up_project(&project, state).await {
            Ok(()) => Response::Ok,
            Err(message) => Response::Error { message },
        },
        Request::Down { project } => {
            let mut state = state.lock().await;
            match state.stop(&project).await {
                Ok(()) => Response::Ok,
                Err(message) => Response::Error { message },
            }
        }
    }
}

async fn up_project(name: &str, state: &Arc<Mutex<AppState>>) -> Result<(), String> {
    {
        let state = state.lock().await;
        if state.is_running(name) {
            return Err(format!("project `{name}` is already running"));
        }
    }

    let project = {
        let registry = Registry::load().map_err(|e| e.to_string())?;
        let project = registry
            .get(name)
            .ok_or_else(|| format!("project `{name}` is not registered"))?
            .clone();
        if !project.path.is_dir() {
            return Err(format!(
                "project `{name}` path is missing: {}",
                project.path.display()
            ));
        }
        project
    };

    require_tools().map_err(|e| e.to_string())?;

    let services = read_neals_services(&project.path).map_err(|e| e.to_string())?;

    let runtime_proj = project_runtime_dir(name).map_err(|e| e.to_string())?;
    ensure_dir(&runtime_proj).map_err(|e| e.to_string())?;
    ensure_neals_symlinks(&project.path, name, &services).map_err(|e| e.to_string())?;

    let bound = {
        let mut state = state.lock().await;
        match bind_services(&mut state, &services) {
            Ok(bound) => bound,
            Err(err) => return Err(err),
        }
    };

    {
        let mut state = state.lock().await;
        let mut snapshot = state.proxy_snapshot();
        let proxied: Vec<_> = bound.iter().filter(|r| r.proxy).cloned().collect();
        if !proxied.is_empty() {
            snapshot.push((name.to_string(), proxied));
        }
        if let Err(e) = state.caddy.apply_routes(&snapshot).await {
            release_bound_ports(&mut state, &bound);
            return Err(e.to_string());
        }
    }

    let log_path = project_log_path(name).map_err(|e| e.to_string())?;
    let log_file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .map_err(|e| format!("failed to open log {}: {e}", log_path.display()))?;
    let log_err = log_file
        .try_clone()
        .map_err(|e| format!("failed to clone log handle: {e}"))?;

    let (program, args) = up_command();
    let mut cmd = bwrap_command(&program, &args, &project.path);
    cmd.env("NEALS_RUNTIME", &runtime_proj)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log_file))
        .stderr(Stdio::from(log_err));

    for route in &bound {
        if let Some(guest) = route.guest_tcp_port() {
            cmd.env(env_port_var(&route.service), guest.to_string());
        }
    }

    #[cfg(unix)]
    {
        cmd.process_group(0);
    }

    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) => {
            let mut state = state.lock().await;
            release_bound_ports(&mut state, &bound);
            let snapshot = state.proxy_snapshot();
            let _ = state.caddy.apply_routes(&snapshot).await;
            return Err(format!("failed to spawn `{program}`: {e}"));
        }
    };
    let pid = match child.id() {
        Some(pid) => pid,
        None => {
            let mut state = state.lock().await;
            release_bound_ports(&mut state, &bound);
            let snapshot = state.proxy_snapshot();
            let _ = state.caddy.apply_routes(&snapshot).await;
            return Err(format!("spawned `{program}` has no pid"));
        }
    };

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    if let Ok(Some(status)) = child.try_wait() {
        let mut state = state.lock().await;
        release_bound_ports(&mut state, &bound);
        let snapshot = state.proxy_snapshot();
        let _ = state.caddy.apply_routes(&snapshot).await;
        return Err(format!(
            "bwrap exited early ({status}); need unprivileged user namespaces \
             (bubblewrap + kernel.userns)"
        ));
    }
    let netns_pid = match wait_netns_pid(pid).await {
        Ok(p) => p,
        Err(e) => {
            stop_spawned(pid, &mut child).await;
            let mut state = state.lock().await;
            release_bound_ports(&mut state, &bound);
            let snapshot = state.proxy_snapshot();
            let _ = state.caddy.apply_routes(&snapshot).await;
            return Err(e.to_string());
        }
    };

    let slirp = match netns::start_slirp(netns_pid).await {
        Ok(handle) => handle,
        Err(e) => {
            stop_spawned(pid, &mut child).await;
            let mut state = state.lock().await;
            release_bound_ports(&mut state, &bound);
            let snapshot = state.proxy_snapshot();
            let _ = state.caddy.apply_routes(&snapshot).await;
            return Err(format!("slirp4netns: {e:#}"));
        }
    };

    let cancel = CancellationToken::new();
    let proxy_tasks = start_proxies(&bound, netns_pid, &cancel);

    let mut state = state.lock().await;
    if state.is_running(name) {
        cancel.cancel();
        slirp.stop().await;
        stop_spawned(pid, &mut child).await;
        release_bound_ports(&mut state, &bound);
        let snapshot = state.proxy_snapshot();
        let _ = state.caddy.apply_routes(&snapshot).await;
        return Err(format!("project `{name}` is already running"));
    }
    state.projects.insert(
        name.to_string(),
        RunningProject {
            name: name.to_string(),
            child,
            pid,
            netns_pid,
            started_at: Instant::now(),
            bound,
            project_path: project.path,
            slirp,
            cancel,
            _proxy_tasks: proxy_tasks,
        },
    );
    Ok(())
}

fn bind_services(
    state: &mut AppState,
    services: &[ServiceDecl],
) -> Result<Vec<BoundRoute>, String> {
    let mut bound = Vec::with_capacity(services.len());
    for decl in services {
        match bind_one(state, decl) {
            Ok(route) => bound.push(route),
            Err(err) => {
                release_bound_ports(state, &bound);
                return Err(err);
            }
        }
    }
    Ok(bound)
}

fn bind_one(state: &mut AppState, decl: &ServiceDecl) -> Result<BoundRoute, String> {
    match &decl.kind {
        ServiceKind::Unix { socket_file } => Ok(BoundRoute {
            service: decl.service.clone(),
            target: BoundTarget::Unix {
                socket_file: socket_file.clone(),
            },
            proxy: true,
        }),
        ServiceKind::Tcp {
            preferred_port,
            proxy,
        } => {
            let host_port = match preferred_port {
                Some(start) => state.leases.allocate_preferred(*start),
                None => state.leases.allocate(),
            }
            .map_err(|e| format!("TCP port alloc for `{}`: {e}", decl.service))?;

            // Preferred stays fixed inside the ns; host may differ if busy.
            let guest_port = preferred_port.unwrap_or(host_port);

            Ok(BoundRoute {
                service: decl.service.clone(),
                target: BoundTarget::Tcp {
                    host_port,
                    guest_port,
                },
                proxy: *proxy,
            })
        }
    }
}

fn release_bound_ports(state: &mut AppState, bound: &[BoundRoute]) {
    for route in bound {
        if let Some(port) = route.tcp_port() {
            state.leases.release(port);
        }
    }
}

fn project_log_path(name: &str) -> anyhow::Result<PathBuf> {
    let dir = state_dir()?;
    ensure_dir(&dir)?;
    Ok(dir.join(format!("{name}.log")))
}

fn up_command() -> (String, Vec<String>) {
    match std::env::var("NEALS_UP_CMD") {
        Ok(raw) if !raw.trim().is_empty() => {
            let mut parts = raw.split_whitespace().map(str::to_string);
            let program = parts.next().unwrap_or_else(|| "devenv".into());
            (program, parts.collect())
        }
        _ => ("devenv".into(), vec!["up".into()]),
    }
}

async fn stop_spawned(pid: u32, child: &mut tokio::process::Child) {
    let _ = tokio::process::Command::new("kill")
        .args(["-TERM", &format!("-{pid}")])
        .status()
        .await;
    let _ = child.wait().await;
}
