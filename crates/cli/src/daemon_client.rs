use anyhow::{bail, Context, Result};
use neals_common::{call_daemon, ensure_dir, state_dir, Request, Response};
use std::fs::OpenOptions;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

pub fn with_daemon(request: Request) -> Result<Response> {
    ensure_daemon()?;
    call_daemon(&request)
}

pub fn ensure_daemon() -> Result<()> {
    if ping_ok() {
        return Ok(());
    }
    start_nealsd()?;
    for _ in 0..10 {
        thread::sleep(Duration::from_millis(50));
        if ping_ok() {
            return Ok(());
        }
    }
    bail!("failed to start nealsd (is the `nealsd` binary available?)");
}

fn ping_ok() -> bool {
    matches!(call_daemon(&Request::Ping), Ok(Response::Pong))
}

fn start_nealsd() -> Result<()> {
    let bin = find_nealsd();
    let log_path = nealsd_log_path()?;
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .with_context(|| format!("failed to open {}", log_path.display()))?;
    let log_err = log
        .try_clone()
        .context("failed to clone nealsd log handle")?;

    let mut cmd = Command::new(&bin);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(log_err));

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }

    cmd.spawn()
        .with_context(|| format!("failed to spawn `{}`", bin.display()))?;
    Ok(())
}

fn find_nealsd() -> PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        let sibling = exe.with_file_name("nealsd");
        if sibling.is_file() {
            return sibling;
        }
    }
    PathBuf::from("nealsd")
}

fn nealsd_log_path() -> Result<PathBuf> {
    let dir = state_dir()?;
    ensure_dir(&dir)?;
    Ok(dir.join("nealsd.log"))
}
