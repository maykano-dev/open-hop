//! The engine: runs either as a server (shares this computer's keyboard and
//! mouse) or a client (is controlled by a server).
//!
//! Design rule: the engine thread never blocks on the network. Every
//! connection has a [`Link`] (a writer thread with an input lane and a bulk
//! lane), and big payloads (clipboard images, files) are streamed by worker
//! threads with back-pressure. A peer that stops reading is dropped.

use crate::clipboard;
use crate::config::{Config, Role};
use crate::discovery::{Discovered, Discovery};
use crate::files::{self, Finished, Inbox, Outbox, TransferInfo};
use crate::keys;
use crate::layout::{apply_delta, edge_fraction, entry_point, touching_edge, Layout, Side, SERVER};
use crate::net::{derive_psk, handshake, Link, SecureReceiver, CHUNK};
use crate::platform::{self, dnd, Capture, InputEvent, Injector};
use crate::protocol::{ClipData, FileMeta, MouseButton, Msg, OfferKind, Os, Rect, DEFAULT_PORT, PROTOCOL_VERSION};
use crate::wol;
use anyhow::{anyhow, bail, Context, Result};
use crossbeam_channel::{select, tick, Receiver, Sender};
use parking_lot::Mutex;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

const PEER_TIMEOUT: Duration = Duration::from_secs(15);
/// Clipboard file offers up to this size are fetched straight away; bigger
/// ones wait until the pointer arrives on this computer.
const AUTO_FETCH_BYTES: u64 = 64 * 1024 * 1024;
const ESC: u16 = 0x29;

#[derive(Debug, Clone, Serialize)]
pub struct PeerStatus {
    pub name: String,
    pub os: Os,
    pub addr: String,
    pub screen: Rect,
}

#[derive(Debug, Clone, Serialize)]
pub struct Status {
    pub running: bool,
    pub role: Role,
    pub name: String,
    pub os: Os,
    /// One-line human-readable state.
    pub message: String,
    pub error: Option<String>,
    /// Server: which screen has the cursor ("" = this one).
    pub active: String,
    pub peers: Vec<PeerStatus>,
    pub discovered: Vec<Discovered>,
    pub layout: Layout,
    pub screen: Option<Rect>,
    pub transfers: Vec<TransferInfo>,
    /// Server: computers that can be woken with Wake-on-LAN.
    pub wakeable: Vec<String>,
}

/// A small notification for the user, with optional actions.
#[derive(Debug, Clone, Serialize)]
pub struct Note {
    pub id: u64,
    pub title: String,
    pub body: String,
    pub actions: Vec<NoteAction>,
}

#[derive(Debug, Clone, Serialize)]
pub struct NoteAction {
    pub label: String,
    /// "open_url", "open_path" or "reveal".
    pub kind: String,
    pub target: String,
}

/// Run a notification action (open a link, open a file, show it in its folder).
pub fn run_action(kind: &str, target: &str) -> Result<()> {
    match kind {
        "open_url" => {
            if !(target.starts_with("http://") || target.starts_with("https://")) {
                bail!("not a web link");
            }
            opener::open_browser(target)?
        }
        "open_path" => opener::open(target)?,
        "reveal" => opener::reveal(target)?,
        other => bail!("unknown action {other}"),
    }
    Ok(())
}

enum Control {
    Stop,
    SetLayout(Layout),
}

pub struct Engine {
    stop: Arc<AtomicBool>,
    ctl: Sender<Control>,
    status: Arc<Mutex<Status>>,
    notes: Arc<Mutex<Vec<Note>>>,
    inbox: Inbox,
    discovery: Arc<Discovery>,
    main: Option<std::thread::JoinHandle<()>>,
}

impl Engine {
    pub fn start(cfg: Config, config_path: Option<PathBuf>) -> Result<Engine> {
        if cfg.passphrase.trim().len() < 4 {
            bail!("Set a passphrase of at least 4 characters (use the same one on every computer).");
        }
        let status = Arc::new(Mutex::new(Status {
            running: true,
            role: cfg.role,
            name: cfg.name.clone(),
            os: Os::current(),
            message: "Starting…".into(),
            error: None,
            active: String::new(),
            peers: vec![],
            discovered: vec![],
            layout: cfg.layout.clone(),
            screen: None,
            transfers: vec![],
            wakeable: vec![],
        }));
        let discovery = Arc::new(Discovery::start(cfg.name.clone(), cfg.role, cfg.port).context("starting LAN discovery")?);
        let stop = Arc::new(AtomicBool::new(false));
        let (ctl_tx, ctl_rx) = crossbeam_channel::unbounded();
        let notes: Arc<Mutex<Vec<Note>>> = Default::default();
        let root = cfg.download_dir.as_ref().map(PathBuf::from).unwrap_or_else(files::download_root);
        let inbox = Inbox::new(root);
        dnd::init();

        let ctx = Ctx {
            cfg: cfg.clone(),
            config_path,
            status: status.clone(),
            discovery: discovery.clone(),
            stop: stop.clone(),
            notes: notes.clone(),
            inbox: inbox.clone(),
            outbox: Outbox::default(),
        };
        let main = match cfg.role {
            Role::Server => {
                let capture = shared_capture(cfg.screen)?;
                status.lock().screen = Some(capture.0.screen());
                let listener = TcpListener::bind(("0.0.0.0", cfg.port))
                    .with_context(|| format!("port {} is busy (is OpenHop already running?)", cfg.port))?;
                std::thread::Builder::new().name("server".into()).spawn(move || {
                    if let Err(e) = Server::run(ctx.clone(), capture, listener, ctl_rx) {
                        log::error!("server stopped: {e:#}");
                        ctx.set_error(Some(format!("{e:#}")));
                    }
                    ctx.status.lock().running = false;
                })?
            }
            Role::Client => {
                let injector = platform::create_injector(cfg.screen, &cfg.linux_backend)?;
                status.lock().screen = Some(injector.screen());
                std::thread::Builder::new().name("client".into()).spawn(move || {
                    if let Err(e) = Client::run(ctx.clone(), injector, ctl_rx) {
                        log::error!("client stopped: {e:#}");
                        ctx.set_error(Some(format!("{e:#}")));
                    }
                    ctx.status.lock().running = false;
                })?
            }
        };
        Ok(Engine { stop, ctl: ctl_tx, status, notes, inbox, discovery, main: Some(main) })
    }

    pub fn status(&self) -> Status {
        let mut s = self.status.lock().clone();
        s.discovered = self.discovery.peers();
        s.transfers = self.inbox.history();
        s
    }

