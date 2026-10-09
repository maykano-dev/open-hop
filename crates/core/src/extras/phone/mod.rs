//! Phones (iPhone and Android) without installing anything from a store:
//! OpenHop's phone app is a web app the phone keeps on its home screen.
//!
//! Pairing: the island shows a QR code with the app's address and a secret
//! (in the part of the address that never leaves the phone). From then on:
//!
//! - Anywhere: the phone and this computer meet through public relays
//!   ([`signal`], everything sealed with the secret) and connect directly
//!   with WebRTC ([`rtc`]), with a second layer of encryption inside
//!   ([`crypto::Session`]).
//! - Same Wi-Fi, no internet: a page served by this computer ([`http`]).
//!
//! Both carry the same requests (see [`Phone::op`]): files both ways, to any
//! computer, all of them or the shelf; clipboard history; the shelf; music;
//! a trackpad; Focus, Lock All, Sleep All.

pub mod crypto;
pub mod http;
pub mod rtc;
pub mod signal;

use crate::extras::Hub;
use crate::files::TransferInfo;
use crate::platform::InjectOp;
use crate::protocol::{ClipData, MouseButton};
use parking_lot::Mutex;
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

pub use http::qr_url;

/// Where the phone app lives (HTTPS, so it can be installed).
pub const APP_URL: &str = "https://maykano-dev.github.io/open-hop/";

/// Pieces of a file on the way (fits comfortably in a WebRTC message).
pub const CHUNK: usize = 60 * 1024;

/// What the rest of OpenHop does for the phone.
pub trait PhoneHost: Send + Sync + 'static {
    fn hub(&self) -> Option<Arc<Hub>>;
    fn send_files(&self, to: &str, paths: Vec<String>);
    fn shelf_add(&self, paths: Vec<String>);
    fn shelf_take(&self, origin: &str, id: u64);
    /// Files of this computer's own shelf item.
    fn local_files(&self, offer: u64) -> Option<Vec<(PathBuf, String)>>;
    fn transfers(&self) -> Vec<TransferInfo>;
    fn event(&self, ev: PhoneEvent);
    fn name(&self) -> String;
}

#[derive(Debug, Clone)]
pub enum PhoneEvent {
    /// A file arrived from the phone (and was passed on to `to`, if any).
    Received { name: String, path: PathBuf, size: u64, device: String, to: String },
    /// Text from the phone, now on the clipboard.
    Text { text: String, device: String },
    /// The phone has a file.
    Sent { name: String, device: String },
    Connected { device: String, how: &'static str },
    Disconnected { device: String },
    /// Something is moving (the island shows it).
    Progress,
}

#[derive(Debug, Clone, Serialize)]
pub struct Offered {
    pub id: u64,
    pub name: String,
    pub size: u64,
    #[serde(skip)]
    pub path: PathBuf,
    #[serde(skip)]
    pub added: Instant,
}

/// The island's view.
#[derive(Debug, Clone, Serialize)]
pub struct PhoneState {
    /// Address in the QR code.
    pub url: String,
    pub qr: String,
    /// Same Wi-Fi only (no internet here).
    pub lan_url: String,
    pub connected: bool,
    pub device: String,
    /// "direct", "local" (same Wi-Fi page) or "".
    pub how: String,
    /// Meeting points reachable (0: phones elsewhere can't find us).
    pub relays: usize,
    pub offered: Vec<String>,
}

/// A connected phone.
pub(crate) struct Conn {
    id: u64,
    device: Mutex<String>,
    kind: ConnKind,
    /// Files coming in, by the phone's number for them.
    uploads: Mutex<HashMap<u32, Upload>>,
    seen: Mutex<Instant>,
    /// Offered files this phone has (or is getting).
    has: Mutex<Vec<u64>>,
}

pub(crate) enum ConnKind {
    Rtc { sess: u64, session: Mutex<crypto::Session> },
    Http,
}

pub(crate) struct Upload {
    pub name: String,
    pub size: u64,
    pub got: u64,
    pub to: String,
    pub tmp: PathBuf,
    pub file: Option<std::fs::File>,
    pub xfer: u64,
}

struct Pending {
    phone_nonce: Vec<u8>,
    device: String,
    born: Instant,
}

pub struct Phone {
    me: Weak<Phone>,
    host: Arc<dyn PhoneHost>,
    config_dir: PathBuf,
    dir: PathBuf,
    pairing: Mutex<crypto::Pairing>,
    relays: Vec<String>,
    app_url: String,
    signal: Mutex<Option<Arc<signal::Signal>>>,
    rtc: Mutex<Option<Arc<rtc::RtcHub>>>,
    lan: Mutex<Option<http::LanServer>>,
    conns: Mutex<Vec<Arc<Conn>>>,
    /// Connections being set up: WebRTC connection → the phone's values.
    pending: Mutex<HashMap<u64, Pending>>,
    offered: Mutex<Vec<Offered>>,
    transfers: Mutex<Vec<TransferInfo>>,
    next: AtomicU64,
    loopback: bool,
}

/// Files offered to the phone are listed this long.
const KEEP: Duration = Duration::from_secs(6 * 3600);

impl Phone {
    /// Start: the meeting point and direct connections (when there's a
    /// network), and the same-Wi-Fi page. `dir`: where files from the phone go.
    pub fn start(host: Arc<dyn PhoneHost>, config_dir: &Path, dir: PathBuf, relays: Vec<String>, app_url: Option<String>) -> Arc<Phone> {
        let pairing = crypto::Pairing::load_or_create(config_dir);
        let loopback = std::env::var_os("OPENHOP_PHONE_LOOPBACK").is_some();
        let me = Arc::new_cyclic(|me| Phone {
            me: me.clone(),
            host,
            config_dir: config_dir.to_path_buf(),
            dir,
            pairing: Mutex::new(pairing),
            relays,
            app_url: app_url.filter(|u| u.starts_with("https://") || u.starts_with("http://")).unwrap_or_else(|| APP_URL.to_string()),
            signal: Mutex::new(None),
            rtc: Mutex::new(None),
            lan: Mutex::new(None),
            conns: Mutex::new(vec![]),
            pending: Mutex::new(HashMap::new()),
            offered: Mutex::new(vec![]),
            transfers: Mutex::new(vec![]),
            next: AtomicU64::new(1),
            loopback,
        });
        me.start_links();
        me
    }

