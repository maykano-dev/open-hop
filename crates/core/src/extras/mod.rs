//! Features beyond sharing the keyboard and mouse: live windows from other
//! computers, a shared window list, dark mode and Do Not Disturb sync,
//! battery saving, sounds and arrival animations.
//!
//! The [`Hub`] lives next to the engine. It talks to other computers with
//! [`Ext`] messages addressed by computer name (the server forwards them).

pub mod sound;
pub mod system;

use crate::platform::InjectOp;
use crate::protocol::{Ext, MouseButton, Os, Patch, WinEvent, WinInfo};
use crate::wins;
use parking_lot::{Condvar, Mutex, RwLock};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Something for the app to show.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum UiEvent {
    /// Files dragged here from another computer are on their way.
    Incoming { x: i32, y: i32, label: String, from: String },
    /// They arrived (and were dropped where the pointer is).
    Landed { x: i32, y: i32, label: String },
    /// Show a live window from another computer.
    OpenViewer { stream: u64, origin: String, title: String, w: i32, h: i32, at: Option<(i32, i32)> },
    CloseViewer { stream: u64 },
}

/// An update to a live window's picture.
#[derive(Clone)]
pub struct Frame {
    pub seq: u64,
    pub w: u32,
    pub h: u32,
    pub title: String,
    pub patches: Arc<Vec<Patch>>,
}

impl Frame {
    /// Covers the whole picture (so earlier updates don't matter).
    fn is_full(&self) -> bool {
        self.patches.iter().any(|p| p.x == 0 && p.y == 0 && p.w == self.w && p.h == self.h)
    }
}

pub enum FrameWait {
    /// Updates to draw, oldest first.
    Frames(Vec<Frame>),
    /// Paused, with the reason to show.
    Paused(String),
    Closed,
    Timeout,
}

/// Settings the hub uses (from the config).
#[derive(Clone, Debug)]
pub struct Settings {
    pub theme_sync: bool,
    pub dnd_sync: bool,
    pub sound: String,
    pub sound_volume: u32,
    pub battery_saver: bool,
    pub quality: String,
    pub swap_cmd_ctrl: bool,
    pub window_drag: bool,
}

impl Settings {
    pub fn from_config(c: &crate::Config) -> Settings {
        Settings {
            theme_sync: c.theme_sync,
            dnd_sync: c.dnd_sync,
            sound: c.sound.clone(),
            sound_volume: c.sound_volume,
            battery_saver: c.battery_saver,
            quality: c.stream_quality.clone(),
            swap_cmd_ctrl: c.swap_cmd_ctrl,
            window_drag: c.window_drag,
        }
    }
}

/// Sends an [`Ext`] to a computer by name ("*" = everyone).
pub type Out = Box<dyn Fn(&str, Ext) + Send + Sync>;
/// Injects input on this computer (bypassing OpenHop's own capture).
pub type Inject = Box<dyn FnMut(&[InjectOp]) + Send>;

struct Viewer {
    origin: String,
    /// Updates not shown yet.
    queue: Vec<Frame>,
    paused: Option<String>,
    closed: bool,
}

struct Source {
    viewer: String,
    window: u64,
    viewer_os: Os,
    stop: AtomicBool,
    acked: AtomicU64,
    remote_pause: Mutex<Option<String>>,
    /// (frame w, frame h, window w, window h) of the last picture sent.
    scale: Mutex<(u32, u32, u32, u32)>,
    input: Mutex<InputState>,
    /// Input just went in: look for the change right away (and keep looking
    /// quickly for a moment) instead of waiting for the next regular look.
    busy_until: Mutex<Instant>,
    poke: Condvar,
}

impl Source {
    /// Sleep up to `d`, waking early if input arrives.
    fn nap(&self, d: Duration) {
        let mut until = self.busy_until.lock();
        let deadline = Instant::now() + d;
        let was = *until;
        while Instant::now() < deadline && *until == was {
            if self.poke.wait_until(&mut until, deadline).timed_out() {
                break;
            }
        }
    }

    fn busy(&self) -> bool {
        *self.busy_until.lock() > Instant::now()
    }
}

#[derive(Default)]
struct InputState {
    last_activate: Option<Instant>,
    last_move: Option<Instant>,
    buttons: u8,
}

