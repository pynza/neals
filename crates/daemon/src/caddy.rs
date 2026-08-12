use anyhow::{bail, Context, Result};
use crate::state::{BoundRoute, BoundTarget};
use neals_common::{ensure_dir, runtime_dir, state_dir, ServiceDecl, ServiceKind};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use tokio::process::{Child, Command};
use tokio::time::{sleep, Duration};

pub struct CaddyManager {
    child: Option<Child>,
    admin_sock: PathBuf,
    config_path: PathBuf,
    http_addr: String,
    // When true, Admin API failures are ignored (`NEALS_CADDY_CMD` fakes).
    loose: bool,
}

impl Default for CaddyManager {
    fn default() -> Self {
        Self::disabled()
    }
}

impl CaddyManager {
    pub fn disabled() -> Self {
        Self {
            child: None,
            admin_sock: PathBuf::new(),
            config_path: PathBuf::new(),
            http_addr: String::new(),
            loose: true,
        }
    }

    pub async fn start() -> Result<Self> {
        if caddy_disabled() {
            return Ok(Self::disabled());
        }

        let runtime = runtime_dir()?;
        ensure_dir(&runtime)?;
        let state = state_dir()?;
        ensure_dir(&state)?;

        let admin_sock = runtime.join("caddy-admin.sock");
        let config_path = state.join("caddy.json");
        let log_path = state.join("caddy.log");
        let http_addr = http_listen_addr();
        let loose = std::env::var_os("NEALS_CADDY_CMD").is_some();

        if admin_sock.exists() {
            let _ = tokio::fs::remove_file(&admin_sock).await;
        }

        let initial = build_caddy_config(&admin_sock, &http_addr, &log_path, &[]);
        write_config(&config_path, &initial)?;

        let (program, mut args) = caddy_command();
        if args.is_empty() && program == "caddy" {
            args = vec![
                "run".into(),
                "--config".into(),
                config_path.display().to_string(),
            ];
        }

        let log_file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .with_context(|| format!("failed to open {}", log_path.display()))?;
        let log_err = log_file.try_clone()?;

        let mut cmd = Command::new(&program);
        cmd.args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::from(log_file))
            .stderr(Stdio::from(log_err));

        #[cfg(unix)]
        {
            cmd.process_group(0);
        }

        let child = cmd
            .spawn()
            .with_context(|| format!("failed to spawn `{program}`"))?;

        let manager = Self {
            child: Some(child),
            admin_sock,
            config_path,
            http_addr,
            loose,
        };

        if !manager.loose {
            manager.wait_admin_ready().await?;
        }

        Ok(manager)
    }

    async fn wait_admin_ready(&self) -> Result<()> {
        for _ in 0..50 {
            if self.admin_sock.exists() {
                if UnixStream::connect(&self.admin_sock).await.is_ok() {
                    return Ok(());
                }
            }
            sleep(Duration::from_millis(100)).await;
        }
        bail!(
            "caddy admin socket not ready: {} (listen {})",
            self.admin_sock.display(),
            self.http_addr
        )
    }

    pub fn http_addr(&self) -> &str {
        &self.http_addr
    }

    pub async fn apply_routes(&mut self, projects: &[(String, Vec<BoundRoute>)]) -> Result<()> {
        if self.loose && self.admin_sock.as_os_str().is_empty() {
            return Ok(());
        }

        let log_path = state_dir()?.join("caddy.log");
        let config = build_caddy_config(&self.admin_sock, &self.http_addr, &log_path, projects);
        write_config(&self.config_path, &config)?;

        let body = serde_json::to_string(&config)?;
        match post_load(&self.admin_sock, &body).await {
            Ok(()) => Ok(()),
            Err(err) if self.loose => {
                eprintln!("caddy apply skipped: {err:#}");
                Ok(())
            }
            Err(err) => Err(err),
        }
    }

    pub async fn shutdown(&mut self) {
        if let Some(mut child) = self.child.take() {
            if let Some(pid) = child.id() {
                let _ = Command::new("kill")
                    .args(["-TERM", &format!("-{pid}")])
                    .status()
                    .await;
            }
            let _ = child.wait().await;
        }
        if !self.admin_sock.as_os_str().is_empty() {
            let _ = tokio::fs::remove_file(&self.admin_sock).await;
        }
    }
}

