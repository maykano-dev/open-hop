//! Meeting point: how a phone finds this computer from any network.
//!
//! Uses public Nostr relays (any of several, at the same time) as a notice
//! board. Messages are sealed with the pairing key: a relay only ever sees
//! a random topic name and unreadable bytes, and can't forge or change a
//! message. A school or company can run its own relay and list it in the
//! settings instead.
//!
//! Nostr wants every message signed; each run uses a throwaway key for that
//! (it identifies nothing). The real proof of who's talking is the seal.

use super::crypto;
use crossbeam_channel::{Receiver, Sender};
use parking_lot::Mutex;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::VecDeque;
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Ephemeral kind: relays pass it on and don't keep it.
pub const KIND: u32 = 21071;

pub const DEFAULT_RELAYS: &[&str] = &["wss://relay.damus.io", "wss://nos.lol", "wss://relay.primal.net", "wss://nostr.mom", "wss://relay.snort.social"];

/// How long a message stays believable (stops replays).
pub const FRESH_SECS: u64 = 120;

pub struct Signal {
    topic: String,
    key: [u8; 32],
    signer: k256::schnorr::SigningKey,
    outs: Vec<Sender<String>>,
    seen: Mutex<VecDeque<String>>,
    connected: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
}

pub fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

impl Signal {
    /// Listen on `topic`; `on_msg` gets each opened (and fresh) message.
    pub fn start(relays: &[String], secret: &[u8], on_msg: impl Fn(Value) + Send + Sync + 'static) -> Arc<Signal> {
        let mut sk = [0u8; 32];
        let signer = loop {
            getrandom::fill(&mut sk).expect("random numbers");
            if let Ok(k) = k256::schnorr::SigningKey::from_slice(&sk) {
                break k;
            }
        };
        let topic = crypto::topic(secret);
        let stop = Arc::new(AtomicBool::new(false));
        let connected = Arc::new(AtomicUsize::new(0));
        let mut outs = vec![];
        let (in_tx, in_rx) = crossbeam_channel::unbounded::<Value>();
        for url in relays {
            let (tx, rx) = crossbeam_channel::unbounded::<String>();
            outs.push(tx);
            let (url, topic, in_tx, stop, connected) = (url.clone(), topic.clone(), in_tx.clone(), stop.clone(), connected.clone());
            let _ = std::thread::Builder::new().name("phone-relay".into()).spawn(move || relay_loop(&url, &topic, rx, in_tx, stop, connected));
        }
        let me = Arc::new(Signal { topic, key: crypto::signal_key(secret), signer, outs, seen: Mutex::new(VecDeque::new()), connected, stop });
        let weak = Arc::downgrade(&me);
        let _ = std::thread::Builder::new().name("phone-signal".into()).spawn(move || {
            while let Ok(ev) = in_rx.recv() {
                let Some(me) = weak.upgrade() else { break };
                if let Some(msg) = me.accept(&ev) {
                    on_msg(msg);
                }
            }
        });
        me
    }

    pub fn topic(&self) -> &str {
        &self.topic
    }

    /// Relays connected right now.
    pub fn relays_up(&self) -> usize {
        self.connected.load(Ordering::Relaxed)
    }

    /// An event from a relay: ours, fresh, not seen before? Then opened.
    fn accept(&self, ev: &Value) -> Option<Value> {
        let id = ev.get("id")?.as_str()?.to_string();
        {
            let mut seen = self.seen.lock();
            if seen.contains(&id) {
                return None;
            }
            seen.push_back(id);
            while seen.len() > 512 {
                seen.pop_front();
            }
        }
        if ev.get("pubkey")?.as_str()? == crypto::hex(&self.signer.verifying_key().to_bytes()) {
            return None; // our own
        }
        let content = crypto::b64url_decode(ev.get("content")?.as_str()?)?;
        let plain = crypto::open(&self.key, self.topic.as_bytes(), &content)?;
        let msg: Value = serde_json::from_slice(&plain).ok()?;
        let ts = msg.get("ts")?.as_u64()?;
        if now_secs().abs_diff(ts) > FRESH_SECS {
            log::debug!("phone: stale message");
            return None;
        }
        Some(msg)
    }