pub struct Hub {
    me: String,
    settings: RwLock<Settings>,
    out: RwLock<Option<Out>>,
    inject: Mutex<Option<Inject>>,
    ui: Mutex<Vec<UiEvent>>,
    lists: Mutex<BTreeMap<String, Vec<WinInfo>>>,
    viewers: Mutex<HashMap<u64, Viewer>>,
    frames: Condvar,
    sources: Mutex<HashMap<u64, Arc<Source>>>,
    quiet_from: Mutex<BTreeSet<String>>,
    /// Do Not Disturb state from before we turned it on for another computer.
    dnd_before: Mutex<Option<bool>>,
    /// A dark mode change we made ourselves (not to be sent back).
    theme_expect: Mutex<Option<bool>>,
    low_battery: AtomicBool,
    /// A live window of ours that has the keyboard focus on another computer:
    /// (that computer, window).
    focused: Mutex<Option<(String, u64, u64)>>,
    /// Send our window list again now (we just connected).
    resend: AtomicBool,
    stop: Arc<AtomicBool>,
}

/// Pictures per second, JPEG quality, widest picture, full colour detail.
fn quality(q: &str) -> (u32, u8, u32, bool) {
    match q {
        "low" => (15, 60, 1280, false),
        "high" => (40, 92, 3840, true),
        _ => (30, 82, 2560, true),
    }
}

impl Hub {
    pub fn new(me: String, settings: Settings) -> Arc<Hub> {
        let hub = Arc::new(Hub {
            me,
            settings: RwLock::new(settings),
            out: RwLock::new(None),
            inject: Mutex::new(None),
            ui: Mutex::new(vec![]),
            lists: Mutex::new(BTreeMap::new()),
            viewers: Mutex::new(HashMap::new()),
            frames: Condvar::new(),
            sources: Mutex::new(HashMap::new()),
            quiet_from: Mutex::new(BTreeSet::new()),
            dnd_before: Mutex::new(None),
            theme_expect: Mutex::new(None),
            low_battery: AtomicBool::new(false),
            focused: Mutex::new(None),
            resend: AtomicBool::new(true),
            stop: Arc::new(AtomicBool::new(false)),
        });
        let h = hub.clone();
        let _ = std::thread::Builder::new().name("extras".into()).spawn(move || {
            // Windows a crashed run left hidden come back.
            wins::restore_all();
            h.monitor()
        });
        hub
    }

    pub fn me(&self) -> &str {
        &self.me
    }

    pub fn settings(&self) -> Settings {
        self.settings.read().clone()
    }

    /// Apply changed settings right away (no restart needed).
    pub fn set_settings(&self, s: Settings) {
        let dnd_off = self.settings.read().dnd_sync && !s.dnd_sync;
        *self.settings.write() = s;
        if dnd_off {
            self.update_dnd();
        }
    }

    pub fn shutdown(&self) {
        self.stop.store(true, Ordering::SeqCst);
        for s in self.sources.lock().values() {
            s.stop.store(true, Ordering::SeqCst);
        }
        self.restore_dnd();
    }

    /// Connected (or reconnected): how to reach the others.
    pub fn set_out(&self, out: Option<Out>) {
        let connected = out.is_some();
        *self.out.write() = out;
        self.resend.store(connected, Ordering::SeqCst);
        if !connected {
            self.disconnected_all();
        }
    }

    pub fn set_injector(&self, inject: Inject) {
        *self.inject.lock() = Some(inject);
    }

    fn send(&self, to: &str, ext: Ext) {
        if to == self.me {
            return;
        }
        if let Some(out) = self.out.read().as_ref() {
            out(to, ext);
        }
    }

    pub fn take_ui(&self) -> Vec<UiEvent> {
        std::mem::take(&mut *self.ui.lock())
    }

    fn ui(&self, ev: UiEvent) {
        self.ui.lock().push(ev);
    }

    /// Another computer is quiet (Do Not Disturb or presenting): hold notifications.
    pub fn quiet(&self) -> bool {
        self.settings.read().dnd_sync && !self.quiet_from.lock().is_empty()
    }

    pub fn quiet_from(&self) -> Vec<String> {
        self.quiet_from.lock().iter().cloned().collect()
    }

    /// Windows by computer (this one first).
    pub fn windows(&self) -> Vec<(String, Vec<WinInfo>)> {
        let lists = self.lists.lock();
        let mut v: Vec<(String, Vec<WinInfo>)> = lists.iter().filter(|(_, l)| !l.is_empty()).map(|(k, l)| (k.clone(), l.clone())).collect();
        v.sort_by_key(|(k, _)| (k != &self.me, k.clone()));
        v
    }

    /// A live window of this computer is focused on `viewer`'s screen: typing
    /// there is meant for it. Returns (viewer computer, window).
    pub fn key_target(&self) -> Option<(String, u64)> {
        self.focused.lock().as_ref().map(|(v, _, w)| (v.clone(), *w))
    }

    /// The pointer is back on this computer: put windows that are open
    /// elsewhere (and hidden here) behind everything, out of the way.
    pub fn lower_hidden(&self) {
        let windows: Vec<u64> = self.sources.lock().values().map(|s| s.window).collect();
        if !windows.is_empty() {
            std::thread::spawn(move || {
                for w in windows {
                    wins::lower(w);
                }
            });
        }
    }

