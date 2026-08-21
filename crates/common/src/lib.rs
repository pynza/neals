pub mod client;
pub mod devenv;
pub mod ipc;
pub mod registry;
pub mod xdg;

pub use client::call_daemon;
pub use devenv::{
    env_port_var, env_service_key, parse_neals_name, parse_neals_routes, parse_neals_services,
    read_neals_routes, read_neals_services, resolve_project_name, ProjectName, RouteDecl, RouteKind,
    ServiceDecl, ServiceKind,
};
pub use ipc::{
    decode_request, decode_response, encode_request, encode_response, ProjectRuntime, Request,
    Response,
};
pub use registry::{Project, Registry};

/// Single-quote for `/bin/sh`-style consumption; safe inside bwrap's `sh -c`.
pub fn shell_quote(s: &str) -> String {
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

pub use xdg::{
    config_dir, daemon_socket, ensure_dir, is_system_daemon_socket, open_log, projects_file,
    runtime_dir, state_dir, LOG_MAX_BYTES, SYSTEM_DAEMON_SOCKET,
};
