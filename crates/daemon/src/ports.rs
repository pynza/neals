use anyhow::{bail, Result};
use std::collections::HashSet;
use std::net::{Ipv4Addr, SocketAddr, TcpListener};

#[derive(Debug, Default)]
pub struct LoopbackLeases {
    used: HashSet<(u8, u8)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HostEndpoint {
    pub ip: Ipv4Addr,
    pub port: u16,
}

impl LoopbackLeases {
    pub fn allocate(&mut self, preferred_port: Option<u16>) -> Result<HostEndpoint> {
        let ip = self.next_ip()?;
        let port = match preferred_port {
            Some(0) | None => bind_ephemeral(ip)?,
            Some(start) => bind_preferred(ip, start)?,
        };
        Ok(HostEndpoint { ip, port })
    }

    pub fn release(&mut self, ip: Ipv4Addr) {
        if let Some(key) = octet_key(ip) {
            self.used.remove(&key);
        }
    }

    pub fn contains(&self, ip: Ipv4Addr) -> bool {
        octet_key(ip).is_some_and(|k| self.used.contains(&k))
    }

    fn next_ip(&mut self) -> Result<Ipv4Addr> {
        for x in 0..=255u8 {
            for y in 1..=255u8 {
                let key = (x, y);
                if self.used.contains(&key) {
                    continue;
                }
                let ip = Ipv4Addr::new(127, 53, x, y);
                // Probe that the stack accepts this address before leasing it.
                match TcpListener::bind(SocketAddr::from((ip, 0))) {
                    Ok(_) => {
                        self.used.insert(key);
                        return Ok(ip);
                    }
                    Err(_) => continue,
                }
            }
        }
        bail!("failed to allocate a free 127.53.x.y loopback address");
    }
}

fn octet_key(ip: Ipv4Addr) -> Option<(u8, u8)> {
    let o = ip.octets();
    (o[0] == 127 && o[1] == 53).then_some((o[2], o[3]))
}

fn bind_ephemeral(ip: Ipv4Addr) -> Result<u16> {
    for _ in 0..64 {
        let listener = TcpListener::bind(SocketAddr::from((ip, 0)))?;
        let port = listener.local_addr()?.port();
        drop(listener);
        return Ok(port);
    }
    bail!("failed to allocate an ephemeral port on {ip}");
}

fn bind_preferred(ip: Ipv4Addr, start: u16) -> Result<u16> {
    let mut port = start;
    for _ in 0..1024 {
        match TcpListener::bind(SocketAddr::from((ip, port))) {
            Ok(listener) => {
                let bound = listener.local_addr()?.port();
                drop(listener);
                return Ok(bound);
            }
            Err(_) => {
                if port == u16::MAX {
                    break;
                }
                port += 1;
            }
        }
    }
    bail!("failed to allocate a free TCP port on {ip} starting at {start}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allocate_unique_ips_then_reuse_after_release() {
        let mut leases = LoopbackLeases::default();
        let a = leases.allocate(Some(0)).unwrap();
        let b = leases.allocate(Some(0)).unwrap();
        assert_ne!(a.ip, b.ip);
        assert!(leases.contains(a.ip));
        leases.release(a.ip);
        assert!(!leases.contains(a.ip));
        let c = leases.allocate(Some(0)).unwrap();
        assert!(leases.contains(c.ip));
    }

    #[test]
    fn preferred_port_available_on_distinct_ips() {
        let mut leases = LoopbackLeases::default();
        let start = 45100 + (std::process::id() as u16 % 200);
        let a = leases.allocate(Some(start)).unwrap();
        let b = leases.allocate(Some(start)).unwrap();
        assert_eq!(a.port, start);
        assert_eq!(b.port, start);
        assert_ne!(a.ip, b.ip);
        leases.release(a.ip);
        leases.release(b.ip);
    }

    #[test]
    fn preferred_skips_busy_port_on_ip() {
        let start = 45500 + (std::process::id() as u16 % 200);
        let ip = Ipv4Addr::new(127, 53, 200, 1);
        let _hold = TcpListener::bind(SocketAddr::from((ip, start))).unwrap();
        let got = bind_preferred(ip, start).unwrap();
        assert_eq!(got, start + 1);
    }
}