    pub fn play(&self, _why: &str) {
        let s = self.settings.read();
        sound::play(&s.sound, s.sound_volume);
    }

    // ------------------------------------------------------ arrivals

    pub fn incoming(&self, label: &str, from: &str) {
        if let Some((x, y)) = crate::platform::cursor_pos() {
            self.ui(UiEvent::Incoming { x, y, label: label.into(), from: from.into() });
        }
    }

    pub fn landed(&self, label: &str) {
        if let Some((x, y)) = crate::platform::cursor_pos() {
            self.ui(UiEvent::Landed { x, y, label: label.into() });
        }
        self.play("landed");
    }

    // ------------------------------------------------------ connections

    /// Server: someone joined. Bring them up to date.
    pub fn peer_joined(&self, name: &str, send_as: impl Fn(&str, Ext)) {
        let lists = self.lists.lock().clone();
        for (origin, list) in lists {
            if origin != name {
                send_as(&origin, Ext::Windows { list });
            }
        }
        let quiet: Vec<String> = self.quiet_from.lock().iter().cloned().collect();
        for q in quiet {
            if q != name {
                send_as(&q, Ext::Quiet { on: true });
            }
        }
    }

    /// A computer went away: forget its windows and live windows.
    pub fn peer_left(&self, name: &str) {
        self.lists.lock().remove(name);
        self.quiet_from.lock().remove(name);
        self.update_dnd();
        let gone: Vec<u64> = self.viewers.lock().iter().filter(|(_, v)| v.origin == name).map(|(k, _)| *k).collect();
        for s in gone {
            self.viewer_closed(s);
        }
        self.sources.lock().retain(|_, s| {
            if s.viewer == name {
                s.stop.store(true, Ordering::SeqCst);
                false
            } else {
                true
            }
        });
    }

    fn disconnected_all(&self) {
        let names: Vec<String> = self.lists.lock().keys().filter(|k| **k != self.me).cloned().collect();
        for n in names {
            self.peer_left(&n);
        }
        let viewers: Vec<u64> = self.viewers.lock().keys().copied().collect();
        for s in viewers {
            self.viewer_closed(s);
        }
        for s in self.sources.lock().drain() {
            s.1.stop.store(true, Ordering::SeqCst);
        }
        self.quiet_from.lock().clear();
        self.update_dnd();
    }

    // ------------------------------------------------------ live windows (viewer side)

    /// Open `origin`'s window here. Returns the stream id.
    pub fn open(&self, origin: &str, window: u64, at: Option<(i32, i32)>) -> Option<u64> {
        if origin == self.me {
            wins::activate(window);
            return None;
        }
        let info = self.lists.lock().get(origin).and_then(|l| l.iter().find(|w| w.id == window).cloned());
        let (title, w, h) = info.map(|i| (i.title, i.w, i.h)).unwrap_or_else(|| ("Window".into(), 960, 640));
        // Fits in a JavaScript number.
        let stream = crate::files::new_id() & ((1 << 52) - 1);
        self.viewers.lock().insert(stream, Viewer { origin: origin.into(), queue: vec![], paused: None, closed: false });
        self.send(origin, Ext::WinOpen { stream, window, os: Os::current() });
        self.ui(UiEvent::OpenViewer { stream, origin: origin.into(), title, w, h, at });
        self.play("window");
        log::info!("opening a live window from {origin}");
        if self.low_battery.load(Ordering::SeqCst) {
            self.send(origin, Ext::WinPause { stream, paused: true, reason: low_battery_reason() });
        }
        Some(stream)
    }

    /// The viewer window was closed here.
    pub fn close(&self, stream: u64) {
        if let Some(v) = self.viewers.lock().remove(&stream) {
            self.send(&v.origin, Ext::WinClose { stream });
        }
        self.frames.notify_all();
    }

    pub fn input(&self, stream: u64, ev: WinEvent) {
        log::trace!("{} view input {:?}", ms(), ev);
        let origin = self.viewers.lock().get(&stream).map(|v| v.origin.clone());
        if let Some(o) = origin {
            self.send(&o, Ext::WinInput { stream, ev });
        }
    }

