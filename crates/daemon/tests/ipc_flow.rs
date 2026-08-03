use neals_common::{
    decode_response, encode_request, Project, Registry, Request, Response,
};
use nealsd::{handle_request, AppState};
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::Mutex;

static ENV_LOCK: StdMutex<()> = StdMutex::new(());

fn unique_temp(prefix: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "{prefix}-{}-{}",
        std::process::id(),
        nanos
    ))
}

struct TestEnv {
    root: PathBuf,
    _guard: std::sync::MutexGuard<'static, ()>,
}

impl TestEnv {
    fn setup() -> Self {
        let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let root = unique_temp("nealsd-test");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let config = root.join("config");
        let state = root.join("state");
        let runtime = root.join("runtime");
        fs::create_dir_all(&config).unwrap();
        fs::create_dir_all(&state).unwrap();
        fs::create_dir_all(&runtime).unwrap();
        std::env::set_var("XDG_CONFIG_HOME", &config);
        std::env::set_var("XDG_STATE_HOME", &state);
        std::env::set_var("XDG_RUNTIME_DIR", &runtime);
        std::env::set_var("NEALS_UP_CMD", "sleep 3600");

        let project_dir = root.join("demo-project");
        fs::create_dir_all(&project_dir).unwrap();
        let mut registry = Registry::default();
        registry
            .add(Project {
                name: "demo".into(),
                path: project_dir,
            })
            .unwrap();
        registry.save().unwrap();

        Self {
            root,
            _guard: guard,
        }
    }
}

impl Drop for TestEnv {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

async fn roundtrip(state: &Arc<Mutex<AppState>>, request: Request) -> Response {
    let response = handle_request(request, state).await;
    let line = neals_common::encode_response(&response).unwrap();
    decode_response(&line).unwrap()
}

#[tokio::test]
async fn ping_up_status_down() {
    let _env = TestEnv::setup();
    let state = Arc::new(Mutex::new(AppState::default()));

    assert_eq!(roundtrip(&state, Request::Ping).await, Response::Pong);

    assert_eq!(
        roundtrip(
            &state,
            Request::Up {
                project: "demo".into()
            }
        )
        .await,
        Response::Ok
    );

    match roundtrip(&state, Request::Status).await {
        Response::Status { projects } => {
            assert_eq!(projects.len(), 1);
            assert_eq!(projects[0].name, "demo");
            assert!(projects[0].pid > 0);
        }
        other => panic!("expected Status, got {other:?}"),
    }

    let log = std::env::var("XDG_STATE_HOME").unwrap();
    let log_path = PathBuf::from(log).join("neals").join("demo.log");
    assert!(log_path.is_file(), "expected log at {}", log_path.display());

    assert_eq!(
        roundtrip(
            &state,
            Request::Up {
                project: "demo".into()
            }
        )
        .await,
        Response::Error {
            message: "project `demo` is already running".into()
        }
    );

    assert_eq!(
        roundtrip(
            &state,
            Request::Down {
                project: "demo".into()
            }
        )
        .await,
        Response::Ok
    );

    match roundtrip(&state, Request::Status).await {
        Response::Status { projects } => assert!(projects.is_empty()),
        other => panic!("expected empty Status, got {other:?}"),
    }

    assert_eq!(
        roundtrip(
            &state,
            Request::Down {
                project: "demo".into()
            }
        )
        .await,
        Response::Error {
            message: "project `demo` is not running".into()
        }
    );
}

#[tokio::test]
async fn up_missing_project_errors() {
    let _env = TestEnv::setup();
    let state = Arc::new(Mutex::new(AppState::default()));
    assert_eq!(
        roundtrip(
            &state,
            Request::Up {
                project: "missing".into()
            }
        )
        .await,
        Response::Error {
            message: "project `missing` is not registered".into()
        }
    );
}

#[tokio::test]
async fn socket_ping_roundtrip() {
    let _env = TestEnv::setup();
    let sock = _env.root.join("runtime").join("neals").join("test.sock");
    fs::create_dir_all(sock.parent().unwrap()).unwrap();
    let listener = UnixListener::bind(&sock).unwrap();
    let state = Arc::new(Mutex::new(AppState::default()));

    let server = {
        let state = Arc::clone(&state);
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (reader, mut writer) = stream.into_split();
            let mut lines = BufReader::new(reader).lines();
            let line = lines.next_line().await.unwrap().unwrap();
            let request = neals_common::decode_request(&line).unwrap();
            let response = handle_request(request, &state).await;
            let encoded = neals_common::encode_response(&response).unwrap();
            writer.write_all(encoded.as_bytes()).await.unwrap();
        })
    };

    let mut client = UnixStream::connect(&sock).await.unwrap();
    client
        .write_all(encode_request(&Request::Ping).unwrap().as_bytes())
        .await
        .unwrap();
    let mut lines = BufReader::new(client).lines();
    let reply = lines.next_line().await.unwrap().unwrap();
    assert_eq!(decode_response(&reply).unwrap(), Response::Pong);
    server.await.unwrap();
}
