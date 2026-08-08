use crate::daemon_client::find_nealsd;
use anyhow::Result;
use neals_common::{
    call_daemon, config_dir, ensure_dir, runtime_dir, state_dir, Request, Response,
    SYSTEM_DAEMON_SOCKET,
};
use std::io::ErrorKind;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

struct Check {
    name: String,
    ok: bool,
    required: bool,
    detail: String,
}

pub fn run_doctor() -> Result<ExitCode> {
    let checks = vec![
        check_self(),
        check_nealsd(),
        check_on_path("nix", &["--version"], true),
        check_devenv(),
        check_on_path("caddy", &["version"], true),
        check_system_daemon(),
        check_caddy_http_bind(),
        check_dir_writable("config", config_dir()?),
        check_dir_writable("state", state_dir()?),
        check_dir_writable("runtime", runtime_dir()?),
        check_daemon_ping(),
    ];

    let mut failed = 0usize;
    for check in &checks {
        let mark = if check.ok {
            "ok"
        } else if check.required {
            failed += 1;
            "FAIL"
        } else {
            "WARN"
        };
        println!("{:<8} {:<10} {}", mark, check.name, check.detail);
    }

    if failed == 0 {
        println!("\nall required checks passed");
        Ok(ExitCode::SUCCESS)
    } else {
        println!("\n{failed} required check(s) failed");
        Ok(ExitCode::FAILURE)
    }
}

fn check_self() -> Check {
    match std::env::current_exe() {
        Ok(path) => Check {
            name: "neals".into(),
            ok: true,
            required: true,
            detail: path.display().to_string(),
        },
        Err(err) => Check {
            name: "neals".into(),
            ok: false,
            required: true,
            detail: format!("cannot resolve current exe: {err}"),
        },
    }
}

fn check_nealsd() -> Check {
    let path = find_nealsd();
    if path.is_file() || which_exists(&path) {
        Check {
            name: "nealsd".into(),
            ok: true,
            required: true,
            detail: path.display().to_string(),
        }
    } else {
        Check {
            name: "nealsd".into(),
            ok: false,
            required: true,
            detail: format!(
                "{} not found (build/install nealsd next to neals or on PATH)",
                path.display()
            ),
        }
    }
}

fn which_exists(cmd: &Path) -> bool {
    if cmd.components().count() > 1 {
        return false;
    }
    let Some(name) = cmd.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    Command::new("sh")
        .args(["-c", &format!("command -v {name} >/dev/null 2>&1")])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn check_on_path(name: &str, version_args: &[&str], required: bool) -> Check {
    match Command::new(name).args(version_args).output() {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            let text = if !stdout.trim().is_empty() {
                stdout
            } else {
                stderr
            };
            let detail = text.lines().next().unwrap_or("(no version output)").trim();
            if output.status.success() || !detail.is_empty() {
                Check {
                    name: name.into(),
                    ok: true,
                    required,
                    detail: detail.to_string(),
                }
            } else {
                Check {
                    name: name.into(),
                    ok: false,
                    required,
                    detail: format!("`{name}` exited with {}", output.status),
                }
            }
        }
        Err(err) => Check {
            name: name.into(),
            ok: false,
            required,
            detail: format!("not found on PATH ({err})"),
        },
    }
}

fn check_devenv() -> Check {
    let version = check_on_path("devenv", &["version"], true);
    if version.ok {
        return version;
    }
    check_on_path("devenv", &["--version"], true)
}

fn check_system_daemon() -> Check {
    let sock = Path::new(SYSTEM_DAEMON_SOCKET);
    if sock.exists() {
        Check {
            name: "system".into(),
            ok: true,
            required: false,
            detail: format!(
                "{SYSTEM_DAEMON_SOCKET} present (HTTP :80, URLs without port)"
            ),
        }
    } else {
        Check {
            name: "system".into(),
            ok: false,
            required: false,
            detail: "not installed — ad-hoc daemon uses :2015. For clean URLs: contrib/systemd/"
                .into(),
        }
    }
}

fn check_caddy_http_bind() -> Check {
    let addr = std::env::var("NEALS_CADDY_HTTP_ADDR")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| {
            if Path::new(SYSTEM_DAEMON_SOCKET).exists() {
                "127.0.0.1:80".into()
            } else {
                "127.0.0.1:2015".into()
            }
        });
    let Some((host, port_s)) = addr.rsplit_once(':') else {
        return Check {
            name: "http-bind".into(),
            ok: false,
            required: true,
            detail: format!("invalid NEALS_CADDY_HTTP_ADDR `{addr}` (want host:port)"),
        };
    };
    let Ok(port) = port_s.parse::<u16>() else {
        return Check {
            name: "http-bind".into(),
            ok: false,
            required: true,
            detail: format!("invalid port in `{addr}`"),
        };
    };
    match TcpListener::bind((host, port)) {
        Ok(listener) => {
            drop(listener);
            Check {
                name: "http-bind".into(),
                ok: true,
                required: true,
                detail: format!("can bind {addr}"),
            }
        }
        Err(err) if err.kind() == ErrorKind::AddrInUse => Check {
            name: "http-bind".into(),
            ok: true,
            required: true,
            detail: format!("{addr} already in use (ok if neals caddy owns it)"),
        },
        Err(err) if err.kind() == ErrorKind::PermissionDenied => Check {
            name: "http-bind".into(),
            ok: false,
            required: true,
            detail: format!(
                "cannot bind {addr}: permission denied (use an unprivileged port, \
                 e.g. NEALS_CADDY_HTTP_ADDR=127.0.0.1:2015)"
            ),
        },
        Err(err) => Check {
            name: "http-bind".into(),
            ok: false,
            required: true,
            detail: format!("cannot bind {addr}: {err}"),
        },
    }
}

fn check_dir_writable(label: &str, path: PathBuf) -> Check {
    match ensure_dir(&path).and_then(|_| probe_write(&path)) {
        Ok(()) => Check {
            name: label.into(),
            ok: true,
            required: true,
            detail: path.display().to_string(),
        },
        Err(err) => Check {
            name: label.into(),
            ok: false,
            required: true,
            detail: format!("{} ({err})", path.display()),
        },
    }
}

fn probe_write(dir: &Path) -> Result<()> {
    let probe = dir.join(".neals-doctor-write-test");
    std::fs::write(&probe, b"ok")?;
    std::fs::remove_file(&probe)?;
    Ok(())
}

fn check_daemon_ping() -> Check {
    match call_daemon(&Request::Ping) {
        Ok(Response::Pong) => Check {
            name: "daemon".into(),
            ok: true,
            required: true,
            detail: "nealsd responding on socket".into(),
        },
        Ok(other) => Check {
            name: "daemon".into(),
            ok: false,
            required: true,
            detail: format!("unexpected response: {other:?}"),
        },
        Err(_) => {
            let detail = if Path::new(SYSTEM_DAEMON_SOCKET).exists() {
                format!(
                    "not running — start with: sudo systemctl start 'nealsd@$USER'"
                )
            } else {
                "not running (will auto-start on `neals up`)".into()
            };
            Check {
                name: "daemon".into(),
                ok: true,
                required: true,
                detail,
            }
        }
    }
}
