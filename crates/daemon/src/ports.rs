use anyhow::{bail, Result};
use std::collections::HashSet;
use std::net::TcpListener;

/// Leased TCP ports owned by running Neals projects.
///
/// ponytail: allocate via bind(127.0.0.1:0) then drop so the app can bind —
/// tiny TOCTOU vs external processes. Upgrade path: socket activation / FD pass.
#[derive(Debug, Default)]
pub struct PortLeases {
    leased: HashSet<u16>,
}

impl PortLeases {
    pub fn allocate(&mut self) -> Result<u16> {
        for _ in 0..64 {
            let listener = TcpListener::bind(("127.0.0.1", 0))?;
            let port = listener.local_addr()?.port();
            drop(listener);
            if self.leased.insert(port) {
                return Ok(port);
            }
        }
        bail!("failed to allocate a free TCP port on 127.0.0.1");
    }

    pub fn release(&mut self, port: u16) {
        self.leased.remove(&port);
    }

    pub fn contains(&self, port: u16) -> bool {
        self.leased.contains(&port)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allocate_unique_then_reuse_after_release() {
        let mut leases = PortLeases::default();
        let a = leases.allocate().unwrap();
        let b = leases.allocate().unwrap();
        assert_ne!(a, b);
        assert!(leases.contains(a));
        assert!(leases.contains(b));
        leases.release(a);
        assert!(!leases.contains(a));
        let c = leases.allocate().unwrap();
        assert!(leases.contains(c));
    }
}
