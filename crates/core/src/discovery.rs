//! Zero-config LAN discovery via UDP broadcast beacons.
//!
//! Every running instance broadcasts a small beacon every 2 seconds and
//! listens for others. Beacons carry no secrets: just a name, OS, role and
//! port. Authentication happens later in the encrypted handshake.

use crate::config::Role;
use crate::protocol::{Os, DISCOVERY_PORT, PROTOCOL_VERSION};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use socket2::{Domain, Protocol, Socket, Type};
use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

const MAGIC: [u8; 4] = *b"OHOP";
const INTERVAL: Duration = Duration::from_secs(2);
const EXPIRY: Duration = Duration::from_secs(7);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Beacon {
    pub magic: [u8; 4],
    pub version: u32,
    pub id: u64,
    pub name: String,
    pub os: Os,
    pub role: Role,
    pub port: u16,
}

#[derive(Debug, Clone, Serialize)]
pub struct Discovered {
    pub name: String,
    pub os: Os,
    pub role: Role,
    pub addr: SocketAddr,
    #[serde(skip)]
    pub last_seen: Instant,
}

pub struct Discovery {
    peers: Arc<Mutex<HashMap<u64, Discovered>>>,
    stop: Arc<AtomicBool>,
}

fn socket() -> std::io::Result<UdpSocket> {
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

impl Discovery {
    /// Start broadcasting `name` and listening for peers.
    pub fn start(name: String, role: Role, port: u16) -> std::io::Result<Discovery> {
        let sock = socket()?;
        let id: u64 = {
            use std::hash::{BuildHasher, Hasher};
            let mut h = std::collections::hash_map::RandomState::new().build_hasher();
            h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos());
            h.finish()
        };
        let beacon = Beacon { magic: MAGIC, version: PROTOCOL_VERSION, id, name, os: Os::current(), role, port };
        let payload = bincode::serialize(&beacon).expect("beacon");
        let peers: Arc<Mutex<HashMap<u64, Discovered>>> = Default::default();
        let stop = Arc::new(AtomicBool::new(false));

        let send_sock = sock.try_clone()?;
        let stop_tx = stop.clone();
        std::thread::Builder::new().name("discovery-tx".into()).spawn(move || {
            let dest = SocketAddr::from((Ipv4Addr::BROADCAST, DISCOVERY_PORT));
            while !stop_tx.load(Ordering::Relaxed) {
                if let Err(e) = send_sock.send_to(&payload, dest) {
                    log::debug!("beacon send failed: {e}");
                }
                std::thread::sleep(INTERVAL);
            }
        })?;

        let peers_rx = peers.clone();
        let stop_rx = stop.clone();
        std::thread::Builder::new().name("discovery-rx".into()).spawn(move || {
            let mut buf = [0u8; 1500];
            while !stop_rx.load(Ordering::Relaxed) {
                let Ok((n, from)) = sock.recv_from(&mut buf) else { continue };
                let Ok(b) = bincode::deserialize::<Beacon>(&buf[..n]) else { continue };
                if b.magic != MAGIC || b.id == id {
                    continue;
                }
                let addr = SocketAddr::new(from.ip(), b.port);
                peers_rx.lock().insert(
                    b.id,
                    Discovered { name: b.name, os: b.os, role: b.role, addr, last_seen: Instant::now() },
                );
            }
        })?;
        Ok(Discovery { peers, stop })
    }

    pub fn peers(&self) -> Vec<Discovered> {
        let mut map = self.peers.lock();
        map.retain(|_, p| p.last_seen.elapsed() < EXPIRY);
        let mut v: Vec<_> = map.values().cloned().collect();
        v.sort_by(|a, b| a.name.cmp(&b.name));
        v
    }

    pub fn servers(&self) -> Vec<Discovered> {
        self.peers().into_iter().filter(|p| p.role == Role::Server).collect()
    }
}

impl Drop for Discovery {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}
