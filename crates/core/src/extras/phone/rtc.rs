//! The phone's direct connection: WebRTC (what video calls use), so it works
//! on the same Wi-Fi and, most of the time, across networks too: both sides
//! learn how the internet sees them (STUN) and reach each other through
//! their routers. It's encrypted on its own (DTLS); the phone link adds a
//! second layer on top (see `crypto::Session`).
//!
//! Sans-IO (str0m): this module owns the UDP sockets and the clock.

use crossbeam_channel::{select, Receiver, Sender};
use parking_lot::Mutex;
use std::collections::{HashMap, VecDeque};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, ToSocketAddrs, UdpSocket};
use std::sync::Arc;
use std::time::{Duration, Instant};
use str0m::channel::ChannelId;
use str0m::change::SdpOffer;
use str0m::net::{Protocol, Receive};
use str0m::{Candidate, Event, IceConnectionState, Input, Output, Rtc};

pub const STUN_SERVERS: &[&str] = &["stun.l.google.com:19302", "stun.cloudflare.com:3478", "stun1.l.google.com:19302"];

/// Bytes waiting in a connection before writers should hold off.
pub const HIGH_WATER: usize = 4 << 20;

#[derive(Debug)]
pub enum RtcEvent {
    Open(u64),
    Data(u64, Vec<u8>),
    Closed(u64),
}

enum Cmd {
    Offer { sdp: String, reply: Sender<Result<(u64, String), String>> },
    Send(u64, Vec<u8>),
    Close(u64),
}

pub struct RtcHub {
    tx: Sender<Cmd>,
    queued: Arc<Mutex<HashMap<u64, usize>>>,
}

struct Sess {
    id: u64,
    rtc: Rtc,
    channel: Option<ChannelId>,
    out: VecDeque<Vec<u8>>,
    born: Instant,
    open: bool,
}

struct Net {
    sockets: Vec<UdpSocket>,
    /// Our public address per socket (from STUN).
    reflexive: HashMap<SocketAddr, SocketAddr>,
    stun_ids: HashMap<[u8; 12], SocketAddr>,
    stun_asked: Option<Instant>,
}

