use neals_common::{
    decode_response, encode_request, Project, Registry, Request, Response,
};
use nealsd::{handle_request, AppState};
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex as StdMutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::Mutex;

static ENV_LOCK: StdMutex<()> = StdMutex::new(());

fn netns_available() -> bool {
    static OK: OnceLock<bool> = OnceLock::new();
    *OK.get_or_init(nealsd::netns::userns_works)
}

macro_rules! require_netns {
    () => {
        if !netns_available() {
            eprintln!("skip: unprivileged user namespaces unavailable");
            return;
        }
    };
}

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
        std::env::set_var("NEALS_CADDY_CMD", "-");

        let project_dir = root.join("demo-project");
        fs::create_dir_all(&project_dir).unwrap();
        fs::write(
            project_dir.join("devenv.nix"),
            r#"{
              neals.name = "demo";
              neals.route.backend = "backend.sock";
              neals.route.api = "tcp";
            }"#,
        )
        .unwrap();
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

    fn add_project(&self, name: &str, devenv_nix: &str) {
        let project_dir = self.root.join(format!("{name}-project"));
        fs::create_dir_all(&project_dir).unwrap();
        fs::write(project_dir.join("devenv.nix"), devenv_nix).unwrap();
        let mut registry = Registry::load().unwrap();
        registry
            .add(Project {
                name: name.into(),
                path: project_dir,
            })
            .unwrap();
        registry.save().unwrap();
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
    require_netns!();
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
            assert_eq!(projects[0].routes.len(), 2);
            assert!(projects[0]
                .routes
                .iter()
                .any(|r| r == "http://backend.demo.localhost:2015/"));
            assert!(projects[0].routes.iter().any(|r| {
                r.starts_with("http://api.demo.localhost:2015/ → 127.0.0.1:")
            }));
        }
        other => panic!("expected Status, got {other:?}"),
    }

    let link = _env
        .root
        .join("demo-project")
        .join(".neals")
        .join("backend.sock");
    assert!(link.symlink_metadata().is_ok(), "expected .neals symlink");
    assert!(
        !_env
            .root
            .join("demo-project")
            .join(".neals")
            .join("api")
            .exists(),
        "TCP routes must not create .neals symlinks"
    );

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

#[tokio::test]
async fn crashed_up_process_is_reaped_and_ports_released() {
    require_netns!();
    let _env = TestEnv::setup();
    std::env::set_var("NEALS_UP_CMD", "true");
    let state = Arc::new(Mutex::new(AppState::default()));

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

    // Wait until the short-lived child exits, then reap.
    for _ in 0..50 {
        {
            let mut st = state.lock().await;
            st.reap_exited().await;
            if st.projects.is_empty() {
                break;
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }

    match roundtrip(&state, Request::Status).await {
        Response::Status { projects } => assert!(projects.is_empty()),
        other => panic!("expected empty Status after reap, got {other:?}"),
    }

    // A second up must succeed (leases released; no "already running").
    std::env::set_var("NEALS_UP_CMD", "sleep 3600");
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
}

#[tokio::test]
async fn multi_project_tcp_routes_have_distinct_hosts_and_ports() {
    require_netns!();
    let env = TestEnv::setup();
    env.add_project(
        "other",
        r#"{
          neals.name = "other";
          neals.route.be = "tcp";
        }"#,
    );
    // demo already has api=tcp from setup; give it a be=tcp too via rewrite
    fs::write(
        env.root.join("demo-project").join("devenv.nix"),
        r#"{
          neals.name = "demo";
          neals.route.be = "tcp";
        }"#,
    )
    .unwrap();

    let state = Arc::new(Mutex::new(AppState::default()));

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
    assert_eq!(
        roundtrip(
            &state,
            Request::Up {
                project: "other".into()
            }
        )
        .await,
        Response::Ok
    );

    match roundtrip(&state, Request::Status).await {
        Response::Status { projects } => {
            assert_eq!(projects.len(), 2);
            let demo = projects.iter().find(|p| p.name == "demo").unwrap();
            let other = projects.iter().find(|p| p.name == "other").unwrap();

            let demo_route = demo
                .routes
                .iter()
                .find(|r| r.starts_with("http://be.demo.localhost:2015/ → 127.0.0.1:"))
                .expect("demo be route");
            let other_route = other
                .routes
                .iter()
                .find(|r| r.starts_with("http://be.other.localhost:2015/ → 127.0.0.1:"))
                .expect("other be route");

            let demo_port: u16 = demo_route.rsplit(':').next().unwrap().parse().unwrap();
            let other_port: u16 = other_route.rsplit(':').next().unwrap().parse().unwrap();
            assert_ne!(demo_port, other_port, "projects must not share TCP ports");
            assert_ne!(demo_route, other_route);
        }
        other => panic!("expected Status, got {other:?}"),
    }

    let _ = roundtrip(
        &state,
        Request::Down {
            project: "demo".into()
        },
    )
    .await;
    let _ = roundtrip(
        &state,
        Request::Down {
            project: "other".into()
        },
    )
    .await;
}

