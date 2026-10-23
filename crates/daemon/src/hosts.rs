use crate::state::{BoundRoute, BoundTarget};
use anyhow::{Context, Result};
use neals_common::{
    apply_hosts_file, mirror_system_hosts_fragment, render_host_block, service_host_name,
    state_hosts_path, system_hosts_path,
};
use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};

pub fn guest_hosts_body(project: &str, services: &[String]) -> String {
    let mut body = String::from("127.0.0.1 localhost\n::1 localhost\n");
    for service in services {
        let long = service_host_name(project, service);
        body.push_str(&format!("127.0.0.1 {service} {long}\n"));
    }
    body
}

pub fn write_guest_hosts(dir: &Path, project: &str, services: &[String]) -> Result<PathBuf> {
    let path = dir.join("hosts");
    std::fs::write(&path, guest_hosts_body(project, services))
        .with_context(|| format!("failed to write {}", path.display()))?;
    Ok(path)
}

pub fn host_entries(projects: &[(String, Vec<BoundRoute>)]) -> Vec<(Ipv4Addr, String)> {
    let mut entries = Vec::new();
    for (project, routes) in projects {
        for route in routes {
            let BoundTarget::Tcp { host_ip, .. } = route.target else {
                continue;
            };
            entries.push((host_ip, service_host_name(project, &route.service)));
        }
    }
    entries.sort_by(|a, b| a.1.cmp(&b.1));
    entries
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostsSync {
    Applied,
    Deferred,
}

pub fn sync_host_aliases(projects: &[(String, Vec<BoundRoute>)]) -> Result<HostsSync> {
    let entries = host_entries(projects);
    let block = if entries.is_empty() {
        String::new()
    } else {
        render_host_block(&entries)
    };

    let state_path = state_hosts_path()?;
    std::fs::write(&state_path, &block)
        .with_context(|| format!("failed to write {}", state_path.display()))?;

    let mirrored = mirror_system_hosts_fragment(&block);

    let system = system_hosts_path();
    match apply_hosts_file(&system, &block) {
        Ok(()) => Ok(HostsSync::Applied),
        Err(err) => {
            if !mirrored {
                eprintln!(
                    "nealsd: host DNS not applied to {} ({err:#}); \
                     enable neals-hosts-sync.path or run `sudo neals hosts apply` \
                     (fragment: {})",
                    system.display(),
                    state_path.display()
                );
            }
            Ok(HostsSync::Deferred)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guest_hosts_lists_short_and_long() {
        let body = guest_hosts_body("demo", &["db".into(), "mailpit".into()]);
        assert!(body.contains("127.0.0.1 db demo_db\n"));
        assert!(body.contains("127.0.0.1 mailpit demo_mailpit\n"));
        assert!(body.contains("127.0.0.1 localhost\n"));
    }

    #[test]
    fn host_entries_only_tcp_long_names() {
        let projects = [(
            "demo".into(),
            vec![
                BoundRoute {
                    service: "db".into(),
                    target: BoundTarget::Tcp {
                        host_ip: Ipv4Addr::new(127, 53, 0, 1),
                        host_port: 3306,
                        guest_port: 3306,
                    },
                    proxy: false,
                },
                BoundRoute {
                    service: "api".into(),
                    target: BoundTarget::Unix {
                        socket_file: "api.sock".into(),
                    },
                    proxy: true,
                },
            ],
        )];
        let entries = host_entries(&projects);
        assert_eq!(
            entries,
            vec![(Ipv4Addr::new(127, 53, 0, 1), "demo_db".into())]
        );
    }
}