    fn start_links(self: &Arc<Self>) {
        let secret = self.pairing.lock().secret_bytes();
        let weak = self.me.clone();
        match rtc::RtcHub::start(
            move |ev| {
                if let Some(me) = weak.upgrade() {
                    me.on_rtc(ev);
                }
            },
            self.loopback,
        ) {
            Ok(r) => *self.rtc.lock() = Some(r),
            Err(e) => log::warn!("phone: no direct connections: {e}"),
        }
        let weak = self.me.clone();
        *self.signal.lock() = Some(signal::Signal::start(&self.relays, &secret, move |m| {
            if let Some(me) = weak.upgrade() {
                me.on_signal(m);
            }
        }));
        let token = self.pairing.lock().lan.clone();
        let weak = self.me.clone();
        match http::LanServer::start(&token, weak) {
            Ok(s) => *self.lan.lock() = Some(s),
            Err(e) => log::warn!("phone: no local page: {e}"),
        }
    }

    /// Phones paired before can't connect anymore (a new QR code).
    pub fn renew(self: &Arc<Self>) {
        *self.pairing.lock() = crypto::Pairing::renew(&self.config_dir);
        for c in self.conns.lock().drain(..) {
            if let (ConnKind::Rtc { sess, .. }, Some(r)) = (&c.kind, self.rtc.lock().as_ref()) {
                r.close(*sess);
            }
        }
        *self.signal.lock() = None;
        *self.lan.lock() = None;
        *self.rtc.lock() = None;
        self.start_links();
    }

    pub(crate) fn lan_token(&self) -> String {
        self.pairing.lock().lan.clone()
    }

    pub fn lan_url(&self) -> Option<String> {
        let port = self.lan.lock().as_ref()?.port();
        let ip = crate::engine::local_addresses().into_iter().next()?;
        Some(format!("http://{ip}:{port}/{}/", self.lan_token()))
    }

    /// The QR code's address: the app, with this computer's id, the secret,
    /// its name and the same-Wi-Fi address after the "#" (browsers never
    /// send that part anywhere).
    pub fn pair_url(&self) -> String {
        let p = self.pairing.lock().clone();
        let lan = self.lan_url().map(|u| u.trim_start_matches("http://").trim_end_matches('/').to_string()).unwrap_or_default();
        let name = crypto::b64url(self.host.name().as_bytes());
        let mut url = format!("{}#p={}.{}.{}.{}", self.app_url, p.id, p.secret, name, crypto::b64url(lan.as_bytes()));
        // Its own meeting points (a school's, a company's): the phone needs them too.
        if !self.relays.iter().map(String::as_str).eq(signal::DEFAULT_RELAYS.iter().copied()) {
            url.push_str(&format!("&r={}", crypto::b64url(self.relays.join(" ").as_bytes())));
        }
        url
    }