    /// Wait (up to `timeout`) for updates newer than `after`. Taking them
    /// tells the owner to send more, so a slow screen gets fewer updates
    /// instead of a backlog.
    pub fn frame(&self, stream: u64, after: u64, timeout: Duration) -> FrameWait {
        let deadline = Instant::now() + timeout;
        let mut viewers = self.viewers.lock();
        loop {
            match viewers.get_mut(&stream) {
                None => return FrameWait::Closed,
                Some(v) if v.closed => return FrameWait::Closed,
                Some(v) => {
                    v.queue.retain(|f| f.seq > after);
                    if !v.queue.is_empty() {
                        let frames = std::mem::take(&mut v.queue);
                        log::trace!("{} hand {} update(s) to the view", ms(), frames.len());
                        let (origin, last) = (v.origin.clone(), frames.last().map(|f| f.seq).unwrap_or(0));
                        drop(viewers);
                        self.send(&origin, Ext::WinAck { stream, seq: last });
                        return FrameWait::Frames(frames);
                    }
                    if let Some(p) = &v.paused {
                        if Instant::now() + Duration::from_millis(50) >= deadline {
                            return FrameWait::Paused(p.clone());
                        }
                    }
                }
            }
            if self.frames.wait_until(&mut viewers, deadline).timed_out() {
                return match viewers.get(&stream) {
                    Some(v) if v.paused.is_some() => FrameWait::Paused(v.paused.clone().unwrap_or_default()),
                    Some(_) => FrameWait::Timeout,
                    None => FrameWait::Closed,
                };
            }
        }
    }

    fn viewer_closed(&self, stream: u64) {
        if let Some(v) = self.viewers.lock().get_mut(&stream) {
            v.closed = true;
        }
        self.viewers.lock().remove(&stream);
        self.frames.notify_all();
        self.ui(UiEvent::CloseViewer { stream });
    }

    /// Ask `to` to open `origin`'s window (it was dragged onto `to`'s screen).
    pub fn offer_window(&self, to: &str, origin: &str, window: u64) {
        if to == self.me {
            self.open(origin, window, crate::platform::cursor_pos());
        } else {
            self.send(to, Ext::WinOffer { origin: origin.into(), window });
        }
    }

    // ------------------------------------------------------ incoming messages

    pub fn handle(self: &Arc<Self>, from: &str, ext: Ext) {
        match ext {
            Ext::Theme { dark } => {
                if self.settings.read().theme_sync && system::dark_mode() != Some(dark) {
                    *self.theme_expect.lock() = Some(dark);
                    std::thread::spawn(move || system::set_dark_mode(dark));
                }
            }
            Ext::Quiet { on } => {
                if on {
                    self.quiet_from.lock().insert(from.into());
                } else {
                    self.quiet_from.lock().remove(from);
                }
                self.update_dnd();
            }
            Ext::Windows { list } => {
                self.lists.lock().insert(from.into(), list);
            }
            Ext::WinOffer { origin, window } => {
                self.open(&origin, window, crate::platform::cursor_pos());
            }
            Ext::WinOpen { stream, window, os } => self.start_source(from, stream, window, os),
            Ext::WinFrame { stream, seq, w, h, title, patches } => {
                log::trace!("{} got update {seq}", ms());
                let known = {
                    let mut viewers = self.viewers.lock();
                    match viewers.get_mut(&stream) {
                        Some(v) => {
                            let f = Frame { seq, w, h, title, patches: Arc::new(patches) };
                            if f.is_full() {
                                v.queue.clear();
                            }
                            v.queue.push(f);
                            v.paused = None;
                            true
                        }
                        None => false,
                    }
                };
                if seq == 1 {
                    log::debug!("live window: first picture from {from} (known stream: {known})");
                }
                if known {
                    self.frames.notify_all();
                } else {
                    self.send(from, Ext::WinClose { stream });
                }
            }
            Ext::WinAck { stream, seq } => {
                if let Some(s) = self.sources.lock().get(&stream) {
                    s.acked.fetch_max(seq, Ordering::SeqCst);
                }
            }
            Ext::WinInput { stream, ev } => {
                log::trace!("{} input {:?}", ms(), ev);
                let src = self.sources.lock().get(&stream).cloned();
                if let Some(s) = src {
                    self.inject_for(stream, &s, ev);
                }
            }
            Ext::WinClose { stream } => {
                if let Some(s) = self.sources.lock().remove(&stream) {
                    s.stop.store(true, Ordering::SeqCst);
                }
                if self.viewers.lock().contains_key(&stream) {
                    self.viewer_closed(stream);
                }
            }
            Ext::WinPause { stream, paused, reason } => {
                if let Some(s) = self.sources.lock().get(&stream) {
                    *s.remote_pause.lock() = paused.then_some(reason.clone());
                }
                if let Some(v) = self.viewers.lock().get_mut(&stream) {
                    v.paused = paused.then_some(reason);
                }
                self.frames.notify_all();
            }
        }
    }

    // ------------------------------------------------------ live windows (owner side)