    /// Notifications produced since the last call.
    pub fn take_notes(&self) -> Vec<Note> {
        std::mem::take(&mut *self.notes.lock())
    }

    /// Server: apply a new arrangement immediately (and save it).
    pub fn set_layout(&self, layout: Layout) {
        let _ = self.ctl.send(Control::SetLayout(layout));
    }

    pub fn stop(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = self.ctl.send(Control::Stop);
        if let Some(h) = self.main.take() {
            let _ = h.join();
        }
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Input hooks are process-wide, so the capture backend is created once and
/// reused if the engine is restarted.
fn shared_capture(screen: Option<Rect>) -> Result<(Arc<dyn Capture>, Receiver<InputEvent>)> {
    static CAPTURE: OnceLock<(Arc<dyn Capture>, Receiver<InputEvent>)> = OnceLock::new();
    static INIT: Mutex<()> = Mutex::new(());
    let _g = INIT.lock();
    if let Some((c, rx)) = CAPTURE.get() {
        while rx.try_recv().is_ok() {}
        return Ok((c.clone(), rx.clone()));
    }
    let (tx, rx) = crossbeam_channel::bounded(8192);
    let cap = platform::start_capture(tx, screen)?;
    let _ = CAPTURE.set((cap.clone(), rx.clone()));
    Ok((cap, rx))
}

#[derive(Clone)]
struct Ctx {
    cfg: Config,
    config_path: Option<PathBuf>,
    status: Arc<Mutex<Status>>,
    discovery: Arc<Discovery>,
    stop: Arc<AtomicBool>,
    notes: Arc<Mutex<Vec<Note>>>,
    inbox: Inbox,
    outbox: Outbox,
}

impl Ctx {
    fn set_message(&self, m: impl Into<String>) {
        self.status.lock().message = m.into();
    }
    fn set_error(&self, e: Option<String>) {
        self.status.lock().error = e;
    }
    fn stopped(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }
    fn me(&self) -> &str {
        &self.cfg.name
    }
    fn note(&self, note: Note) {
        log::info!("{}: {}", note.title, note.body);
        if self.cfg.notifications {
            let mut v = self.notes.lock();
            v.push(note);
            if v.len() > 20 {
                v.remove(0);
            }
        }
    }
    fn save_config(&self) {
        if let Some(p) = &self.config_path {
            if let Err(e) = self.cfg.save(p) {
                log::warn!("saving config: {e}");
            }
        }
    }
}

fn note(title: impl Into<String>, body: impl Into<String>, actions: Vec<NoteAction>) -> Note {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    Note { id: NEXT.fetch_add(1, Ordering::Relaxed), title: title.into(), body: body.into(), actions }
}

fn action(label: &str, kind: &str, target: impl Into<String>) -> NoteAction {
    NoteAction { label: label.into(), kind: kind.into(), target: target.into() }
}

fn should_swap(cfg: &Config, a: Os, b: Os) -> bool {
    cfg.swap_cmd_ctrl && ((a == Os::MacOs) != (b == Os::MacOs))
}

fn is_link(t: &str) -> bool {
    let t = t.trim();
    (t.starts_with("https://") || t.starts_with("http://")) && t.len() < 4096 && !t.contains(char::is_whitespace)
}

/// Turn a clipboard item into messages: small items go as one message, big
/// images are split so they can't hold up the connection.
fn clip_messages(origin: &str, data: ClipData) -> Vec<Msg> {
    match data {
        ClipData::Png(p) if p.len() > CHUNK => {
            let id = files::new_id();
            let total = p.len() as u64;
            p.chunks(CHUNK)
                .map(|c| Msg::ClipPart { origin: origin.to_string(), id, total, data: c.to_vec() })
                .collect()
        }
        data => vec![Msg::Clip { origin: origin.to_string(), data }],
    }
}

/// Send bulk messages on a worker thread (waits for room instead of dropping).
fn send_bulk_async(link: Link, msgs: Vec<Msg>) {
    let _ = std::thread::Builder::new().name("bulk-send".into()).spawn(move || {
        for m in msgs {
            if !link.send_bulk_wait(m) {
                return;
            }
        }
    });
}

/// Reassembles clipboard images that arrive in parts.
#[derive(Default)]
struct ClipAssembler {
    parts: HashMap<u64, (String, u64, Vec<u8>, Instant)>,
}

impl ClipAssembler {
    fn add(&mut self, origin: String, id: u64, total: u64, data: Vec<u8>) -> Option<(String, ClipData)> {
        self.parts.retain(|_, (_, _, _, t)| t.elapsed() < Duration::from_secs(120));
        if total > crate::net::MAX_MSG as u64 {
            return None;
        }
        let e = self.parts.entry(id).or_insert_with(|| (origin, total, Vec::with_capacity(total as usize), Instant::now()));
        e.2.extend_from_slice(&data);
        if e.2.len() as u64 >= e.1 {
            let (origin, _, bytes, _) = self.parts.remove(&id).unwrap();
            return Some((origin, ClipData::Png(bytes)));
        }
        None
    }
}

/// Logic shared by both roles: clipboard, file offers, notifications.
struct Common {
    ctx: Ctx,
    clip_set: Option<Sender<ClipData>>,
    assembler: ClipAssembler,
    /// A clipboard file offer waiting for the pointer to arrive here.
    pending_offer: Option<(u64, String, Vec<FileMeta>)>,
    /// A notification to show when the pointer arrives here.
    pending_note: Option<Note>,
}

impl Common {
    fn new(ctx: Ctx, clip_set: Option<Sender<ClipData>>) -> Common {
        Common { ctx, clip_set, assembler: ClipAssembler::default(), pending_offer: None, pending_note: None }
    }

    /// Clipboard content arrived from another computer.
    fn on_remote_clip(&mut self, origin: &str, data: ClipData, here: bool) {
        if let ClipData::Text(t) = &data {
            if is_link(t) {
                let n = note(format!("Link copied on {origin}"), t.trim(), vec![action("Open", "open_url", t.trim())]);
                if here {
                    self.ctx.note(n);
                } else {
                    self.pending_note = Some(n);
                }
            }
        }
        if let Some(s) = &self.clip_set {
            let _ = s.send(data);
        }
    }

    /// Local files were copied: make an offer. Returns the message to send.
    fn offer_local_files(&mut self, paths: &[String], kind: OfferKind) -> Option<Msg> {
        let paths: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
        match files::collect(&paths) {
            Ok(entries) => {
                let offer = files::new_id();
                let metas: Vec<FileMeta> = entries.iter().map(|e| e.meta.clone()).collect();
                log::info!("offering {} ({})", files::label_for(&metas), files::human(files::total_size(&metas)));
                self.ctx.outbox.add(offer, entries);
                Some(Msg::FileOffer { offer, origin: self.ctx.me().to_string(), kind, files: metas })
            }
            Err(e) => {
                log::warn!("can't offer copied files: {e}");
                None
            }
        }
    }

    /// Someone offered files. Returns a request to send, if we want them now.
    fn on_offer(&mut self, offer: u64, origin: String, kind: OfferKind, files: Vec<FileMeta>, here: bool) -> Option<Msg> {
        if origin == self.ctx.me() || self.ctx.inbox.is_active(offer) {
            return None;
        }
        let total = files::total_size(&files);
        if kind == OfferKind::Clipboard && !here && total > AUTO_FETCH_BYTES {
            log::info!("{} offered {} ({}); fetching when the pointer arrives", origin, files::label_for(&files), files::human(total));
            self.pending_offer = Some((offer, origin, files));
            return None;
        }
        self.pending_offer = None;
        self.request(offer, origin, kind, files)
    }

    fn request(&mut self, offer: u64, origin: String, kind: OfferKind, files: Vec<FileMeta>) -> Option<Msg> {
        if let Err(e) = self.ctx.inbox.start(offer, &origin, kind, files) {
            self.ctx.note(note("Couldn't receive files", e, vec![]));
            return None;
        }
        Some(Msg::FileRequest { offer, origin, requester: self.ctx.me().to_string() })
    }

    /// The pointer arrived on this computer.
    fn arrived(&mut self) -> Option<Msg> {
        if let Some(n) = self.pending_note.take() {
            self.ctx.note(n);
        }
        let (offer, origin, files) = self.pending_offer.take()?;
        self.request(offer, origin, OfferKind::Clipboard, files)
    }

    fn on_finished(&mut self, f: Finished) {
        let label = f
            .tops
            .first()
            .and_then(|p| p.file_name())
            .map(|n| if f.tops.len() == 1 { n.to_string_lossy().into_owned() } else { format!("{} items", f.tops.len()) })
            .unwrap_or_default();
        if let Some(e) = f.error {
            self.ctx.note(note(format!("Transfer from {} failed", f.origin), e, vec![]));
            return;
        }
        let first = f.tops.first().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
        match f.kind {
            OfferKind::Clipboard => {
                if let Some(s) = &self.clip_set {
                    let _ = s.send(ClipData::Files(f.tops.iter().map(|p| p.to_string_lossy().into_owned()).collect()));
                }
                self.ctx.note(note(
                    format!("Ready to paste: {label}"),
                    format!("Copied on {}. Paste it into any folder, or find it in Downloads › OpenHop.", f.origin),
                    vec![action("Show in folder", "reveal", first)],
                ));
            }
            OfferKind::Drop => {
                let mut actions = vec![];
                if f.tops.len() == 1 && f.tops[0].is_file() {
                    actions.push(action("Open", "open_path", first.clone()));
                }
                actions.push(action("Show in folder", "reveal", first));
                self.ctx.note(note(format!("Received {label}"), format!("From {}. Saved to Downloads › OpenHop.", f.origin), actions));
            }
        }
    }
}

// ---------------------------------------------------------------- server

enum NetEvent {
    Connected { id: u64, name: String, os: Os, screen: Rect, addr: SocketAddr, link: Link },
    Msg(u64, Msg),
    Disconnected(u64),
    Finished(Finished),
}

type Routes = Arc<Mutex<HashMap<String, Link>>>;

struct Peer {
    name: String,
    os: Os,
    screen: Rect,
    addr: SocketAddr,
    link: Link,
}

/// A file drag carried from one screen to another.
struct Drag {
    origin: String,
    offer: Option<(u64, Vec<FileMeta>)>,
    query: Option<u64>,
    /// Where it was dropped, if the drop happened before we knew the files.
    dropped_on: Option<String>,
    started: Instant,
}

struct Server {
    ctx: Ctx,
    common: Common,
    capture: Arc<dyn Capture>,
    peers: HashMap<u64, Peer>,
    routes: Routes,
    /// Which client has the cursor, and where it is on that client.
    active: Option<(u64, i32, i32)>,
    held_keys: HashSet<u16>,
    held_buttons: HashSet<MouseButton>,
    drag: Option<Drag>,
    last_wake: HashMap<String, Instant>,
}

impl Server {
    fn run(ctx: Ctx, (capture, input_rx): (Arc<dyn Capture>, Receiver<InputEvent>), listener: TcpListener, ctl_rx: Receiver<Control>) -> Result<()> {
        let (net_tx, net_rx) = crossbeam_channel::unbounded();
        let psk = derive_psk(&ctx.cfg.passphrase);
        let routes: Routes = Default::default();
        spawn_acceptor(listener, psk, ctx.clone(), routes.clone(), net_tx, ctx.stop.clone())?;
        let (clip_set, clip_rx) = if ctx.cfg.clipboard_sync {
            let (s, r) = clipboard::start();
            (Some(s), r)
        } else {
            (None, crossbeam_channel::never())
        };
        ctx.set_message(format!("Waiting for other computers… (port {})", ctx.cfg.port));
        let mut s = Server {
            common: Common::new(ctx.clone(), clip_set),
            ctx,
            capture,
            peers: HashMap::new(),
            routes,
            active: None,
            held_keys: HashSet::new(),
            held_buttons: HashSet::new(),
            drag: None,
            last_wake: HashMap::new(),
        };
        s.update_status();
        let pinger = tick(Duration::from_secs(3));
        let fast = tick(Duration::from_millis(100));
        loop {
            select! {
                recv(input_rx) -> ev => if let Ok(ev) = ev { s.on_input(ev) },
                recv(net_rx) -> ev => if let Ok(ev) = ev { s.on_net(ev) },
                recv(clip_rx) -> c => if let Ok(c) = c { s.on_local_clip(c) },
                recv(ctl_rx) -> c => match c {
                    Ok(Control::SetLayout(l)) => s.set_layout(l),
                    Ok(Control::Stop) | Err(_) => break,
                },
                recv(pinger) -> _ => s.broadcast(&Msg::Ping, None),
                recv(fast) -> _ => s.check_local_drop(),
            }
        }
        if s.active.is_some() {
            s.go_local(Side::Left, 0.5);
        }
        for p in s.peers.values() {
            p.link.close();
        }
        Ok(())
    }

    fn me(&self) -> String {
        self.ctx.cfg.name.clone()
    }

    fn update_status(&self) {
        let mut st = self.ctx.status.lock();
        st.peers = self
            .peers
            .values()
            .map(|p| PeerStatus { name: p.name.clone(), os: p.os, addr: p.addr.ip().to_string(), screen: p.screen })
            .collect();
        st.peers.sort_by(|a, b| a.name.cmp(&b.name));
        st.active = self.active.and_then(|(id, _, _)| self.peers.get(&id)).map(|p| p.name.clone()).unwrap_or_default();
        st.layout = self.ctx.cfg.layout.clone();
        st.wakeable = if self.ctx.cfg.wake_on_lan { self.ctx.cfg.macs.keys().cloned().collect() } else { vec![] };
        st.message = if self.peers.is_empty() {
            format!("Waiting for other computers… (port {})", self.ctx.cfg.port)
        } else {
            format!("Sharing with {} computer(s)", self.peers.len())
        };
    }

    fn set_layout(&mut self, l: Layout) {
        self.ctx.cfg.layout = l;
        self.ctx.save_config();
        self.update_status();
    }

    fn peer_by_name(&self, name: &str) -> Option<u64> {
        self.peers.iter().find(|(_, p)| p.name == name).map(|(&id, _)| id)
    }

    fn send(&mut self, id: u64, msg: Msg) {
        let ok = self.peers.get(&id).map(|p| p.link.send(msg)).unwrap_or(true);
        if !ok && self.peers.get(&id).map(|p| p.link.is_dead()).unwrap_or(false) {
            self.drop_peer(id);
        }
    }

    fn send_to_name(&mut self, name: &str, msg: Msg) {
        if let Some(id) = self.peer_by_name(name) {
            self.send(id, msg);
        }
    }

    fn broadcast(&mut self, msg: &Msg, except: Option<u64>) {
        let ids: Vec<u64> = self.peers.keys().copied().filter(|&i| Some(i) != except).collect();
        for id in ids {
            self.send(id, msg.clone());
        }
    }

    fn drop_peer(&mut self, id: u64) {
        if let Some(p) = self.peers.remove(&id) {
            log::info!("{} disconnected", p.name);
            p.link.close();
            {
                let mut routes = self.routes.lock();
                if routes.get(&p.name).map(|l| l.is_dead()).unwrap_or(false) {
                    routes.remove(&p.name);
                }
            }
            for f in self.ctx.inbox.abort_from(&p.name) {
                self.common.on_finished(f);
            }
            if matches!(self.active, Some((a, _, _)) if a == id) {
                self.active = None;
                self.held_keys.clear();
                self.held_buttons.clear();
                let (cx, cy) = self.capture.screen().center();
                self.capture.release(cx, cy);
            }
            self.update_status();
        }
    }

    // ---------- network

    fn on_net(&mut self, ev: NetEvent) {
        match ev {
            NetEvent::Connected { id, mut name, os, screen, addr, link } => {
                // A reconnect from the same computer replaces the stale session
                // (keeps its place in the arrangement).
                if let Some(old) = self.peer_by_name(&name) {
                    if self.peers[&old].addr.ip() == addr.ip() {
                        log::info!("{name} reconnected; replacing its previous connection");
                        let was_active = matches!(self.active, Some((a, _, _)) if a == old);
                        self.drop_peer(old);
                        if was_active {
                            log::info!("cursor returned to this computer");
                        }
                    } else {
                        name = format!("{name} ({})", addr.ip());
                    }
                }
                if name == self.me() {
                    name = format!("{name} ({})", addr.ip());
                }
                log::info!("{name} connected from {addr} ({} {}x{})", os.label(), screen.w, screen.h);
                if self.ctx.cfg.layout.position(&name).is_none() {
                    self.ctx.cfg.layout.auto_place(&name);
                }
                self.ctx.cfg.last_ips.insert(name.clone(), addr.ip().to_string());
                self.ctx.save_config();
                self.routes.lock().insert(name.clone(), link.clone());
                self.peers.insert(id, Peer { name, os, screen, addr, link });
                self.update_status();
            }
            NetEvent::Disconnected(id) => self.drop_peer(id),
            NetEvent::Finished(f) => self.common.on_finished(f),
            NetEvent::Msg(id, msg) => self.on_peer_msg(id, msg),
        }
    }

    fn on_peer_msg(&mut self, id: u64, msg: Msg) {
        let Some(from) = self.peers.get(&id).map(|p| p.name.clone()) else { return };
        let here = self.active.is_none();
        match msg {
            Msg::Screen(r) => {
                if let Some(p) = self.peers.get_mut(&id) {
                    p.screen = r;
                }
                if let Some((a, x, y)) = self.active {
                    if a == id {
                        self.active = Some((a, x.min(r.w - 1), y.min(r.h - 1)));
                    }
                }
                self.update_status();
            }
            Msg::Clip { data, .. } => self.relay_clip(from, data, Some(id)),
            Msg::ClipPart { id: part, total, data, .. } => {
                if let Some((origin, data)) = self.common.assembler.add(from, part, total, data) {
                    self.relay_clip(origin, data, Some(id));
                }
            }
            Msg::FileOffer { offer, origin, kind, files } => {
                // Clipboard offers go to everyone; the server is a destination too.
                let m = Msg::FileOffer { offer, origin: origin.clone(), kind, files: files.clone() };
                self.broadcast(&m, Some(id));
                if let Some(req) = self.common.on_offer(offer, origin.clone(), kind, files, here) {
                    self.send_to_name(&origin, req);
                }
            }
            Msg::FileRequest { offer, requester, .. } => {
                // Requests for other origins are forwarded by the reader thread.
                if let Some(link) = self.routes.lock().get(&requester).cloned() {
                    self.ctx.outbox.serve(offer, requester, link);
                }
            }
            Msg::DragReply { id: q, offer, files } => self.on_drag_reply(q, offer, files),
            Msg::Mac(mac) => {
                if wol::parse_mac(&mac).is_some() && self.ctx.cfg.macs.get(&from) != Some(&mac) {
                    self.ctx.cfg.macs.insert(from, mac);
                    self.ctx.save_config();
                    self.update_status();
                }
            }
            Msg::Ping => self.send(id, Msg::Pong),
            _ => {}
        }
    }

    fn relay_clip(&mut self, origin: String, data: ClipData, from: Option<u64>) {
        let here = self.active.is_none();
        let links: Vec<Link> = self.peers.iter().filter(|(&i, _)| Some(i) != from).map(|(_, p)| p.link.clone()).collect();
        for l in links {
            send_bulk_async(l, clip_messages(&origin, data.clone()));
        }
        self.common.on_remote_clip(&origin, data, here);
    }

    fn on_local_clip(&mut self, c: ClipData) {
        match c {
            ClipData::Files(paths) => {
                if let Some(offer) = self.common.offer_local_files(&paths, OfferKind::Clipboard) {
                    self.broadcast(&offer, None);
                }
            }
            data => {
                let me = self.me();
                let links: Vec<Link> = self.peers.values().map(|p| p.link.clone()).collect();
                for l in links {
                    send_bulk_async(l, clip_messages(&me, data.clone()));
                }
            }
        }
    }

    // ---------- input

    fn on_input(&mut self, ev: InputEvent) {
        match (ev, self.active) {
            (InputEvent::LocalMove { x, y }, None) => {
                let screen = self.capture.screen();
                let Some(side) = touching_edge(&screen, x, y) else { return };
                let Some(target) = self.ctx.cfg.layout.neighbor(SERVER, side).map(str::to_string) else { return };
                let frac = edge_fraction(&screen, side, x, y);
                match self.peer_by_name(&target) {
                    Some(id) => {
                        self.start_local_drag_if_any();
                        self.enter(id, side, frac);
                    }
                    None => self.maybe_wake(&target),
                }
            }
            (InputEvent::Delta { dx, dy }, Some((id, x, y))) => {
                let Some(p) = self.peers.get(&id) else { return };
                let (w, h, pname) = (p.screen.w, p.screen.h, p.name.clone());
                match apply_delta(w, h, x, y, dx, dy) {
                    Ok((nx, ny)) => {
                        self.active = Some((id, nx, ny));
                        self.send(id, Msg::Move { x: nx, y: ny });
                    }
                    Err((side, frac)) => {
                        let neighbor = self.ctx.cfg.layout.neighbor(&pname, side).map(str::to_string);
                        match neighbor.as_deref() {
                            Some(SERVER) => {
                                self.start_remote_drag_if_any(id, &pname);
                                self.go_local(side, frac);
                            }
                            Some(n) if self.peer_by_name(n).is_some() => {
                                let next = self.peer_by_name(n).unwrap();
                                self.start_remote_drag_if_any(id, &pname);
                                self.enter(next, side, frac);
                            }
                            other => {
                                if let Some(n) = other {
                                    let n = n.to_string();
                                    self.maybe_wake(&n);
                                }
                                let nx = (x + dx).clamp(0, w - 1);
                                let ny = (y + dy).clamp(0, h - 1);
                                if (nx, ny) != (x, y) {
                                    self.active = Some((id, nx, ny));
                                    self.send(id, Msg::Move { x: nx, y: ny });
                                }
                            }
                        }
                    }
                }
            }
            (InputEvent::Button { button, down }, Some((id, _, _))) => {
                if button == MouseButton::Left && !down && self.drag.is_some() && !self.held_buttons.contains(&button) {
                    // Releasing a file drag that was carried onto this screen.
                    let target = self.peers.get(&id).map(|p| p.name.clone()).unwrap_or_default();
                    self.drop_drag(target);
                    return;
                }
                if !down && !self.held_buttons.contains(&button) {
                    return; // pressed on another screen; nothing to release here
                }
                if down {
                    self.held_buttons.insert(button);
                } else {
                    self.held_buttons.remove(&button);
                }
                self.send(id, Msg::Button { button, down });
            }
            (InputEvent::Wheel { dx, dy }, Some((id, _, _))) => self.send(id, Msg::Wheel { dx, dy }),
            (InputEvent::Key { key, down }, Some((id, _, _))) => {
                let os = self.peers.get(&id).map(|p| p.os).unwrap_or(Os::Other);
                let key = if should_swap(&self.ctx.cfg, Os::current(), os) { keys::swap_ctrl_meta(key) } else { key };
                if down {
                    self.held_keys.insert(key);
                } else if !self.held_keys.remove(&key) {
                    return;
                }
                self.send(id, Msg::Key { key, down });
            }
            _ => {}
        }
    }

    fn maybe_wake(&mut self, name: &str) {
        if !self.ctx.cfg.wake_on_lan {
            return;
        }
        let Some(mac) = self.ctx.cfg.macs.get(name).cloned() else { return };
        if self.last_wake.get(name).map(|t| t.elapsed() < Duration::from_secs(20)).unwrap_or(false) {
            return;
        }
        self.last_wake.insert(name.to_string(), Instant::now());
        let ip = self.ctx.cfg.last_ips.get(name).and_then(|s| s.parse::<IpAddr>().ok());
        if wol::wake(&mac, ip) {
            self.ctx.note(note(format!("Waking {name}…"), "It will appear here once it's awake.", vec![]));
        }
    }

    /// Lift every key and button the active client thinks is held.
    fn release_held(&mut self, id: u64) {
        let keys: Vec<u16> = self.held_keys.drain().collect();
        for key in keys {
            self.send(id, Msg::Key { key, down: false });
        }
        let buttons: Vec<MouseButton> = self.held_buttons.drain().collect();
        for button in buttons {
            self.send(id, Msg::Button { button, down: false });
        }
    }

    fn enter(&mut self, id: u64, exit_side: Side, frac: f64) {
        match self.active {
            None => {
                if !self.capture.grab() {
                    log::warn!("couldn't capture the mouse and keyboard; staying on this computer");
                    return;
                }
            }
            Some((old, _, _)) => {
                self.release_held(old);
                self.send(old, Msg::Leave);
            }
        }
        let Some(p) = self.peers.get(&id) else { return };
        let (x, y) = entry_point(p.screen.w, p.screen.h, exit_side, frac, 1);
        log::debug!("cursor -> {} at ({x},{y})", p.name);
        self.active = Some((id, x, y));
        self.send(id, Msg::Enter { x, y });
        self.update_status();
    }

    fn go_local(&mut self, exit_side: Side, frac: f64) {
        if let Some((old, _, _)) = self.active.take() {
            self.release_held(old);
            self.send(old, Msg::Leave);
        }
        let r = self.capture.screen();
        let (x, y) = entry_point(r.w, r.h, exit_side, frac, 3);
        log::debug!("cursor -> local at ({x},{y})");
        self.capture.release(r.x + x, r.y + y);
        if let Some(req) = self.common.arrived() {
            if let Msg::FileRequest { origin, .. } = &req {
                let origin = origin.clone();
                self.send_to_name(&origin, req);
            }
        }
        self.update_status();
    }

    // ---------- drag and drop

    /// Leaving this computer with the left button held: is it a file drag?
    fn start_local_drag_if_any(&mut self) {
        if !dnd::left_button_down() {
            return;
        }
        let Some(paths) = dnd::drag_files() else { return };
        let paths: Vec<String> = paths.iter().map(|p| p.to_string_lossy().into_owned()).collect();
        if let Some(Msg::FileOffer { offer, files, .. }) = self.common.offer_local_files(&paths, OfferKind::Drop) {
            log::info!("carrying a drag of {} across", files::label_for(&files));
            dnd::cancel_drag();
            self.drag = Some(Drag { origin: self.me(), offer: Some((offer, files)), query: None, dropped_on: None, started: Instant::now() });
        }
    }

    /// Leaving a client with the left button held there: ask it what's being dragged.
    fn start_remote_drag_if_any(&mut self, id: u64, name: &str) {
        if !self.held_buttons.contains(&MouseButton::Left) {
            return;
        }
        let q = files::new_id();
        self.send(id, Msg::DragQuery { id: q });
        // The query is answered before the button-up that Leave sends.
        self.drag = Some(Drag { origin: name.to_string(), offer: None, query: Some(q), dropped_on: None, started: Instant::now() });
    }

    fn on_drag_reply(&mut self, q: u64, offer: Option<u64>, files: Vec<FileMeta>) {
        let Some(d) = self.drag.as_mut() else { return };
        if d.query != Some(q) {
            return;
        }
        match offer {
            Some(o) if !files.is_empty() => {
                d.offer = Some((o, files));
                if let Some(target) = d.dropped_on.take() {
                    self.drop_drag(target);
                }
            }
            _ => self.drag = None,
        }
    }

    /// While a drag carried onto this computer is in progress, watch for the
    /// button to be released here (we aren't capturing input locally).
    fn check_local_drop(&mut self) {
        if let Some(d) = &self.drag {
            if d.started.elapsed() > Duration::from_secs(120) {
                self.drag = None;
                return;
            }
            if self.active.is_none() && d.origin != self.me() && !dnd::left_button_down() {
                let me = self.me();
                self.drop_drag(me);
            }
        }
    }

    fn drop_drag(&mut self, target: String) {
        let Some(mut d) = self.drag.take() else { return };
        let Some((offer, files)) = d.offer.clone() else {
            // Still waiting to hear what was dragged.
            d.dropped_on = Some(target);
            self.drag = Some(d);
            return;
        };
        if target == d.origin {
            return;
        }
        log::info!("dropped {} from {} onto {}", files::label_for(&files), d.origin, target);
        if target == self.me() {
            if let Some(req) = self.common.on_offer(offer, d.origin.clone(), OfferKind::Drop, files, true) {
                self.send_to_name(&d.origin, req);
            }
        } else {
            self.send_to_name(&target, Msg::FileOffer { offer, origin: d.origin, kind: OfferKind::Drop, files });
        }
    }
}

fn spawn_acceptor(listener: TcpListener, psk: [u8; 32], ctx: Ctx, routes: Routes, net_tx: Sender<NetEvent>, stop: Arc<AtomicBool>) -> Result<()> {
    static NEXT_ID: AtomicU64 = AtomicU64::new(1);
    listener.set_nonblocking(true)?;
    std::thread::Builder::new().name("acceptor".into()).spawn(move || {
        while !stop.load(Ordering::Relaxed) {
            let (stream, addr) = match listener.accept() {
                Ok(c) => c,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(150));
                    continue;
                }
                Err(e) => {
                    log::warn!("accept: {e}");
                    continue;
                }
            };
            let net_tx = net_tx.clone();
            let ctx = ctx.clone();
            let routes = routes.clone();
            let _ = std::thread::Builder::new().name(format!("peer-{addr}")).spawn(move || {
                let _ = stream.set_nonblocking(false);
                let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
                let me = ctx.cfg.name.clone();
                let (link, mut rx, name, os, screen) = match server_handshake(stream, &psk, &me, id, net_tx.clone()) {
                    Ok(v) => v,
                    Err(e) => {
                        log::warn!("rejected connection from {addr}: {e:#}");
                        return;
                    }
                };
                if net_tx.send(NetEvent::Connected { id, name, os, screen, addr, link }).is_err() {
                    return;
                }
                loop {
                    match rx.recv() {
                        Ok(m) => {
                            if !route_or_handle(m, &me, &ctx.inbox, &routes, |ev| net_tx.send(ev).is_ok(), id) {
                                return;
                            }
                        }
                        Err(e) => {
                            log::debug!("peer {addr}: {e:#}");
                            let _ = net_tx.send(NetEvent::Disconnected(id));
                            return;
                        }
                    }
                }
            });
        }
    })?;
    Ok(())
}

