use anyhow::{bail, Context, Result};
use neals_common::{
    call_daemon, daemon_socket, ensure_dir, is_system_daemon_socket, open_log, state_dir, Request,
    Response, PROTOCOL_VERSION, SYSTEM_DAEMON_SOCKET,
};
use std::io::ErrorKind;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

pub fn with_daemon(request: Request) -> Result<Response> {
    ensure_daemon()?;
    call_daemon(&request)
}

pub fn ensure_daemon() -> Result<()> {
    if ping()? {
        return Ok(());
    }

    let sock = daemon_socket()?;
    if is_system_daemon_socket(&sock) {
        let unit = match std::env::var("USER") {
            Ok(u) if !u.is_empty() => format!("\"nealsd@{u}\""),
            _ => "\"nealsd@$USER\"".into(),
        };
        bail!(
            "system nealsd is not running (socket {SYSTEM_DAEMON_SOCKET}).\n\
             Start it with:\n\
               sudo systemctl start {unit}\n\
             Or see contrib/systemd/README.md"
        );
    }

    start_nealsd()?;
    for _ in 0..40 {
        thread::sleep(Duration::from_millis(250));
        if ping()? {
            return Ok(());
        }
    }
    bail!("failed to start nealsd (check ~/.local/state/neals/nealsd.log and caddy.log)");
}

/// `Ok(false)` means nothing is listening; a daemon speaking another protocol is an error.
pub(crate) fn ping() -> Result<bool> {
    match call_daemon(&Request::Ping) {
        Ok(Response::Pong {
            protocol: PROTOCOL_VERSION,
        }) => Ok(true),
        Ok(Response::Pong { protocol }) => bail!(
            "nealsd speaks protocol {protocol} but neals expects {PROTOCOL_VERSION}; restart it \
             (sudo systemctl restart nealsd@$USER, or `pkill nealsd` for an ad-hoc daemon)"
        ),
        Ok(other) => bail!("unexpected response to Ping: {other:?}"),
        Err(err) if is_unreachable(&err) => Ok(false),
        Err(err) => Err(err),
    }
}

fn is_unreachable(err: &anyhow::Error) -> bool {
    err.downcast_ref::<std::io::Error>()
        .is_some_and(|e| matches!(e.kind(), ErrorKind::NotFound | ErrorKind::ConnectionRefused))
}

fn start_nealsd() -> Result<()> {
    let bin = find_nealsd();
    let log_path = nealsd_log_path()?;
    let log = open_log(&log_path)?;
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

pub(crate) fn find_nealsd() -> PathBuf {
    if let Ok(env_path) = std::env::var("NEALSD_BIN") {
        let p = PathBuf::from(env_path.trim());
        if p.is_file() {
            return p;
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        let sibling = exe.with_file_name("nealsd");
        if sibling.is_file() {
            return sibling;
        }
        if let Some(parent) = exe.parent().and_then(|p| p.parent()) {
            let target_nealsd = parent.join("nealsd");
            if target_nealsd.is_file() {
                return target_nealsd;
            }
        }
    }
    PathBuf::from("nealsd")
}

fn nealsd_log_path() -> Result<PathBuf> {
    let dir = state_dir()?;
    ensure_dir(&dir)?;
    Ok(dir.join("nealsd.log"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixListener;

    fn serve_once(path: &std::path::Path, reply: String) -> thread::JoinHandle<()> {
        let _ = std::fs::remove_file(path);
        let listener = UnixListener::bind(path).unwrap();
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            BufReader::new(&stream)
                .read_line(&mut String::new())
                .unwrap();
            stream.write_all(reply.as_bytes()).unwrap();
        })
    }

    #[test]
    fn ping_tells_absent_current_and_incompatible_daemons_apart() {
        let sock = std::env::temp_dir().join(format!("neals-ping-{}.sock", std::process::id()));
        let _ = std::fs::remove_file(&sock);
        std::env::set_var("NEALS_SOCKET", &sock);

        assert!(!ping().unwrap(), "no socket");

        let pong = |protocol| format!("{{\"Pong\":{{\"protocol\":{protocol}}}}}\n");
        drop(UnixListener::bind(&sock).unwrap());
        assert!(!ping().unwrap(), "stale socket without a listener");

        let server = serve_once(&sock, pong(PROTOCOL_VERSION));
        assert!(ping().unwrap(), "matching protocol");
        server.join().unwrap();

        let server = serve_once(&sock, pong(PROTOCOL_VERSION + 1));
        let err = ping().unwrap_err().to_string();
        assert!(err.contains("protocol") && err.contains("restart"), "{err}");
        server.join().unwrap();

        let server = serve_once(&sock, "\"Pong\"\n".into());
        let err = format!("{:#}", ping().unwrap_err());
        assert!(err.contains("restart"), "pre-1.0 daemon: {err}");
        server.join().unwrap();

        std::env::remove_var("NEALS_SOCKET");
        let _ = std::fs::remove_file(&sock);
    }
}
