//! The engine: runs either as a server (shares this computer's keyboard and
//! mouse) or a client (is controlled by a server).

use crate::clipboard;
use crate::config::{Config, Role};
use crate::discovery::{Discovered, Discovery};
use crate::keys;
use crate::layout::{apply_delta, edge_fraction, entry_point, touching_edge, Layout, Side, SERVER};
use crate::net::{derive_psk, handshake, SecureReceiver, SecureSender};
use crate::platform::{self, Capture, InputEvent, Injector};
use crate::protocol::{ClipData, MouseButton, Msg, Os, Rect, DEFAULT_PORT, PROTOCOL_VERSION};
use anyhow::{anyhow, bail, Context, Result};
use crossbeam_channel::{select, tick, Receiver, Sender};
use parking_lot::Mutex;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::net::{SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

const PEER_TIMEOUT: Duration = Duration::from_secs(15);

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
}

enum Control {
    Stop,
    SetLayout(Layout),
}

pub struct Engine {
    stop: Arc<AtomicBool>,
    ctl: Sender<Control>,
    status: Arc<Mutex<Status>>,
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
        }));
        let discovery = Arc::new(Discovery::start(cfg.name.clone(), cfg.role, cfg.port).context("starting LAN discovery")?);
        let stop = Arc::new(AtomicBool::new(false));
        let (ctl_tx, ctl_rx) = crossbeam_channel::unbounded();

        let main = match cfg.role {
            Role::Server => {
                let capture = shared_capture(cfg.screen)?;
                status.lock().screen = Some(capture.0.screen());
                let listener = TcpListener::bind(("0.0.0.0", cfg.port))
                    .with_context(|| format!("port {} is busy (is OpenHop already running?)", cfg.port))?;
                let ctx = Ctx { cfg, config_path, status: status.clone(), discovery: discovery.clone(), stop: stop.clone() };
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
                let ctx = Ctx { cfg, config_path, status: status.clone(), discovery: discovery.clone(), stop: stop.clone() };
                std::thread::Builder::new().name("client".into()).spawn(move || {
                    if let Err(e) = Client::run(ctx.clone(), injector, ctl_rx) {
                        log::error!("client stopped: {e:#}");
                        ctx.set_error(Some(format!("{e:#}")));
                    }
                    ctx.status.lock().running = false;
                })?
            }
        };
        Ok(Engine { stop, ctl: ctl_tx, status, discovery, main: Some(main) })
    }

    pub fn status(&self) -> Status {
        let mut s = self.status.lock().clone();
        s.discovered = self.discovery.peers();
        s
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
}

fn should_swap(cfg: &Config, a: Os, b: Os) -> bool {
    cfg.swap_cmd_ctrl && ((a == Os::MacOs) != (b == Os::MacOs))
}

// ---------------------------------------------------------------- server

enum NetEvent {
    Connected { id: u64, name: String, os: Os, screen: Rect, addr: SocketAddr, tx: SecureSender },
    Msg(u64, Msg),
    Disconnected(u64),
}

struct Peer {
    name: String,
    os: Os,
    screen: Rect,
    addr: SocketAddr,
    tx: SecureSender,
}

struct Server {
    ctx: Ctx,
    capture: Arc<dyn Capture>,
    peers: HashMap<u64, Peer>,
    /// Which client has the cursor, and where it is on that client.
    active: Option<(u64, i32, i32)>,
    held_keys: HashSet<u16>,
    held_buttons: HashSet<MouseButton>,
    clip_set: Option<Sender<ClipData>>,
}