/// Runs on a connection's reader thread. File data is written (or relayed)
/// right here so a big transfer never passes through the engine thread.
fn route_or_handle(m: Msg, me: &str, inbox: &Inbox, routes: &Routes, emit: impl Fn(NetEvent) -> bool, id: u64) -> bool {
    match m {
        Msg::FileData { offer, dest, index, data } => {
            if dest == me {
                if let Err(e) = inbox.data(offer, index, &data) {
                    if let Some(f) = inbox.finish(offer, Some(e)) {
                        return emit(NetEvent::Finished(f));
                    }
                }
            } else if let Some(l) = routes.lock().get(&dest).cloned() {
                l.send_bulk_wait(Msg::FileData { offer, dest, index, data });
            }
            true
        }
        Msg::FileEnd { offer, dest, error } => {
            if dest == me {
                if let Some(f) = inbox.finish(offer, error) {
                    return emit(NetEvent::Finished(f));
                }
            } else if let Some(l) = routes.lock().get(&dest).cloned() {
                l.send_bulk_wait(Msg::FileEnd { offer, dest, error });
            }
            true
        }
        Msg::FileRequest { offer, origin, requester } if origin != me => {
            if let Some(l) = routes.lock().get(&origin).cloned() {
                l.send(Msg::FileRequest { offer, origin, requester });
            }
            true
        }
        m => emit(NetEvent::Msg(id, m)),
    }
}

