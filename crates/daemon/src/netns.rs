//! Per-project network namespace (always on).
//!
//! - `bwrap --unshare-user --unshare-net` runs devenv
//! - `slirp4netns` gives the guest outbound IP only
//! - host→guest TCP: listen on host, `setns`+connect on a blocking thread, then
//!   async `copy_bidirectional` (Caddy / tools reach `127.0.0.1` binds inside)

use crate::state::BoundRoute;
use anyhow::{bail, Context, Result};
use nix::sched::{setns, CloneFlags};
use std::fs::File;
use std::net::SocketAddr;
use std::os::fd::AsFd;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::copy_bidirectional;
use tokio::net::{TcpListener, TcpStream};
use tokio::process::{Child, Command};
use tokio::task::JoinHandle;
use tokio::time::sleep;
use tokio_util::sync::CancellationToken;

pub fn require_tools() -> Result<()> {
    for bin in ["bwrap", "slirp4netns"] {
        if which(bin).is_none() {
            bail!("nealsd requires `{bin}` on PATH (install bubblewrap / slirp4netns)");
        }
    }
    if !userns_works() {
        bail!(
            "nealsd requires unprivileged user namespaces \
             (bubblewrap cannot create a user ns on this host)"
        );
    }
    Ok(())
}

fn which(bin: &str) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths).find_map(|dir| {
            let p = dir.join(bin);
            p.is_file().then_some(p)
        })
    })
}

/// Wrap `program`/`args` in a user+net namespace. Brings `lo` up inside.
pub fn bwrap_command(program: &str, args: &[String], project_dir: &Path) -> Command {
    let uid = nix::unistd::getuid();
    let gid = nix::unistd::getgid();
    let mut shell = String::from("ip link set lo up 2>/dev/null || true; exec ");
    shell.push_str(&shell_quote(program));
    for a in args {
        shell.push(' ');
        shell.push_str(&shell_quote(a));
    }

    let mut cmd = Command::new("bwrap");
    cmd.args(["--die-with-parent", "--unshare-user", "--uid"])
        .arg(uid.to_string())
        .arg("--gid")
        .arg(gid.to_string())
        .args([
            "--unshare-net",
            "--cap-add",
            "CAP_NET_ADMIN",
            "--dev-bind",
            "/",
            "/",
            "--chdir",
        ])
        .arg(project_dir)
        .args(["--", "/bin/sh", "-c"])
        .arg(shell);
    cmd
}

fn shell_quote(s: &str) -> String {
    if s.is_empty() {
        return "''".into();
    }
    if s.bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'/' | b':' | b'='))
    {
        return s.to_string();
    }
    format!("'{}'", s.replace('\'', "'\\''"))
}

pub struct SlirpHandle {
    child: Child,
}

impl SlirpHandle {
    pub async fn stop(&self) {
        if let Some(pid) = self.child.id() {
            let _ = Command::new("kill")
                .args(["-TERM", &pid.to_string()])
                .status()
                .await;
        }
    }
}

/// Outbound-only user-mode networking for the guest netns.
pub async fn start_slirp(netns_pid: u32) -> Result<SlirpHandle> {
    let mut child = Command::new("slirp4netns")
        .args([
            "--configure",
            "--mtu=65520",
            "--disable-host-loopback",
            &netns_pid.to_string(),
            "tap0",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .context("spawn slirp4netns")?;

    sleep(Duration::from_millis(200)).await;
    if let Ok(Some(status)) = child.try_wait() {
        bail!("slirp4netns exited early ({status})");
    }
    Ok(SlirpHandle { child })
}

/// Host listener → guest `127.0.0.1:guest_port` via setns on a blocking thread.
pub fn spawn_port_proxy(
    host_port: u16,
    guest_port: u16,
    netns_pid: u32,
    cancel: CancellationToken,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let listener = match TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], host_port))).await
        {
            Ok(l) => l,
            Err(err) => {
                eprintln!("nealsd: bind 127.0.0.1:{host_port}: {err}");
                return;
            }
        };
        loop {
            tokio::select! {
                _ = cancel.cancelled() => break,
                accepted = listener.accept() => {
                    let Ok((client, _)) = accepted else { break };
                    let cancel_conn = cancel.clone();
                    tokio::spawn(async move {
                        if let Err(err) = proxy_one(client, guest_port, netns_pid, cancel_conn).await {
                            eprintln!("nealsd: proxy :{host_port}→guest:{guest_port}: {err:#}");
                        }
                    });
                }
            }
        }
    })
}

