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
use crate::extras::{self, Hub};
use crate::files::{self, Finished, Inbox, Outbox, TransferInfo};
use crate::keys;
use crate::layout::{apply_delta, edge_fraction, entry_point, touching_edge, Layout, Side, SERVER};
use crate::net::{derive_psk, handshake, Link, SecureReceiver, CHUNK};
use crate::platform::{self, dnd, Capture, Injector, InputEvent};
use crate::protocol::{ClipData, DriveEv, Ext, FileMeta, MouseButton, Msg, OfferKind, Os, Rect, WinInfo, DEFAULT_PORT, PROTOCOL_VERSION};
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
/// Pointer positions OpenHop set itself lately, to tell them apart from the
/// user moving this computer's own mouse.
#[derive(Default)]
struct Placed(std::collections::VecDeque<((i32, i32), Instant)>);

impl Placed {
    fn add(&mut self, p: (i32, i32)) {
        self.0.push_back((p, Instant::now()));
        while self.0.len() > 32 {
            self.0.pop_front();
        }
    }
    fn ours(&self, (x, y): (i32, i32)) -> bool {
        self.0.iter().any(|((px, py), t)| (x - px).abs() <= 2 && (y - py).abs() <= 2 && t.elapsed() < Duration::from_millis(400))
    }
}

/// How long the pointer rests against a screen edge before hopping (so it
/// doesn't hop by accident), and corners that never hop.
const EDGE_DWELL: Duration = Duration::from_millis(70);
const CORNER: f64 = 0.03;
/// Pushing this far past another computer's screen edge hops on.
const EDGE_PUSH: i32 = 20;

/// Keyboard shortcuts OpenHop handles itself (even while the keyboard is in
/// use on another screen): Ctrl+Alt+Shift+arrows jump to the next screen that
/// way, Ctrl+Alt+Shift+L keeps the pointer on the screen it's on.
enum Hotkey {
    Jump(Side),
    Pin,
}

fn hotkey(held: &HashSet<u16>, key: u16) -> Option<Hotkey> {
    let any = |a: u16, b: u16| held.contains(&a) || held.contains(&b);
    if !(any(0xE0, 0xE4) && any(0xE2, 0xE6) && any(0xE1, 0xE5)) {
        return None;
    }
    match key {
        0x4F => Some(Hotkey::Jump(Side::Right)),
        0x50 => Some(Hotkey::Jump(Side::Left)),
        0x51 => Some(Hotkey::Jump(Side::Bottom)),
        0x52 => Some(Hotkey::Jump(Side::Top)),
        0x0F => Some(Hotkey::Pin),
        _ => None,
    }
}