impl RtcHub {
    /// `loopback`: also accept connections on 127.0.0.1 (tests).
    pub fn start(on_event: impl Fn(RtcEvent) + Send + 'static, loopback: bool) -> std::io::Result<Arc<RtcHub>> {
        static CRYPTO: std::sync::Once = std::sync::Once::new();
        CRYPTO.call_once(|| str0m::crypto::from_feature_flags().install_process_default());
        let mut ips: Vec<IpAddr> = crate::engine::local_addresses().iter().filter_map(|a| a.parse().ok()).collect();
        if loopback || ips.is_empty() {
            ips.push(IpAddr::V4(Ipv4Addr::LOCALHOST));
        }
        let mut sockets = vec![];
        for ip in ips {
            match UdpSocket::bind(SocketAddr::new(ip, 0)) {
                Ok(s) => sockets.push(s),
                Err(e) => log::debug!("phone: no UDP on {ip}: {e}"),
            }
        }
        if sockets.is_empty() {
            return Err(std::io::Error::other("no network"));
        }
        let (pkt_tx, pkt_rx) = crossbeam_channel::bounded::<(SocketAddr, SocketAddr, Vec<u8>)>(4096);
        for s in &sockets {
            let s = s.try_clone()?;
            let local = s.local_addr()?;
            let tx = pkt_tx.clone();
            std::thread::Builder::new().name("phone-udp".into()).spawn(move || {
                let mut buf = vec![0u8; 2048];
                loop {
                    match s.recv_from(&mut buf) {
                        Ok((n, from)) => {
                            if tx.send((local, from, buf[..n].to_vec())).is_err() {
                                break;
                            }
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => {} // Windows: an earlier send bounced
                        Err(_) => break,
                    }
                }
            })?;
        }
        let (tx, rx) = crossbeam_channel::unbounded();
        let queued = Arc::new(Mutex::new(HashMap::new()));
        let q = queued.clone();
        let net = Net { sockets, reflexive: HashMap::new(), stun_ids: HashMap::new(), stun_asked: None };
        std::thread::Builder::new().name("phone-rtc".into()).spawn(move || run(net, rx, pkt_rx, q, on_event))?;
        Ok(Arc::new(RtcHub { tx, queued }))
    }

    /// Answer a phone's offer: (connection, answer).
    pub fn accept(&self, sdp: &str) -> Result<(u64, String), String> {
        let (reply, r) = crossbeam_channel::bounded(1);
        self.tx.send(Cmd::Offer { sdp: sdp.to_string(), reply }).map_err(|_| "stopped".to_string())?;
        r.recv_timeout(Duration::from_secs(5)).map_err(|_| "no answer".to_string())?
    }

    pub fn send(&self, sess: u64, data: Vec<u8>) {
        *self.queued.lock().entry(sess).or_default() += data.len();
        let _ = self.tx.send(Cmd::Send(sess, data));
    }

    /// Bytes not yet on their way.
    pub fn queued(&self, sess: u64) -> usize {
        self.queued.lock().get(&sess).copied().unwrap_or(0)
    }

    pub fn close(&self, sess: u64) {
        let _ = self.tx.send(Cmd::Close(sess));
    }
}

fn stun_request(id: &[u8; 12]) -> Vec<u8> {
    let mut v = vec![0x00, 0x01, 0x00, 0x00, 0x21, 0x12, 0xA4, 0x42];
    v.extend_from_slice(id);
    v
}

/// Our address as the STUN server saw it.
fn stun_mapped(p: &[u8]) -> Option<([u8; 12], SocketAddr)> {
    if p.len() < 20 || p[0..2] != [0x01, 0x01] || p[4..8] != [0x21, 0x12, 0xA4, 0x42] {
        return None;
    }
    let id: [u8; 12] = p[8..20].try_into().ok()?;
    let mut i = 20;
    while i + 4 <= p.len() {
        let t = u16::from_be_bytes([p[i], p[i + 1]]);
        let l = u16::from_be_bytes([p[i + 2], p[i + 3]]) as usize;
        let v = p.get(i + 4..i + 4 + l)?;
        if (t == 0x0020 || t == 0x0001) && l >= 8 && v[1] == 0x01 {
            let xor = t == 0x0020;
            let port = u16::from_be_bytes([v[2], v[3]]) ^ if xor { 0x2112 } else { 0 };
            let mut a = [v[4], v[5], v[6], v[7]];
            if xor {
                for (k, b) in a.iter_mut().enumerate() {
                    *b ^= [0x21, 0x12, 0xA4, 0x42][k];
                }
            }
            return Some((id, SocketAddr::new(IpAddr::from(a), port)));
        }
        i += 4 + l.div_ceil(4) * 4;
    }
    None
}

impl Net {
    fn socket_for(&self, src: SocketAddr) -> &UdpSocket {
        self.sockets.iter().find(|s| s.local_addr().ok() == Some(src)).unwrap_or(&self.sockets[0])
    }

    /// Ask the STUN servers how the internet sees each socket.
    fn ask_stun(&mut self) {
        self.stun_asked = Some(Instant::now());
        let servers: Vec<SocketAddr> = STUN_SERVERS.iter().filter_map(|s| s.to_socket_addrs().ok()?.find(|a| a.is_ipv4())).collect();
        for s in &self.sockets {
            let Ok(local) = s.local_addr() else { continue };
            if local.ip().is_loopback() {
                continue;
            }
            for srv in &servers {
                let mut id = [0u8; 12];
                let _ = getrandom::fill(&mut id);
                self.stun_ids.insert(id, local);
                let _ = s.send_to(&stun_request(&id), srv);
            }
        }
    }