    fn start_source(self: &Arc<Self>, viewer: &str, stream: u64, window: u64, viewer_os: Os) {
        if wins::geometry(window).is_none() {
            self.send(viewer, Ext::WinClose { stream });
            return;
        }
        log::info!("{viewer} opened a live window of {window:#x}");
        let src = Arc::new(Source {
            viewer: viewer.into(),
            window,
            viewer_os,
            stop: AtomicBool::new(false),
            acked: AtomicU64::new(0),
            remote_pause: Mutex::new(None),
            scale: Mutex::new((1, 1, 1, 1)),
            input: Mutex::new(InputState::default()),
            busy_until: Mutex::new(Instant::now()),
            poke: Condvar::new(),
        });
        self.sources.lock().insert(stream, src.clone());
        // It has moved to the other screen: hide it here (it keeps running).
        wins::set_hidden(window, true);
        let hub = self.clone();
        let _ = std::thread::Builder::new().name("live-window".into()).spawn(move || {
            hub.stream_loop(stream, &src);
            hub.sources.lock().remove(&stream);
            // Back on this screen, unless it's still open somewhere else.
            if !hub.sources.lock().values().any(|s| s.window == src.window) {
                wins::set_hidden(src.window, false);
            }
            let mut f = hub.focused.lock();
            if f.as_ref().map(|x| x.1) == Some(stream) {
                *f = None;
            }
        });
    }

    fn stream_loop(&self, stream: u64, src: &Source) {
        let (fps, q, max_w, full_colour) = quality(&self.settings.read().quality);
        let interval = Duration::from_millis(1000 / fps as u64);
        let mut seq = 0u64;
        // The last picture sent (after scaling), to find what changed.
        let mut prev: Option<wins::Picture> = None;
        let mut last_full = Instant::now();
        let mut sent_at = Instant::now();
        let mut paused_sent = false;
        while !src.stop.load(Ordering::SeqCst) && !self.stop.load(Ordering::SeqCst) {
            let local = self.low_battery.load(Ordering::SeqCst).then(low_battery_reason);
            let pause = local.or_else(|| src.remote_pause.lock().clone());
            if let Some(reason) = pause {
                if !paused_sent {
                    self.send(&src.viewer, Ext::WinPause { stream, paused: true, reason });
                    paused_sent = true;
                }
                std::thread::sleep(Duration::from_millis(300));
                continue;
            } else if paused_sent {
                self.send(&src.viewer, Ext::WinPause { stream, paused: false, reason: String::new() });
                paused_sent = false;
                prev = None;
            }
            // At most two updates on their way: enough to hide the network
            // delay, without building a queue on a slow link.
            if seq >= 2 && src.acked.load(Ordering::SeqCst) + 2 <= seq && sent_at.elapsed() < Duration::from_secs(2) {
                std::thread::sleep(Duration::from_millis(2));
                continue;
            }
            let started = Instant::now();
            let Some(pic) = wins::capture(src.window) else {
                if wins::geometry(src.window).is_none() {
                    log::info!("live window closed on this computer");
                    self.send(&src.viewer, Ext::WinClose { stream });
                    return;
                }
                std::thread::sleep(Duration::from_millis(300));
                continue;
            };
            let (ow, oh) = (pic.w, pic.h);
            let pic = scale_down(pic, max_w);
            let (fw, fh) = (pic.w, pic.h);
            // A full picture now and then heals anything that went wrong.
            let full = prev.as_ref().map(|p| p.w != fw || p.h != fh).unwrap_or(true) || last_full.elapsed() > Duration::from_secs(15);
            let rects = if full { vec![(0, 0, fw, fh)] } else { changed_rects(prev.as_ref().unwrap(), &pic) };
            if rects.is_empty() {
                // Right after input, look again soon: the app is about to redraw.
                let wait = if src.busy() { Duration::from_millis(8) } else { interval };
                src.nap(wait.saturating_sub(started.elapsed()).max(Duration::from_millis(4)));
                continue;
            }
            let patches: Vec<Patch> = rects
                .into_iter()
                .filter_map(|(x, y, w, h)| encode_patch(&pic, (x, y, w, h), q, full_colour).map(|jpeg| Patch { x, y, w, h, jpeg }))
                .collect();
            if patches.is_empty() {
                std::thread::sleep(interval);
                continue;
            }
            if full {
                last_full = Instant::now();
            }
            *src.scale.lock() = (fw, fh, ow, oh);
            prev = Some(pic);
            seq += 1;
            let title = self.lists.lock().get(&self.me).and_then(|l| l.iter().find(|w| w.id == src.window).map(|w| w.title.clone())).unwrap_or_default();
            let len: usize = patches.iter().map(|p| p.jpeg.len()).sum();
            if seq == 1 || seq.is_multiple_of(200) {
                log::debug!("live window: update {seq} ({fw}x{fh}, {} patch(es), {} KB, {} ms) -> {}", patches.len(), len / 1024, started.elapsed().as_millis(), src.viewer);
            }
            log::trace!("{} send update {seq} ({} bytes, took {} ms)", ms(), len, started.elapsed().as_millis());
            self.send(&src.viewer, Ext::WinFrame { stream, seq, w: fw, h: fh, title, patches });
            crate::files::pace(len);
            sent_at = Instant::now();
            let wait = if src.busy() { Duration::from_millis(8) } else { interval };
            src.nap(wait.saturating_sub(started.elapsed()));
        }
    }