    pub fn state(&self) -> PhoneState {
        let url = self.pair_url();
        let conns = self.conns.lock();
        let live: Vec<&Arc<Conn>> = conns.iter().filter(|c| c.seen.lock().elapsed() < Duration::from_secs(12)).collect();
        let best = live.iter().find(|c| matches!(c.kind, ConnKind::Rtc { .. })).or(live.first());
        PhoneState {
            qr: qr_url(&url),
            url,
            lan_url: self.lan_url().unwrap_or_default(),
            connected: best.is_some(),
            device: best.map(|c| c.device.lock().clone()).unwrap_or_default(),
            how: best.map(|c| if matches!(c.kind, ConnKind::Rtc { .. }) { "direct" } else { "local" }.to_string()).unwrap_or_default(),
            relays: self.signal.lock().as_ref().map(|s| s.relays_up()).unwrap_or(0),
            offered: self.offered().into_iter().map(|o| o.name).collect(),
        }
    }

    /// Files moving between the phone and this computer, for the island.
    pub fn transfers(&self) -> Vec<TransferInfo> {
        let mut t = self.transfers.lock();
        t.retain(|x| !x.finished);
        t.clone()
    }

    fn xfer_start(&self, label: &str, device: &str, incoming: bool, total: u64) -> u64 {
        let id = (1 << 50) | self.next.fetch_add(1, Ordering::Relaxed);
        self.transfers.lock().push(TransferInfo { offer: id, label: label.into(), peer: device.into(), incoming, done: 0, total, finished: false, error: None });
        self.host.event(PhoneEvent::Progress);
        id
    }

    fn xfer_update(&self, id: u64, done: u64, finished: bool) {
        let mut changed = false;
        for t in self.transfers.lock().iter_mut().filter(|t| t.offer == id) {
            // The island needs to hear about every percent or so, no more.
            changed = finished || (done.saturating_sub(t.done)) * 100 >= t.total.max(1);
            t.done = done;
            t.finished |= finished;
        }
        if changed {
            self.host.event(PhoneEvent::Progress);
        }
    }

    // ------------------------------------------------------------ offered files

    /// Files for the phone (folders are skipped). Phones connected directly
    /// get them straight away. Returns how many.
    pub fn offer(self: &Arc<Self>, paths: &[PathBuf]) -> usize {
        let mut added = vec![];
        {
            let mut list = self.offered.lock();
            for p in paths {
                let Ok(meta) = std::fs::metadata(p) else { continue };
                if !meta.is_file() {
                    continue;
                }
                let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "file".into());
                list.retain(|o| o.path != *p);
                let o = Offered { id: self.next.fetch_add(1, Ordering::Relaxed), name, size: meta.len(), path: p.clone(), added: Instant::now() };
                added.push(o.clone());
                list.insert(0, o);
            }
            list.truncate(60);
        }
        for c in self.conns.lock().clone() {
            if matches!(c.kind, ConnKind::Rtc { .. }) {
                for o in &added {
                    self.stream(&c, o.clone());
                }
            }
        }
        added.len()
    }

    pub fn offered(&self) -> Vec<Offered> {
        let mut list = self.offered.lock();
        list.retain(|o| o.added.elapsed() < KEEP && o.path.is_file());
        list.clone()
    }

    pub(crate) fn offered_file(&self, id: u64) -> Option<Offered> {
        self.offered().into_iter().find(|o| o.id == id)
    }

    // ------------------------------------------------------------ connections

    fn conn_rtc(&self, sess: u64) -> Option<Arc<Conn>> {
        self.conns.lock().iter().find(|c| matches!(c.kind, ConnKind::Rtc { sess: s, .. } if s == sess)).cloned()
    }

    /// The same-Wi-Fi page's phone (one per browser, by its own id).
    pub(crate) fn http_conn(&self, key: u64, device: &str) -> Arc<Conn> {
        let mut conns = self.conns.lock();
        if let Some(c) = conns.iter().find(|c| c.id == key && matches!(c.kind, ConnKind::Http)) {
            *c.seen.lock() = Instant::now();
            return c.clone();
        }
        let c = Arc::new(Conn {
            id: key,
            device: Mutex::new(device.to_string()),
            kind: ConnKind::Http,
            uploads: Mutex::new(HashMap::new()),
            seen: Mutex::new(Instant::now()),
            has: Mutex::new(vec![]),
        });
        conns.push(c.clone());
        drop(conns);
        self.host.event(PhoneEvent::Connected { device: device.to_string(), how: "local" });
        c
    }

