//! Zero-config discovery: every running instance announces itself every 2
//! seconds and listens for others, on every network interface.
//!
//! - IPv4 broadcast on each interface (home/office networks, and direct
//!   cables on Windows/macOS, which self-assign 169.254.x.x addresses).
//! - IPv6 link-local multicast on each interface. Every OS gives every
//!   connected port an `fe80::` address with no router or DHCP, so two
//!   computers joined by an Ethernet, USB-C or Thunderbolt cable always find
//!   each other, while each keeps its own Wi-Fi connection.
//!
//! Beacons carry no secrets: a name, OS, role and port. Authentication
//! happens in the encrypted handshake.

use crate::config::Role;
use crate::protocol::{Os, DISCOVERY_PORT, PROTOCOL_VERSION};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use socket2::{Domain, Protocol, Socket, Type};
use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV6, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

const MAGIC: [u8; 4] = *b"OHOP";
const INTERVAL: Duration = Duration::from_secs(2);
const EXPIRY: Duration = Duration::from_secs(7);
/// Link-local multicast group for OpenHop beacons ("OHOP").
pub const GROUP_V6: Ipv6Addr = Ipv6Addr::new(0xff02, 0, 0, 0, 0, 0, 0x4f48, 0x4f50);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Beacon {
    pub magic: [u8; 4],
    pub version: u32,
    /// Random per run (so we can ignore our own beacons).
    pub id: u64,
    /// Stable per installation (used for pairing).
    pub device: String,
    pub name: String,
    pub os: Os,
    pub role: Role,
    pub port: u16,
}

#[derive(Debug, Clone, Serialize)]
pub struct Discovered {
    pub name: String,
    pub device: String,
    pub os: Os,
    pub role: Role,
    pub version: u32,
    /// Best address to connect to.
    pub addr: SocketAddr,
    /// "network" (through your router) or "direct" (cable / link-local only).
    pub link: &'static str,
    #[serde(skip)]
    pub last_seen: Instant,
    #[serde(skip)]
    v4: Option<(SocketAddr, Instant)>,
    #[serde(skip)]
    v6: Option<(SocketAddr, Instant)>,
}

pub struct Discovery {
    peers: Arc<Mutex<HashMap<u64, Discovered>>>,
    stop: Arc<AtomicBool>,
}

fn bind_v4() -> std::io::Result<UdpSocket> {
    let s = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    s.set_reuse_address(true)?;
    #[cfg(all(unix, not(target_os = "solaris")))]
    s.set_reuse_port(true)?;
    s.set_broadcast(true)?;
    s.bind(&SocketAddr::from((Ipv4Addr::UNSPECIFIED, DISCOVERY_PORT)).into())?;
    let s: UdpSocket = s.into();
    s.set_read_timeout(Some(Duration::from_millis(500)))?;
    Ok(s)
}

fn bind_v6() -> std::io::Result<UdpSocket> {
    let s = Socket::new(Domain::IPV6, Type::DGRAM, Some(Protocol::UDP))?;
    s.set_only_v6(true)?;
    s.set_reuse_address(true)?;
    #[cfg(all(unix, not(target_os = "solaris")))]
    s.set_reuse_port(true)?;
    s.set_multicast_loop_v6(false)?;
    s.bind(&SocketAddr::from((Ipv6Addr::UNSPECIFIED, DISCOVERY_PORT)).into())?;
    let s: UdpSocket = s.into();
    s.set_read_timeout(Some(Duration::from_millis(500)))?;
    Ok(s)
}

/// Up, non-loopback interfaces: (IPv6 index if it has a link-local address, IPv4 broadcast addresses).
fn interfaces() -> Vec<(Option<u32>, Vec<Ipv4Addr>)> {
    netdev::get_interfaces()
        .into_iter()
        .filter(|i| i.is_up() && !i.is_loopback())
        .map(|i| {
            let v6 = i.ipv6.iter().any(|n| (n.addr().segments()[0] & 0xffc0) == 0xfe80).then_some(i.index);
            let bc = i.ipv4.iter().map(|n| n.broadcast()).filter(|b| !b.is_loopback()).collect();
            (v6, bc)
        })
        .collect()
}

pub fn is_link_local(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_link_local(),
        IpAddr::V6(v6) => (v6.segments()[0] & 0xffc0) == 0xfe80,
    }
}