    /// Seal and publish `msg` (a JSON object; the time is added).
    pub fn send(&self, mut msg: Value) {
        msg["ts"] = json!(now_secs());
        let content = crypto::b64url(&crypto::seal(&self.key, self.topic.as_bytes(), msg.to_string().as_bytes()));
        let ev = self.event(&content);
        let text = json!(["EVENT", ev]).to_string();
        for o in &self.outs {
            let _ = o.send(text.clone());
        }
    }

    fn event(&self, content: &str) -> Value {
        let pubkey = crypto::hex(&self.signer.verifying_key().to_bytes());
        let created = now_secs();
        let tags = json!([["t", self.topic]]);
        let ser = json!([0, pubkey, created, KIND, tags, content]).to_string();
        let id: [u8; 32] = Sha256::digest(ser.as_bytes()).into();
        let mut aux = [0u8; 32];
        getrandom::fill(&mut aux).expect("random numbers");
        let sig = self.signer.sign_raw(&id, &aux).map(|s| crypto::hex(&s.to_bytes())).unwrap_or_default();
        json!({ "id": crypto::hex(&id), "pubkey": pubkey, "created_at": created, "kind": KIND, "tags": tags, "content": content, "sig": sig })
    }
}

impl Drop for Signal {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// A stream that is plain TCP or TLS (with a read timeout either way).
fn connect(url: &str) -> Result<tungstenite::WebSocket<tungstenite::stream::MaybeTlsStream<TcpStream>>, String> {
    let req = url.parse::<tungstenite::http::Uri>().map_err(|e| e.to_string())?;
    let host = req.host().ok_or("no host")?.to_string();
    let port = req.port_u16().unwrap_or(if req.scheme_str() == Some("ws") { 80 } else { 443 });
    let addr = (host.as_str(), port).to_socket_addrs().map_err(|e| e.to_string())?.next().ok_or("no address")?;
    let tcp = TcpStream::connect_timeout(&addr, Duration::from_secs(8)).map_err(|e| e.to_string())?;
    tcp.set_read_timeout(Some(Duration::from_secs(10))).map_err(|e| e.to_string())?;
    let _ = tcp.set_nodelay(true);
    let (ws, _) = tungstenite::client_tls_with_config(url, tcp, None, None).map_err(|e| e.to_string())?;
    // Short reads from now on, so sending isn't held up.
    set_timeout(&ws, Duration::from_millis(200));
    Ok(ws)
}

fn set_timeout(ws: &tungstenite::WebSocket<tungstenite::stream::MaybeTlsStream<TcpStream>>, t: Duration) {
    use tungstenite::stream::MaybeTlsStream;
    match ws.get_ref() {
        MaybeTlsStream::Plain(s) => {
            let _ = s.set_read_timeout(Some(t));
        }
        MaybeTlsStream::Rustls(s) => {
            let _ = s.get_ref().set_read_timeout(Some(t));
        }
        _ => {}
    }
}

fn relay_loop(url: &str, topic: &str, out: Receiver<String>, events: Sender<Value>, stop: Arc<AtomicBool>, connected: Arc<AtomicUsize>) {
    let mut backoff = 2;
    while !stop.load(Ordering::Relaxed) {
        match connect(url) {
            Ok(mut ws) => {
                log::info!("phone: meeting point {url} connected");
                backoff = 2;
                connected.fetch_add(1, Ordering::Relaxed);
                let since = now_secs().saturating_sub(30);
                let req = json!(["REQ", "oh", { "kinds": [KIND], "#t": [topic], "since": since }]).to_string();
                let r = session(&mut ws, &req, &out, &events, &stop);
                connected.fetch_sub(1, Ordering::Relaxed);
                log::info!("phone: meeting point {url} closed ({r})");
                let _ = ws.close(None);
            }
            Err(e) => log::debug!("phone: meeting point {url}: {e}"),
        }
        // Wait (dropping messages meanwhile: they'd be stale).
        let until = Instant::now() + Duration::from_secs(backoff);
        while Instant::now() < until && !stop.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(250));
            while out.try_recv().is_ok() {}
        }
        backoff = (backoff * 2).min(60);
    }
}