    fn on_signal(self: &Arc<Self>, m: Value) {
        let my_id = self.pairing.lock().id.clone();
        match m.get("t").and_then(|t| t.as_str()) {
            // A phone looking for its computers.
            Some("hi") => {
                let from = m.get("from").and_then(|v| v.as_str()).unwrap_or("").to_string();
                if let Some(s) = self.signal.lock().clone() {
                    s.send(json!({ "t": "here", "to": from, "pc": my_id, "name": self.host.name() }));
                }
            }
            Some("offer") if m.get("to").and_then(|v| v.as_str()) == Some(my_id.as_str()) => {
                let (Some(sdp), Some(from), Some(sid)) = (m.get("sdp").and_then(|v| v.as_str()), m.get("from").and_then(|v| v.as_str()), m.get("sid")) else {
                    return;
                };
                let Some(phone_nonce) = m.get("n").and_then(|v| v.as_str()).and_then(crypto::b64url_decode) else { return };
                let device = m.get("device").and_then(|v| v.as_str()).unwrap_or("Phone").chars().take(40).collect::<String>();
                match self.answer_offer(sdp, &phone_nonce, &device) {
                    Ok((answer, pc_nonce)) => {
                        if let Some(s) = self.signal.lock().clone() {
                            s.send(json!({ "t": "answer", "to": from, "pc": my_id, "sid": sid, "sdp": answer, "n": crypto::b64url(&pc_nonce) }));
                        }
                    }
                    Err(e) => log::info!("phone: couldn't answer: {e}"),
                }
            }
            _ => {}
        }
    }

    /// Answer a phone's WebRTC offer: (answer, this side's random value).
    fn answer_offer(&self, sdp: &str, phone_nonce: &[u8], device: &str) -> Result<(String, [u8; 16]), String> {
        let r = self.rtc.lock().clone().ok_or("no direct connections here")?;
        let (sess, answer) = r.accept(sdp)?;
        let mut pc_nonce = [0u8; 16];
        let _ = getrandom::fill(&mut pc_nonce);
        self.pending.lock().insert(sess, Pending { phone_nonce: [phone_nonce, &pc_nonce[..]].concat(), device: device.to_string(), born: Instant::now() });
        Ok((answer, pc_nonce))
    }

    fn on_rtc(self: &Arc<Self>, ev: rtc::RtcEvent) {
        match ev {
            rtc::RtcEvent::Open(sess) => {
                let Some(p) = self.pending.lock().remove(&sess) else { return };
                let (phone_n, pc_n) = p.phone_nonce.split_at(p.phone_nonce.len() - 16);
                let secret = self.pairing.lock().secret_bytes();
                let c = Arc::new(Conn {
                    id: (1 << 40) | sess,
                    device: Mutex::new(p.device.clone()),
                    kind: ConnKind::Rtc { sess, session: Mutex::new(crypto::Session::new(&secret, phone_n, pc_n)) },
                    uploads: Mutex::new(HashMap::new()),
                    seen: Mutex::new(Instant::now()),
                    has: Mutex::new(vec![]),
                });
                self.conns.lock().push(c.clone());
                self.pending.lock().retain(|_, v| v.born.elapsed() < Duration::from_secs(60));
                log::info!("phone: {} connected directly", p.device);
                self.host.event(PhoneEvent::Connected { device: p.device, how: "direct" });
            }
            rtc::RtcEvent::Data(sess, data) => {
                let Some(c) = self.conn_rtc(sess) else { return };
                let plain = match &c.kind {
                    ConnKind::Rtc { session, .. } => session.lock().open(&data),
                    ConnKind::Http => None,
                };
                let Some(plain) = plain else {
                    log::warn!("phone: a message that didn't check out; closing");
                    if let Some(r) = self.rtc.lock().as_ref() {
                        r.close(sess);
                    }
                    return;
                };
                *c.seen.lock() = Instant::now();
                self.on_frame(&c, &plain);
            }
            rtc::RtcEvent::Closed(sess) => {
                let gone: Vec<Arc<Conn>> = {
                    let mut conns = self.conns.lock();
                    let (gone, keep) = conns.drain(..).partition(|c| matches!(c.kind, ConnKind::Rtc { sess: s, .. } if s == sess));
                    *conns = keep;
                    gone
                };
                for c in gone {
                    for (_, u) in c.uploads.lock().drain() {
                        let _ = std::fs::remove_file(&u.tmp);
                        self.xfer_update(u.xfer, u.got, true);
                    }
                    self.host.event(PhoneEvent::Disconnected { device: c.device.lock().clone() });
                }
            }
        }
    }

