//! Per-project netns: bwrap + outbound slirp + host→guest TCP via setns.

use crate::state::BoundRoute;
use anyhow::{bail, Context, Result};
use nix::libc;
use nix::sched::{setns, CloneFlags};
use std::fs::File;
use std::net::SocketAddr;
use std::os::fd::AsFd;
use std::os::unix::process::CommandExt as _;
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
            "nealsd cannot create a user namespace (bwrap failed); \
             check kernel.userns and that the unit is not blocking it"
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

// bwrap user+net ns; brings `lo` up inside.
// `runtime_neals` is host `$XDG_RUNTIME_DIR/neals` (or `/run/neals`); remounted after a
// writable tmpfs /run so devenv can create `/run/devenv-*`.
pub fn bwrap_command(
    program: &str,
    args: &[String],
    project_dir: &Path,
    runtime_neals: &Path,
) -> Command {
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
        ]);
    mount_writable_run(&mut cmd, runtime_neals);
    cmd.arg("--chdir")
        .arg(project_dir)
        .args(["--", "/bin/sh", "-c"])
        .arg(shell);
    unsafe {
        cmd.pre_exec(clear_caps_for_userns);
    }
    cmd
}

/// Host `/run` is root-owned; devenv needs to mkdir `/run/devenv-*`.
fn mount_writable_run(cmd: &mut Command, runtime_neals: &Path) {
    cmd.arg("--tmpfs").arg("/run");
    let Ok(rel) = runtime_neals.strip_prefix("/run") else {
        return;
    };
    let mut acc = PathBuf::from("/run");
    for c in rel.components() {
        acc.push(c);
        if acc.as_path() != runtime_neals {
            cmd.arg("--dir").arg(&acc);
        }
    }
    cmd.arg("--bind").arg(runtime_neals).arg(runtime_neals);
}

fn clear_caps_for_userns() -> std::io::Result<()> {
    unsafe {
        let _ = libc::prctl(
            libc::PR_CAP_AMBIENT,
            libc::PR_CAP_AMBIENT_CLEAR_ALL as libc::c_ulong,
            0,
            0,
            0,
        );
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct CapHeader {
        version: u32,
        pid: i32,
    }
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct CapData {
        effective: u32,
        permitted: u32,
        inheritable: u32,
    }
    // LINUX_CAPABILITY_VERSION_3
    let mut hdr = CapHeader {
        version: 0x2008_0522,
        pid: 0,
    };
    let data = [CapData {
        effective: 0,
        permitted: 0,
        inheritable: 0,
    }; 2];
    let rc = unsafe {
        libc::syscall(
            libc::SYS_capset,
            std::ptr::addr_of_mut!(hdr),
            data.as_ptr(),
        )
    };
    if rc != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
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
    // userns then netns — required for rootless setns.
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

// Inner pid (bwrap itself stays on the host netns).
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

pub fn userns_works() -> bool {
    let uid = nix::unistd::getuid();
    let gid = nix::unistd::getgid();
    let mut cmd = std::process::Command::new("bwrap");
    cmd.args([
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
    .stderr(Stdio::null());
    unsafe {
        cmd.pre_exec(clear_caps_for_userns);
    }
    cmd.status().map(|s| s.success()).unwrap_or(false)
}
