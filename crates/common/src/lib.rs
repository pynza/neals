pub mod client;
pub mod devenv;
pub mod hosts;
pub mod ipc;
pub mod registry;
pub mod xdg;

pub use client::call_daemon;
pub use devenv::{
    env_port_var, env_service_key, has_neals_config, inject_neals_stanza, parse_neals_name,
    parse_neals_routes, parse_neals_services, read_neals_routes, read_neals_services,
    resolve_project_name, service_host_name, ProjectName, RouteDecl, RouteKind, ServiceDecl,
    ServiceKind,
};
pub use hosts::{
    apply_from_state, apply_hosts_file, apply_hosts_fragment_file, merge_hosts_file,
    mirror_system_hosts_fragment, read_hosts_fragment, render_host_block, state_hosts_path,
    system_hosts_path, BEGIN_MARKER, END_MARKER, SYSTEM_HOSTS_MIRROR,
};
pub use ipc::{
    decode_request, decode_response, encode_request, encode_response, ProjectRuntime, Request,
    Response,
};
pub use registry::{Project, Registry};

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