    fn inject_for(&self, stream: u64, src: &Source, ev: WinEvent) {
        let Some(g) = wins::geometry(src.window) else { return };
        let (fw, fh, ow, oh) = *src.scale.lock();
        let map = |x: i32, y: i32| {
            let x = (x as i64 * ow as i64 / fw.max(1) as i64) as i32;
            let y = (y as i64 * oh as i64 / fh.max(1) as i64) as i32;
            (g.x + x.clamp(0, g.w - 1), g.y + y.clamp(0, g.h - 1))
        };
        let mut st = src.input.lock();
        let mut ops: Vec<InjectOp> = vec![];
        let activate = |st: &mut InputState| {
            // Bring it forward when the view is first used (or after a while,
            // in case something else took the focus meanwhile).
            if st.last_activate.map(|t| t.elapsed() > Duration::from_secs(15)).unwrap_or(true) {
                wins::activate(src.window);
                st.last_activate = Some(Instant::now());
                std::thread::sleep(Duration::from_millis(15));
            }
        };
        match ev {
            WinEvent::Focus => {
                st.last_activate = None;
                activate(&mut st);
                *self.focused.lock() = Some((src.viewer.clone(), stream, src.window));
            }
            WinEvent::Resize { w, h } => {
                let w = (w as i64 * ow as i64 / fw.max(1) as i64) as i32;
                let h = (h as i64 * oh as i64 / fh.max(1) as i64) as i32;
                if (w - g.w).abs() > 2 || (h - g.h).abs() > 2 {
                    wins::resize(src.window, w, h);
                }
            }
            WinEvent::Blur => {
                let mut f = self.focused.lock();
                if f.as_ref().map(|x| x.1) == Some(stream) {
                    *f = None;
                }
            }
            WinEvent::Move { x, y } => {
                // Hover moves are thinned out; drags go through in full.
                let busy = st.buttons != 0;
                if busy || st.last_move.map(|t| t.elapsed() > Duration::from_millis(50)).unwrap_or(true) {
                    let (ax, ay) = map(x, y);
                    ops.push(InjectOp::MoveTo(ax, ay));
                    st.last_move = Some(Instant::now());
                }
            }
            WinEvent::Button { button, down, x, y } => {
                if down {
                    activate(&mut st);
                }
                let bit = match button {
                    MouseButton::Left => 1,
                    MouseButton::Right => 2,
                    MouseButton::Middle => 4,
                    MouseButton::Back => 8,
                    MouseButton::Forward => 16,
                };
                if down {
                    st.buttons |= bit;
                } else {
                    st.buttons &= !bit;
                }
                let (ax, ay) = map(x, y);
                ops.push(InjectOp::MoveTo(ax, ay));
                ops.push(InjectOp::Button(button, down));
            }
            WinEvent::Wheel { dx, dy, x, y } => {
                let (ax, ay) = map(x, y);
                ops.push(InjectOp::MoveTo(ax, ay));
                ops.push(InjectOp::Wheel(dx, dy));
            }
            WinEvent::Key { key, down } => {
                if down {
                    activate(&mut st);
                }
                let swap = self.settings.read().swap_cmd_ctrl && ((src.viewer_os == Os::MacOs) != (Os::current() == Os::MacOs));
                let key = if swap { crate::keys::swap_ctrl_meta(key) } else { key };
                ops.push(InjectOp::Key(key, down));
            }
        }
        drop(st);
        if !ops.is_empty() {
            if let Some(inj) = self.inject.lock().as_mut() {
                inj(&ops);
            }
        }
        // Show the result as soon as the app has drawn it.
        if !matches!(ev, WinEvent::Move { .. } | WinEvent::Blur) || src.input.lock().buttons != 0 {
            *src.busy_until.lock() = Instant::now() + Duration::from_millis(400);
            src.poke.notify_all();
        }
    }

    // ------------------------------------------------------ background checks