fn session<S: Read + Write>(ws: &mut tungstenite::WebSocket<S>, req: &str, out: &Receiver<String>, events: &Sender<Value>, stop: &AtomicBool) -> String {
    use tungstenite::Message;
    if let Err(e) = ws.send(Message::text(req)) {
        return e.to_string();
    }
    let mut last_ping = Instant::now();
    loop {
        if stop.load(Ordering::Relaxed) {
            return "stopped".into();
        }
        while let Ok(m) = out.try_recv() {
            if let Err(e) = ws.send(Message::text(m)) {
                return e.to_string();
            }
        }
        if last_ping.elapsed() > Duration::from_secs(25) {
            last_ping = Instant::now();
            if let Err(e) = ws.send(Message::Ping(Vec::new().into())) {
                return e.to_string();
            }
        }
        match ws.read() {
            Ok(Message::Text(t)) => {
                if let Ok(Value::Array(a)) = serde_json::from_str::<Value>(t.as_str()) {
                    match a.first().and_then(|v| v.as_str()) {
                        Some("EVENT") => {
                            if let Some(ev) = a.get(2) {
                                let _ = events.send(ev.clone());
                            }
                        }
                        Some("NOTICE") | Some("CLOSED") => log::debug!("phone relay: {t}"),
                        Some("OK") if a.get(2) == Some(&Value::Bool(false)) => log::info!("phone relay refused a message: {t}"),
                        _ => {}
                    }
                }
            }
            Ok(Message::Close(_)) => return "closed by relay".into(),
            Ok(_) => {}
            Err(tungstenite::Error::Io(e)) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
            Err(e) => return e.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_are_signed_and_sealed() {
        let s = Signal::start(&[], b"0123456789abcdef0123456789abcdef", |_| {});
        let ev = s.event("abc");
        // The id is the hash of the canonical form, and the signature checks out.
        let ser = json!([0, ev["pubkey"], ev["created_at"], KIND, ev["tags"], "abc"]).to_string();
        let id: [u8; 32] = Sha256::digest(ser.as_bytes()).into();
        assert_eq!(ev["id"], crypto::hex(&id));
        let pk = k256::schnorr::VerifyingKey::from_slice(&crypto::unhex(ev["pubkey"].as_str().unwrap()).unwrap()).unwrap();
        let sig = k256::schnorr::Signature::try_from(&crypto::unhex(ev["sig"].as_str().unwrap()).unwrap()[..]).unwrap();
        pk.verify_raw(&id, &sig).unwrap();
        // Another Signal with the same secret opens what this one sends.
        let other = Signal::start(&[], b"0123456789abcdef0123456789abcdef", |_| {});
        let content = crypto::b64url(&crypto::seal(&s.key, s.topic.as_bytes(), json!({"t": "hi", "ts": now_secs()}).to_string().as_bytes()));
        let ev = s.event(&content);
        assert_eq!(other.accept(&ev).unwrap()["t"], "hi");
        assert!(other.accept(&ev).is_none(), "seen already");
        // Stale ones are dropped.
        let content = crypto::b64url(&crypto::seal(&s.key, s.topic.as_bytes(), json!({"t": "hi", "ts": 5}).to_string().as_bytes()));
        assert!(other.accept(&s.event(&content)).is_none());
        // A different secret can't.
        let third = Signal::start(&[], b"another secret, another phone!!!", |_| {});
        let ev = s.event(&crypto::b64url(&crypto::seal(&s.key, s.topic.as_bytes(), b"{\"ts\":1}")));
        assert!(third.accept(&ev).is_none());
    }
}