pub fn build_caddy_config(
    admin_sock: &Path,
    http_addr: &str,
    log_path: &Path,
    projects: &[(String, Vec<BoundRoute>)],
) -> Value {
    let runtime = runtime_dir().unwrap_or_else(|_| PathBuf::from("/tmp/neals"));
    let mut routes = Vec::new();

    for (project, decls) in projects {
        for route in decls {
            if !route.proxy {
                continue;
            }
            let host = route.public_host(project);
            let dial = match &route.target {
                BoundTarget::Unix { socket_file } => {
                    let sock = runtime.join(project).join(socket_file);
                    format!("unix/{}", sock.display())
                }
                BoundTarget::Tcp { host_port, .. } => format!("127.0.0.1:{host_port}"),
            };
            routes.push(json!({
                "match": [{ "host": [host] }],
                "handle": [{
                    "handler": "reverse_proxy",
                    "upstreams": [{ "dial": dial }]
                }]
            }));
        }
    }

    json!({
        "admin": {
            "listen": format!("unix/{}", admin_sock.display())
        },
        "logging": {
            "logs": {
                "default": {
                    "writer": {
                        "output": "file",
                        "filename": log_path.display().to_string()
                    }
                }
            }
        },
        "apps": {
            "http": {
                "http_port": http_port_from_addr(http_addr),
                "https_port": https_port_from_addr(http_addr),
                "servers": {
                    "neals": {
                        "listen": [http_addr],
                        "automatic_https": { "disable": true },
                        "routes": routes
                    }
                }
            }
        }
    })
}

fn write_config(path: &Path, config: &Value) -> Result<()> {
    let text = serde_json::to_string_pretty(config)?;
    std::fs::write(path, text).with_context(|| format!("failed to write {}", path.display()))?;
    Ok(())
}

async fn post_load(admin_sock: &Path, body: &str) -> Result<()> {
    let mut stream = UnixStream::connect(admin_sock)
        .await
        .with_context(|| format!("connect {}", admin_sock.display()))?;

    let request = format!(
        "POST /load HTTP/1.1\r\n\
         Host: localhost\r\n\
         Content-Type: application/json\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\
         \r\n\
         {body}",
        body.len()
    );
    stream.write_all(request.as_bytes()).await?;

    let mut response = Vec::new();
    stream.read_to_end(&mut response).await?;
    let text = String::from_utf8_lossy(&response);
    let status_line = text.lines().next().unwrap_or("");
    if !status_line.contains("200") {
        let body = text.split("\r\n\r\n").nth(1).unwrap_or("").trim();
        bail!("caddy /load failed: {status_line} {body}");
    }
    Ok(())
}

fn caddy_disabled() -> bool {
    matches!(
        std::env::var("NEALS_CADDY_CMD").as_deref(),
        Ok("-") | Ok("")
    )
}

fn caddy_command() -> (String, Vec<String>) {
    match std::env::var("NEALS_CADDY_CMD") {
        Ok(raw) if !raw.trim().is_empty() && raw.trim() != "-" => {
            let mut parts = raw.split_whitespace().map(str::to_string);
            let program = parts.next().unwrap_or_else(|| "caddy".into());
            (program, parts.collect())
        }
        _ => ("caddy".into(), Vec::new()),
    }
}

fn http_listen_addr() -> String {
    if let Ok(addr) = std::env::var("NEALS_CADDY_HTTP_ADDR") {
        if !addr.trim().is_empty() {
            return addr;
        }
    }
    if is_system_mode() {
        "127.0.0.1:80".into()
    } else {
        "127.0.0.1:2015".into()
    }
}

fn is_system_mode() -> bool {
    matches!(
        std::env::var("NEALS_MODE").as_deref().map(str::trim),
        Ok("system")
    )
}

pub fn http_port_from_addr(addr: &str) -> u16 {
    addr.rsplit_once(':')
        .and_then(|(_, p)| p.parse().ok())
        .unwrap_or(2015)
}

fn https_port_from_addr(addr: &str) -> u16 {
    // Avoid :443 while automatic_https is off (EACCES without CAP).
    let p = http_port_from_addr(addr);
    if p < 1024 {
        2016
    } else {
        p.saturating_add(1)
    }
}

pub fn project_runtime_dir(project: &str) -> Result<PathBuf> {
    Ok(runtime_dir()?.join(project))
}

pub fn ensure_neals_symlinks(
    project_dir: &Path,
    project: &str,
    services: &[ServiceDecl],
) -> Result<()> {
    let runtime_proj = project_runtime_dir(project)?;
    ensure_dir(&runtime_proj)?;

    let unix_sockets: Vec<_> = services
        .iter()
        .filter_map(|s| match &s.kind {
            ServiceKind::Unix { socket_file } => Some(socket_file.as_str()),
            ServiceKind::Tcp { .. } => None,
        })
        .collect();

    if unix_sockets.is_empty() {
        return Ok(());
    }

    let neals_dir = project_dir.join(".neals");
    ensure_dir(&neals_dir)?;

    for socket_file in unix_sockets {
        let target = runtime_proj.join(socket_file);
        let link = neals_dir.join(socket_file);
        if link.exists() || link.symlink_metadata().is_ok() {
            std::fs::remove_file(&link)
                .with_context(|| format!("failed to remove {}", link.display()))?;
        }
        std::os::unix::fs::symlink(&target, &link).with_context(|| {
            format!(
                "failed to symlink {} -> {}",
                link.display(),
                target.display()
            )
        })?;
    }
    Ok(())
}