    fn monitor(self: Arc<Self>) {
        let mut last_list: Option<Vec<WinInfo>> = None;
        let mut list_sent = Instant::now() - Duration::from_secs(60);
        let mut last_dark = system::dark_mode();
        let mut last_quiet = false;
        let mut tick = 0u64;
        while !self.stop.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(500));
            tick += 1;
            let connected = self.out.read().is_some();
            // Window list: every second, sent when it changes (and now and then).
            if tick.is_multiple_of(2) {
                let list = wins::list();
                self.lists.lock().insert(self.me.clone(), list.clone());
                let resend = self.resend.swap(false, Ordering::SeqCst);
                if connected && (resend || last_list.as_ref() != Some(&list) || list_sent.elapsed() > Duration::from_secs(20)) {
                    self.send("*", Ext::Windows { list: list.clone() });
                    list_sent = Instant::now();
                    last_list = Some(list);
                }
            }
            if !tick.is_multiple_of(4) {
                continue;
            }
            let s = self.settings.read().clone();
            // Dark mode.
            if s.theme_sync {
                let now = system::dark_mode();
                if now.is_some() && now != last_dark {
                    let ours = *self.theme_expect.lock() == now;
                    if ours {
                        *self.theme_expect.lock() = None;
                    } else if connected {
                        log::info!("dark mode turned {} here; telling the others", if now == Some(true) { "on" } else { "off" });
                        self.send("*", Ext::Theme { dark: now == Some(true) });
                    }
                    last_dark = now;
                }
            }
            // Do Not Disturb / presenting (ignoring the DND we turned on ourselves).
            if s.dnd_sync {
                let own_dnd = self.dnd_before.lock().is_none() && system::dnd() == Some(true);
                let quiet = own_dnd || system::presenting();
                if quiet != last_quiet || (quiet && tick.is_multiple_of(40)) {
                    if connected {
                        self.send("*", Ext::Quiet { on: quiet });
                    }
                    last_quiet = quiet;
                }
            }
            // Battery.
            let low = s.battery_saver && system::battery_low();
            if low != self.low_battery.swap(low, Ordering::SeqCst) {
                log::info!("battery {}: live windows {}", if low { "low" } else { "ok" }, if low { "paused" } else { "resumed" });
                let viewers: Vec<(u64, String)> = self.viewers.lock().iter().map(|(k, v)| (*k, v.origin.clone())).collect();
                for (stream, origin) in viewers {
                    self.send(&origin, Ext::WinPause { stream, paused: low, reason: if low { low_battery_reason() } else { String::new() } });
                }
            }
        }
    }

    fn update_dnd(&self) {
        let want = self.settings.read().dnd_sync && !self.quiet_from.lock().is_empty();
        let mut before = self.dnd_before.lock();
        if want && before.is_none() {
            let now = system::dnd();
            if now != Some(true) {
                *before = Some(now.unwrap_or(false));
                std::thread::spawn(|| system::set_dnd(true));
            }
        } else if !want {
            if let Some(prev) = before.take() {
                std::thread::spawn(move || system::set_dnd(prev));
            }
        }
    }

    fn restore_dnd(&self) {
        if let Some(prev) = self.dnd_before.lock().take() {
            system::set_dnd(prev);
        }
    }
}

/// Milliseconds clock for timing traces.
fn ms() -> u128 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() % 100_000).unwrap_or(0)
}

fn low_battery_reason() -> String {
    "Paused to save battery".into()
}

/// Shrink pictures wider than `max_w` (keeps BGRA order).
fn scale_down(pic: wins::Picture, max_w: u32) -> wins::Picture {
    if pic.w <= max_w {
        return pic;
    }
    let h = (pic.h as u64 * max_w as u64 / pic.w as u64).max(1) as u32;
    match image::RgbaImage::from_raw(pic.w, pic.h, pic.bgra) {
        Some(img) => {
            let small = image::imageops::resize(&img, max_w, h, image::imageops::FilterType::Triangle);
            wins::Picture { w: max_w, h, bgra: small.into_raw() }
        }
        None => wins::Picture { w: 0, h: 0, bgra: vec![] },
    }
}

