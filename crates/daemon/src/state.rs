use neals_common::ProjectRuntime;
use std::collections::HashMap;
use std::time::Instant;
use tokio::process::Child;
use tokio::time::{timeout, Duration};

pub struct RunningProject {
    pub name: String,
    pub child: Child,
    pub pid: u32,
    pub started_at: Instant,
}

#[derive(Default)]
pub struct AppState {
    pub projects: HashMap<String, RunningProject>,
}

impl AppState {
    pub fn status(&self) -> Vec<ProjectRuntime> {
        self.projects
            .values()
            .map(|p| ProjectRuntime {
                name: p.name.clone(),
                pid: p.pid,
                uptime_secs: p.started_at.elapsed().as_secs(),
            })
            .collect()
    }

    pub fn is_running(&self, name: &str) -> bool {
        self.projects.contains_key(name)
    }

    pub async fn stop(&mut self, name: &str) -> Result<(), String> {
        let Some(mut running) = self.projects.remove(name) else {
            return Err(format!("project `{name}` is not running"));
        };
        stop_process_group(running.pid, &mut running.child).await;
        Ok(())
    }

    pub async fn stop_all(&mut self) {
        let names: Vec<String> = self.projects.keys().cloned().collect();
        for name in names {
            let _ = self.stop(&name).await;
        }
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
