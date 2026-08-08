use crate::caddy::{ensure_neals_symlinks, project_runtime_dir};
use crate::state::{AppState, BoundRoute, BoundTarget, RunningProject};
use neals_common::{
    ensure_dir, env_service_key, read_neals_routes, state_dir, Registry, Request, Response,
    RouteKind,
};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Instant;
use tokio::process::Command;
use tokio::sync::Mutex;

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

    let routes = read_neals_routes(&project.path).map_err(|e| e.to_string())?;

    let runtime_proj = project_runtime_dir(name).map_err(|e| e.to_string())?;
    ensure_dir(&runtime_proj).map_err(|e| e.to_string())?;
    ensure_neals_symlinks(&project.path, name, &routes).map_err(|e| e.to_string())?;

    let bound = {
        let mut state = state.lock().await;
        match bind_routes(&mut state, &routes) {
            Ok(bound) => bound,
            Err(err) => return Err(err),
        }
    };

    {
        let mut state = state.lock().await;
        let mut snapshot = state.route_snapshot();
        snapshot.push((name.to_string(), bound.clone()));
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
    let mut cmd = Command::new(&program);
    cmd.args(&args)
        .current_dir(&project.path)
        .env("NEALS_RUNTIME", &runtime_proj)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log_file))
        .stderr(Stdio::from(log_err));

    for route in &bound {
        if let BoundTarget::Tcp { port } = route.target {
            let key = env_service_key(&route.service);
            cmd.env(format!("NEALS_PORT_{key}"), port.to_string());
            cmd.env(
                format!("NEALS_LISTEN_{key}"),
                format!("127.0.0.1:{port}"),
            );
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
            let snapshot = state.route_snapshot();
            let _ = state.caddy.apply_routes(&snapshot).await;
            return Err(format!("failed to spawn `{program}`: {e}"));
        }
    };
    let pid = match child.id() {
        Some(pid) => pid,
        None => {
            let mut state = state.lock().await;
            release_bound_ports(&mut state, &bound);
            let snapshot = state.route_snapshot();
            let _ = state.caddy.apply_routes(&snapshot).await;
            return Err(format!("spawned `{program}` has no pid"));
        }
    };

    let mut state = state.lock().await;
    if state.is_running(name) {
        stop_spawned(pid, &mut child).await;
        release_bound_ports(&mut state, &bound);
        let snapshot = state.route_snapshot();
        let _ = state.caddy.apply_routes(&snapshot).await;
        return Err(format!("project `{name}` is already running"));
    }
    state.projects.insert(
        name.to_string(),
        RunningProject {
            name: name.to_string(),
            child,
            pid,
            started_at: Instant::now(),
            bound,
            project_path: project.path,
        },
    );
    Ok(())
}

fn bind_routes(
    state: &mut AppState,
    routes: &[neals_common::RouteDecl],
) -> Result<Vec<BoundRoute>, String> {
    let mut bound = Vec::with_capacity(routes.len());
    for decl in routes {
        let target = match &decl.kind {
            RouteKind::Unix { socket_file } => BoundTarget::Unix {
                socket_file: socket_file.clone(),
            },
            RouteKind::Tcp => {
                let port = state
                    .leases
                    .allocate()
                    .map_err(|e| format!("TCP port alloc for `{}`: {e}", decl.service))?;
                BoundTarget::Tcp { port }
            }
        };
        bound.push(BoundRoute {
            service: decl.service.clone(),
            target,
        });
    }
    Ok(bound)
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