    /// Send a message on a direct connection.
    fn push(&self, c: &Conn, kind: u8, body: &[u8]) {
        if let ConnKind::Rtc { sess, session } = &c.kind {
            let Some(r) = self.rtc.lock().clone() else { return };
            let mut m = Vec::with_capacity(body.len() + 1);
            m.push(kind);
            m.extend_from_slice(body);
            // Sealing and queuing together, so the numbers stay in order.
            let mut s = session.lock();
            let sealed = s.seal(&m);
            r.send(*sess, sealed);
        }
    }

    fn push_json(&self, c: &Conn, v: &Value) {
        self.push(c, 1, v.to_string().as_bytes());
    }

    fn queued(&self, c: &Conn) -> usize {
        match (&c.kind, self.rtc.lock().as_ref()) {
            (ConnKind::Rtc { sess, .. }, Some(r)) => r.queued(*sess),
            _ => 0,
        }
    }

    /// A message from a phone on a direct connection: JSON (1) or part of a
    /// file (2: the phone's number for the file, then the bytes).
    fn on_frame(self: &Arc<Self>, c: &Arc<Conn>, m: &[u8]) {
        match m.first() {
            Some(1) => {
                let Ok(req) = serde_json::from_slice::<Value>(&m[1..]) else { return };
                let rid = req.get("rid").cloned();
                let mut out = self.op(c, &req);
                if let Some(rid) = rid {
                    out["re"] = rid;
                    self.push_json(c, &out);
                }
            }
            Some(2) if m.len() >= 5 => {
                let x = u32::from_be_bytes([m[1], m[2], m[3], m[4]]);
                self.upload_bytes(c, x, &m[5..]);
            }
            _ => {}
        }
    }

    // ------------------------------------------------------------ uploads

    /// A file starts coming in. `Some(name)`: it was empty, and is done.
    pub(crate) fn upload_begin(&self, c: &Conn, x: u32, name: &str, size: u64, to: &str) -> Result<Option<String>, String> {
        let name = http::clean_name(name);
        std::fs::create_dir_all(&self.dir).map_err(|e| e.to_string())?;
        let tmp = self.dir.join(format!(".{}.{}.part", name, crate::config::random_hex(4)));
        let file = std::fs::File::create(&tmp).map_err(|e| e.to_string())?;
        let device = c.device.lock().clone();
        let xfer = self.xfer_start(&name, &device, true, size);
        let mut u = Upload { name, size, got: 0, to: to.to_string(), tmp, file: Some(file), xfer };
        if size == 0 {
            return Ok(self.upload_finish(c, &mut u));
        }
        c.uploads.lock().insert(x, u);
        Ok(None)
    }

    pub(crate) fn upload_bytes(&self, c: &Conn, x: u32, data: &[u8]) {
        let done = {
            let mut ups = c.uploads.lock();
            let Some(u) = ups.get_mut(&x) else { return };
            if let Some(f) = u.file.as_mut() {
                if f.write_all(data).is_err() {
                    u.file = None;
                }
            }
            u.got += data.len() as u64;
            self.xfer_update(u.xfer, u.got, false);
            if u.got >= u.size {
                ups.remove(&x)
            } else {
                None
            }
        };
        if let Some(mut u) = done {
            let ok = u.file.is_some() && u.got == u.size;
            if ok {
                let saved = self.upload_finish(c, &mut u);
                self.push_json(c, &json!({ "push": "put_done", "x": x, "ok": saved.is_some(), "name": saved.unwrap_or_default() }));
            } else {
                let _ = std::fs::remove_file(&u.tmp);
                self.xfer_update(u.xfer, u.got, true);
                self.push_json(c, &json!({ "push": "put_done", "x": x, "ok": false }));
            }
        }
    }

    /// Put an arrived file in place and pass it on. Returns its name.
    pub(crate) fn upload_finish(&self, c: &Conn, u: &mut Upload) -> Option<String> {
        if let Some(mut f) = u.file.take() {
            let _ = f.flush();
        }
        let dest = http::free_path(&self.dir, &u.name);
        self.xfer_update(u.xfer, u.size, true);
        if std::fs::rename(&u.tmp, &dest).is_err() {
            let _ = std::fs::remove_file(&u.tmp);
            return None;
        }
        let name = dest.file_name().unwrap_or_default().to_string_lossy().into_owned();
        let p = dest.to_string_lossy().into_owned();
        let me = self.host.name();
        match u.to.as_str() {
            "" | "here" => {}
            "shelf" => self.host.shelf_add(vec![p]),
            to if to == me => {}
            to => self.host.send_files(to, vec![p]),
        }
        log::info!("phone sent {} ({} bytes{})", dest.display(), u.size, if u.to.is_empty() { String::new() } else { format!(", for {}", u.to) });
        self.host.event(PhoneEvent::Received { name: name.clone(), path: dest, size: u.size, device: c.device.lock().clone(), to: u.to.clone() });
        Some(name)
    }