impl Server {
    fn run(ctx: Ctx, (capture, input_rx): (Arc<dyn Capture>, Receiver<InputEvent>), listener: TcpListener, ctl_rx: Receiver<Control>) -> Result<()> {
        let (net_tx, net_rx) = crossbeam_channel::unbounded();
        let psk = derive_psk(&ctx.cfg.passphrase);
        spawn_acceptor(listener, psk, ctx.cfg.name.clone(), net_tx, ctx.stop.clone())?;
        let (clip_set, clip_rx) = if ctx.cfg.clipboard_sync {
            let (s, r) = clipboard::start();
            (Some(s), r)
        } else {
            (None, crossbeam_channel::never())
        };
        ctx.set_message(format!("Waiting for other computers… (port {})", ctx.cfg.port));
        let mut s = Server {
            ctx,
            capture,
            peers: HashMap::new(),
            active: None,
            held_keys: HashSet::new(),
            held_buttons: HashSet::new(),
            clip_set,
        };
        let ticker = tick(Duration::from_secs(3));
        loop {
            select! {
                recv(input_rx) -> ev => if let Ok(ev) = ev { s.on_input(ev) },
                recv(net_rx) -> ev => if let Ok(ev) = ev { s.on_net(ev) },
                recv(clip_rx) -> c => if let Ok(c) = c { s.broadcast(&Msg::Clipboard(c), None) },
                recv(ctl_rx) -> c => match c {
                    Ok(Control::SetLayout(l)) => s.set_layout(l),
                    Ok(Control::Stop) | Err(_) => break,
                },
                recv(ticker) -> _ => s.broadcast(&Msg::Ping, None),
            }
        }
        if s.active.is_some() {
            s.go_local(Side::Left, 0.5);
        }
        for p in s.peers.values() {
            p.tx.shutdown();
        }
        Ok(())
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
        st.message = if self.peers.is_empty() {
            format!("Waiting for other computers… (port {})", self.ctx.cfg.port)
        } else {
            format!("Sharing with {} computer(s)", self.peers.len())
        };
    }

    fn save_config(&self) {
        if let Some(p) = &self.ctx.config_path {
            if let Err(e) = self.ctx.cfg.save(p) {
                log::warn!("saving config: {e}");
            }
        }
    }

    fn set_layout(&mut self, l: Layout) {
        self.ctx.cfg.layout = l;
        self.save_config();
        self.update_status();
    }

    fn peer_by_name(&self, name: &str) -> Option<u64> {
        self.peers.iter().find(|(_, p)| p.name == name).map(|(&id, _)| id)
    }

    fn send(&mut self, id: u64, msg: &Msg) {
        let failed = match self.peers.get(&id) {
            Some(p) => p.tx.send(msg).is_err(),
            None => false,
        };
        if failed {
            self.drop_peer(id);
        }
    }

    fn broadcast(&mut self, msg: &Msg, except: Option<u64>) {
        let ids: Vec<u64> = self.peers.keys().copied().filter(|&i| Some(i) != except).collect();
        for id in ids {
            self.send(id, msg);
        }
    }

