pub mod client;
pub mod devenv;
pub mod ipc;
pub mod registry;
pub mod xdg;

pub use client::call_daemon;
pub use devenv::{
    env_service_key, parse_neals_name, parse_neals_routes, read_neals_routes, resolve_project_name,
    ProjectName, RouteDecl, RouteKind,
};
pub use ipc::{
    decode_request, decode_response, encode_request, encode_response, ProjectRuntime, Request,
    Response,
};
pub use registry::{Project, Registry};
pub use xdg::{
    config_dir, daemon_socket, ensure_dir, is_system_daemon_socket, projects_file, runtime_dir,
    state_dir, SYSTEM_DAEMON_SOCKET,
};
