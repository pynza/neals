mod caddy;
mod handler;
pub mod hosts;
pub mod netns;
mod ports;
mod state;

pub use handler::handle_request;
pub use state::AppState;

use anyhow::{bail, Context, Result};
use caddy::CaddyManager;
use neals_common::{
    daemon_socket, decode_request, encode_response, ensure_dir, runtime_dir, Response,
};
use std::fs::Permissions;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::signal;
use tokio::sync::Mutex;
use tokio::time::{interval, Duration};

pub async fn run() -> Result<()> {
    let runtime = runtime_dir()?;
    ensure_dir(&runtime)?;
    let sock = daemon_socket()?;
    let listener = bind_socket(&sock).await?;
    eprintln!("nealsd: ipc {}", sock.display());

    let caddy = CaddyManager::start()
        .await
        .context("failed to start caddy")?;
    let http = caddy.http_addr();
    if http.is_empty() {
        eprintln!("nealsd: caddy disabled");
    } else {
        eprintln!("nealsd: http  http://{http}/  (Host: {{service}}.{{project}}.localhost)");
    }
    let state = Arc::new(Mutex::new(AppState {
        projects: Default::default(),
        caddy,
        leases: Default::default(),
    }));
    state.lock().await.reconcile().await;
    let state_accept = Arc::clone(&state);
    let state_reap = Arc::clone(&state);

    let accept_loop = async move {
        loop {
            match listener.accept().await {
                Ok((stream, _)) => {
                    let state = Arc::clone(&state_accept);
                    tokio::spawn(async move {
                        if let Err(err) = handle_connection(stream, state).await {
                            eprintln!("connection error: {err:#}");
                        }
                    });
                }
                Err(err) => {
                    eprintln!("accept error: {err}");
                }
            }
        }
    };

    let reap_loop = async move {
        let mut tick = interval(Duration::from_secs(1));
        loop {
            tick.tick().await;
            let mut state = state_reap.lock().await;
            state.reap_exited().await;
        }
    };

    tokio::select! {
        _ = accept_loop => {}
        _ = reap_loop => {}
        _ = shutdown_signal() => {
            eprintln!("shutting down");
        }
    }

    {
        let mut state = state.lock().await;
        state.stop_all().await;
    }
    let _ = tokio::fs::remove_file(&sock).await;
    Ok(())
}

async fn bind_socket(path: &Path) -> Result<UnixListener> {
    prepare_socket(path).await?;
    let listener =
        UnixListener::bind(path).with_context(|| format!("failed to bind {}", path.display()))?;
    tokio::fs::set_permissions(path, Permissions::from_mode(0o600))
        .await
        .with_context(|| format!("failed to restrict {}", path.display()))?;
    Ok(listener)
}

async fn prepare_socket(path: &Path) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    match UnixStream::connect(path).await {
        Ok(_) => bail!("nealsd already running ({})", path.display()),
        Err(_) => {
            tokio::fs::remove_file(path)
                .await
                .with_context(|| format!("failed to remove stale socket {}", path.display()))?;
        }
    }
    Ok(())
}

async fn handle_connection(stream: UnixStream, state: Arc<Mutex<AppState>>) -> Result<()> {
    let (reader, mut writer) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();
    let line = lines
        .next_line()
        .await
        .context("failed to read request")?
        .context("client closed without sending a request")?;
    let response = match decode_request(&line) {
        Ok(request) => handle_request(request, &state).await,
        Err(err) => Response::Error {
            message: format!("{err:#}"),
        },
    };
    let encoded = encode_response(&response)?;
    writer
        .write_all(encoded.as_bytes())
        .await
        .context("failed to write response")?;
    writer.flush().await.ok();
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        signal::ctrl_c().await.ok();
    };
    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {}
        _ = terminate => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use neals_common::{decode_response, PROTOCOL_VERSION};

    fn temp_socket(tag: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("nealsd-{tag}-{}.sock", std::process::id()))
    }

    async fn exchange(request: &str) -> Response {
        let (client, server) = UnixStream::pair().unwrap();
        let state = Arc::new(Mutex::new(AppState::default()));
        let served = tokio::spawn(handle_connection(server, state));
        let (reader, mut writer) = client.into_split();
        writer.write_all(request.as_bytes()).await.unwrap();
        let reply = BufReader::new(reader).lines().next_line().await.unwrap();
        served.await.unwrap().unwrap();
        decode_response(&reply.unwrap()).unwrap()
    }

    #[tokio::test]
    async fn socket_is_owner_only() {
        let path = temp_socket("mode");
        let _listener = bind_socket(&path).await.unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn live_socket_is_refused_and_stale_socket_is_replaced() {
        let path = temp_socket("stale");
        let live = bind_socket(&path).await.unwrap();
        assert!(bind_socket(&path).await.is_err());
        drop(live);
        bind_socket(&path).await.unwrap();
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn ping_reports_the_protocol_version() {
        assert_eq!(
            exchange("{\"Ping\":null}\n").await,
            Response::Pong {
                protocol: PROTOCOL_VERSION
            }
        );
    }

    #[tokio::test]
    async fn undecodable_request_gets_an_error_instead_of_a_dropped_connection() {
        for request in ["{\"FromTheFuture\":null}\n", "garbage\n"] {
            assert!(matches!(exchange(request).await, Response::Error { .. }));
        }
    }
}