#[tokio::test]
async fn preferred_ports_private_and_proxy_status() {
    require_netns!();
    let env = TestEnv::setup();
    fs::write(
        env.root.join("demo-project").join("devenv.nix"),
        r#"{
          neals.name = "demo";
          neals.services.redis.port = 45901;
          neals.services.api = { port = 45911; proxy = true; };
        }"#,
    )
    .unwrap();

    let state = Arc::new(Mutex::new(AppState::default()));
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
            let demo = &projects[0];
            assert!(
                demo.routes.iter().any(|r| r == "redis → 127.0.0.1:45901"),
                "private redis label missing: {:?}",
                demo.routes
            );
            assert!(
                demo.routes.iter().any(|r| {
                    r.starts_with("http://api.demo.localhost:2015/ → 127.0.0.1:45911")
                }),
                "proxied api label missing: {:?}",
                demo.routes
            );
        }
        other => panic!("expected Status, got {other:?}"),
    }

    let _ = roundtrip(
        &state,
        Request::Down {
            project: "demo".into()
        },
    )
    .await;
}

#[tokio::test]
async fn two_projects_same_preferred_get_distinct_ports() {
    require_netns!();
    let env = TestEnv::setup();
    let nix = r#"{
      neals.name = "NAME";
      neals.services.redis.port = 46101;
    }"#;
    fs::write(
        env.root.join("demo-project").join("devenv.nix"),
        nix.replace("NAME", "demo"),
    )
    .unwrap();
    env.add_project("other", &nix.replace("NAME", "other"));

    let state = Arc::new(Mutex::new(AppState::default()));
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
    assert_eq!(
        roundtrip(
            &state,
            Request::Up {
                project: "other".into()
            }
        )
        .await,
        Response::Ok
    );

    match roundtrip(&state, Request::Status).await {
        Response::Status { projects } => {
            let port = |name: &str| -> u16 {
                let p = projects.iter().find(|p| p.name == name).unwrap();
                let label = p
                    .routes
                    .iter()
                    .find(|r| r.starts_with("redis → 127.0.0.1:"))
                    .unwrap();
                label.rsplit(':').next().unwrap().parse().unwrap()
            };
            let a = port("demo");
            let b = port("other");
            assert_ne!(a, b);
            assert!(a >= 46101);
            assert!(b >= 46101);
            assert!([a, b].contains(&46101));
        }
        other => panic!("expected Status, got {other:?}"),
    }

    let _ = roundtrip(
        &state,
        Request::Down {
            project: "demo".into()
        },
    )
    .await;
    let _ = roundtrip(
        &state,
        Request::Down {
            project: "other".into()
        },
    )
    .await;
}

#[tokio::test]
async fn concurrent_up_same_preferred_no_collision() {
    require_netns!();
    let env = TestEnv::setup();
    let nix = r#"{
      neals.name = "NAME";
      neals.services.redis.port = 46301;
    }"#;
    fs::write(
        env.root.join("demo-project").join("devenv.nix"),
        nix.replace("NAME", "demo"),
    )
    .unwrap();
    env.add_project("other", &nix.replace("NAME", "other"));

    let state = Arc::new(Mutex::new(AppState::default()));
    let s1 = Arc::clone(&state);
    let s2 = Arc::clone(&state);
    let (r1, r2) = tokio::join!(
        handle_request(
            Request::Up {
                project: "demo".into()
            },
            &s1
        ),
        handle_request(
            Request::Up {
                project: "other".into()
            },
            &s2
        ),
    );
    assert_eq!(r1, Response::Ok);
    assert_eq!(r2, Response::Ok);

    match roundtrip(&state, Request::Status).await {
        Response::Status { projects } => {
            assert_eq!(projects.len(), 2);
            let ports: Vec<u16> = projects
                .iter()
                .map(|p| {
                    let label = p
                        .routes
                        .iter()
                        .find(|r| r.starts_with("redis → 127.0.0.1:"))
                        .unwrap();
                    label.rsplit(':').next().unwrap().parse().unwrap()
                })
                .collect();
            assert_ne!(ports[0], ports[1]);
        }
        other => panic!("expected Status, got {other:?}"),
    }

    let _ = roundtrip(
        &state,
        Request::Down {
            project: "demo".into()
        },
    )
    .await;
    let _ = roundtrip(
        &state,
        Request::Down {
            project: "other".into()
        },
    )
    .await;
}