pub fn cleanup_neals_dir(project_dir: &Path) -> Result<()> {
    let neals_dir = project_dir.join(".neals");
    if neals_dir.is_dir() {
        std::fs::remove_dir_all(&neals_dir)
            .with_context(|| format!("failed to remove {}", neals_dir.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_config_contains_unix_and_tcp_upstreams() {
        let admin = PathBuf::from("/tmp/neals/caddy-admin.sock");
        let log = PathBuf::from("/tmp/neals/caddy.log");
        let projects = [(
            "demo".into(),
            vec![
                BoundRoute {
                    service: "backend".into(),
                    target: BoundTarget::Unix {
                        socket_file: "backend.sock".into(),
                    },
                    proxy: true,
                },
                BoundRoute {
                    service: "api".into(),
                    target: BoundTarget::Tcp {
                        host_port: 38471,
                        guest_port: 38471,
                    },
                    proxy: true,
                },
            ],
        )];
        let cfg = build_caddy_config(&admin, "127.0.0.1:2015", &log, &projects);
        let text = cfg.to_string();
        assert!(text.contains("backend.demo.localhost"));
        assert!(text.contains("unix/"));
        assert!(text.contains("backend.sock"));
        assert!(text.contains("api.demo.localhost"));
        assert!(text.contains("127.0.0.1:38471"));
        assert!(text.contains("127.0.0.1:2015"));
        assert!(text.contains("automatic_https"));
    }

    #[test]
    fn build_config_keeps_multi_project_hosts_and_ports_distinct() {
        let admin = PathBuf::from("/tmp/neals/caddy-admin.sock");
        let log = PathBuf::from("/tmp/neals/caddy.log");
        let projects = [
            (
                "demo".into(),
                vec![
                    BoundRoute {
                        service: "be".into(),
                        target: BoundTarget::Tcp {
                            host_port: 11111,
                            guest_port: 11111,
                        },
                        proxy: true,
                    },
                    BoundRoute {
                        service: "admin".into(),
                        target: BoundTarget::Tcp {
                            host_port: 11112,
                            guest_port: 11112,
                        },
                        proxy: true,
                    },
                ],
            ),
            (
                "demo2".into(),
                vec![BoundRoute {
                    service: "be".into(),
                    target: BoundTarget::Tcp {
                        host_port: 22222,
                        guest_port: 22222,
                    },
                    proxy: true,
                }],
            ),
        ];
        let cfg = build_caddy_config(&admin, "127.0.0.1:2015", &log, &projects);
        let text = cfg.to_string();
        assert!(text.contains("be.demo.localhost"));
        assert!(text.contains("admin.demo.localhost"));
        assert!(text.contains("be.demo2.localhost"));
        assert!(text.contains("127.0.0.1:11111"));
        assert!(text.contains("127.0.0.1:11112"));
        assert!(text.contains("127.0.0.1:22222"));
        assert!(text.contains(r#""be.demo.localhost""#));
        assert!(text.contains(r#""be.demo2.localhost""#));
    }

    #[test]
    fn build_config_skips_private_tcp_services() {
        let admin = PathBuf::from("/tmp/neals/caddy-admin.sock");
        let log = PathBuf::from("/tmp/neals/caddy.log");
        let projects = [(
            "demo".into(),
            vec![
                BoundRoute {
                    service: "redis".into(),
                    target: BoundTarget::Tcp {
                        host_port: 6379,
                        guest_port: 6379,
                    },
                    proxy: false,
                },
                BoundRoute {
                    service: "api".into(),
                    target: BoundTarget::Tcp {
                        host_port: 8000,
                        guest_port: 8000,
                    },
                    proxy: true,
                },
            ],
        )];
        let cfg = build_caddy_config(&admin, "127.0.0.1:2015", &log, &projects);
        let text = cfg.to_string();
        assert!(!text.contains("redis.demo.localhost"));
        assert!(text.contains("api.demo.localhost"));
        assert!(text.contains("127.0.0.1:8000"));
        assert!(!text.contains("127.0.0.1:6379"));
    }

    #[test]
    fn http_port_follows_listen_addr() {
        assert_eq!(http_port_from_addr("127.0.0.1:80"), 80);
        assert_eq!(http_port_from_addr("127.0.0.1:2015"), 2015);
        assert_eq!(https_port_from_addr("127.0.0.1:80"), 2016);
        assert_eq!(https_port_from_addr("127.0.0.1:2015"), 2016);
    }
}