fn server_handshake(
    stream: TcpStream,
    psk: &[u8; 32],
    my_name: &str,
    id: u64,
    net_tx: Sender<NetEvent>,
) -> Result<(Link, SecureReceiver, String, Os, Rect)> {
    let (tx, mut rx) = handshake(stream, psk, false)?;
    rx.set_timeout(Some(Duration::from_secs(10)))?;
    let hello = rx.recv().context("client did not say hello (wrong passphrase?)")?;
    let Msg::Hello { version, name, os, screen } = hello else { bail!("unexpected first message") };
    if version != PROTOCOL_VERSION {
        let _ = tx.send(&Msg::Welcome { version: PROTOCOL_VERSION, name: my_name.to_string(), os: Os::current() });
        bail!("{name} runs OpenHop protocol v{version}, this computer runs v{PROTOCOL_VERSION}: update both to the same version");
    }
    tx.send(&Msg::Welcome { version: PROTOCOL_VERSION, name: my_name.to_string(), os: Os::current() })?;
    rx.set_timeout(Some(PEER_TIMEOUT))?;
    let link = Link::new(tx, move || {
        let _ = net_tx.send(NetEvent::Disconnected(id));
    });
    Ok((link, rx, name, os, screen))
}

// ---------------------------------------------------------------- client

