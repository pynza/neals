use crate::daemon_client::find_nealsd;
use crate::style;
use anyhow::Result;
use neals_common::{
    call_daemon, config_dir, ensure_dir, runtime_dir, state_dir, Request, Response,
    SYSTEM_DAEMON_SOCKET,
};
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::time::Duration;

struct Check {
    name: &'static str,
    ok: bool,
    required: bool,
    detail: String,
}

impl Check {
    fn ok(name: &'static str, detail: impl Into<String>) -> Self {
        Self {
            name,
            ok: true,
            required: true,
            detail: detail.into(),
        }
    }

    fn fail(name: &'static str, detail: impl Into<String>) -> Self {
        Self {
            name,
            ok: false,
            required: true,
            detail: detail.into(),
        }
    }

    fn warn(name: &'static str, detail: impl Into<String>) -> Self {
        Self {
            name,
            ok: false,
            required: false,
            detail: detail.into(),
        }
    }

    fn info(name: &'static str, detail: impl Into<String>) -> Self {
        Self {
            name,
            ok: true,
            required: false,
            detail: detail.into(),
        }
    }
}

pub fn run_doctor() -> Result<ExitCode> {
    let checks = vec![
        check_self(),
        check_nealsd(),
        check_on_path("nix", &["--version"], true),
        check_devenv(),
        check_on_path("caddy", &["version"], true),
        check_on_path("bwrap", &["--version"], true),
        check_on_path("slirp4netns", &["--version"], true),
        check_on_path("nsenter", &["--version"], true),
        check_system_daemon(),
        check_http(),
        check_dir_writable("config", config_dir()?),
        check_dir_writable("state", state_dir()?),
        check_dir_writable("runtime", runtime_dir()?),
        check_daemon_ping(),
    ];

    let mut failed = 0usize;
    for check in &checks {
        if !check.ok && check.required {
            failed += 1;
        }
        let mark = style::format_mark(check.ok, check.required);
        println!("{mark:<18} {:<10} {}", check.name, check.detail);
    }

    if failed == 0 {
        style::print_ok("all required checks passed");
        Ok(ExitCode::SUCCESS)
    } else {
        style::print_err(&format!("{failed} required check(s) failed"));
        Ok(ExitCode::FAILURE)
    }
}

fn check_self() -> Check {
    match std::env::current_exe() {
        Ok(path) => Check::ok("neals", path.display().to_string()),
        Err(err) => Check::fail("neals", format!("cannot resolve current exe: {err}")),
    }
}

fn check_nealsd() -> Check {
    let path = find_nealsd();
    if path.is_file() || which_exists(&path) {
        Check::ok("nealsd", path.display().to_string())
    } else {
        Check::fail(
            "nealsd",
            format!("{} not found (next to neals or on PATH)", path.display()),
        )
    }
}

fn which_exists(cmd: &Path) -> bool {
    let Some(name) = cmd.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    if cmd.components().count() > 1 {
        return false;
    }
    Command::new("sh")
        .args(["-c", &format!("command -v {name} >/dev/null 2>&1")])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn check_on_path(name: &'static str, version_args: &[&str], required: bool) -> Check {
    match Command::new(name).args(version_args).output() {
        Ok(output) => {
            let text = {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let stderr = String::from_utf8_lossy(&output.stderr);
                if stdout.trim().is_empty() {
                    stderr
                } else {
                    stdout
                }
            };
            let detail = text.lines().next().unwrap_or("(no version)").trim();
            if output.status.success() || !detail.is_empty() {
                let mut c = Check::ok(name, detail.to_string());
                c.required = required;
                c
            } else {
                let mut c = Check::fail(name, format!("exited {status}", status = output.status));
                c.required = required;
                c
            }
        }
        Err(err) => {
            let mut c = Check::fail(name, format!("not on PATH ({err})"));
            c.required = required;
            c
        }
    }
}

fn check_devenv() -> Check {
    let v = check_on_path("devenv", &["version"], true);
    if v.ok {
        v
    } else {
        check_on_path("devenv", &["--version"], true)
    }
}

fn check_system_daemon() -> Check {
    if Path::new(SYSTEM_DAEMON_SOCKET).exists() {
        Check::info("system", format!("{SYSTEM_DAEMON_SOCKET} (portless :80)"))
    } else {
        Check::warn("system", "ad-hoc mode (:2015); see contrib/systemd/")
    }
}

fn check_http() -> Check {
    if caddy_on("127.0.0.1", 80) == Some(true) {
        return Check::ok("http", "portless");
    }
    if caddy_on("127.0.0.1", 2015) == Some(true) {
        return Check::warn("http", "requires port in URLs");
    }
    Check::warn("http", "Caddy not listening")
}

// None = closed; Some(true) = Caddy; Some(false) = other.
fn caddy_on(host: &str, port: u16) -> Option<bool> {
    let sa = (host, port).to_socket_addrs().ok()?.next()?;
    let mut stream = TcpStream::connect_timeout(&sa, Duration::from_millis(300)).ok()?;
    let _ = stream.set_read_timeout(Some(Duration::from_millis(300)));
    let _ = stream.set_write_timeout(Some(Duration::from_millis(300)));
    stream
        .write_all(format!("GET / HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n").as_bytes())
        .ok()?;
    let mut buf = [0u8; 1024];
    let n = stream.read(&mut buf).unwrap_or(0);
    if n == 0 {
        return Some(false);
    }
    let head = String::from_utf8_lossy(&buf[..n]).to_ascii_lowercase();
    Some(
        head.lines()
            .any(|l| l.starts_with("server:") && l.contains("caddy")),
    )
}

fn check_dir_writable(label: &'static str, path: PathBuf) -> Check {
    match ensure_dir(&path).and_then(|_| {
        let probe = path.join(".neals-doctor-write-test");
        std::fs::write(&probe, b"ok")?;
        std::fs::remove_file(&probe)?;
        Ok(())
    }) {
        Ok(()) => Check::ok(label, path.display().to_string()),
        Err(err) => Check::fail(label, format!("{} ({err})", path.display())),
    }
}

fn check_daemon_ping() -> Check {
    match call_daemon(&Request::Ping) {
        Ok(Response::Pong) => Check::ok("daemon", "responding"),
        Ok(other) => Check::fail("daemon", format!("unexpected: {other:?}")),
        Err(_) if Path::new(SYSTEM_DAEMON_SOCKET).exists() => {
            Check::ok("daemon", "not running — sudo systemctl start 'nealsd@$USER'")
        }
        Err(_) => Check::ok("daemon", "not running (auto-starts on `neals up`)"),
    }
}
