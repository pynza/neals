use anyhow::{bail, Result};
use std::collections::HashSet;
use std::net::TcpListener;

// Leased TCP ports for running projects.
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

    pub fn allocate_preferred(&mut self, start: u16) -> Result<u16> {
        if start == 0 {
            return self.allocate();
        }
        let mut port = start;
        for _ in 0..1024 {
            if !self.leased.contains(&port) {
                match TcpListener::bind(("127.0.0.1", port)) {
                    Ok(listener) => {
                        let bound = listener.local_addr()?.port();
                        drop(listener);
                        if self.leased.insert(bound) {
                            return Ok(bound);
                        }
                    }
                    Err(_) => {}
                }
            }
            if port == u16::MAX {
                break;
            }
            port += 1;
        }
        bail!("failed to allocate a free TCP port on 127.0.0.1 starting at {start}");
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
    use std::net::TcpListener;

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

    #[test]
    fn preferred_returns_start_when_free() {
        let mut leases = PortLeases::default();
        // High port unlikely to be busy in CI.
        let start = 45100 + (std::process::id() as u16 % 200);
        let got = leases.allocate_preferred(start).unwrap();
        assert_eq!(got, start);
        leases.release(got);
    }

    #[test]
    fn preferred_skips_leased_then_next() {
        let mut leases = PortLeases::default();
        let start = 45300 + (std::process::id() as u16 % 200);
        let first = leases.allocate_preferred(start).unwrap();
        assert_eq!(first, start);
        let second = leases.allocate_preferred(start).unwrap();
        assert_eq!(second, start + 1);
        leases.release(first);
        leases.release(second);
    }

    #[test]
    fn preferred_skips_externally_bound_port() {
        let mut leases = PortLeases::default();
        let start = 45500 + (std::process::id() as u16 % 200);
        let hold = TcpListener::bind(("127.0.0.1", start)).expect("bind hold port");
        let got = leases.allocate_preferred(start).unwrap();
        assert_eq!(got, start + 1);
        drop(hold);
        leases.release(got);
    }

    #[test]
    fn preferred_services_independent() {
        let mut leases = PortLeases::default();
        let redis = 45700 + (std::process::id() as u16 % 100);
        let postgres = redis + 200;
        // Occupy redis preferred; postgres preferred must still win.
        let _hold = TcpListener::bind(("127.0.0.1", redis)).unwrap();
        let r = leases.allocate_preferred(redis).unwrap();
        let p = leases.allocate_preferred(postgres).unwrap();
        assert_eq!(r, redis + 1);
        assert_eq!(p, postgres);
        leases.release(r);
        leases.release(p);
    }
}