/// The areas that changed between two same-size pictures: one rectangle per
/// band of changed rows (so a clock in one corner and typing in another
/// don't make the whole window resend), aligned to 16 pixels.
fn changed_rects(a: &wins::Picture, b: &wins::Picture) -> Vec<(u32, u32, u32, u32)> {
    let (w, h) = (b.w as usize, b.h as usize);
    let stride = w * 4;
    let px_eq = |r1: &[u8], r2: &[u8], x: usize| r1[x * 4..x * 4 + 3] == r2[x * 4..x * 4 + 3];
    let mut out = vec![];
    let mut band: Option<(usize, usize, usize, usize)> = None; // top, bottom, left, right
    let mut clean_run = 0;
    for y in 0..h {
        let (ra, rb) = (&a.bgra[y * stride..(y + 1) * stride], &b.bgra[y * stride..(y + 1) * stride]);
        if ra == rb {
            clean_run += 1;
            if clean_run >= 32 {
                if let Some(bd) = band.take() {
                    out.push(bd);
                }
            }
            continue;
        }
        clean_run = 0;
        let mut l = 0;
        while l < w && px_eq(ra, rb, l) {
            l += 1;
        }
        if l == w {
            continue; // only the unused alpha byte differs
        }
        let mut r = w - 1;
        while r > l && px_eq(ra, rb, r) {
            r -= 1;
        }
        band = Some(match band {
            Some((t, _, bl, br)) => (t, y, bl.min(l), br.max(r)),
            None => (y, y, l, r),
        });
    }
    if let Some(bd) = band {
        out.push(bd);
    }
    out.into_iter()
        .map(|(t, bt, l, r)| {
            let x0 = l / 16 * 16;
            let y0 = t / 16 * 16;
            let x1 = ((r + 16) / 16 * 16).min(w);
            let y1 = ((bt + 16) / 16 * 16).min(h);
            (x0 as u32, y0 as u32, (x1 - x0) as u32, (y1 - y0) as u32)
        })
        .collect()
}

/// JPEG of one area of a picture.
fn encode_patch(pic: &wins::Picture, (x, y, w, h): (u32, u32, u32, u32), quality: u8, full_colour: bool) -> Option<Vec<u8>> {
    if w == 0 || h == 0 || w > u16::MAX as u32 || h > u16::MAX as u32 {
        return None;
    }
    let stride = pic.w as usize * 4;
    let mut area = Vec::with_capacity((w * h * 4) as usize);
    for row in y..y + h {
        let start = row as usize * stride + x as usize * 4;
        area.extend_from_slice(pic.bgra.get(start..start + w as usize * 4)?);
    }
    let mut out = Vec::with_capacity(area.len() / 10);
    let mut enc = jpeg_encoder::Encoder::new(&mut out, quality);
    if full_colour {
        // Sharp coloured text (no colour blur around letters).
        enc.set_sampling_factor(jpeg_encoder::SamplingFactor::R_4_4_4);
    }
    enc.encode(&area, w as u16, h as u16, jpeg_encoder::ColorType::Bgra).ok()?;
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patches_cover_only_changes() {
        let (w, h) = (200u32, 300u32);
        let a = wins::Picture { w, h, bgra: vec![200; (w * h * 4) as usize] };
        let mut b = wins::Picture { w, h, bgra: a.bgra.clone() };
        assert!(changed_rects(&a, &b).is_empty());
        // A letter typed near the top, and a clock far below.
        for (x, y) in [(20usize, 10usize), (21, 12), (150, 280)] {
            b.bgra[(y * w as usize + x) * 4] = 0;
        }
        let r = changed_rects(&a, &b);
        assert_eq!(r, vec![(16, 0, 16, 16), (144, 272, 16, 16)]);
        let jpeg = encode_patch(&b, r[0], 80, true).unwrap();
        assert_eq!(&jpeg[..2], &[0xFF, 0xD8]);
        assert!(jpeg.len() < 2000, "{}", jpeg.len());
        let small = scale_down(wins::Picture { w: 4000, h: 1000, bgra: vec![1; 4000 * 1000 * 4] }, 1920);
        assert_eq!((small.w, small.h, small.bgra.len()), (1920, 480, 1920 * 480 * 4));
    }

    #[test]
    fn viewer_waits_for_frames() {
        let hub = Hub::new("me".into(), Settings::from_config(&crate::Config::default()));
        hub.viewers.lock().insert(7, Viewer { origin: "pc".into(), queue: vec![], paused: None, closed: false });
        assert!(matches!(hub.frame(7, 0, Duration::from_millis(20)), FrameWait::Timeout));
        let h2 = hub.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(30));
            h2.handle("pc", Ext::WinFrame { stream: 7, seq: 1, w: 2, h: 2, title: "t".into(), patches: vec![Patch { x: 0, y: 0, w: 2, h: 2, jpeg: vec![1, 2, 3] }] });
        });
        match hub.frame(7, 0, Duration::from_secs(2)) {
            FrameWait::Frames(f) => assert_eq!((f[0].seq, f[0].patches[0].jpeg.len()), (1, 3)),
            _ => panic!("no frame"),
        }
        hub.handle("pc", Ext::WinPause { stream: 7, paused: true, reason: "low".into() });
        assert!(matches!(hub.frame(7, 1, Duration::from_millis(100)), FrameWait::Paused(_)));
        hub.handle("pc", Ext::WinClose { stream: 7 });
        assert!(matches!(hub.frame(7, 1, Duration::from_millis(10)), FrameWait::Closed));
        assert!(matches!(hub.take_ui().last(), Some(UiEvent::CloseViewer { stream: 7 })));
        hub.shutdown();
    }
}
