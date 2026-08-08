mod caddy;
mod handler;
mod ports;
mod state;

pub use handler::handle_request;
pub use state::AppState;

use anyhow::{bail, Context, Result};
use caddy::CaddyManager;
use neals_common::{
    daemon_socket, decode_request, encode_response, ensure_dir, runtime_dir,
};
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
    prepare_socket(&sock).await?;

    let listener = UnixListener::bind(&sock)
        .with_context(|| format!("failed to bind {}", sock.display()))?;
    eprintln!("nealsd listening on {}", sock.display());

    let caddy = CaddyManager::start()
        .await
        .context("failed to start caddy")?;
    let state = Arc::new(Mutex::new(AppState {
        projects: Default::default(),
        caddy,
        leases: Default::default(),
    }));
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
    let request = decode_request(&line)?;
    let response = handle_request(request, &state).await;
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