/// Acceptance-style: fixtures under tests/projects bind `$NEALS_REDIS_PORT`.
#[tokio::test]
async fn redis_project_fixtures_bind_allocated_ports() {
    require_netns!();
    let env = TestEnv::setup();
    let binder = env.root.join("bind_redis.py");
    fs::write(
        &binder,
        r#"#!/usr/bin/env python3
import os, socket, time
port = int(os.environ["NEALS_REDIS_PORT"])
s = socket.socket()
s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
s.bind(("127.0.0.1", port))
s.listen(1)
open(os.environ["NEALS_BIND_READY"], "w").write(str(port))
time.sleep(3600)
"#,
    )
    .unwrap();

    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/projects");
    for (name, dir) in [
        ("redis-project-1", "redis-project-1"),
        ("redis-project-2", "redis-project-2"),
    ] {
        let src = fixture.join(dir);
        let dst = env.root.join(format!("{name}-project"));
        fs::create_dir_all(&dst).unwrap();
        fs::copy(src.join("devenv.nix"), dst.join("devenv.nix")).unwrap();
        let mut registry = Registry::load().unwrap();
        let _ = registry.remove(name); // ok if missing
        registry
            .add(Project {
                name: name.into(),
                path: dst,
            })
            .unwrap();
        registry.save().unwrap();
    }

    let ready1 = env.root.join("ready1");
    let ready2 = env.root.join("ready2");
    let wrap1 = env.root.join("up1.sh");
    let wrap2 = env.root.join("up2.sh");
    fs::write(
        &wrap1,
        format!(
            "#!/bin/sh\nexport NEALS_BIND_READY='{}'\nexec python3 '{}'\n",
            ready1.display(),
            binder.display()
        ),
    )
    .unwrap();
    fs::write(
        &wrap2,
        format!(
            "#!/bin/sh\nexport NEALS_BIND_READY='{}'\nexec python3 '{}'\n",
            ready2.display(),
            binder.display()
        ),
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(&wrap1, fs::Permissions::from_mode(0o755)).unwrap();
    fs::set_permissions(&wrap2, fs::Permissions::from_mode(0o755)).unwrap();

    let state = Arc::new(Mutex::new(AppState::default()));
    std::env::set_var("NEALS_UP_CMD", wrap1.to_str().unwrap());
    assert_eq!(
        roundtrip(
            &state,
            Request::Up {
                project: "redis-project-1".into()
            }
        )
        .await,
        Response::Ok
    );
    std::env::set_var("NEALS_UP_CMD", wrap2.to_str().unwrap());
    assert_eq!(
        roundtrip(
            &state,
            Request::Up {
                project: "redis-project-2".into()
            }
        )
        .await,
        Response::Ok
    );

    let mut port1 = None;
    let mut port2 = None;
    for _ in 0..100 {
        if ready1.is_file() {
            port1 = Some(fs::read_to_string(&ready1).unwrap().trim().parse::<u16>().unwrap());
        }
        if ready2.is_file() {
            port2 = Some(fs::read_to_string(&ready2).unwrap().trim().parse::<u16>().unwrap());
        }
        if port1.is_some() && port2.is_some() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    let port1 = port1.expect("redis-project-1 bound");
    let port2 = port2.expect("redis-project-2 bound");
    assert_ne!(port1, port2);
    assert!(port1 >= 6379);
    assert!(port2 >= 6379);

    match roundtrip(&state, Request::Status).await {
        Response::Status { projects } => {
            let label_port = |name: &str| -> u16 {
                let p = projects.iter().find(|p| p.name == name).unwrap();
                p.routes
                    .iter()
                    .find(|r| r.starts_with("redis → 127.0.0.1:"))
                    .unwrap()
                    .rsplit(':')
                    .next()
                    .unwrap()
                    .parse()
                    .unwrap()
            };
            assert_eq!(label_port("redis-project-1"), port1);
            assert_eq!(label_port("redis-project-2"), port2);
        }
        other => panic!("expected Status, got {other:?}"),
    }

    // Both listeners must accept connections.
    for port in [port1, port2] {
        let stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .unwrap_or_else(|e| panic!("connect {port}: {e}"));
        drop(stream);
    }

    let _ = roundtrip(
        &state,
        Request::Down {
            project: "redis-project-1".into()
        },
    )
    .await;
    let _ = roundtrip(
        &state,
        Request::Down {
            project: "redis-project-2".into()
        },
    )
    .await;
}