impl Discovery {
    /// Start announcing this computer and listening for others.
    pub fn start(device: String, name: String, role: Role, port: u16) -> std::io::Result<Discovery> {
        let v4 = bind_v4()?;
        let v6 = match bind_v6() {
            Ok(s) => Some(s),
            Err(e) => {
                log::info!("IPv6 discovery unavailable ({e}); direct-cable discovery limited to IPv4");
                None
            }
        };
        let id: u64 = crate::files::new_id();
        let beacon = Beacon { magic: MAGIC, version: PROTOCOL_VERSION, id, device, name, os: Os::current(), role, port };
        let payload = bincode::serialize(&beacon).expect("beacon");
        let peers: Arc<Mutex<HashMap<u64, Discovered>>> = Default::default();
        let stop = Arc::new(AtomicBool::new(false));

        // Sender: every interface, both IP versions.
        {
            let v4 = v4.try_clone()?;
            let v6 = v6.as_ref().map(|s| s.try_clone()).transpose()?;
            let stop = stop.clone();
            std::thread::Builder::new().name("discovery-tx".into()).spawn(move || {
                let mut joined: HashSet<u32> = HashSet::new();
                while !stop.load(Ordering::Relaxed) {
                    let mut targets: HashSet<Ipv4Addr> = HashSet::from([Ipv4Addr::BROADCAST]);
                    for (idx6, bcasts) in interfaces() {
                        targets.extend(bcasts);
                        if let (Some(idx), Some(s6)) = (idx6, v6.as_ref()) {
                            // Join on interfaces that appeared since last time (cable plugged in).
                            if joined.insert(idx) {
                                if let Err(e) = s6.join_multicast_v6(&GROUP_V6, idx) {
                                    log::debug!("join multicast on if {idx}: {e}");
                                }
                            }
                            let dest = SocketAddrV6::new(GROUP_V6, DISCOVERY_PORT, 0, idx);
                            if let Err(e) = s6.send_to(&payload, dest) {
                                log::debug!("beacon v6 if {idx}: {e}");
                            }
                        }
                    }
                    for t in targets {
                        let _ = v4.send_to(&payload, SocketAddr::from((t, DISCOVERY_PORT)));
                    }
                    std::thread::sleep(INTERVAL);
                }
            })?;
        }

        // Receivers.
        for sock in std::iter::once(v4).chain(v6) {
            let peers = peers.clone();
            let stop = stop.clone();
            std::thread::Builder::new().name("discovery-rx".into()).spawn(move || {
                let mut buf = [0u8; 1500];
                while !stop.load(Ordering::Relaxed) {
                    let Ok((n, from)) = sock.recv_from(&mut buf) else {
                        continue;
                    };
                    let Ok(b) = bincode::deserialize::<Beacon>(&buf[..n]) else {
                        continue;
                    };
                    if b.magic != MAGIC || b.id == id {
                        continue;
                    }
                    record(&peers, b, from);
                }
            })?;
        }
        Ok(Discovery { peers, stop })
    }

    pub fn peers(&self) -> Vec<Discovered> {
        let mut map = self.peers.lock();
        map.retain(|_, p| p.last_seen.elapsed() < EXPIRY);
        // The same computer may restart with a new run id; keep the newest.
        let mut by_device: HashMap<String, Discovered> = HashMap::new();
        for p in map.values() {
            match by_device.get(&p.device) {
                Some(q) if q.last_seen >= p.last_seen => {}
                _ => {
                    by_device.insert(p.device.clone(), p.clone());
                }
            }
        }
        let mut v: Vec<_> = by_device.into_values().collect();
        v.sort_by(|a, b| a.name.cmp(&b.name));
        v
    }

    pub fn servers(&self) -> Vec<Discovered> {
        self.peers().into_iter().filter(|p| p.role == Role::Server).collect()
    }
}

fn record(peers: &Mutex<HashMap<u64, Discovered>>, b: Beacon, from: SocketAddr) {
    let now = Instant::now();
    let addr = match from {
        SocketAddr::V6(a) => SocketAddr::V6(SocketAddrV6::new(*a.ip(), b.port, 0, a.scope_id())),
        SocketAddr::V4(a) => SocketAddr::new(IpAddr::V4(*a.ip()), b.port),
    };
    let mut map = peers.lock();
    let e = map.entry(b.id).or_insert_with(|| Discovered {
        name: b.name.clone(),
        device: b.device.clone(),
        os: b.os,
        role: b.role,
        version: b.version,
        addr,
        link: "network",
        last_seen: now,
        v4: None,
        v6: None,
    });
    e.name = b.name;
    e.role = b.role;
    e.last_seen = now;
    match addr {
        SocketAddr::V4(_) => e.v4 = Some((addr, now)),
        SocketAddr::V6(_) => e.v6 = Some((addr, now)),
    }
    // Prefer a routable IPv4 address seen recently, else IPv6 link-local.
    let fresh = |x: &Option<(SocketAddr, Instant)>| x.filter(|(_, t)| t.elapsed() < EXPIRY).map(|(a, _)| a);
    let v4 = fresh(&e.v4);
    let v6 = fresh(&e.v6);
    e.addr = match (v4, v6) {
        (Some(a), _) if !is_link_local(a.ip()) => a,
        (_, Some(b)) => b,
        (Some(a), None) => a,
        (None, None) => addr,
    };
    e.link = if is_link_local(e.addr.ip()) { "direct" } else { "network" };
}

impl Drop for Discovery {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefers_routable_v4_then_link_local_v6() {
        let peers: Mutex<HashMap<u64, Discovered>> = Default::default();
        let b =
            Beacon { magic: MAGIC, version: PROTOCOL_VERSION, id: 7, device: "d".into(), name: "mac".into(), os: Os::MacOs, role: Role::Server, port: 24850 };
        let v6: SocketAddr = "[fe80::1%3]:5000".parse().unwrap();
        record(&peers, b.clone(), v6);
        let p = peers.lock()[&7].clone();
        assert_eq!(p.link, "direct");
        assert_eq!(p.addr.port(), 24850);
        record(&peers, b, "192.168.1.9:5000".parse().unwrap());
        let p = peers.lock()[&7].clone();
        assert_eq!(p.addr, "192.168.1.9:24850".parse::<SocketAddr>().unwrap());
        assert_eq!(p.link, "network");
    }
}