enum ClientEvent {
    Msg(Msg),
    Finished(Finished),
    Lost(anyhow::Error),
}

struct Client {
    ctx: Ctx,
    common: Common,
    injector: Box<dyn Injector>,
    held_keys: HashSet<u16>,
    held_buttons: HashSet<MouseButton>,
    /// Is the pointer on this computer right now?
    here: bool,
}

impl Client {
    fn run(ctx: Ctx, injector: Box<dyn Injector>, ctl_rx: Receiver<Control>) -> Result<()> {
        let (clip_set, clip_rx) = if ctx.cfg.clipboard_sync {
            let (s, r) = clipboard::start();
            (Some(s), r)
        } else {
            (None, crossbeam_channel::never())
        };
        let mut c = Client {
            common: Common::new(ctx.clone(), clip_set),
            ctx,
            injector,
            held_keys: HashSet::new(),
            held_buttons: HashSet::new(),
            here: false,
        };
        let psk = derive_psk(&c.ctx.cfg.passphrase);
        let mut last_failed: HashMap<SocketAddr, Instant> = HashMap::new();
        while !c.ctx.stopped() {
            let Some((addr, label)) = c.pick_server(&last_failed) else {
                c.ctx.set_message("Looking for a server on your network…");
                if let Ok(Control::Stop) = ctl_rx.recv_timeout(Duration::from_secs(1)) {
                    break;
                }
                continue;
            };
            c.ctx.set_message(format!("Connecting to {label}…"));
            match c.session(addr, &label, &psk, &ctl_rx, &clip_rx) {
                Ok(()) => break, // asked to stop
                Err(e) => {
                    log::warn!("{label}: {e:#}");
                    c.ctx.set_error(Some(format!("{label}: {e:#}")));
                    last_failed.insert(addr, Instant::now());
                    c.release_all();
                    c.here = false;
                    {
                        let mut st = c.ctx.status.lock();
                        st.peers.clear();
                        st.active.clear();
                    }
                    if let Ok(Control::Stop) = ctl_rx.recv_timeout(Duration::from_secs(2)) {
                        break;
                    }
                }
            }
        }
        c.release_all();
        Ok(())
    }