    // ------------------------------------------------------------ downloads

    /// Send a file to a phone on a direct connection, in the background.
    fn stream(self: &Arc<Self>, c: &Arc<Conn>, o: Offered) {
        {
            let mut has = c.has.lock();
            if has.contains(&o.id) {
                return;
            }
            has.push(o.id);
        }
        let (me, c) = (self.clone(), c.clone());
        let _ = std::thread::Builder::new().name("phone-send".into()).spawn(move || {
            let device = c.device.lock().clone();
            let x = (o.id & 0xffff_ffff) as u32;
            let mime = http::mime_of(&o.name);
            me.push_json(&c, &json!({ "push": "file", "x": x, "id": o.id, "name": o.name, "size": o.size, "mime": mime }));
            let xfer = me.xfer_start(&o.name, &device, false, o.size);
            let mut sent = 0u64;
            let ok = (|| -> std::io::Result<bool> {
                let mut f = std::fs::File::open(&o.path)?;
                let mut buf = vec![0u8; CHUNK];
                loop {
                    let n = f.read(&mut buf)?;
                    if n == 0 {
                        return Ok(true);
                    }
                    // Don't run ahead of the connection.
                    let mut waited = 0;
                    while me.queued(&c) > rtc::HIGH_WATER {
                        std::thread::sleep(Duration::from_millis(15));
                        waited += 1;
                        if waited > 4000 || !me.conns.lock().iter().any(|x| Arc::ptr_eq(x, &c)) {
                            return Ok(false);
                        }
                    }
                    let mut m = x.to_be_bytes().to_vec();
                    m.extend_from_slice(&buf[..n]);
                    me.push(&c, 2, &m);
                    sent += n as u64;
                    me.xfer_update(xfer, sent, false);
                }
            })()
            .unwrap_or(false);
            me.push_json(&c, &json!({ "push": "file_end", "x": x, "ok": ok }));
            me.xfer_update(xfer, sent, true);
            if ok {
                me.host.event(PhoneEvent::Sent { name: o.name, device });
            } else {
                c.has.lock().retain(|i| *i != o.id);
            }
        });
    }

    // ------------------------------------------------------------ requests