async fn proxy_one(
    mut client: TcpStream,
    guest_port: u16,
    netns_pid: u32,
    cancel: CancellationToken,
) -> Result<()> {
    // ponytail: setns is thread-local and permanent — dedicated OS thread dies
    // after connect so the tokio blocking pool is never poisoned.
    let (tx, rx) = tokio::sync::oneshot::channel();
    std::thread::spawn(move || {
        let _ = tx.send(connect_in_netns(netns_pid, guest_port));
    });
    let std_stream = rx.await.context("connect thread dropped")??;

    std_stream
        .set_nonblocking(true)
        .context("set_nonblocking")?;
    let mut guest = TcpStream::from_std(std_stream).context("from_std")?;

    tokio::select! {
        _ = cancel.cancelled() => Ok(()),
        res = copy_bidirectional(&mut client, &mut guest) => {
            res.context("copy_bidirectional")?;
            Ok(())
        }
    }
}

fn connect_in_netns(netns_pid: u32, guest_port: u16) -> Result<std::net::TcpStream> {
    // Enter the project's userns first so rootless netns join is allowed.
    let userns = File::open(format!("/proc/{netns_pid}/ns/user"))
        .with_context(|| format!("open userns of pid {netns_pid}"))?;
    let netns = File::open(format!("/proc/{netns_pid}/ns/net"))
        .with_context(|| format!("open netns of pid {netns_pid}"))?;
    setns(userns.as_fd(), CloneFlags::CLONE_NEWUSER).context("setns CLONE_NEWUSER")?;
    setns(netns.as_fd(), CloneFlags::CLONE_NEWNET).context("setns CLONE_NEWNET")?;
    std::net::TcpStream::connect(SocketAddr::from(([127, 0, 0, 1], guest_port)))
        .with_context(|| format!("connect 127.0.0.1:{guest_port} in netns"))
}

pub fn start_proxies(
    bound: &[BoundRoute],
    netns_pid: u32,
    cancel: &CancellationToken,
) -> Vec<JoinHandle<()>> {
    bound
        .iter()
        .filter_map(|r| match r.target {
            crate::state::BoundTarget::Tcp {
                host_port,
                guest_port,
            } => Some(spawn_port_proxy(
                host_port,
                guest_port,
                netns_pid,
                cancel.clone(),
            )),
            _ => None,
        })
        .collect()
}

/// PID inside the sandbox netns (bwrap supervisor stays on the host netns).
pub fn netns_pid_for_bwrap(bwrap_pid: u32) -> Option<u32> {
    let path = format!("/proc/{bwrap_pid}/task/{bwrap_pid}/children");
    if let Ok(raw) = std::fs::read_to_string(&path) {
        if let Some(child) = raw.split_whitespace().next() {
            if let Ok(pid) = child.parse::<u32>() {
                return Some(pid);
            }
        }
    }
    if let Ok(tasks) = std::fs::read_dir(format!("/proc/{bwrap_pid}/task")) {
        for task in tasks.flatten() {
            let children = task.path().join("children");
            if let Ok(raw) = std::fs::read_to_string(children) {
                if let Some(child) = raw.split_whitespace().next() {
                    if let Ok(pid) = child.parse::<u32>() {
                        return Some(pid);
                    }
                }
            }
        }
    }
    None
}

pub async fn wait_netns_pid(bwrap_pid: u32) -> Result<u32> {
    for _ in 0..50 {
        if let Some(pid) = netns_pid_for_bwrap(bwrap_pid) {
            return Ok(pid);
        }
        sleep(Duration::from_millis(20)).await;
    }
    bail!("timed out waiting for process inside bwrap netns (pid {bwrap_pid})")
}

/// True if this host can create a rootless user+net namespace (needed by nealsd).
pub fn userns_works() -> bool {
    let uid = nix::unistd::getuid();
    let gid = nix::unistd::getgid();
    std::process::Command::new("bwrap")
        .args([
            "--unshare-user",
            "--uid",
            &uid.to_string(),
            "--gid",
            &gid.to_string(),
            "--unshare-net",
            "--dev-bind",
            "/",
            "/",
            "--",
            "true",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}