    fn pick_server(&self, failed: &HashMap<SocketAddr, Instant>) -> Option<(SocketAddr, String)> {
        if let Some(a) = &self.ctx.cfg.server_addr {
            let with_port = if a.contains(':') { a.clone() } else { format!("{a}:{DEFAULT_PORT}") };
            return with_port.to_socket_addrs().ok()?.next().map(|s| (s, a.clone()));
        }
        let recently_failed = |a: &SocketAddr| failed.get(a).map(|t| t.elapsed() < Duration::from_secs(5)).unwrap_or(false);
        self.ctx
            .discovery
            .servers()
            .into_iter()
            .filter(|s| self.ctx.cfg.server_name.as_ref().map(|n| n == &s.name).unwrap_or(true))
            .find(|s| !recently_failed(&s.addr))
            .map(|s| (s.addr, s.name))
    }

    fn session(&mut self, addr: SocketAddr, label: &str, psk: &[u8; 32], ctl_rx: &Receiver<Control>, clip_rx: &Receiver<ClipData>) -> Result<()> {
        let stream = TcpStream::connect_timeout(&addr, Duration::from_secs(4)).context("could not connect")?;
        let local_ip = stream.local_addr().ok().map(|a| a.ip());
        let (tx, mut rx) = handshake(stream, psk, true)?;
        let mut screen = self.injector.screen();
        tx.send(&Msg::Hello { version: PROTOCOL_VERSION, name: self.ctx.cfg.name.clone(), os: Os::current(), screen })?;
        rx.set_timeout(Some(Duration::from_secs(10)))?;
        let (server_name, server_os) = match rx.recv().map_err(|_| anyhow!("server closed the connection: is the passphrase the same on both computers?"))? {
            Msg::Welcome { version, name, os } if version == PROTOCOL_VERSION => (name, os),
            Msg::Welcome { version, .. } => bail!("the server runs OpenHop protocol v{version}, this computer runs v{PROTOCOL_VERSION}: update both to the same version"),
            _ => bail!("unexpected reply"),
        };
        rx.set_timeout(Some(PEER_TIMEOUT))?;
        log::info!("connected to {server_name} ({}) at {addr}", server_os.label());
        self.ctx.set_error(None);
        {
            let mut st = self.ctx.status.lock();
            st.message = format!("Connected to {label}");
            st.peers = vec![PeerStatus { name: server_name.clone(), os: server_os, addr: addr.ip().to_string(), screen: Rect { x: 0, y: 0, w: 0, h: 0 } }];
        }

        let (ev_tx, ev_rx) = crossbeam_channel::unbounded::<ClientEvent>();
        let link = {
            let ev_tx = ev_tx.clone();
            Link::new(tx, move || {
                let _ = ev_tx.send(ClientEvent::Lost(anyhow!("connection lost")));
            })
        };
        if let Some(mac) = local_ip.and_then(wol::mac_for_ip) {
            link.send(Msg::Mac(mac));
        }
        {
            let inbox = self.ctx.inbox.clone();
            let me = self.ctx.cfg.name.clone();
            std::thread::Builder::new().name("client-rx".into()).spawn(move || loop {
                match rx.recv() {
                    Ok(Msg::FileData { offer, dest, index, data }) if dest == me => {
                        if let Err(e) = inbox.data(offer, index, &data) {
                            if let Some(f) = inbox.finish(offer, Some(e)) {
                                let _ = ev_tx.send(ClientEvent::Finished(f));
                            }
                        }
                    }
                    Ok(Msg::FileEnd { offer, dest, error }) if dest == me => {
                        if let Some(f) = inbox.finish(offer, error) {
                            let _ = ev_tx.send(ClientEvent::Finished(f));
                        }
                    }
                    Ok(m) => {
                        if ev_tx.send(ClientEvent::Msg(m)).is_err() {
                            return;
                        }
                    }
                    Err(e) => {
                        let _ = ev_tx.send(ClientEvent::Lost(e.context("connection lost")));
                        return;
                    }
                }
            })?;
        }

        let ticker = tick(Duration::from_secs(2));
        let result = loop {
            select! {
                recv(ev_rx) -> ev => match ev {
                    Ok(ClientEvent::Msg(m)) => self.on_msg(m, screen, &link, &server_name),
                    Ok(ClientEvent::Finished(f)) => self.common.on_finished(f),
                    Ok(ClientEvent::Lost(e)) => break Err(e),
                    Err(_) => break Err(anyhow!("connection lost")),
                },
                recv(clip_rx) -> c => if let Ok(c) = c {
                    match c {
                        ClipData::Files(paths) => {
                            if let Some(offer) = self.common.offer_local_files(&paths, OfferKind::Clipboard) {
                                link.send(offer);
                            }
                        }
                        data => send_bulk_async(link.clone(), clip_messages(&self.ctx.cfg.name, data)),
                    }
                },
                recv(ctl_rx) -> c => match c {
                    Ok(Control::Stop) | Err(_) => break Ok(()),
                    Ok(Control::SetLayout(_)) => {}
                },
                recv(ticker) -> _ => {
                    let now = self.injector.screen();
                    if now != screen {
                        screen = now;
                        self.ctx.status.lock().screen = Some(now);
                        link.send(Msg::Screen(now));
                    }
                },
            }
        };
        link.close();
        for f in self.ctx.inbox.abort_from(&server_name) {
            self.common.on_finished(f);
        }
        result
    }