    /// Everything the phone asks for. Answers JSON.
    pub(crate) fn op(self: &Arc<Self>, c: &Arc<Conn>, req: &Value) -> Value {
        let s = |k: &str| req.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
        let n = |k: &str| req.get(k).and_then(|v| v.as_f64()).unwrap_or(0.0);
        let hub = self.host.hub();
        let off = || json!({ "ok": false, "error": "Turn OpenHop on on this computer first." });
        match s("op").as_str() {
            "hello" => {
                let d = s("device");
                if !d.is_empty() {
                    *c.device.lock() = d.chars().take(40).collect();
                }
                json!({ "ok": true, "me": self.host.name(), "pc": self.pairing.lock().id, "version": env!("CARGO_PKG_VERSION") })
            }
            "state" => self.full_state(c, hub.as_deref()),
            "put" => {
                let x = n("x") as u32;
                match self.upload_begin(c, x, &s("name"), n("size") as u64, &s("to")) {
                    Ok(Some(name)) => {
                        self.push_json(c, &json!({ "push": "put_done", "x": x, "ok": true, "name": name }));
                        json!({ "ok": true })
                    }
                    Ok(None) => json!({ "ok": true }),
                    Err(e) => json!({ "ok": false, "error": e }),
                }
            }
            "get" => {
                let id = n("id") as u64;
                match self.offered_file(id) {
                    Some(o) => {
                        c.has.lock().retain(|i| *i != id);
                        self.stream(c, o);
                        json!({ "ok": true })
                    }
                    None => json!({ "ok": false, "error": "That file isn't shared anymore." }),
                }
            }
            "got" => {
                // The phone has it (the same-Wi-Fi page downloads by itself).
                let id = n("id") as u64;
                c.has.lock().push(id);
                if let Some(o) = self.offered_file(id) {
                    self.host.event(PhoneEvent::Sent { name: o.name, device: c.device.lock().clone() });
                }
                json!({ "ok": true })
            }
            "forget" => {
                let id = n("id") as u64;
                self.offered.lock().retain(|o| o.id != id);
                json!({ "ok": true })
            }
            "clip_set" => {
                let text = s("text");
                let Some(h) = hub else { return off() };
                if text.is_empty() {
                    return json!({ "ok": false });
                }
                let ok = h.set_clipboard(ClipData::Text(text.clone()));
                h.clip_seen(&c.device.lock(), &ClipData::Text(text.clone()));
                self.host.event(PhoneEvent::Text { text, device: c.device.lock().clone() });
                json!({ "ok": ok })
            }
            "clip_use" => {
                let Some(h) = hub else { return off() };
                h.clip_use(n("id") as u64);
                json!({ "ok": true })
            }
            "media" => {
                use crate::extras::media::MediaCmd;
                let Some(h) = hub else { return off() };
                let cmd = match s("cmd").as_str() {
                    "toggle" => MediaCmd::PlayPause,
                    "next" => MediaCmd::Next,
                    "previous" => MediaCmd::Previous,
                    "shuffle" => MediaCmd::Shuffle,
                    "seek" => MediaCmd::Seek(n("at").max(0.0)),
                    _ => return json!({ "ok": false }),
                };
                let on = s("on");
                h.media_cmd(if on.is_empty() { h.me() } else { &on }, cmd);
                json!({ "ok": true })
            }
            "focus" => {
                let Some(h) = hub else { return off() };
                h.set_focus(req.get("on").and_then(|v| v.as_bool()).unwrap_or(false));
                json!({ "ok": true })
            }
            "lock_all" => {
                let Some(h) = hub else { return off() };
                h.lock_all();
                json!({ "ok": true })
            }
            "sleep_all" => {
                let Some(h) = hub else { return off() };
                h.sleep_all();
                json!({ "ok": true })
            }
            "open_url" => {
                let url = s("url");
                if !(url.starts_with("https://") || url.starts_with("http://")) {
                    return json!({ "ok": false, "error": "Only web links can be opened." });
                }
                json!({ "ok": opener::open_browser(&url).is_ok() })
            }
            "shelf_get" => {
                let origin = s("origin");
                let id = n("id") as u64;
                if origin == self.host.name() {
                    let files = self.host.local_files(id).unwrap_or_default();
                    let paths: Vec<PathBuf> = files.into_iter().map(|(p, _)| p).collect();
                    let k = self.offer(&paths);
                    json!({ "ok": k > 0, "ids": self.offered().iter().take(k).map(|o| o.id).collect::<Vec<_>>() })
                } else {
                    // It comes to this computer first, then the phone.
                    self.host.shelf_take(&origin, id);
                    self.watch_for_shelf(origin, id);
                    json!({ "ok": true, "later": true })
                }
            }
            "shelf_put_offered" => json!({ "ok": false }),
            "mouse" | "click" | "scroll" | "key" | "type" => {
                let Some(h) = hub else { return off() };
                let ops = match self.input_ops(&h, req) {
                    Some(o) => o,
                    None => return json!({ "ok": false }),
                };
                if h.inject(&ops) {
                    json!({ "ok": true })
                } else {
                    json!({ "ok": false, "error": "This computer can't be controlled from the phone." })
                }
            }
            "ping" => json!({ "ok": true }),
            _ => json!({ "ok": false, "error": "unknown request" }),
        }
    }

    /// Once a shelf item fetched from another computer has landed here,
    /// offer it to the phone.
    fn watch_for_shelf(self: &Arc<Self>, origin: String, id: u64) {
        let Some(h) = self.host.hub() else { return };
        let Some(item) = h.shelf_item(&origin, id) else { return };
        let me = self.clone();
        let root = self.dir.clone();
        let _ = std::thread::spawn(move || {
            let tops: Vec<String> =
                item.files.iter().filter_map(|f| f.path.split(['/', '\\']).next().map(str::to_string)).collect::<std::collections::BTreeSet<_>>().into_iter().collect();
            let started = std::time::SystemTime::now();
            for _ in 0..240 {
                std::thread::sleep(Duration::from_millis(500));
                let found: Vec<PathBuf> = tops
                    .iter()
                    .filter_map(|t| {
                        let p = root.join(t);
                        let fresh = std::fs::metadata(&p).and_then(|m| m.modified()).map(|m| m >= started - Duration::from_secs(2)).unwrap_or(false);
                        fresh.then_some(p)
                    })
                    .collect();
                if found.len() == tops.len() {
                    // Give the last file a moment to be complete.
                    std::thread::sleep(Duration::from_millis(500));
                    let mut files = vec![];
                    for p in found {
                        if p.is_dir() {
                            if let Ok(entries) = crate::files::collect(&[p]) {
                                files.extend(entries.into_iter().map(|e| e.abs));
                            }
                        } else {
                            files.push(p);
                        }
                    }
                    me.offer(&files);
                    return;
                }
            }
        });
    }