    fn candidates(&self) -> Vec<Candidate> {
        let mut v = vec![];
        for s in &self.sockets {
            let Ok(local) = s.local_addr() else { continue };
            if let Ok(c) = Candidate::host(local, "udp") {
                v.push(c);
            }
            if let Some(public) = self.reflexive.get(&local) {
                if *public != local {
                    if let Ok(c) = Candidate::server_reflexive(*public, local, "udp") {
                        v.push(c);
                    }
                }
            }
        }
        v
    }
}

fn run(
    mut net: Net,
    cmds: Receiver<Cmd>,
    pkts: Receiver<(SocketAddr, SocketAddr, Vec<u8>)>,
    queued: Arc<Mutex<HashMap<u64, usize>>>,
    on_event: impl Fn(RtcEvent),
) {
    let mut sessions: Vec<Sess> = vec![];
    let mut next_id = 1u64;
    // STUN can take a moment (DNS first): in the background.
    let stun_thread = {
        let servers: Vec<String> = STUN_SERVERS.iter().map(|s| s.to_string()).collect();
        let (t, r) = crossbeam_channel::bounded(1);
        std::thread::spawn(move || {
            let ok = servers.iter().any(|s| s.to_socket_addrs().is_ok());
            let _ = t.send(ok);
        });
        r
    };
    let mut stun_ready = false;
    loop {
        if !stun_ready {
            if let Ok(ok) = stun_thread.try_recv() {
                stun_ready = true;
                if ok {
                    net.ask_stun();
                }
            }
        }
        // Refresh our public address now and then (routers forget).
        if net.stun_asked.map(|t| t.elapsed() > Duration::from_secs(240)).unwrap_or(false) {
            net.ask_stun();
        }

        // Everything each connection wants to send; when it next needs the clock.
        let mut wake = Instant::now() + Duration::from_millis(200);
        for s in sessions.iter_mut() {
            flush(s);
            loop {
                if !s.rtc.is_alive() {
                    break;
                }
                match s.rtc.poll_output() {
                    Ok(Output::Timeout(t)) => {
                        wake = wake.min(t);
                        break;
                    }
                    Ok(Output::Transmit(t)) => {
                        let _ = net.socket_for(t.source).send_to(&t.contents, t.destination);
                    }
                    Ok(Output::Event(e)) => match e {
                        Event::ChannelOpen(cid, label) => {
                            log::info!("phone connection {} open ({label})", s.id);
                            s.channel = Some(cid);
                            if let Some(mut ch) = s.rtc.channel(cid) {
                                ch.set_buffered_amount_low_threshold(512 * 1024);
                            }
                            s.open = true;
                            on_event(RtcEvent::Open(s.id));
                        }
                        Event::ChannelData(d) => on_event(RtcEvent::Data(s.id, d.data)),
                        Event::ChannelBufferedAmountLow(_) => flush(s),
                        Event::ChannelClose(_) => s.rtc.disconnect(),
                        Event::IceConnectionStateChange(IceConnectionState::Disconnected) => {
                            log::info!("phone connection {} lost", s.id);
                            s.rtc.disconnect();
                        }
                        _ => {}
                    },
                    Err(e) => {
                        log::debug!("phone connection {}: {e}", s.id);
                        s.rtc.disconnect();
                        break;
                    }
                }
            }
            // Never connected: give up after a while.
            if !s.open && s.born.elapsed() > Duration::from_secs(40) {
                s.rtc.disconnect();
            }
        }
        sessions.retain(|s| {
            if s.rtc.is_alive() {
                return true;
            }
            queued.lock().remove(&s.id);
            if s.open {
                on_event(RtcEvent::Closed(s.id));
            }
            false
        });
        {
            let mut q = queued.lock();
            for s in sessions.iter_mut() {
                let buffered = s.channel.and_then(|c| s.rtc.channel(c)).map(|mut c| c.buffered_amount()).unwrap_or(0);
                q.insert(s.id, s.out.iter().map(Vec::len).sum::<usize>() + buffered);
            }
        }

        let wait = wake.saturating_duration_since(Instant::now()).max(Duration::from_millis(1));
        select! {
            recv(cmds) -> c => match c {
                Ok(Cmd::Offer { sdp, reply }) => {
                    // Wait briefly for our public address, the first time.
                    if !stun_ready {
                        if let Ok(ok) = stun_thread.recv_timeout(Duration::from_millis(1500)) {
                            stun_ready = true;
                            if ok { net.ask_stun(); }
                        }
                    }
                    if net.reflexive.is_empty() && net.stun_asked.map(|t| t.elapsed() < Duration::from_millis(800)).unwrap_or(false) {
                        let until = Instant::now() + Duration::from_millis(800);
                        while Instant::now() < until && net.reflexive.is_empty() {
                            if let Ok((local, from, data)) = pkts.recv_timeout(Duration::from_millis(50)) {
                                on_packet(&mut net, &mut sessions, local, from, &data);
                            }
                        }
                    }
                    let r = answer(&net, &sdp, next_id);
                    let r = r.map(|(rtc, answer)| {
                        let id = next_id;
                        next_id += 1;
                        sessions.push(Sess { id, rtc, channel: None, out: VecDeque::new(), born: Instant::now(), open: false });
                        (id, answer)
                    });
                    let _ = reply.send(r);
                }
                Ok(Cmd::Send(id, data)) => {
                    if let Some(s) = sessions.iter_mut().find(|s| s.id == id) {
                        s.out.push_back(data);
                    } else {
                        queued.lock().remove(&id);
                    }
                }
                Ok(Cmd::Close(id)) => {
                    if let Some(s) = sessions.iter_mut().find(|s| s.id == id) {
                        s.rtc.disconnect();
                    }
                }
                Err(_) => return,
            },
            recv(pkts) -> p => {
                if let Ok((local, from, data)) = p {
                    on_packet(&mut net, &mut sessions, local, from, &data);
                    // Take whatever else has arrived too.
                    while let Ok((local, from, data)) = pkts.try_recv() {
                        on_packet(&mut net, &mut sessions, local, from, &data);
                    }
                }
            },
            default(wait) => {}
        }
        let now = Instant::now();
        for s in sessions.iter_mut() {
            let _ = s.rtc.handle_input(Input::Timeout(now));
        }
    }
}

fn answer(net: &Net, sdp: &str, id: u64) -> Result<(Rtc, String), String> {
    let offer = SdpOffer::from_sdp_string(sdp).map_err(|e| format!("bad offer: {e}"))?;
    let mut rtc = Rtc::builder().build(Instant::now());
    for c in net.candidates() {
        let _ = rtc.add_local_candidate(c);
    }
    let answer = rtc.sdp_api().accept_offer(offer).map_err(|e| format!("can't answer: {e}"))?;
    log::info!("phone connection {id}: answering ({} ways to reach us)", net.candidates().len());
    Ok((rtc, answer.to_sdp_string()))
}

fn on_packet(net: &mut Net, sessions: &mut [Sess], local: SocketAddr, from: SocketAddr, data: &[u8]) {
    if let Some((id, public)) = stun_mapped(data) {
        if let Some(base) = net.stun_ids.remove(&id) {
            if net.reflexive.insert(base, public) != Some(public) {
                log::info!("phone: reachable from the internet at {public} (through the router)");
            }
            return;
        }
    }
    let Ok(contents) = data.try_into() else { return };
    let input = Input::Receive(Instant::now(), Receive { proto: Protocol::Udp, source: from, destination: local, contents });
    if let Some(s) = sessions.iter_mut().find(|s| s.rtc.accepts(&input)) {
        let _ = s.rtc.handle_input(input);
    }
}

/// Hand queued messages to the connection while it has room.
fn flush(s: &mut Sess) {
    let Some(cid) = s.channel else { return };
    while let Some(m) = s.out.front() {
        let Some(mut ch) = s.rtc.channel(cid) else { return };
        if ch.buffered_amount() > (2 << 20) {
            return;
        }
        match ch.write(true, m) {
            Ok(true) => {
                s.out.pop_front();
            }
            Ok(false) => return,
            Err(e) => {
                log::debug!("phone: write failed: {e}");
                s.out.pop_front();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn stun_answer() {
        // A Binding Success with XOR-MAPPED-ADDRESS 203.0.113.5:40000.
        let id = [7u8; 12];
        let mut p = vec![0x01, 0x01, 0x00, 0x0c, 0x21, 0x12, 0xA4, 0x42];
        p.extend_from_slice(&id);
        let port = 40000u16 ^ 0x2112;
        let ip = [203 ^ 0x21, 0 ^ 0x12, 113 ^ 0xA4, 5 ^ 0x42];
        p.extend_from_slice(&[0x00, 0x20, 0x00, 0x08, 0x00, 0x01]);
        p.extend_from_slice(&port.to_be_bytes());
        p.extend_from_slice(&ip);
        let (got, addr) = super::stun_mapped(&p).unwrap();
        assert_eq!(got, id);
        assert_eq!(addr.to_string(), "203.0.113.5:40000");
        assert_eq!(super::stun_request(&id).len(), 20);
    }
}