    fn on_msg(&mut self, m: Msg, screen: Rect, link: &Link, server: &str) {
        let r = match m {
            Msg::Enter { x, y } => {
                self.here = true;
                self.ctx.status.lock().active = self.ctx.cfg.name.clone();
                if let Some(req) = self.common.arrived() {
                    link.send(req);
                }
                self.injector.move_to(screen.x + x, screen.y + y)
            }
            Msg::Move { x, y } => self.injector.move_to(screen.x + x, screen.y + y),
            Msg::Leave => {
                self.here = false;
                self.ctx.status.lock().active.clear();
                self.release_all();
                Ok(())
            }
            Msg::Button { button, down } => {
                if down {
                    self.held_buttons.insert(button);
                    if button == MouseButton::Left {
                        dnd::note_left_down();
                    }
                } else {
                    self.held_buttons.remove(&button);
                }
                self.injector.button(button, down)
            }
            Msg::Wheel { dx, dy } => self.injector.wheel(dx, dy),
            Msg::Key { key, down } => {
                if down {
                    self.held_keys.insert(key);
                } else {
                    self.held_keys.remove(&key);
                }
                self.injector.key(key, down)
            }
            Msg::Ping => {
                link.send(Msg::Pong);
                Ok(())
            }
            Msg::Clip { origin, data } => {
                self.common.on_remote_clip(&origin, data, self.here);
                Ok(())
            }
            Msg::ClipPart { origin, id, total, data } => {
                if let Some((origin, data)) = self.common.assembler.add(origin, id, total, data) {
                    self.common.on_remote_clip(&origin, data, self.here);
                }
                Ok(())
            }
            Msg::FileOffer { offer, origin, kind, files } => {
                if let Some(req) = self.common.on_offer(offer, origin, kind, files, self.here) {
                    link.send(req);
                }
                Ok(())
            }
            Msg::FileRequest { offer, requester, .. } => {
                // The server relays our data to the requester.
                self.ctx.outbox.serve(offer, requester, link.clone());
                Ok(())
            }
            Msg::DragQuery { id } => {
                let reply = match dnd::drag_files() {
                    Some(paths) => {
                        let paths: Vec<String> = paths.iter().map(|p| p.to_string_lossy().into_owned()).collect();
                        match self.common.offer_local_files(&paths, OfferKind::Drop) {
                            Some(Msg::FileOffer { offer, files, .. }) => {
                                // Cancel the drag here before the button is released.
                                let _ = self.injector.key(ESC, true);
                                let _ = self.injector.key(ESC, false);
                                std::thread::sleep(Duration::from_millis(60));
                                Msg::DragReply { id, offer: Some(offer), files }
                            }
                            _ => Msg::DragReply { id, offer: None, files: vec![] },
                        }
                    }
                    None => Msg::DragReply { id, offer: None, files: vec![] },
                };
                link.send(reply);
                Ok(())
            }
            _ => {
                let _ = server;
                Ok(())
            }
        };
        if let Err(e) = r {
            log::warn!("input injection failed: {e:#}");
        }
    }

    fn release_all(&mut self) {
        for key in self.held_keys.drain().collect::<Vec<_>>() {
            let _ = self.injector.key(key, false);
        }
        for b in self.held_buttons.drain().collect::<Vec<_>>() {
            let _ = self.injector.button(b, false);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_are_detected() {
        assert!(is_link("https://example.com/a?b=c"));
        assert!(is_link("  http://x.io  "));
        assert!(!is_link("see https://example.com"));
        assert!(!is_link("javascript:alert(1)"));
    }

    #[test]
    fn big_clip_images_are_split_and_reassembled() {
        let img = vec![9u8; CHUNK * 3 + 10];
        let msgs = clip_messages("pc", ClipData::Png(img.clone()));
        assert_eq!(msgs.len(), 4);
        let mut a = ClipAssembler::default();
        let mut out = None;
        for m in msgs {
            if let Msg::ClipPart { origin, id, total, data } = m {
                out = a.add(origin, id, total, data).or(out);
            }
        }
        assert_eq!(out, Some(("pc".to_string(), ClipData::Png(img))));
        assert_eq!(clip_messages("pc", ClipData::Text("hi".into())).len(), 1);
    }
}