    fn drop_peer(&mut self, id: u64) {
        if let Some(p) = self.peers.remove(&id) {
            log::info!("{} disconnected", p.name);
            p.tx.shutdown();
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

    fn on_net(&mut self, ev: NetEvent) {
        match ev {
            NetEvent::Connected { id, mut name, os, screen, addr, tx } => {
                if self.peer_by_name(&name).is_some() || name == self.ctx.cfg.name {
                    name = format!("{name} ({})", addr.ip());
                }
                log::info!("{name} connected from {addr} ({} {}x{})", os.label(), screen.w, screen.h);
                if self.ctx.cfg.layout.position(&name).is_none() {
                    self.ctx.cfg.layout.auto_place(&name);
                    self.save_config();
                }
                self.peers.insert(id, Peer { name, os, screen, addr, tx });
                self.update_status();
            }
            NetEvent::Disconnected(id) => self.drop_peer(id),
            NetEvent::Msg(id, msg) => match msg {
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
                Msg::Clipboard(c) => {
                    if let Some(set) = &self.clip_set {
                        let _ = set.send(c.clone());
                    }
                    self.broadcast(&Msg::Clipboard(c), Some(id));
                }
                Msg::Ping => self.send(id, &Msg::Pong),
                _ => {}
            },
        }
    }

    fn on_input(&mut self, ev: InputEvent) {
        match (ev, self.active) {
            (InputEvent::LocalMove { x, y }, None) => {
                let screen = self.capture.screen();
                let Some(side) = touching_edge(&screen, x, y) else { return };
                let Some(target) = self.ctx.cfg.layout.neighbor(SERVER, side).map(str::to_string) else { return };
                if let Some(id) = self.peer_by_name(&target) {
                    let frac = edge_fraction(&screen, side, x, y);
                    self.enter(id, side, frac);
                }
            }
            (InputEvent::Delta { dx, dy }, Some((id, x, y))) => {
                let Some(p) = self.peers.get(&id) else { return };
                let (w, h) = (p.screen.w, p.screen.h);
                match apply_delta(w, h, x, y, dx, dy) {
                    Ok((nx, ny)) => {
                        self.active = Some((id, nx, ny));
                        self.send(id, &Msg::Move { x: nx, y: ny });
                    }
                    Err((side, frac)) => {
                        let neighbor = self.ctx.cfg.layout.neighbor(&p.name, side).map(str::to_string);
                        match neighbor.as_deref() {
                            Some(SERVER) => self.go_local(side, frac),
                            Some(n) if self.peer_by_name(n).is_some() => {
                                let next = self.peer_by_name(n).unwrap();
                                self.enter(next, side, frac);
                            }
                            _ => {
                                let nx = (x + dx).clamp(0, w - 1);
                                let ny = (y + dy).clamp(0, h - 1);
                                if (nx, ny) != (x, y) {
                                    self.active = Some((id, nx, ny));
                                    self.send(id, &Msg::Move { x: nx, y: ny });
                                }
                            }
                        }
                    }
                }
            }
            (InputEvent::Button { button, down }, Some((id, _, _))) => {
                if down {
                    self.held_buttons.insert(button);
                } else {
                    self.held_buttons.remove(&button);
                }
                self.send(id, &Msg::Button { button, down });
            }
            (InputEvent::Wheel { dx, dy }, Some((id, _, _))) => self.send(id, &Msg::Wheel { dx, dy }),
            (InputEvent::Key { key, down }, Some((id, _, _))) => {
                let os = self.peers.get(&id).map(|p| p.os).unwrap_or(Os::Other);
                let key = if should_swap(&self.ctx.cfg, Os::current(), os) { keys::swap_ctrl_meta(key) } else { key };
                if down {
                    self.held_keys.insert(key);
                } else {
                    self.held_keys.remove(&key);
                }
                self.send(id, &Msg::Key { key, down });
            }
            _ => {}
        }
    }

    /// Lift every key and button the active client thinks is held.
    fn release_held(&mut self, id: u64) {
        let keys: Vec<u16> = self.held_keys.drain().collect();
        for key in keys {
            self.send(id, &Msg::Key { key, down: false });
        }
        let buttons: Vec<MouseButton> = self.held_buttons.drain().collect();
        for button in buttons {
            self.send(id, &Msg::Button { button, down: false });
        }
    }

    fn enter(&mut self, id: u64, exit_side: Side, frac: f64) {
        match self.active {
            None => self.capture.grab(),
            Some((old, _, _)) => {
                self.release_held(old);
                self.send(old, &Msg::Leave);
            }
        }
        let Some(p) = self.peers.get(&id) else { return };
        let (x, y) = entry_point(p.screen.w, p.screen.h, exit_side, frac, 1);
        log::debug!("cursor -> {} at ({x},{y})", p.name);
        self.active = Some((id, x, y));
        self.send(id, &Msg::Enter { x, y });
        self.update_status();
    }

    fn go_local(&mut self, exit_side: Side, frac: f64) {
        if let Some((old, _, _)) = self.active.take() {
            self.release_held(old);
            self.send(old, &Msg::Leave);
        }
        let r = self.capture.screen();
        let (x, y) = entry_point(r.w, r.h, exit_side, frac, 3);
        log::debug!("cursor -> local at ({x},{y})");
        self.capture.release(r.x + x, r.y + y);
        self.update_status();
    }
}

fn spawn_acceptor(listener: TcpListener, psk: [u8; 32], my_name: String, net_tx: Sender<NetEvent>, stop: Arc<AtomicBool>) -> Result<()> {
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
            let my_name = my_name.clone();
            let _ = std::thread::Builder::new().name(format!("peer-{addr}")).spawn(move || {
                let _ = stream.set_nonblocking(false);
                let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
                match server_handshake(stream, &psk, &my_name) {
                    Ok((tx, mut rx, name, os, screen)) => {
                        if net_tx.send(NetEvent::Connected { id, name, os, screen, addr, tx }).is_err() {
                            return;
                        }
                        loop {
                            match rx.recv() {
                                Ok(m) => {
                                    if net_tx.send(NetEvent::Msg(id, m)).is_err() {
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
                    }
                    Err(e) => log::warn!("rejected connection from {addr}: {e:#}"),
                }
            });
        }
    })?;
    Ok(())
}

fn server_handshake(stream: TcpStream, psk: &[u8; 32], my_name: &str) -> Result<(SecureSender, SecureReceiver, String, Os, Rect)> {
    let (tx, mut rx) = handshake(stream, psk, false)?;
    rx.set_timeout(Some(Duration::from_secs(10)))?;
    let hello = rx.recv().context("client did not say hello (wrong passphrase?)")?;
    let Msg::Hello { version, name, os, screen } = hello else { bail!("unexpected first message") };
    if version != PROTOCOL_VERSION {
        bail!("{name} runs protocol v{version}, we run v{PROTOCOL_VERSION}; update both");
    }
    tx.send(&Msg::Welcome { version: PROTOCOL_VERSION, name: my_name.to_string(), os: Os::current() })?;
    rx.set_timeout(Some(PEER_TIMEOUT))?;
    Ok((tx, rx, name, os, screen))
}

// ---------------------------------------------------------------- client

struct Client {
    ctx: Ctx,
    injector: Box<dyn Injector>,
    held_keys: HashSet<u16>,
    held_buttons: HashSet<MouseButton>,
}

impl Client {
    fn run(ctx: Ctx, injector: Box<dyn Injector>, ctl_rx: Receiver<Control>) -> Result<()> {
        let (clip_set, clip_rx) = if ctx.cfg.clipboard_sync {
            let (s, r) = clipboard::start();
            (Some(s), r)
        } else {
            (None, crossbeam_channel::never())
        };
        let mut c = Client { ctx, injector, held_keys: HashSet::new(), held_buttons: HashSet::new() };
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
            match c.session(addr, &label, &psk, &ctl_rx, clip_set.as_ref(), &clip_rx) {
                Ok(()) => break, // asked to stop
                Err(e) => {
                    log::warn!("{label}: {e:#}");
                    c.ctx.set_error(Some(format!("{label}: {e:#}")));
                    last_failed.insert(addr, Instant::now());
                    c.release_all();
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

    fn session(
        &mut self,
        addr: SocketAddr,
        label: &str,
        psk: &[u8; 32],
        ctl_rx: &Receiver<Control>,
        clip_set: Option<&Sender<ClipData>>,
        clip_rx: &Receiver<ClipData>,
    ) -> Result<()> {
        let stream = TcpStream::connect_timeout(&addr, Duration::from_secs(4)).context("could not connect")?;
        let (tx, mut rx) = handshake(stream, psk, true)?;
        let mut screen = self.injector.screen();
        tx.send(&Msg::Hello { version: PROTOCOL_VERSION, name: self.ctx.cfg.name.clone(), os: Os::current(), screen })?;
        rx.set_timeout(Some(Duration::from_secs(10)))?;
        let (server_name, server_os) = match rx.recv().map_err(|_| anyhow!("server closed the connection: is the passphrase the same on both computers?"))? {
            Msg::Welcome { version, name, os } if version == PROTOCOL_VERSION => (name, os),
            Msg::Welcome { version, .. } => bail!("server runs protocol v{version}, we run v{PROTOCOL_VERSION}"),
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

        let (net_tx, net_rx) = crossbeam_channel::unbounded();
        std::thread::Builder::new().name("client-rx".into()).spawn(move || loop {
            let r = rx.recv();
            let end = r.is_err();
            if net_tx.send(r).is_err() || end {
                return;
            }
        })?;

        let ticker = tick(Duration::from_secs(2));
        let result = loop {
            select! {
                recv(net_rx) -> m => match m {
                    Ok(Ok(Msg::Ping)) => { let _ = tx.send(&Msg::Pong); }
                    Ok(Ok(Msg::Clipboard(c))) => if let Some(s) = clip_set { let _ = s.send(c); },
                    Ok(Ok(m)) => self.apply(m, screen),
                    Ok(Err(e)) => break Err(e.context("connection lost")),
                    Err(_) => break Err(anyhow!("connection lost")),
                },
                recv(clip_rx) -> c => if let Ok(c) = c {
                    if let Err(e) = tx.send(&Msg::Clipboard(c)) { break Err(e) }
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
                        let _ = tx.send(&Msg::Screen(now));
                    }
                },
            }
        };
        tx.shutdown();
        result
    }

    fn apply(&mut self, m: Msg, screen: Rect) {
        let r = match m {
            Msg::Enter { x, y } => {
                self.ctx.status.lock().active = self.ctx.cfg.name.clone();
                self.injector.move_to(screen.x + x, screen.y + y)
            }
            Msg::Move { x, y } => self.injector.move_to(screen.x + x, screen.y + y),
            Msg::Leave => {
                self.ctx.status.lock().active.clear();
                self.release_all();
                Ok(())
            }
            Msg::Button { button, down } => {
                if down {
                    self.held_buttons.insert(button);
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
            _ => Ok(()),
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