    fn input_ops(&self, _h: &Hub, req: &Value) -> Option<Vec<InjectOp>> {
        let n = |k: &str| req.get(k).and_then(|v| v.as_f64()).unwrap_or(0.0);
        let s = |k: &str| req.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
        let mut ops = vec![];
        match s("op").as_str() {
            "mouse" => {
                let (x, y) = crate::platform::cursor_pos()?;
                ops.push(InjectOp::MoveTo(x + n("dx").round() as i32, y + n("dy").round() as i32));
            }
            "click" => {
                let b = if s("b") == "right" { MouseButton::Right } else { MouseButton::Left };
                for _ in 0..(n("n") as usize).clamp(1, 2) {
                    ops.push(InjectOp::Button(b, true));
                    ops.push(InjectOp::Button(b, false));
                }
            }
            "scroll" => ops.push(InjectOp::Wheel(n("dx").round() as i32, n("dy").round() as i32)),
            "key" => {
                let hid = match s("k").as_str() {
                    "enter" => 0x28,
                    "esc" => 0x29,
                    "backspace" => 0x2A,
                    "tab" => 0x2B,
                    "space" => 0x2C,
                    "right" => 0x4F,
                    "left" => 0x50,
                    "down" => 0x51,
                    "up" => 0x52,
                    _ => return None,
                };
                ops.push(InjectOp::Key(hid, true));
                ops.push(InjectOp::Key(hid, false));
            }
            "type" => {
                // Typed by pasting: works for every language and symbol.
                let h = self.host.hub()?;
                let text = s("text");
                if text.is_empty() || !h.set_clipboard(ClipData::Text(text)) {
                    return None;
                }
                std::thread::sleep(Duration::from_millis(120));
                let m = if cfg!(target_os = "macos") { crate::keys::HID_LMETA } else { crate::keys::HID_LCTRL };
                ops.extend([InjectOp::Key(m, true), InjectOp::Key(0x19, true), InjectOp::Key(0x19, false), InjectOp::Key(m, false)]);
            }
            _ => return None,
        }
        Some(ops)
    }

    fn full_state(&self, c: &Conn, hub: Option<&Hub>) -> Value {
        let me = self.host.name();
        let Some(h) = hub else {
            return json!({ "ok": true, "me": me, "running": false, "pcs": [{ "name": me, "this": true }], "offered": self.offered() });
        };
        let pcs: Vec<Value> = h
            .computers()
            .into_iter()
            .map(|p| json!({ "name": p.name, "this": p.this, "os": p.status.os, "battery": p.status.battery, "locked": p.status.locked, "focus": p.status.focus }))
            .collect();
        let shelf: Vec<Value> = h
            .shelf()
            .into_iter()
            .map(|e| json!({ "origin": e.origin, "id": e.item.id, "label": e.item.label, "size": e.item.size, "count": e.item.files.len() }))
            .collect();
        let clips: Vec<Value> = h
            .clip_history()
            .into_iter()
            .take(60)
            .map(|i| json!({ "id": i.id, "kind": i.kind, "text": i.text, "thumb": i.thumb, "from": i.from, "at": i.at, "pinned": i.pinned }))
            .collect();
        let media: Vec<Value> = h
            .media()
            .into_iter()
            .map(|(name, mut now)| {
                if now.art.as_ref().map(|a| a.len() > 300_000).unwrap_or(false) {
                    now.art = None;
                }
                json!({ "name": name, "now": now })
            })
            .collect();
        let s = h.settings();
        let mut transfers = self.host.transfers();
        transfers.retain(|t| !t.finished);
        json!({
            "ok": true,
            "running": true,
            "me": me,
            "pcs": pcs,
            "shelf": shelf,
            "clips": clips,
            "media": media,
            "focus": h.focus(),
            "features": { "focus": s.allow_focus, "lock": s.allow_lock, "sleep": s.allow_sleep, "control": s.allow_control },
            "transfers": transfers,
            "offered": self.offered(),
            "has": c.has.lock().clone(),
        })
    }
}

#[cfg(test)]
mod tests;