/// In `Server::active`: the pointer is on this (the server's) screen, moved
/// by another computer's keyboard and mouse.
const THIS: u64 = u64::MAX;

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
    /// This computer's stable id (matches `device` in discovered entries).
    pub device: String,
    /// Server: the 6-digit code other computers enter to pair.
    pub pairing_code: Option<String>,
    /// Computers paired with this one.
    pub paired: Vec<PairedInfo>,
    /// Client: not paired and no passphrase set; choose a server to pair with.
    pub needs_pairing: bool,
    /// Client: device id of a pairing attempt in progress.
    pub pairing_with: Option<String>,
    pub pair_error: Option<String>,
    /// Server: the TCP port actually in use.
    pub port: u16,
    /// This computer's addresses (shown so others can connect by address).
    pub addresses: Vec<String>,
    /// Open windows on every connected computer (this one first).
    pub windows: Vec<ComputerWindows>,
    /// Computers in Do Not Disturb or presenting right now.
    pub quiet_from: Vec<String>,
    /// Client: the computer the others connect through (it keeps the
    /// arrangement and the pairing code).
    pub hub: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ComputerWindows {
    pub name: String,
    pub windows: Vec<WinInfo>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PairedInfo {
    pub device: String,
    pub name: String,
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
    /// Client: pair with the server whose device id is given, using its code.
    Pair {
        device: String,
        code: String,
    },
    /// Client: pair with the server at this address (when it isn't discovered).
    PairAddr {
        addr: String,
        code: String,
    },
    /// Forget a paired computer.
    Forget(String),
    /// Move the pointer to the next screen that way.
    Jump(Side),
    /// Keep the pointer on the screen it's on (or let it move again).
    Pin,
    /// Send files to a computer ("*" = everyone).
    SendFiles {
        to: String,
        paths: Vec<String>,
    },
    ShelfAdd(Vec<String>),
    ShelfTake {
        origin: String,
        id: u64,
    },
    /// Move the pointer to this computer's screen.
    GoTo(String),
    /// Arrange these computers by moving the mouse toward each in turn.
    Learn(Vec<String>),
}

/// Server-side pairing state: the current code and brute-force protection.
struct PairState {
    code: String,
    failures: u32,
    locked_until: Option<Instant>,
}

/// "192.168.1.20", "192.168.1.20:24852", "[fe80::1%3]:24850", "fe80::1", "desk.local".
fn parse_addr(a: &str) -> Option<SocketAddr> {
    let a = a.trim();
    if a.is_empty() {
        return None;
    }
    if let Ok(sa) = a.parse::<SocketAddr>() {
        return Some(sa);
    }
    if let Ok(ip) = a.parse::<IpAddr>() {
        return Some(SocketAddr::new(ip, DEFAULT_PORT));
    }
    let with_port = if a.contains(':') { a.to_string() } else { format!("{a}:{DEFAULT_PORT}") };
    with_port.to_socket_addrs().ok()?.next()
}

fn new_code() -> String {
    let mut b = [0u8; 4];
    getrandom::fill(&mut b).expect("random");
    format!("{:06}", u32::from_le_bytes(b) % 1_000_000)
}

fn paired_list(t: &std::collections::BTreeMap<String, crate::config::Trusted>) -> Vec<PairedInfo> {
    t.iter().map(|(d, v)| PairedInfo { device: d.clone(), name: v.name.clone() }).collect()
}

pub struct Engine {
    stop: Arc<AtomicBool>,
    ctl: Sender<Control>,
    status: Arc<Mutex<Status>>,
    notes: Arc<Mutex<Vec<Note>>>,
    inbox: Inbox,
    discovery: Arc<Discovery>,
    hub: Arc<Hub>,
    main: Option<std::thread::JoinHandle<()>>,
}

impl Engine {
    pub fn start(cfg: Config, config_path: Option<PathBuf>) -> Result<Engine> {
        if !cfg.passphrase.trim().is_empty() && cfg.passphrase.trim().len() < 4 {
            bail!("The passphrase must be at least 4 characters (or leave it empty and pair with a code).");
        }
        let mut cfg = cfg;
        if cfg.device_id.is_empty() {
            cfg.device_id = crate::config::random_hex(8);
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
            device: cfg.device_id.clone(),
            pairing_code: None,
            paired: paired_list(&cfg.trusted),
            needs_pairing: false,
            pairing_with: None,
            pair_error: None,
            port: cfg.port,
            addresses: local_addresses(),
            windows: vec![],
            quiet_from: vec![],
            hub: None,
        }));
        // The server binds first so discovery can announce the port it really got.
        let listeners = match cfg.role {
            Role::Server => {
                let (l4, port) = bind_v4_listener(cfg.port)?;
                if port != cfg.port {
                    log::warn!("port {} is taken by another program; using port {port} instead", cfg.port);
                }
                status.lock().port = port;
                Some((l4, bind_v6_listener(port), port))
            }
            Role::Client => None,
        };
        let announce_port = listeners.as_ref().map(|l| l.2).unwrap_or(cfg.port);
        let discovery = Arc::new(Discovery::start(cfg.device_id.clone(), cfg.name.clone(), cfg.role, announce_port).context("starting network discovery")?);
        let stop = Arc::new(AtomicBool::new(false));
        let (ctl_tx, ctl_rx) = crossbeam_channel::unbounded();
        let notes: Arc<Mutex<Vec<Note>>> = Default::default();
        let root = cfg.download_dir.as_ref().map(PathBuf::from).unwrap_or_else(files::download_root);
        let inbox = Inbox::new(root);
        files::set_speed_limit_mbps(cfg.transfer_limit_mbps);
        dnd::init();
        let hub = Hub::new(cfg.name.clone(), extras::Settings::from_config(&cfg));

        let ctx = Ctx {
            hub: hub.clone(),
            cfg: cfg.clone(),
            config_path,
            status: status.clone(),
            discovery: discovery.clone(),
            stop: stop.clone(),
            notes: notes.clone(),
            inbox: inbox.clone(),
            outbox: Outbox::default(),
            trust: Arc::new(Mutex::new(cfg.trusted.clone())),
            pair: Arc::new(Mutex::new(PairState { code: new_code(), failures: 0, locked_until: None })),
        };
        let main = match cfg.role {
            Role::Server => {
                // Without a way to capture the keyboard and mouse here (Wayland),
                // the others can still use this screen.
                let capture = match shared_capture(cfg.screen) {
                    Ok(c) => c,
                    Err(e) => {
                        log::warn!("this computer's keyboard and mouse can't be shared: {e:#}");
                        let screen =
                            platform::create_injector(cfg.screen, &cfg.linux_backend).map(|i| i.screen()).unwrap_or(Rect { x: 0, y: 0, w: 1920, h: 1080 });
                        (Arc::new(NoCapture(screen)) as Arc<dyn Capture>, crossbeam_channel::never())
                    }
                };
                status.lock().screen = Some(capture.0.screen());
                // Also listening on IPv6 so direct-cable (link-local) connections work.
                let (listener, listener6, _) = listeners.expect("server listeners");
                std::thread::Builder::new().name("server".into()).spawn(move || {
                    if let Err(e) = Server::run(ctx.clone(), capture, listener, listener6, ctl_rx) {
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
        Ok(Engine { stop, ctl: ctl_tx, status, notes, inbox, discovery, hub, main: Some(main) })
    }

    pub fn status(&self) -> Status {
        let mut s = self.status.lock().clone();
        s.discovered = self.discovery.peers();
        s.transfers = self.inbox.history();
        s.windows = self.hub.windows().into_iter().map(|(name, windows)| ComputerWindows { name, windows }).collect();
        s.quiet_from = self.hub.quiet_from();
        s
    }

    /// Live windows, animations, theme and Do Not Disturb sync.
    pub fn hub(&self) -> Arc<Hub> {
        self.hub.clone()
    }

    /// Notifications produced since the last call.
    pub fn take_notes(&self) -> Vec<Note> {
        std::mem::take(&mut *self.notes.lock())
    }

    /// Server: apply a new arrangement immediately (and save it).
    pub fn set_layout(&self, layout: Layout) {
        let _ = self.ctl.send(Control::SetLayout(layout));
    }

    /// Client: pair with a discovered server using the code it shows.
    pub fn pair(&self, device: String, code: String) {
        let _ = self.ctl.send(Control::Pair { device, code });
    }

    /// Client: pair with a server by its address (e.g. "192.168.1.20").
    pub fn pair_addr(&self, addr: String, code: String) {
        let _ = self.ctl.send(Control::PairAddr { addr, code });
    }

    /// Move the pointer to the next screen in that direction (a shortcut).
    pub fn jump(&self, side: Side) {
        let _ = self.ctl.send(Control::Jump(side));
    }

    /// Keep the pointer on the screen it's on, or let it move again.
    pub fn pin(&self) {
        let _ = self.ctl.send(Control::Pin);
    }

    /// Send files to computer `to` ("*" = every computer).
    pub fn send_files(&self, to: String, paths: Vec<String>) {
        let _ = self.ctl.send(Control::SendFiles { to, paths });
    }

    /// Put files on the shelf, reachable from every computer.
    pub fn shelf_add(&self, paths: Vec<String>) {
        let _ = self.ctl.send(Control::ShelfAdd(paths));
    }

    /// Bring a shelf item here (it lands on the clipboard).
    pub fn shelf_take(&self, origin: String, id: u64) {
        let _ = self.ctl.send(Control::ShelfTake { origin, id });
    }

    /// Move the pointer to computer `name`'s screen.
    pub fn go_to(&self, name: String) {
        let _ = self.ctl.send(Control::GoTo(name));
    }

    /// Arrange the screens by moving the mouse toward each computer in turn.
    pub fn learn(&self, names: Vec<String>) {
        let _ = self.ctl.send(Control::Learn(names));
    }

    /// Forget a paired computer (it will need the code again).
    pub fn forget(&self, device: String) {
        let _ = self.ctl.send(Control::Forget(device));
    }

    pub fn stop(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        self.hub.shutdown();
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
/// Bind the server port. Reuses it straight after a restart (on Linux/macOS
/// recently closed connections otherwise block it for a minute) and waits a
/// moment for a previous run to release it.
fn bind_v4_listener(port: u16) -> Result<(TcpListener, u16)> {
    let try_bind = |port: u16| -> std::io::Result<TcpListener> {
        let s = socket2::Socket::new(socket2::Domain::IPV4, socket2::Type::STREAM, Some(socket2::Protocol::TCP))?;
        // On Windows SO_REUSEADDR would let two programs share the port; there
        // the default already ignores old connections.
        #[cfg(unix)]
        s.set_reuse_address(true)?;
        s.bind(&SocketAddr::from((std::net::Ipv4Addr::UNSPECIFIED, port)).into())?;
        s.listen(16)?;
        Ok(s.into())
    };
    // Port taken by an older OpenHop (a leftover background copy): close it
    // and take the port over. Anything else keeps it; we use another one.
    if matches!(try_bind(port), Err(ref e) if e.kind() == std::io::ErrorKind::AddrInUse) && crate::portfree::close_old_openhop(port) {
        log::info!("closed an older OpenHop that held port {port}");
    }
    // Give a previous run a moment to let go of the port...
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match try_bind(port) {
            Ok(l) => return Ok((l, port)),
            Err(e) if e.kind() == std::io::ErrorKind::AddrInUse && Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(200));
            }
            Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => break,
            Err(e) => return Err(e).with_context(|| format!("can't open port {port}")),
        }
    }
    // ...then use the next free one. Other computers learn the real port from discovery.
    for p in (port + 2..port + 40).step_by(2) {
        if let Ok(l) = try_bind(p) {
            return Ok((l, p));
        }
    }
    let l = try_bind(0).context("can't open any network port")?;
    let p = l.local_addr()?.port();
    Ok((l, p))
}

/// IP addresses of this computer's network interfaces (for "connect by address").
pub fn local_addresses() -> Vec<String> {
    let mut v: Vec<(bool, String)> = netdev::get_interfaces()
        .into_iter()
        .filter(|i| i.is_up() && !i.is_loopback() && !is_virtual_interface(&i.name))
        .flat_map(|i| {
            // Prefer the real network: Wi-Fi and Ethernet first.
            let real = i.gateway.is_some();
            i.ipv4.into_iter().map(move |n| (real, n.addr()))
        })
        .filter(|(_, a)| !a.is_loopback() && !a.is_link_local())
        .map(|(real, a)| (!real, a.to_string()))
        .collect();
    v.sort();
    v.dedup_by(|a, b| a.1 == b.1);
    v.into_iter().map(|(_, a)| a).collect()
}

/// Networks that only exist inside this computer (containers, virtual
/// machines): another computer can't reach us through them.
pub fn is_virtual_interface(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    ["docker", "br-", "veth", "virbr", "vboxnet", "vmnet", "lxc", "lxd", "cni", "flannel", "kube", "podman", "cali", "tun", "utun", "vethernet"]
        .iter()
        .any(|p| n.starts_with(p))
}

fn bind_v6_listener(port: u16) -> Option<TcpListener> {
    let s = socket2::Socket::new(socket2::Domain::IPV6, socket2::Type::STREAM, Some(socket2::Protocol::TCP)).ok()?;
    s.set_only_v6(true).ok()?;
    s.set_reuse_address(true).ok()?;
    s.bind(&SocketAddr::from((std::net::Ipv6Addr::UNSPECIFIED, port)).into()).ok()?;
    s.listen(16).ok()?;
    Some(s.into())
}

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

/// Stand-in where this computer's keyboard and mouse can't be captured.
struct NoCapture(Rect);

impl Capture for NoCapture {
    fn grab(&self) -> bool {
        false
    }
    fn release(&self, _: i32, _: i32) {}
    fn screen(&self) -> Rect {
        self.0
    }
}

#[derive(Clone)]
struct Ctx {
    hub: Arc<Hub>,
    cfg: Config,
    config_path: Option<PathBuf>,
    status: Arc<Mutex<Status>>,
    discovery: Arc<Discovery>,
    stop: Arc<AtomicBool>,
    notes: Arc<Mutex<Vec<Note>>>,
    inbox: Inbox,
    outbox: Outbox,
    /// Paired computers, readable from connection threads.
    trust: Arc<Mutex<std::collections::BTreeMap<String, crate::config::Trusted>>>,
    pair: Arc<Mutex<PairState>>,
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
        // Hold pop-ups while another computer is presenting or in Do Not Disturb.
        if self.cfg.notifications && !self.hub.quiet() {
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
            p.chunks(CHUNK).map(|c| Msg::ClipPart { origin: origin.to_string(), id, total, data: c.to_vec() }).collect()
        }
        data => vec![Msg::Clip { origin: origin.to_string(), data }],
    }
}

/// Send bulk messages on a worker thread (waits for room instead of dropping).
fn send_bulk_async(link: Link, msgs: Vec<Msg>) {
    let _ = std::thread::Builder::new().name("bulk-send".into()).spawn(move || {
        for m in msgs {
            if let Msg::ClipPart { data, .. } = &m {
                files::pace(data.len());
            }
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
    /// When files were last dropped onto this computer.
    dropped_at: Option<Instant>,
}

impl Common {
    fn new(ctx: Ctx, clip_set: Option<Sender<ClipData>>) -> Common {
        if let Some(set) = clip_set.clone() {
            // Clipboard history: copy an item again.
            ctx.hub.set_clip_setter(Box::new(move |d| {
                let _ = set.send(d);
            }));
        }
        Common { ctx, clip_set, assembler: ClipAssembler::default(), pending_offer: None, pending_note: None, dropped_at: None }
    }

    /// Files to send to computer `to` ("*" = everyone). Returns the message.
    fn send_files(&mut self, to: &str, paths: &[String]) -> Option<Msg> {
        let Some(Msg::FileOffer { offer, origin, files, .. }) = self.offer_local_files(paths, OfferKind::Send) else { return None };
        let label = files::label_for(&files);
        let whom = if to == "*" { "every computer".to_string() } else { to.to_string() };
        self.ctx.hub.notice_everywhere(&format!("Sending {label}"), &format!("To {whom}."), "files");
        Some(Msg::SendFiles { to: to.into(), offer, origin, files })
    }

    /// Put files on the shelf (kept offered until taken off).
    fn shelf_add(&mut self, paths: &[String]) {
        let paths: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
        match files::collect(&paths) {
            Ok(entries) if !entries.is_empty() => {
                // Small enough for the island's JavaScript to keep exact.
                let offer = files::new_id() & ((1 << 52) - 1);
                let metas: Vec<FileMeta> = entries.iter().map(|e| e.meta.clone()).collect();
                let item = crate::protocol::ShelfItem { id: offer, label: files::label_for(&metas), size: files::total_size(&metas), files: metas };
                self.ctx.outbox.keep(offer, entries);
                self.ctx.hub.shelf_add(item);
            }
            Ok(_) => {}
            Err(e) => self.ctx.note(note("Couldn't put that on the shelf", e, vec![])),
        }
    }

    /// Take a shelf item from another computer: it arrives on the clipboard.
    fn shelf_take(&mut self, origin: &str, id: u64) -> Option<Msg> {
        let Some(item) = self.ctx.hub.shelf_item(origin, id) else {
            log::info!("shelf item {id} from {origin} is gone");
            return None;
        };
        log::info!("taking {} from {origin}'s shelf", item.label);
        self.request(id, origin.to_string(), OfferKind::Clipboard, item.files)
    }

    /// Clipboard content arrived from another computer.
    fn on_remote_clip(&mut self, origin: &str, data: ClipData, here: bool) {
        self.ctx.hub.clip_seen(origin, &data);
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
        if kind == OfferKind::Drop {
            self.dropped_at = Some(Instant::now());
            self.ctx.hub.incoming(&files::label_for(&files), &origin);
        }
        if kind == OfferKind::Send && self.ctx.hub.pointer_here() {
            self.ctx.hub.incoming(&files::label_for(&files), &origin);
        }
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
                    format!("Copied on {}. Paste it into a folder, chat or document, or find it in Downloads › OpenHop.", f.origin),
                    vec![action("Show in folder", "reveal", first)],
                ));
            }
            OfferKind::Send => {
                self.ctx.hub.landed(&label);
                let mut actions = vec![];
                if f.tops.len() == 1 && f.tops[0].is_file() {
                    actions.push(action("Open", "open_path", first.clone()));
                }
                actions.push(action("Show in folder", "reveal", first));
                self.ctx.note(note(format!("Received {label}"), format!("From {}. Saved to Downloads › OpenHop.", f.origin), actions));
            }
            OfferKind::Drop => {
                // Drop the files into whatever is under the pointer, like a local
                // drag. If nothing takes them (or they took long to arrive), keep
                // them in Downloads and put them on the clipboard to paste.
                let recent = self.dropped_at.take().map(|t| t.elapsed() < Duration::from_secs(20)).unwrap_or(false);
                let (ctx, clip, origin) = (self.ctx.clone(), self.clip_set.clone(), f.origin.clone());
                let tops = f.tops.clone();
                let _ = std::thread::Builder::new().name("drop".into()).spawn(move || {
                    if recent && dnd::drop_files(&tops) {
                        ctx.hub.landed(&label);
                        log::info!("dropped {label} into the app under the pointer");
                        ctx.inbox.discard_later(&tops);
                        ctx.note(note(format!("Dropped {label}"), format!("From {origin}."), vec![]));
                        return;
                    }
                    ctx.hub.landed(&label);
                    let kept = ctx.inbox.keep(&tops);
                    let first = kept.first().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
                    let mut actions = vec![];
                    if kept.len() == 1 && kept[0].is_file() {
                        actions.push(action("Open", "open_path", first.clone()));
                    }
                    actions.push(action("Show in folder", "reveal", first));
                    let pasteable =
                        clip.map(|c| c.send(ClipData::Files(kept.iter().map(|p| p.to_string_lossy().into_owned()).collect())).is_ok()).unwrap_or(false);
                    let body = if pasteable {
                        format!("From {origin}. Saved to Downloads › OpenHop and copied: paste it where you want it.")
                    } else {
                        format!("From {origin}. Saved to Downloads › OpenHop.")
                    };
                    ctx.note(note(format!("Received {label}"), body, actions));
                });
            }
        }
    }
}

// ---------------------------------------------------------------- server

enum NetEvent {
    Connected {
        id: u64,
        name: String,
        os: Os,
        screen: Rect,
        addr: SocketAddr,
        link: Link,
        device: Option<String>,
    },
    /// A computer just paired with the code; remember its key.
    Paired {
        device: String,
        name: String,
        key: String,
    },
    Msg(u64, Msg),
    Disconnected(u64),
    Finished(Finished),
}

type Routes = Arc<Mutex<HashMap<String, Link>>>;

struct Peer {
    name: String,
    /// Device id, if it connected with a paired key (not the passphrase).
    device: Option<String>,
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
    /// The screen it was carried onto (for a window dragged across).
    to: String,
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
    /// Window the keyboard types into directly (see update_key_pass).
    key_pass: Option<u64>,
    /// Whose keyboard and mouse are in use: None = this computer's own,
    /// Some(id) = that client's (any computer can drive the others).
    driver: Option<u64>,
    /// Puts another computer's input on this screen.
    local_inj: Option<Box<dyn Injector>>,
    /// Keys and buttons held on this screen by another computer's devices.
    local_keys: HashSet<u16>,
    local_buttons: HashSet<MouseButton>,
    /// Where we put this screen's pointer ourselves lately (the user's own
    /// mouse moving it somewhere else means they're using it).
    injected: Placed,
    /// The arrangement and pairing code last told to the clients.
    group_sent: Option<(Layout, String)>,
    /// Keys physically held on this keyboard / the driving client's (for shortcuts).
    raw_keys: HashSet<u16>,
    drive_keys: HashSet<u16>,
    /// The pointer stays on the screen it's on (Ctrl+Alt+Shift+L).
    pinned: bool,
    /// Resting against this screen's edge since (to hop after a moment).
    edge_wait: Option<(Side, Instant)>,
    /// How far the pointer has pushed past another screen's edge.
    push: i32,
    last_local: (i32, i32),
    /// Arranging the screens by moving the mouse toward each of these in turn.
    learning: Vec<String>,
}

impl Server {
    fn run(
        ctx: Ctx,
        (capture, input_rx): (Arc<dyn Capture>, Receiver<InputEvent>),
        listener: TcpListener,
        listener6: Option<TcpListener>,
        ctl_rx: Receiver<Control>,
    ) -> Result<()> {
        let (net_tx, net_rx) = crossbeam_channel::unbounded();
        let routes: Routes = Default::default();
        let mut acceptors = vec![spawn_acceptor(listener, ctx.clone(), routes.clone(), net_tx.clone(), ctx.stop.clone())?];
        if let Some(l6) = listener6 {
            acceptors.push(spawn_acceptor(l6, ctx.clone(), routes.clone(), net_tx, ctx.stop.clone())?);
        }
        let (clip_set, clip_rx) = if ctx.cfg.clipboard_sync {
            let (s, r) = clipboard::start();
            (Some(s), r)
        } else {
            (None, crossbeam_channel::never())
        };
        let port = ctx.status.lock().port;
        ctx.set_message(format!("Waiting for other computers… (port {port})"));
        {
            let routes = routes.clone();
            let me = ctx.cfg.name.clone();
            ctx.hub.set_out(Some(Box::new(move |to: &str, ext: Ext| {
                let bulk = matches!(ext, Ext::WinFrame { .. } | Ext::Windows { .. });
                let msg = Msg::Ext { to: to.to_string(), from: me.clone(), ext };
                let links: Vec<Link> = if to == "*" { routes.lock().values().cloned().collect() } else { routes.lock().get(to).cloned().into_iter().collect() };
                for l in links {
                    if bulk {
                        l.send_bulk_wait(msg.clone());
                    } else {
                        l.send(msg.clone());
                    }
                }
            })));
            // Controlling this computer's windows from a live view elsewhere:
            // inject here, letting our own events past the capture.
            if let Ok(mut inj) = platform::create_injector(ctx.cfg.screen, "x11") {
                let cap = capture.clone();
                let whole = capture.whole_clicks();
                ctx.hub.set_injector(
                    Box::new(move |ops: &[platform::InjectOp]| {
                        let pointer = ops.iter().any(|o| !matches!(o, platform::InjectOp::Key(..)));
                        let was = cap.suspend(pointer);
                        for op in ops {
                            let _ = platform::apply(inj.as_mut(), *op);
                        }
                        inj.sync();
                        if was {
                            cap.resume();
                        }
                    }),
                    whole,
                );
            }
        }
        let (ctx_screen, linux_backend) = (ctx.cfg.screen, ctx.cfg.linux_backend.clone());
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
            key_pass: None,
            driver: None,
            local_inj: platform::create_injector(ctx_screen, &linux_backend).map_err(|e| log::warn!("other computers can't use this screen: {e:#}")).ok(),
            local_keys: HashSet::new(),
            local_buttons: HashSet::new(),
            injected: Placed::default(),
            group_sent: None,
            raw_keys: HashSet::new(),
            drive_keys: HashSet::new(),
            pinned: false,
            edge_wait: None,
            push: 0,
            last_local: (0, 0),
            learning: vec![],
        };
        s.update_status();
        s.ctx.hub.set_screen(s.capture.screen());
        let pinger = tick(Duration::from_secs(3));
        let fast = tick(Duration::from_millis(100));
        let edge_tick = tick(Duration::from_millis(30));
        loop {
            select! {
                recv(input_rx) -> ev => if let Ok(ev) = ev { s.on_input(ev) },
                recv(net_rx) -> ev => if let Ok(ev) = ev { s.on_net(ev) },
                recv(clip_rx) -> c => if let Ok(c) = c { s.on_local_clip(c) },
                recv(ctl_rx) -> c => match c {
                    Ok(Control::SetLayout(l)) => s.set_layout(l),
                    Ok(Control::Forget(d)) => s.forget(&d),
                    Ok(Control::Pair { .. }) | Ok(Control::PairAddr { .. }) => {}
                    Ok(Control::Jump(side)) => s.jump(side),
                    Ok(Control::Pin) => s.toggle_pin(),
                    Ok(Control::SendFiles { to, paths }) => {
                        if let Some(m) = s.common.send_files(&to, &paths) {
                            s.route_send(m, None);
                        }
                    }
                    Ok(Control::ShelfAdd(paths)) => s.common.shelf_add(&paths),
                    Ok(Control::ShelfTake { origin, id }) => {
                        if let Some(req) = s.common.shelf_take(&origin, id) {
                            s.send_to_name(&origin, req);
                        }
                    }
                    Ok(Control::GoTo(name)) => s.go_to(&name),
                    Ok(Control::Learn(names)) => s.start_learning(names),
                    Ok(Control::Stop) | Err(_) => break,
                },
                recv(pinger) -> _ => {
                    s.broadcast(&Msg::Ping, None);
                    s.update_status();
                    s.send_group(false);
                }
                recv(edge_tick) -> _ => s.check_edge_wait(),
                recv(fast) -> _ => {
                    s.check_local_drop();
                    s.update_key_pass();
                    // A live window shown here was put down (button let go).
                    if s.active.is_none() && s.ctx.hub.dragged_viewer().is_some() && !dnd::left_button_down() {
                        s.ctx.hub.drag_ended();
                    }
                }
            }
        }
        if s.active.is_some() {
            s.go_local(Side::Left, 0.5);
        }
        for p in s.peers.values() {
            p.link.close();
        }
        // Make sure the port is released before a restart tries to bind it again.
        for a in acceptors {
            let _ = a.join();
        }
        Ok(())
    }

    fn me(&self) -> String {
        self.ctx.cfg.name.clone()
    }

    fn update_status(&self) {
        let mut st = self.ctx.status.lock();
        st.peers = self.peers.values().map(|p| PeerStatus { name: p.name.clone(), os: p.os, addr: p.addr.ip().to_string(), screen: p.screen }).collect();
        st.peers.sort_by(|a, b| a.name.cmp(&b.name));
        st.active = self.active.and_then(|(id, _, _)| self.peers.get(&id)).map(|p| p.name.clone()).unwrap_or_default();
        let here = (self.driver.is_none() && self.active.is_none()) || matches!(self.active, Some((THIS, _, _)));
        self.ctx.hub.set_pointer_here(here);
        st.pairing_code = Some(self.ctx.pair.lock().code.clone());
        st.paired = paired_list(&self.ctx.cfg.trusted);
        st.layout = self.ctx.cfg.layout.clone();
        st.wakeable = if self.ctx.cfg.wake_on_lan { self.ctx.cfg.macs.keys().cloned().collect() } else { vec![] };
        st.message = if self.peers.is_empty() {
            format!("Waiting for other computers… (port {})", st.port)
        } else {
            format!("Sharing with {} computer(s)", self.peers.len())
        };
    }

    fn set_layout(&mut self, l: Layout) {
        self.ctx.cfg.layout = l;
        self.ctx.save_config();
        self.update_status();
        self.send_group(true);
    }

    /// Tell the clients the arrangement and pairing code (when they change),
    /// so every computer can show them.
    fn send_group(&mut self, force: bool) {
        let code = self.ctx.pair.lock().code.clone();
        let now = (self.ctx.cfg.layout.clone(), code.clone());
        if !force && self.group_sent.as_ref() == Some(&now) {
            return;
        }
        let msg = Msg::Group { layout: now.0.clone(), code, hub: self.me() };
        self.broadcast(&msg, None);
        self.group_sent = Some(now);
    }

    fn forget(&mut self, device: &str) {
        if let Some(t) = self.ctx.cfg.trusted.remove(device) {
            log::info!("forgot {}", t.name);
            *self.ctx.trust.lock() = self.ctx.cfg.trusted.clone();
            self.ctx.save_config();
        }
        let ids: Vec<u64> = self.peers.iter().filter(|(_, p)| p.device.as_deref() == Some(device)).map(|(&i, _)| i).collect();
        for id in ids {
            self.drop_peer(id);
        }
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
            self.ctx.hub.peer_left(&p.name);
            // Tell the others its windows are gone.
            let gone = Msg::Ext { to: "*".into(), from: p.name.clone(), ext: Ext::Windows { list: vec![] } };
            for q in self.peers.values() {
                q.link.send(gone.clone());
            }
            let quiet_off = Msg::Ext { to: "*".into(), from: p.name.clone(), ext: Ext::Quiet { on: false } };
            for q in self.peers.values() {
                q.link.send(quiet_off.clone());
            }
            {
                let mut routes = self.routes.lock();
                if routes.get(&p.name).map(|l| l.is_dead()).unwrap_or(false) {
                    routes.remove(&p.name);
                }
            }
            for f in self.ctx.inbox.abort_from(&p.name) {
                self.common.on_finished(f);
            }
            if self.driver == Some(id) {
                // Its keyboard and mouse were in use: this computer's take over.
                self.driver = None;
                self.release_local();
                if let Some((a, _, _)) = self.active.take() {
                    if a != THIS {
                        self.release_held(a);
                        self.send(a, Msg::Leave);
                    }
                }
                self.capture.idle_cursor(false);
            } else if matches!(self.active, Some((a, _, _)) if a == id) {
                self.active = None;
                self.held_keys.clear();
                self.held_buttons.clear();
                if self.driver.is_none() {
                    let (cx, cy) = self.capture.screen().center();
                    self.capture.release(cx, cy);
                }
            }
            self.update_status();
        }
    }

    // ---------- network

    fn on_net(&mut self, ev: NetEvent) {
        match ev {
            NetEvent::Paired { device, name, key } => {
                log::info!("paired with {name}");
                self.ctx.cfg.trusted.insert(device, crate::config::Trusted { name: name.clone(), key });
                *self.ctx.trust.lock() = self.ctx.cfg.trusted.clone();
                self.ctx.save_config();
                self.ctx.note(note(format!("Paired with {name}"), "It will connect automatically from now on.", vec![]));
                self.update_status();
            }
            NetEvent::Connected { id, mut name, os, screen, addr, link, device } => {
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
                {
                    let l = link.clone();
                    let to = name.clone();
                    self.ctx.hub.peer_joined(&name, |from, ext| {
                        l.send_bulk_wait(Msg::Ext { to: to.clone(), from: from.to_string(), ext });
                    });
                }
                self.peers.insert(id, Peer { name, device, os, screen, addr, link });
                self.send_group(true);
                self.update_status();
            }
            NetEvent::Disconnected(id) => self.drop_peer(id),
            NetEvent::Finished(f) => self.common.on_finished(f),
            NetEvent::Msg(id, msg) => self.on_peer_msg(id, msg),
        }
    }

    fn on_peer_msg(&mut self, id: u64, msg: Msg) {
        let Some(from) = self.peers.get(&id).map(|p| p.name.clone()) else {
            return;
        };
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
            Msg::DragWindow { id: q, window } => self.on_drag_window(q, window),
            Msg::DragViewer { id: q, stream } => self.on_drag_viewer(q, stream),
            Msg::Mac(mac) => {
                if wol::parse_mac(&mac).is_some() && self.ctx.cfg.macs.get(&from) != Some(&mac) {
                    self.ctx.cfg.macs.insert(from, mac);
                    self.ctx.save_config();
                    self.update_status();
                }
            }
            Msg::Ping => self.send(id, Msg::Pong),
            Msg::EdgeHit { side, frac } => self.on_edge_hit(id, side, frac),
            Msg::Drive(ev) => {
                if self.driver == Some(id) {
                    self.on_drive(id, ev);
                }
            }
            Msg::LocalPos { x, y } => {
                // Its own mouse moved the pointer while we were showing it there.
                if let Some((a, _, _)) = self.active {
                    if a == id {
                        self.active = Some((a, x, y));
                    }
                }
            }
            Msg::SetLayout(l) => {
                self.set_layout(l);
            }
            Msg::SendFiles { to, offer, origin, files } => {
                let me = self.me();
                if to == me || to == "*" {
                    if let Some(req) = self.common.on_offer(offer, origin.clone(), OfferKind::Send, files.clone(), here) {
                        self.send_to_name(&origin, req);
                    }
                }
                if to != me {
                    self.route_send(Msg::SendFiles { to, offer, origin, files }, Some(id));
                }
            }
            Msg::GoTo { name } => {
                if self.driver.is_none() || self.driver == Some(id) {
                    let target = if name == self.me() { SERVER.to_string() } else { name };
                    self.drive_to(id, &target, Side::Left, 0.5);
                    self.center_pointer();
                }
            }
            Msg::Learn { names } => self.start_learning(names),
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
        let me = self.me();
        self.ctx.hub.clip_seen(&me, &c);
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
        if let (Some(c), InputEvent::LocalMove { x, y }) = (self.driver, &ev) {
            // Our own moves (another computer using this screen) aren't the user's.
            if self.injected.ours((*x, *y)) {
                return;
            }
            // This computer's own mouse moved: it's in use here again.
            log::debug!("this computer's mouse is in use again");
            self.stop_driver(c);
        }
        match (ev, self.active) {
            (InputEvent::LocalMove { x, y }, None) => {
                self.last_local = (x, y);
                let screen = self.capture.screen();
                let Some(side) = touching_edge(&screen, x, y) else {
                    self.edge_wait = None;
                    return;
                };
                let frac = edge_fraction(&screen, side, x, y);
                let neighbor = self.ctx.cfg.layout.neighbor(SERVER, side).map(str::to_string).filter(|n| self.can_enter(None, n));
                if neighbor.is_none() && self.learning.is_empty() {
                    return;
                }
                // Corners never hop; a full-screen game or video keeps the
                // pointer; and the pointer rests a moment against the edge.
                if self.pinned || !(CORNER..=1.0 - CORNER).contains(&frac) || self.ctx.hub.fullscreen("") {
                    self.edge_wait = None;
                    return;
                }
                match self.edge_wait {
                    Some((s, t)) if s == side && t.elapsed() >= EDGE_DWELL => self.edge_wait = None,
                    Some((s, _)) if s == side => return,
                    _ => {
                        self.edge_wait = Some((side, Instant::now()));
                        return;
                    }
                }
                if !self.learning.is_empty() {
                    self.learn_place(SERVER, side);
                    return;
                }
                let Some(target) = neighbor else { return };
                match self.peer_by_name(&target) {
                    Some(id) => {
                        self.start_local_drag_if_any();
                        // A live window from another computer, dragged by its title bar?
                        let viewer = self.local_viewer_drag();
                        let window = if viewer.is_none() { self.local_window_drag(x, y) } else { None };
                        self.enter(id, side, frac);
                        if matches!(self.active, Some((a, _, _)) if a == id) {
                            if let Some(stream) = viewer {
                                self.ctx.hub.move_viewer(stream, &target);
                            } else if let Some(w) = window {
                                let me = self.me();
                                self.ctx.hub.offer_window(&target, &me, w);
                            }
                        }
                    }
                    None => self.maybe_wake(&target),
                }
            }
            (InputEvent::Delta { dx, dy }, Some((id, x, y))) => {
                let Some(p) = self.peers.get(&id) else { return };
                let (w, h, pname) = (p.screen.w, p.screen.h, p.name.clone());
                match apply_delta(w, h, x, y, dx, dy) {
                    Ok((nx, ny)) => {
                        self.push = 0;
                        self.active = Some((id, nx, ny));
                        self.send(id, Msg::Move { x: nx, y: ny });
                    }
                    Err((side, frac)) => {
                        // Hop only after pushing a little past the edge, never
                        // from a corner, a pinned screen or a full-screen game.
                        let over = match side {
                            Side::Left | Side::Right => dx.abs(),
                            Side::Top | Side::Bottom => dy.abs(),
                        };
                        self.push += over;
                        let blocked = self.pinned || !(CORNER..=1.0 - CORNER).contains(&frac) || self.ctx.hub.fullscreen(&pname) || self.push < EDGE_PUSH;
                        if !blocked {
                            self.push = 0;
                        }
                        let neighbor = if blocked { None } else { self.ctx.cfg.layout.neighbor(&pname, side).map(str::to_string).filter(|n| self.can_enter(None, n)) };
                        match neighbor.as_deref() {
                            Some(SERVER) => {
                                self.start_remote_drag_if_any(id, &pname, SERVER);
                                self.go_local(side, frac);
                            }
                            Some(n) if self.peer_by_name(n).is_some() => {
                                let next = self.peer_by_name(n).unwrap();
                                self.start_remote_drag_if_any(id, &pname, n);
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
                if down {
                    if let Some(h) = hotkey(&self.raw_keys, key) {
                        self.run_hotkey(h);
                        return;
                    }
                    self.raw_keys.insert(key);
                } else {
                    self.raw_keys.remove(&key);
                }
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
        let Some(mac) = self.ctx.cfg.macs.get(name).cloned() else {
            return;
        };
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
                let carrying = self.drag.as_ref().map(|d| d.origin == self.ctx.cfg.name).unwrap_or(false);
                let ok = if carrying { self.capture.grab_carrying_drag() } else { self.capture.grab() };
                if !ok {
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
        self.key_pass = None;
        self.ctx.hub.lower_hidden();
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

    // ---------- shortcuts and edges

    fn run_hotkey(&mut self, h: Hotkey) {
        match h {
            Hotkey::Jump(side) => self.jump(side),
            Hotkey::Pin => self.toggle_pin(),
        }
    }

    fn toggle_pin(&mut self) {
        self.pinned = !self.pinned;
        let (t, b) = if self.pinned {
            ("Pointer stays on this screen", "Press Ctrl+Alt+Shift+L to let it move between screens again.")
        } else {
            ("Pointer moves between screens again", "")
        };
        self.ctx.hub.notice_everywhere(t, b, "pin");
    }

    /// Jump to the next screen in that direction from the one the pointer is on.
    fn jump(&mut self, side: Side) {
        if let Some(c) = self.driver {
            let from = match self.active {
                Some((THIS, _, _)) => SERVER.to_string(),
                Some((a, _, _)) => self.peers.get(&a).map(|p| p.name.clone()).unwrap_or_default(),
                None => return,
            };
            self.drive_cross(c, &from, side, 0.5);
            return;
        }
        match self.active {
            None => {
                let Some(target) = self.ctx.cfg.layout.neighbor(SERVER, side).map(str::to_string).filter(|n| self.can_enter(None, n)) else { return };
                if let Some(id) = self.peer_by_name(&target) {
                    self.enter(id, side, 0.5);
                }
            }
            Some((id, _, _)) => {
                let Some(pname) = self.peers.get(&id).map(|p| p.name.clone()) else { return };
                match self.ctx.cfg.layout.neighbor(&pname, side).map(str::to_string).filter(|n| self.can_enter(None, n)) {
                    Some(n) if n == SERVER => self.go_local(side, 0.5),
                    Some(n) => {
                        if let Some(next) = self.peer_by_name(&n) {
                            self.enter(next, side, 0.5);
                        }
                    }
                    None => {}
                }
            }
        }
    }

    /// The pointer is resting against this screen's edge: hop once it has
    /// rested long enough.
    fn check_edge_wait(&mut self) {
        let Some((side, t)) = self.edge_wait else { return };
        if t.elapsed() < EDGE_DWELL || self.active.is_some() || self.driver.is_some() {
            return;
        }
        match platform::cursor_pos() {
            Some((x, y)) if touching_edge(&self.capture.screen(), x, y) == Some(side) => self.on_input(InputEvent::LocalMove { x, y }),
            _ => self.edge_wait = None,
        }
    }

    // ---------- sending, launching, arranging

    /// Pass "Send with OpenHop" files on to their computer (or everyone).
    fn route_send(&mut self, m: Msg, from: Option<u64>) {
        let Msg::SendFiles { to, .. } = &m else { return };
        if to == "*" {
            self.broadcast(&m, from);
        } else {
            let to = to.clone();
            self.send_to_name(&to, m);
        }
    }

    /// Put the pointer in the middle of the screen it's on.
    fn center_pointer(&mut self) {
        match self.active {
            Some((THIS, _, _)) => {
                let r = self.capture.screen();
                let (x, y) = (r.w / 2, r.h / 2);
                self.local_move(x, y);
                self.active = Some((THIS, x, y));
            }
            Some((id, _, _)) => {
                let Some((w, h)) = self.peers.get(&id).map(|p| (p.screen.w, p.screen.h)) else { return };
                self.active = Some((id, w / 2, h / 2));
                self.send(id, Msg::Move { x: w / 2, y: h / 2 });
            }
            None => {
                if self.driver.is_none() {
                    let (cx, cy) = self.capture.screen().center();
                    self.injected.add((cx, cy));
                    self.capture.release(cx, cy);
                }
            }
        }
    }

    /// Move the pointer to computer `name`'s screen (from the launcher).
    fn go_to(&mut self, name: &str) {
        let target = if name == self.me() { SERVER.to_string() } else { name.to_string() };
        if !self.can_enter(self.driver, &target) {
            self.ctx.hub.notice_everywhere(&format!("{name} isn't shared"), "Its owner switched off using it from other computers.", "info");
            return;
        }
        if let Some(c) = self.driver {
            let target = if name == self.me() { SERVER.to_string() } else { name.to_string() };
            self.drive_to(c, &target, Side::Left, 0.5);
            self.center_pointer();
            return;
        }
        if name == self.me() {
            if self.active.is_some() {
                self.go_local(Side::Left, 0.5);
            }
        } else if let Some(id) = self.peer_by_name(name) {
            self.enter(id, Side::Left, 0.5);
        }
        self.center_pointer();
    }

    fn start_learning(&mut self, names: Vec<String>) {
        let me = self.me();
        self.learning = names.into_iter().filter(|n| *n != me && n != SERVER).collect();
        if let Some(first) = self.learning.first().cloned() {
            self.ctx.hub.notice_everywhere(&format!("Move the pointer toward {first}"), "Push it against the screen edge on the side where it stands.", "pin");
        }
    }

    /// The pointer was pushed against `next_to`'s edge on `side`: the
    /// computer being arranged stands there.
    fn learn_place(&mut self, next_to: &str, side: Side) {
        if self.learning.is_empty() {
            return;
        }
        let name = self.learning.remove(0);
        let (x, y) = if next_to == SERVER { (0, 0) } else { self.ctx.cfg.layout.position(next_to).unwrap_or((0, 0)) };
        let (dx, dy) = match side {
            Side::Left => (-1, 0),
            Side::Right => (1, 0),
            Side::Top => (0, -1),
            Side::Bottom => (0, 1),
        };
        let mut layout = self.ctx.cfg.layout.clone();
        layout.place(&name, x + dx, y + dy);
        self.set_layout(layout);
        let word = match side {
            Side::Left => "left",
            Side::Right => "right",
            Side::Top => "above",
            Side::Bottom => "below",
        };
        let at = if next_to == SERVER { "this computer".to_string() } else { next_to.to_string() };
        let body = match self.learning.first() {
            Some(n) => format!("Now move the pointer toward {n}."),
            None => "All set: move the pointer across to hop.".to_string(),
        };
        let pos = if word == "above" || word == "below" { format!("is {word} {at}") } else { format!("is to the {word} of {at}") };
        self.ctx.hub.notice_everywhere(&format!("{name} {pos}"), &body, "pin");
    }

    // ---------- any computer drives the others

    /// Let go of keys and buttons another computer held on this screen.
    fn release_local(&mut self) {
        let keys: Vec<u16> = self.local_keys.drain().collect();
        let buttons: Vec<MouseButton> = self.local_buttons.drain().collect();
        if let Some(inj) = self.local_inj.as_mut() {
            for k in keys {
                let _ = inj.key(k, false);
            }
            for b in buttons {
                let _ = inj.button(b, false);
            }
        }
    }

    /// Put this screen's pointer at (x, y) (screen coordinates) for another computer.
    fn local_move(&mut self, x: i32, y: i32) {
        let r = self.capture.screen();
        self.injected.add((r.x + x, r.y + y));
        if let Some(inj) = self.local_inj.as_mut() {
            let _ = inj.move_to(r.x + x, r.y + y);
        }
    }

    /// The client driving stops (its pointer is back home, or this computer's
    /// own mouse took over): it lets go of its keyboard and mouse.
    fn stop_driver(&mut self, c: u64) {
        // (-1, -1): its pointer isn't coming home, it just stays hidden there.
        self.send(c, Msg::DriveStop { x: -1, y: -1 });
        self.driver = None;
        self.release_local();
        if let Some((a, _, _)) = self.active.take() {
            if a != THIS {
                self.release_held(a);
                self.send(a, Msg::Leave);
            }
        }
        self.capture.idle_cursor(false);
        self.update_status();
    }

    /// A client's own mouse reached the edge of its screen: if there's a
    /// screen there, its keyboard and mouse drive from now on.
    fn on_edge_hit(&mut self, c: u64, side: Side, frac: f64) {
        let Some(cname) = self.peers.get(&c).map(|p| p.name.clone()) else { return };
        if self.driver == Some(c) {
            return;
        }
        // Arranging the screens: this tells where the next computer is.
        if !self.learning.is_empty() {
            self.learn_place(&cname, side);
            return;
        }
        let Some(target) = self.ctx.cfg.layout.neighbor(&cname, side).map(str::to_string).filter(|n| self.can_enter(Some(c), n)) else { return };
        self.drive_to(c, &target, side, frac);
    }

    /// May the keyboard and mouse of `source` (None: this computer's own)
    /// go onto screen `target` (SERVER: this one)? Going home always may;
    /// otherwise its owner must share it, and the target's owner allow it.
    fn can_enter(&self, source: Option<u64>, target: &str) -> bool {
        let source_name = match source {
            None => SERVER.to_string(),
            Some(c) => self.peers.get(&c).map(|p| p.name.clone()).unwrap_or_default(),
        };
        if target == source_name {
            return true;
        }
        let me = self.ctx.hub.settings();
        let shares = match source {
            None => me.share_input,
            Some(_) => self.ctx.hub.status_of(&source_name).map(|s| s.shares).unwrap_or(true),
        };
        let open = if target == SERVER { me.allow_control } else { self.ctx.hub.status_of(target).map(|s| s.controllable).unwrap_or(true) };
        shares && open
    }

    /// Client `c`'s keyboard and mouse drive onto screen `target` (SERVER: this one).
    fn drive_to(&mut self, c: u64, target: &str, side: Side, frac: f64) {
        let Some(cname) = self.peers.get(&c).map(|p| p.name.clone()) else { return };
        if target == cname {
            // Home: its own keyboard and mouse work there directly.
            if self.driver == Some(c) {
                self.drive_leave();
                let (cw, ch) = self.peers.get(&c).map(|p| (p.screen.w, p.screen.h)).unwrap_or((0, 0));
                self.send(c, Msg::DriveStop { x: cw / 2, y: ch / 2 });
                self.driver = None;
                self.update_status();
            }
            return;
        }
        let target = target.to_string();
        let to_peer = if target == SERVER { None } else { Some(self.peer_by_name(&target)) };
        if to_peer == Some(None) {
            self.maybe_wake(&target);
            return;
        }
        if self.local_inj.is_none() && to_peer.is_none() {
            return;
        }
        // Whoever was driving stops.
        let r = self.capture.screen();
        let (ex, ey) = entry_point(r.w, r.h, side, frac, 3);
        match self.driver {
            Some(other) => self.stop_driver(other),
            None => {
                if let Some((a, _, _)) = self.active.take() {
                    // This computer's mouse was on another screen: it's put down,
                    // where the other pointer arrives (or in the middle).
                    self.release_held(a);
                    self.send(a, Msg::Leave);
                    let (px, py) = if to_peer.is_none() { (r.x + ex, r.y + ey) } else { r.center() };
                    self.key_pass = None;
                    // Its own warp isn't the user moving this mouse.
                    self.injected.add((px, py));
                    self.capture.release(px, py);
                }
            }
        }
        log::debug!("{cname}'s keyboard and mouse now drive ({target})");
        self.driver = Some(c);
        match to_peer.flatten() {
            None => {
                self.capture.idle_cursor(false);
                self.local_move(ex, ey);
                self.active = Some((THIS, ex, ey));
            }
            Some(d) => {
                self.capture.idle_cursor(true);
                self.drive_enter(d, side, frac);
            }
        }
        self.send(c, Msg::DriveStart);
        self.update_status();
    }

    /// The driving client's pointer goes onto client `d`'s screen.
    fn drive_enter(&mut self, d: u64, side: Side, frac: f64) {
        let Some(p) = self.peers.get(&d) else { return };
        let (x, y) = entry_point(p.screen.w, p.screen.h, side, frac, 1);
        self.active = Some((d, x, y));
        self.send(d, Msg::Enter { x, y });
    }

    /// Leave the screen the driving client's pointer is on now.
    fn drive_leave(&mut self) {
        if let Some((a, _, _)) = self.active.take() {
            if a == THIS {
                self.release_local();
                self.capture.idle_cursor(true);
            } else {
                self.release_held(a);
                self.send(a, Msg::Leave);
            }
        }
    }

    /// The driving client's pointer crossed the edge `side` of screen `from`.
    fn drive_cross(&mut self, c: u64, from: &str, side: Side, frac: f64) -> bool {
        let Some(n) = self.ctx.cfg.layout.neighbor(from, side).map(str::to_string).filter(|n| self.can_enter(Some(c), n)) else { return false };
        let cname = self.peers.get(&c).map(|p| p.name.clone()).unwrap_or_default();
        if n == cname {
            // Home: its keyboard and mouse work there directly again.
            self.drive_leave();
            let (cw, ch) = self.peers.get(&c).map(|p| (p.screen.w, p.screen.h)).unwrap_or((0, 0));
            let (x, y) = entry_point(cw, ch, side, frac, 3);
            self.send(c, Msg::DriveStop { x, y });
            self.driver = None;
            self.active = None;
            self.update_status();
            return true;
        }
        if n == SERVER {
            if self.local_inj.is_none() {
                return false;
            }
            self.drive_leave();
            let r = self.capture.screen();
            let (x, y) = entry_point(r.w, r.h, side, frac, 3);
            self.capture.idle_cursor(false);
            self.local_move(x, y);
            self.active = Some((THIS, x, y));
            self.update_status();
            return true;
        }
        match self.peer_by_name(&n) {
            Some(d) => {
                self.drive_leave();
                self.drive_enter(d, side, frac);
                self.update_status();
                true
            }
            None => {
                self.maybe_wake(&n);
                false
            }
        }
    }

    /// Input from the driving client's own keyboard and mouse.
    fn on_drive(&mut self, c: u64, ev: DriveEv) {
        let Some((a, x, y)) = self.active else { return };
        let from_os = self.peers.get(&c).map(|p| p.os).unwrap_or(Os::Other);
        match ev {
            DriveEv::Delta { dx, dy } => {
                let (w, h, name) = if a == THIS {
                    let r = self.capture.screen();
                    (r.w, r.h, SERVER.to_string())
                } else {
                    match self.peers.get(&a) {
                        Some(p) => (p.screen.w, p.screen.h, p.name.clone()),
                        None => return,
                    }
                };
                match apply_delta(w, h, x, y, dx, dy) {
                    Ok((nx, ny)) => {
                        self.push = 0;
                        self.active = Some((a, nx, ny));
                        if a == THIS {
                            self.local_move(nx, ny);
                        } else {
                            self.send(a, Msg::Move { x: nx, y: ny });
                        }
                    }
                    Err((side, frac)) => {
                        self.push += match side {
                            Side::Left | Side::Right => dx.abs(),
                            Side::Top | Side::Bottom => dy.abs(),
                        };
                        let full = if a == THIS { self.ctx.hub.fullscreen("") } else { self.ctx.hub.fullscreen(&name) };
                        let blocked = self.pinned || !(CORNER..=1.0 - CORNER).contains(&frac) || full || self.push < EDGE_PUSH;
                        if !blocked {
                            self.push = 0;
                        }
                        if blocked || !self.drive_cross(c, &name, side, frac) {
                            let (nx, ny) = ((x + dx).clamp(0, w - 1), (y + dy).clamp(0, h - 1));
                            self.active = Some((a, nx, ny));
                            if a == THIS {
                                self.local_move(nx, ny);
                            } else {
                                self.send(a, Msg::Move { x: nx, y: ny });
                            }
                        }
                    }
                }
            }
            DriveEv::Button { button, down } => {
                if a == THIS {
                    if down {
                        self.local_buttons.insert(button);
                    } else if !self.local_buttons.remove(&button) {
                        return;
                    }
                    if let Some(inj) = self.local_inj.as_mut() {
                        let _ = inj.button(button, down);
                    }
                } else {
                    if down {
                        self.held_buttons.insert(button);
                    } else if !self.held_buttons.remove(&button) {
                        return;
                    }
                    self.send(a, Msg::Button { button, down });
                }
            }
            DriveEv::Wheel { dx, dy } => {
                if a == THIS {
                    if let Some(inj) = self.local_inj.as_mut() {
                        let _ = inj.wheel(dx, dy);
                    }
                } else {
                    self.send(a, Msg::Wheel { dx, dy });
                }
            }
            DriveEv::Key { key, down } => {
                if down {
                    if let Some(h) = hotkey(&self.drive_keys, key) {
                        self.run_hotkey(h);
                        return;
                    }
                    self.drive_keys.insert(key);
                } else {
                    self.drive_keys.remove(&key);
                }
                let to_os = if a == THIS { Os::current() } else { self.peers.get(&a).map(|p| p.os).unwrap_or(Os::Other) };
                let key = if should_swap(&self.ctx.cfg, from_os, to_os) { keys::swap_ctrl_meta(key) } else { key };
                if a == THIS {
                    if down {
                        self.local_keys.insert(key);
                    } else if !self.local_keys.remove(&key) {
                        return;
                    }
                    if let Some(inj) = self.local_inj.as_mut() {
                        let _ = inj.key(key, down);
                    }
                } else {
                    if down {
                        self.held_keys.insert(key);
                    } else if !self.held_keys.remove(&key) {
                        return;
                    }
                    self.send(a, Msg::Key { key, down });
                }
            }
        }
    }

    /// Typing goes straight into one of our windows while its live view is
    /// focused on the screen the pointer is on.
    fn update_key_pass(&mut self) {
        let active = self.active.and_then(|(id, _, _)| self.peers.get(&id)).map(|p| p.name.clone());
        let target = self.ctx.hub.key_target().filter(|(viewer, _)| Some(viewer) == active.as_ref()).map(|(_, w)| w);
        // Only while this computer's own keyboard is the one in use.
        let target = target.filter(|_| self.driver.is_none());
        if target != self.key_pass {
            if let Some(w) = target {
                // Keys held now would be stuck over there: release them first.
                if let Some((id, _, _)) = self.active {
                    for k in self.held_keys.drain().collect::<Vec<_>>() {
                        self.send(id, Msg::Key { key: k, down: false });
                    }
                }
                crate::wins::activate(w);
            }
            self.capture.keyboard_passthrough(target.is_some());
            self.key_pass = target;
        }
    }

    // ---------- drag and drop

    /// Leaving this computer with the left button held: is it a file drag?
    fn start_local_drag_if_any(&mut self) -> Option<u64> {
        if !dnd::left_button_down() {
            return None;
        }
        let Some(paths) = dnd::drag_files() else {
            return None;
        };
        let paths: Vec<String> = paths.iter().map(|p| p.to_string_lossy().into_owned()).collect();
        if let Some(Msg::FileOffer { offer, files, .. }) = self.common.offer_local_files(&paths, OfferKind::Drop) {
            log::info!("carrying a drag of {} across", files::label_for(&files));
            dnd::cancel_drag();
            self.drag =
                Some(Drag { origin: self.me(), offer: Some((offer, files)), query: None, dropped_on: None, to: String::new(), started: Instant::now() });
            return None;
        }
        None
    }

    /// Leaving this computer while dragging a window by its title bar: the
    /// window to open live on the other side.
    fn local_window_drag(&mut self, x: i32, y: i32) -> Option<u64> {
        if !self.ctx.hub.settings().window_drag || !dnd::left_button_down() || self.drag.is_some() {
            return None;
        }
        let w = crate::wins::titlebar_window_at(x, y)?;
        log::info!("a window is being dragged across; opening it live on the other side");
        // Cancel the move here (the window goes back where it was) so the
        // window manager lets go of the mouse.
        if !cfg!(target_os = "macos") {
            dnd::cancel_drag();
        }
        Some(w)
    }

    /// Leaving this computer while dragging a live window (shown here) by its
    /// title bar: it goes on to the other screen.
    fn local_viewer_drag(&mut self) -> Option<u64> {
        if self.drag.is_some() {
            return None;
        }
        if !dnd::left_button_down() {
            self.ctx.hub.drag_ended();
            return None;
        }
        let stream = self.ctx.hub.dragged_viewer()?;
        log::info!("a live window is being dragged off this screen");
        // Let the window manager let go of the mouse.
        if !cfg!(target_os = "macos") {
            dnd::cancel_drag();
        }
        Some(stream)
    }

    /// A client says one of the live windows it shows was dragged across.
    fn on_drag_viewer(&mut self, q: u64, stream: u64) {
        let Some(d) = self.drag.take() else { return };
        if d.query != Some(q) {
            self.drag = Some(d);
            return;
        }
        let to = if d.to == SERVER { self.me() } else { d.to.clone() };
        if !to.is_empty() && to != d.origin {
            self.ctx.hub.tell(&d.origin, Ext::WinMoveTo { stream, to });
        }
    }

    /// Leaving a client with the left button held there: ask it what's being dragged.
    fn start_remote_drag_if_any(&mut self, id: u64, name: &str, to: &str) {
        if !self.held_buttons.contains(&MouseButton::Left) {
            return;
        }
        let q = files::new_id();
        self.send(id, Msg::DragQuery { id: q });
        // The query is answered before the button-up that Leave sends.
        self.drag = Some(Drag { origin: name.to_string(), offer: None, query: Some(q), dropped_on: None, to: to.to_string(), started: Instant::now() });
    }

    /// A client says a window (not files) was being dragged across.
    fn on_drag_window(&mut self, q: u64, window: u64) {
        let Some(d) = self.drag.take() else { return };
        if d.query != Some(q) {
            self.drag = Some(d);
            return;
        }
        let to = if d.to == SERVER { self.me() } else { d.to.clone() };
        if !to.is_empty() && to != d.origin {
            self.ctx.hub.offer_window(&to, &d.origin, window);
        }
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
        let Some(mut d) = self.drag.take() else {
            return;
        };
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

fn spawn_acceptor(listener: TcpListener, ctx: Ctx, routes: Routes, net_tx: Sender<NetEvent>, stop: Arc<AtomicBool>) -> Result<std::thread::JoinHandle<()>> {
    static NEXT_ID: AtomicU64 = AtomicU64::new(1);
    listener.set_nonblocking(true)?;
    Ok(std::thread::Builder::new().name("acceptor".into()).spawn(move || {
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
                let (link, mut rx, name, os, screen, device) = match accept_peer(stream, &ctx, id, net_tx.clone()) {
                    Ok(v) => v,
                    Err(e) => {
                        log::warn!("rejected connection from {addr}: {e:#}");
                        return;
                    }
                };
                let peer_name = name.clone();
                if net_tx.send(NetEvent::Connected { id, name, os, screen, addr, link, device }).is_err() {
                    return;
                }
                loop {
                    match rx.recv() {
                        Ok(Msg::Ext { to, ext, .. }) => {
                            // The sender is who this connection says it is.
                            route_ext(&ctx.hub, &me, &peer_name, &to, ext, &routes);
                        }
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
    })?)
}

/// Work out which secret the connecting computer must prove, run the
/// handshake, and (for a pairing) hand it a key of its own.
#[allow(clippy::type_complexity)]
fn accept_peer(mut stream: TcpStream, ctx: &Ctx, id: u64, net_tx: Sender<NetEvent>) -> Result<(Link, SecureReceiver, String, Os, Rect, Option<String>)> {
    let auth = crate::net::read_auth(&mut stream)?;
    let me = ctx.cfg.name.clone();
    match auth {
        crate::net::Auth::Passphrase => {
            if ctx.cfg.passphrase.trim().is_empty() {
                bail!("this computer uses pairing codes, not a passphrase");
            }
            let (link, rx, name, os, screen) = server_handshake(stream, &derive_psk(&ctx.cfg.passphrase), &me, id, net_tx)?;
            Ok((link, rx, name, os, screen, None))
        }
        crate::net::Auth::Key { device } => {
            let key = ctx.trust.lock().get(&device).and_then(|t| crate::config::hex_key(&t.key));
            let Some(key) = key else { bail!("unknown computer {device}: pair it again with the code") };
            let (link, rx, name, os, screen) = server_handshake(stream, &key, &me, id, net_tx)?;
            Ok((link, rx, name, os, screen, Some(device)))
        }
        crate::net::Auth::Pair { device, .. } => {
            let code = {
                let p = ctx.pair.lock();
                if p.locked_until.map(|t| Instant::now() < t).unwrap_or(false) {
                    bail!("too many wrong pairing codes; try again in a minute");
                }
                p.code.clone()
            };
            match server_handshake(stream, &crate::net::pairing_psk(&code), &me, id, net_tx.clone()) {
                Ok((link, rx, name, os, screen)) => {
                    let key = crate::config::random_hex(32);
                    link.send(Msg::Paired { server_device: ctx.cfg.device_id.clone(), key: key.clone() });
                    ctx.trust.lock().insert(device.clone(), crate::config::Trusted { name: name.clone(), key: key.clone() });
                    {
                        let mut p = ctx.pair.lock();
                        p.code = new_code();
                        p.failures = 0;
                    }
                    let _ = net_tx.send(NetEvent::Paired { device: device.clone(), name: name.clone(), key });
                    Ok((link, rx, name, os, screen, Some(device)))
                }
                Err(e) => {
                    let mut p = ctx.pair.lock();
                    p.failures += 1;
                    if p.failures >= 5 {
                        p.locked_until = Some(Instant::now() + Duration::from_secs(60));
                        p.failures = 0;
                        p.code = new_code();
                        log::warn!("five wrong pairing codes: pairing paused for a minute");
                    }
                    Err(e.context("wrong pairing code"))
                }
            }
        }
    }
}

/// Server reader thread: deliver an [`Ext`] here, to one computer, or to all.
fn route_ext(hub: &Arc<Hub>, me: &str, from: &str, to: &str, ext: Ext, routes: &Routes) {
    let bulk = matches!(ext, Ext::WinFrame { .. } | Ext::Windows { .. });
    let forward = |name: &str, ext: Ext| {
        if let Some(l) = routes.lock().get(name).cloned() {
            let m = Msg::Ext { to: to.to_string(), from: from.to_string(), ext };
            if bulk {
                l.send_bulk_wait(m);
            } else {
                l.send(m);
            }
        }
    };
    if to == me {
        hub.handle(from, ext);
    } else if to == "*" {
        let names: Vec<String> = routes.lock().keys().filter(|n| *n != from).cloned().collect();
        for n in names {
            forward(&n, ext.clone());
        }
        hub.handle(from, ext);
    } else {
        forward(to, ext);
    }
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

fn server_handshake(stream: TcpStream, psk: &[u8; 32], my_name: &str, id: u64, net_tx: Sender<NetEvent>) -> Result<(Link, SecureReceiver, String, Os, Rect)> {
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
    /// Where the pointer is (desktop coordinates), when it's here.
    pos: (i32, i32),
    /// Last keyboard or mouse input from the server.
    last_input: Instant,
    /// This computer's own keyboard and mouse (to drive the others), where
    /// they can be captured (not on Wayland).
    capture: Option<Arc<dyn Capture>>,
    cap_rx: Receiver<InputEvent>,
    /// This computer's keyboard and mouse are driving another screen.
    driving: bool,
    /// Where the server put the pointer lately.
    placed: Placed,
    /// The pointer stays on this screen (Ctrl+Alt+Shift+L).
    pinned: bool,
    /// Resting against the screen edge since.
    edge_wait: Option<(Side, Instant)>,
    last_local: (i32, i32),
    /// When this computer's own mouse last reported the edge / its position.
    edge_sent: Instant,
    pos_sent: Instant,
}

/// Where and how the client connects.
struct Target {
    addr: SocketAddr,
    label: String,
    auth: crate::net::Auth,
    psk: [u8; 32],
    /// Device id being paired with (first connection with a code).
    pairing: Option<String>,
}

/// Who to pair with: a discovered computer, or one typed in by address.
#[derive(Clone)]
enum PairWith {
    Device(String),
    Addr(String),
}

enum SessionEnd {
    Stop,
    /// The user asked to pair (with another server, or again).
    Pair(PairWith, String),
    /// The server was forgotten.
    Forgot,
}

impl Client {
    fn run(ctx: Ctx, injector: Box<dyn Injector>, ctl_rx: Receiver<Control>) -> Result<()> {
        let (clip_set, clip_rx) = if ctx.cfg.clipboard_sync {
            let (s, r) = clipboard::start();
            (Some(s), r)
        } else {
            (None, crossbeam_channel::never())
        };
        // A second injector for controlling this computer's windows from a
        // live view on another one.
        if let Ok(mut inj) = platform::create_injector(ctx.cfg.screen, &ctx.cfg.linux_backend) {
            ctx.hub.set_injector(
                Box::new(move |ops: &[platform::InjectOp]| {
                    for op in ops {
                        let _ = platform::apply(inj.as_mut(), *op);
                    }
                }),
                false,
            );
        }
        let mut c = Client {
            common: Common::new(ctx.clone(), clip_set),
            ctx,
            injector,
            held_keys: HashSet::new(),
            held_buttons: HashSet::new(),
            here: false,
            pos: (0, 0),
            last_input: Instant::now(),
            capture: None,
            cap_rx: crossbeam_channel::never(),
            driving: false,
            placed: Placed::default(),
            pinned: false,
            edge_wait: None,
            last_local: (0, 0),
            edge_sent: Instant::now(),
            pos_sent: Instant::now(),
        };
        // This computer's keyboard and mouse can drive the others too
        // (not on Wayland, which doesn't allow capturing them).
        #[cfg(target_os = "linux")]
        let can_capture = !platform::linux_is_wayland();
        #[cfg(not(target_os = "linux"))]
        let can_capture = true;
        match if can_capture { shared_capture(c.ctx.cfg.screen) } else { Err(anyhow!("Wayland")) } {
            Ok((cap, rx)) => {
                c.capture = Some(cap);
                c.cap_rx = rx;
            }
            Err(e) => log::info!("this computer's keyboard and mouse can't be shared from here: {e:#}"),
        }
        let mut last_failed: HashMap<SocketAddr, Instant> = HashMap::new();
        let mut pending_pair: Option<(PairWith, String)> = None;
        while !c.ctx.stopped() {
            let target = match pending_pair.take() {
                Some((with, code)) => c.pair_target(&with, &code),
                None => c.pick_server(&last_failed),
            };
            let Some(t) = target else {
                let needs = c.needs_pairing();
                {
                    let mut st = c.ctx.status.lock();
                    st.needs_pairing = needs;
                    st.paired = paired_list(&c.ctx.cfg.trusted);
                    st.message =
                        if needs { "Not paired yet: choose the computer to pair with below".into() } else { "Looking for the sharing computer…".into() };
                }
                match ctl_rx.recv_timeout(Duration::from_secs(1)) {
                    Ok(Control::Stop) => break,
                    Ok(Control::Pair { device, code }) => pending_pair = Some((PairWith::Device(device), code)),
                    Ok(Control::PairAddr { addr, code }) => pending_pair = Some((PairWith::Addr(addr), code)),
                    Ok(Control::Forget(d)) => c.forget(&d),
                    _ => {}
                }
                continue;
            };
            let (label, addr, pairing) = (t.label.clone(), t.addr, t.pairing.clone());
            {
                let mut st = c.ctx.status.lock();
                st.needs_pairing = false;
                st.pairing_with = pairing.clone();
                st.pair_error = None;
                st.message = if pairing.is_some() { format!("Pairing with {label}…") } else { format!("Connecting to {label}…") };
            }
            match c.session(t, &ctl_rx, &clip_rx) {
                Ok(SessionEnd::Stop) => break,
                Ok(SessionEnd::Pair(d, code)) => pending_pair = Some((d, code)),
                Ok(SessionEnd::Forgot) => {}
                Err(e) => {
                    log::warn!("{label}: {e:#}");
                    if pairing.is_some() {
                        let mut st = c.ctx.status.lock();
                        st.pairing_with = None;
                        st.pair_error = Some(format!("That code didn't work. Check the code shown on {label} and try again."));
                    } else {
                        c.ctx.set_error(Some(format!("{label}: {e:#}")));
                    }
                    last_failed.insert(addr, Instant::now());
                    c.release_all();
                    c.here = false;
                    {
                        let mut st = c.ctx.status.lock();
                        st.peers.clear();
                        st.active.clear();
                    }
                    match ctl_rx.recv_timeout(Duration::from_secs(2)) {
                        Ok(Control::Stop) => break,
                        Ok(Control::Pair { device, code }) => pending_pair = Some((PairWith::Device(device), code)),
                        Ok(Control::PairAddr { addr, code }) => pending_pair = Some((PairWith::Addr(addr), code)),
                        Ok(Control::Forget(d)) => c.forget(&d),
                        _ => {}
                    }
                }
            }
        }
        c.release_all();
        Ok(())
    }

    /// Not paired with any server and no passphrase to fall back on.
    fn needs_pairing(&self) -> bool {
        let paired = self.ctx.cfg.server_device.as_ref().map(|d| self.ctx.cfg.trusted.contains_key(d)).unwrap_or(false);
        !paired && self.ctx.cfg.passphrase.trim().is_empty()
    }

    fn forget(&mut self, device: &str) {
        self.ctx.cfg.trusted.remove(device);
        if self.ctx.cfg.server_device.as_deref() == Some(device) {
            self.ctx.cfg.server_device = None;
        }
        *self.ctx.trust.lock() = self.ctx.cfg.trusted.clone();
        self.ctx.save_config();
        self.ctx.status.lock().paired = paired_list(&self.ctx.cfg.trusted);
    }

    fn manual_addr(&self) -> Option<(SocketAddr, String)> {
        let a = self.ctx.cfg.server_addr.as_ref()?.trim().to_string();
        parse_addr(&a).map(|sa| (sa, a))
    }

    fn pick_server(&self, failed: &HashMap<SocketAddr, Instant>) -> Option<Target> {
        let me = self.ctx.cfg.device_id.clone();
        // 1. The server we're paired with (by its device id).
        if let Some(dev) = self.ctx.cfg.server_device.clone() {
            if let Some(key) = self.ctx.cfg.trusted.get(&dev).and_then(|t| crate::config::hex_key(&t.key)) {
                let name = self.ctx.cfg.trusted[&dev].name.clone();
                let found = match self.manual_addr() {
                    Some((addr, _)) => Some(addr),
                    None => self.ctx.discovery.servers().into_iter().find(|s| s.device == dev).map(|s| s.addr),
                };
                return found.map(|addr| Target { addr, label: name, auth: crate::net::Auth::Key { device: me }, psk: key, pairing: None });
            }
        }
        // 2. A shared passphrase: any server on the network.
        if self.ctx.cfg.passphrase.trim().is_empty() {
            return None;
        }
        let psk = derive_psk(&self.ctx.cfg.passphrase);
        if let Some((addr, label)) = self.manual_addr() {
            return Some(Target { addr, label, auth: crate::net::Auth::Passphrase, psk, pairing: None });
        }
        let recently_failed = |a: &SocketAddr| failed.get(a).map(|t| t.elapsed() < Duration::from_secs(5)).unwrap_or(false);
        self.ctx
            .discovery
            .servers()
            .into_iter()
            .filter(|s| self.ctx.cfg.server_name.as_ref().map(|n| n == &s.name).unwrap_or(true))
            .find(|s| !recently_failed(&s.addr))
            .map(|s| Target { addr: s.addr, label: s.name, auth: crate::net::Auth::Passphrase, psk, pairing: None })
    }

    fn pair_target(&mut self, with: &PairWith, code: &str) -> Option<Target> {
        let auth = crate::net::Auth::Pair { device: self.ctx.cfg.device_id.clone(), name: self.ctx.cfg.name.clone() };
        let psk = crate::net::pairing_psk(code);
        match with {
            PairWith::Device(device) => {
                // Just (re)started: give discovery a moment to see it again.
                let mut found = None;
                for _ in 0..50 {
                    found = self.ctx.discovery.servers().into_iter().find(|s| &s.device == device);
                    if found.is_some() || self.ctx.stopped() {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
                let Some(s) = found else {
                    self.ctx.status.lock().pair_error = Some("That computer isn't visible any more. Is OpenHop running on it?".into());
                    return None;
                };
                Some(Target { addr: s.addr, label: s.name, auth, psk, pairing: Some(device.clone()) })
            }
            PairWith::Addr(a) => {
                let resolved = parse_addr(a);
                let Some(addr) = resolved else {
                    self.ctx.status.lock().pair_error =
                        Some(format!("\"{a}\" isn't a valid address. Use the address shown on the other computer, like 192.168.1.20."));
                    return None;
                };
                // Remember it: on this network discovery may not work, so reconnect by address.
                self.ctx.cfg.server_addr = Some(a.trim().to_string());
                Some(Target { addr, label: a.trim().to_string(), auth, psk, pairing: Some(a.trim().to_string()) })
            }
        }
    }

    fn session(&mut self, t: Target, ctl_rx: &Receiver<Control>, clip_rx: &Receiver<ClipData>) -> Result<SessionEnd> {
        let (addr, label) = (t.addr, t.label.as_str());
        let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(4)).context("could not connect")?;
        let local_ip = stream.local_addr().ok().map(|a| a.ip());
        crate::net::write_auth(&mut stream, &t.auth)?;
        let (tx, mut rx) = handshake(stream, &t.psk, true)?;
        let mut screen = self.injector.screen();
        self.ctx.hub.set_screen(screen);
        tx.send(&Msg::Hello { version: PROTOCOL_VERSION, name: self.ctx.cfg.name.clone(), os: Os::current(), screen })?;
        rx.set_timeout(Some(Duration::from_secs(10)))?;
        let refused = match t.auth {
            crate::net::Auth::Passphrase => "the server refused: is the passphrase the same on both computers?",
            crate::net::Auth::Key { .. } => "the server doesn't recognise this computer any more: pair again with its code",
            crate::net::Auth::Pair { .. } => "wrong pairing code",
        };
        let (server_name, server_os) = match rx.recv().map_err(|_| anyhow!(refused))? {
            Msg::Welcome { version, name, os } if version == PROTOCOL_VERSION => (name, os),
            Msg::Welcome { version, .. } => {
                bail!("the server runs OpenHop protocol v{version}, this computer runs v{PROTOCOL_VERSION}: update both to the same version")
            }
            _ => bail!("unexpected reply"),
        };
        rx.set_timeout(Some(PEER_TIMEOUT))?;
        log::info!("connected to {server_name} ({}) at {addr}", server_os.label());
        self.ctx.set_error(None);
        {
            let mut st = self.ctx.status.lock();
            st.message = format!("Connected to {label}");
            st.pairing_with = None;
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
            let l = link.clone();
            let me = self.ctx.cfg.name.clone();
            self.ctx.hub.set_out(Some(Box::new(move |to: &str, ext: Ext| {
                let bulk = matches!(ext, Ext::WinFrame { .. } | Ext::Windows { .. });
                let m = Msg::Ext { to: to.to_string(), from: me.clone(), ext };
                if bulk {
                    l.send_bulk_wait(m);
                } else {
                    l.send(m);
                }
            })));
        }
        {
            let inbox = self.ctx.inbox.clone();
            let me = self.ctx.cfg.name.clone();
            let hub = self.ctx.hub.clone();
            std::thread::Builder::new().name("client-rx".into()).spawn(move || loop {
                match rx.recv() {
                    Ok(Msg::Ext { from, ext, .. }) => hub.handle(&from, ext),
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
        // Wi-Fi adapters doze between packets to save power and then hold
        // incoming packets for up to a few hundred milliseconds: typing
        // (sparse packets, unlike mouse movement) would lag. A tiny packet
        // every 40 ms while the keyboard and mouse are here keeps the
        // connection awake on both ends.
        let awake = tick(Duration::from_millis(40));
        let result = loop {
            select! {
                recv(ev_rx) -> ev => match ev {
                    Ok(ClientEvent::Msg(m)) => self.on_msg(m, screen, &link, &server_name),
                    Ok(ClientEvent::Finished(f)) => self.common.on_finished(f),
                    Ok(ClientEvent::Lost(e)) => break Err(e),
                    Err(_) => break Err(anyhow!("connection lost")),
                },
                recv(clip_rx) -> c => if let Ok(c) = c {
                    self.ctx.hub.clip_seen(&self.ctx.cfg.name, &c);
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
                    Ok(Control::Stop) | Err(_) => break Ok(SessionEnd::Stop),
                    Ok(Control::Pair { device, code }) => break Ok(SessionEnd::Pair(PairWith::Device(device), code)),
                    Ok(Control::PairAddr { addr, code }) => break Ok(SessionEnd::Pair(PairWith::Addr(addr), code)),
                    Ok(Control::Forget(d)) => {
                        let current = self.ctx.cfg.server_device.as_deref() == Some(d.as_str());
                        self.forget(&d);
                        if current {
                            break Ok(SessionEnd::Forgot);
                        }
                    }
                    Ok(Control::Jump(side)) => {
                        if !self.driving {
                            link.send(Msg::EdgeHit { side, frac: 0.5 });
                        }
                    }
                    Ok(Control::Pin) => {
                        self.pinned = !self.pinned;
                        let t = if self.pinned { "Pointer stays on this screen" } else { "Pointer moves between screens again" };
                        self.ctx.hub.notice_everywhere(t, "", "pin");
                    }
                    Ok(Control::SetLayout(l)) => {
                        // The arrangement is kept by the hub: change it there.
                        self.ctx.status.lock().layout = l.clone();
                        link.send(Msg::SetLayout(l));
                    }
                    Ok(Control::SendFiles { to, paths }) => {
                        if let Some(m) = self.common.send_files(&to, &paths) {
                            link.send(m);
                        }
                    }
                    Ok(Control::ShelfAdd(paths)) => self.common.shelf_add(&paths),
                    Ok(Control::ShelfTake { origin, id }) => {
                        if let Some(req) = self.common.shelf_take(&origin, id) {
                            link.send(req);
                        }
                    }
                    Ok(Control::GoTo(name)) => {
                        link.send(Msg::GoTo { name });
                    }
                    Ok(Control::Learn(names)) => {
                        link.send(Msg::Learn { names });
                    }
                },
                recv(self.cap_rx) -> ev => if let Ok(ev) = ev { self.on_local_input(ev, screen, &link) },
                recv(awake) -> _ => {
                    if self.here && self.last_input.elapsed() < Duration::from_secs(30) {
                        link.send(Msg::Ping);
                    }
                    // Resting against the edge long enough: hop.
                    if let Some((side, t)) = self.edge_wait {
                        if t.elapsed() >= EDGE_DWELL && !self.driving {
                            match platform::cursor_pos() {
                                Some((x, y)) if touching_edge(&screen, x, y) == Some(side) => {
                                    self.edge_wait = None;
                                    link.send(Msg::EdgeHit { side, frac: edge_fraction(&screen, side, x, y) });
                                }
                                _ => self.edge_wait = None,
                            }
                        }
                    }
                },
                recv(ticker) -> _ => {
                    let now = self.injector.screen();
                    if now != screen {
                        screen = now;
                        self.ctx.hub.set_screen(now);
                        self.ctx.status.lock().screen = Some(now);
                        link.send(Msg::Screen(now));
                    }
                },
            }
        };
        self.ctx.hub.set_out(None);
        if self.driving {
            self.driving = false;
            if let Some(cap) = &self.capture {
                let (cx, cy) = screen.center();
                cap.release(cx, cy);
            }
        }
        if let Some(cap) = &self.capture {
            cap.idle_cursor(false);
        }
        link.close();
        for f in self.ctx.inbox.abort_from(&server_name) {
            self.common.on_finished(f);
        }
        result
    }

    fn on_msg(&mut self, m: Msg, screen: Rect, link: &Link, server: &str) {
        if matches!(m, Msg::Enter { .. } | Msg::Move { .. } | Msg::Button { .. } | Msg::Wheel { .. } | Msg::Key { .. }) {
            self.last_input = Instant::now();
        }
        let r = match m {
            Msg::Enter { x, y } => {
                self.here = true;
                self.ctx.hub.set_pointer_here(true);
                if let Some(cap) = &self.capture {
                    cap.idle_cursor(false);
                }
                self.ctx.hub.lower_hidden();
                self.ctx.status.lock().active = self.ctx.cfg.name.clone();
                if let Some(req) = self.common.arrived() {
                    link.send(req);
                }
                self.pos = (screen.x + x, screen.y + y);
                self.placed.add(self.pos);
                self.injector.move_to(screen.x + x, screen.y + y)
            }
            Msg::Move { x, y } => {
                self.pos = (screen.x + x, screen.y + y);
                self.placed.add(self.pos);
                self.injector.move_to(screen.x + x, screen.y + y)
            }
            Msg::Leave => {
                self.here = false;
                self.ctx.hub.set_pointer_here(false);
                self.ctx.status.lock().active.clear();
                self.release_all();
                // One pointer: this screen's hides until its own mouse moves.
                if let Some(cap) = &self.capture {
                    cap.idle_cursor(true);
                }
                Ok(())
            }
            Msg::DriveStart => {
                // This computer's keyboard and mouse now drive another screen.
                if let Some(cap) = &self.capture {
                    if cap.grab() {
                        self.driving = true;
                        self.here = false;
                        self.ctx.hub.set_pointer_here(false);
                        self.release_all();
                        self.ctx.status.lock().active.clear();
                    }
                }
                Ok(())
            }
            Msg::DriveStop { x, y } => {
                // The pointer is back on this screen: the keyboard and mouse work here again.
                let home = x >= 0 && y >= 0;
                if self.driving {
                    self.driving = false;
                    if let Some(cap) = &self.capture {
                        if home {
                            cap.release(screen.x + x, screen.y + y);
                        } else {
                            // Another computer's mouse took over: this pointer stays out of sight.
                            let (cx, cy) = screen.center();
                            cap.release(cx, cy);
                            cap.idle_cursor(true);
                        }
                    }
                }
                self.here = false;
                self.ctx.hub.set_pointer_here(home);
                if home {
                    self.ctx.status.lock().active = self.ctx.cfg.name.clone();
                }
                Ok(())
            }
            Msg::Group { layout, code, hub } => {
                let mut st = self.ctx.status.lock();
                st.layout = layout;
                st.pairing_code = Some(code);
                st.hub = Some(hub);
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
                    if button == MouseButton::Left {
                        self.ctx.hub.drag_ended();
                    }
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
            Msg::SendFiles { offer, origin, files, .. } => {
                if let Some(req) = self.common.on_offer(offer, origin, OfferKind::Send, files, self.here) {
                    link.send(req);
                }
                Ok(())
            }
            Msg::FileRequest { offer, requester, .. } => {
                // The server relays our data to the requester.
                self.ctx.outbox.serve(offer, requester, link.clone());
                Ok(())
            }
            Msg::Paired { server_device, key } => {
                log::info!("paired with {server}");
                self.ctx.cfg.trusted.insert(server_device.clone(), crate::config::Trusted { name: server.to_string(), key });
                self.ctx.cfg.server_device = Some(server_device);
                *self.ctx.trust.lock() = self.ctx.cfg.trusted.clone();
                self.ctx.save_config();
                self.ctx.status.lock().paired = paired_list(&self.ctx.cfg.trusted);
                self.ctx.note(note(format!("Paired with {server}"), "It will connect automatically from now on.", vec![]));
                Ok(())
            }
            Msg::DragQuery { id } => {
                let (px, py) = self.pos;
                let injector = &mut self.injector;
                let mut nudge = |dx: i32| {
                    let _ = injector.move_to(px + dx.max(0), py);
                };
                // A live window shown here being carried by its title bar?
                // (The button is the one we pressed for the server.)
                let viewer = if self.held_buttons.contains(&MouseButton::Left) { self.ctx.hub.dragged_viewer() } else { None };
                log::info!("drag across the edge: {}", if viewer.is_some() { "a live window" } else { "looking for files or a window" });
                let found = if viewer.is_some() { None } else { dnd::drag_files_at(self.pos, &mut nudge) };
                let reply = match found {
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
                    None if viewer.is_some() => {
                        // A live window shown here, dragged by its title bar.
                        let stream = viewer.unwrap_or_default();
                        if !cfg!(target_os = "macos") {
                            let _ = self.injector.key(ESC, true);
                            let _ = self.injector.key(ESC, false);
                        }
                        Msg::DragViewer { id, stream }
                    }
                    None => {
                        // Not files: a window dragged by its title bar?
                        let window = if self.ctx.hub.settings().window_drag { crate::wins::titlebar_window_at(self.pos.0, self.pos.1) } else { None };
                        match window {
                            Some(window) => {
                                if !cfg!(target_os = "macos") {
                                    // Cancel the move so the window manager lets go.
                                    let _ = self.injector.key(ESC, true);
                                    let _ = self.injector.key(ESC, false);
                                }
                                Msg::DragWindow { id, window }
                            }
                            None => Msg::DragReply { id, offer: None, files: vec![] },
                        }
                    }
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

    /// This computer's own keyboard and mouse.
    fn on_local_input(&mut self, ev: InputEvent, screen: Rect, link: &Link) {
        match ev {
            InputEvent::LocalMove { x, y } => {
                if self.driving {
                    return;
                }
                // Where the server just put the pointer: not the user's own move.
                if self.here && self.placed.ours((x, y)) {
                    return;
                }
                // This computer's own mouse or touchpad: the pointer is here.
                self.ctx.hub.set_pointer_here(true);
                self.last_local = (x, y);
                if self.here {
                    // The touchpad moved the pointer while the server's mouse
                    // was using this screen: keep the server in step.
                    self.pos = (x, y);
                    if self.pos_sent.elapsed() > Duration::from_millis(30) {
                        self.pos_sent = Instant::now();
                        link.send(Msg::LocalPos { x: x - screen.x, y: y - screen.y });
                    }
                }
                match touching_edge(&screen, x, y) {
                    Some(side) => {
                        let frac = edge_fraction(&screen, side, x, y);
                        // Not from corners, a pinned pointer or a full-screen game;
                        // after resting a moment against the edge.
                        if self.pinned || !(CORNER..=1.0 - CORNER).contains(&frac) || self.ctx.hub.fullscreen("") {
                            self.edge_wait = None;
                        } else if self.edge_wait.map(|(s, _)| s != side).unwrap_or(true) {
                            self.edge_wait = Some((side, Instant::now()));
                        } else if self.edge_wait.map(|(_, t)| t.elapsed() >= EDGE_DWELL).unwrap_or(false)
                            && self.edge_sent.elapsed() > Duration::from_millis(250)
                        {
                            self.edge_sent = Instant::now();
                            self.edge_wait = None;
                            link.send(Msg::EdgeHit { side, frac });
                        }
                    }
                    None => self.edge_wait = None,
                }
            }
            _ if !self.driving => {}
            InputEvent::Delta { dx, dy } => {
                link.send(Msg::Drive(DriveEv::Delta { dx, dy }));
            }
            InputEvent::Button { button, down } => {
                link.send(Msg::Drive(DriveEv::Button { button, down }));
            }
            InputEvent::Wheel { dx, dy } => {
                link.send(Msg::Drive(DriveEv::Wheel { dx, dy }));
            }
            InputEvent::Key { key, down } => {
                link.send(Msg::Drive(DriveEv::Key { key, down }));
            }
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
    fn own_pointer_moves_are_recognised() {
        let mut p = Placed::default();
        p.add((100, 200));
        assert!(p.ours((101, 199)));
        assert!(!p.ours((140, 200)));
        let m = Msg::Drive(DriveEv::Key { key: 4, down: true });
        assert_eq!(crate::protocol::decode(&crate::protocol::encode(&m)).unwrap(), m);
        let e = Msg::EdgeHit { side: Side::Left, frac: 0.5 };
        assert!(matches!(crate::protocol::decode(&crate::protocol::encode(&e)).unwrap(), Msg::EdgeHit { side: Side::Left, .. }));
    }

    #[test]
    fn addresses_parse() {
        assert_eq!(parse_addr("192.168.1.20"), Some("192.168.1.20:24850".parse().unwrap()));
        assert_eq!(parse_addr(" 10.0.0.2:24852 "), Some("10.0.0.2:24852".parse().unwrap()));
        assert_eq!(parse_addr("fe80::1"), Some("[fe80::1]:24850".parse().unwrap()));
        assert!(parse_addr("").is_none());
        assert!(parse_addr("not an address !").is_none());
    }

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
