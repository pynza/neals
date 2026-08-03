use crate::state::{AppState, RunningProject};
use neals_common::{ensure_dir, state_dir, Registry, Request, Response};
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
        .stdin(Stdio::null())
        .stdout(Stdio::from(log_file))
        .stderr(Stdio::from(log_err));

    #[cfg(unix)]
    {
        cmd.process_group(0);
    }

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("failed to spawn `{program}`: {e}"))?;
    let pid = child
        .id()
        .ok_or_else(|| format!("spawned `{program}` has no pid"))?;

    let mut state = state.lock().await;
    if state.is_running(name) {
        stop_spawned(pid, &mut child).await;
        return Err(format!("project `{name}` is already running"));
    }
    state.projects.insert(
        name.to_string(),
        RunningProject {
            name: name.to_string(),
            child,
            pid,
            started_at: Instant::now(),
        },
    );
    Ok(())
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
