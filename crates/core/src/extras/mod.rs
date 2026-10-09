//! Features beyond sharing the keyboard and mouse: live windows from other
//! computers, a shared window list, dark mode and Do Not Disturb sync, and
//! arrival animations.
//!
//! The [`Hub`] lives next to the engine. It talks to other computers with
//! [`Ext`] messages addressed by computer name (the server forwards them).

pub mod apps;
pub mod media;
pub mod menus;
pub mod system;
pub mod tasks;

use crate::platform::InjectOp;
use crate::protocol::{AppEntry, ClipData, Ext, MouseButton, Os, Patch, PcStatus, ShelfItem, WinEvent, WinInfo};
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
    Incoming {
        x: i32,
        y: i32,
        label: String,
        from: String,
    },
    /// They arrived (and were dropped where the pointer is).
    Landed {
        x: i32,
        y: i32,
        label: String,
    },
    /// Show a live window from another computer.
    OpenViewer {
        stream: u64,
        origin: String,
        title: String,
        w: i32,
        h: i32,
        at: Option<(i32, i32)>,
    },
    CloseViewer {
        stream: u64,
    },
    /// Maximize (or restore) a live window's view.
    MaximizeViewer {
        stream: u64,
        on: bool,
    },
    /// Show where the pointer is (it's at x, y).
    Locate {
        x: i32,
        y: i32,
    },
    /// Tell the user something (on this screen: they're using it).
    Notice {
        title: String,
        body: String,
        icon: String,
    },
}

/// Something copied on one of the computers (clipboard history).
#[derive(Debug, Clone, Serialize)]
pub struct ClipItem {
    pub id: u64,
    /// "text", "image" or "files".
    pub kind: &'static str,
    /// The text (shortened), or the file names.
    pub text: String,
    /// A small picture of an image, as a data: URL.
    pub thumb: Option<String>,
    pub from: String,
    /// Seconds since 1970.
    pub at: u64,
    pub pinned: bool,
}

/// Shelf items from every computer.
#[derive(Debug, Clone, Serialize)]
pub struct ShelfEntry {
    pub origin: String,
    pub item: ShelfItem,
}

/// Apps from every computer (for the launcher).
#[derive(Debug, Clone, Serialize)]
pub struct AppsOf {
    pub name: String,
    pub apps: Vec<AppEntry>,
}

/// A computer at a glance, for the Control Center.
#[derive(Debug, Clone, Serialize)]
pub struct Computer {
    pub name: String,
    pub this: bool,
    pub status: PcStatus,
}

/// An update to a live window's picture.
#[derive(Clone)]
pub struct Frame {
    /// Order of arrival here (an update can come in several parts).
    pub seq: u64,
    /// The owner's update number (acknowledged when shown).
    pub remote: u64,
    pub w: u32,
    pub h: u32,
    /// Title bar height (top of the picture).
    pub bar: u32,
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
    pub quality: String,
    pub swap_cmd_ctrl: bool,
    pub window_drag: bool,
}

impl Settings {
    pub fn from_config(c: &crate::Config) -> Settings {
        Settings {
            theme_sync: c.theme_sync,
            dnd_sync: c.dnd_sync,
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
    /// The window on `origin`.
    window: u64,
    /// Updates not shown yet.
    queue: Vec<Frame>,
    /// Parts received so far (numbers the queue).
    received: u64,
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
    /// The window was dragged back here: show it under the pointer.
    returning: AtomicBool,
    /// Shown maximized on the viewer's screen.
    maxed: AtomicBool,
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
    /// A press held back until its release (see `Hub::set_injector`):
    /// (button, where it went down), and where the pointer is now.
    press: Option<(MouseButton, (i32, i32))>,
    at: (i32, i32),
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
    /// A live window of ours that has the keyboard focus on another computer:
    /// (that computer, window).
    focused: Mutex<Option<(String, u64, u64)>>,
    /// Send our window list again now (we just connected).
    resend: AtomicBool,
    /// Clicks are injected whole (press and release together) when the
    /// release comes: see `set_injector`.
    whole_clicks: AtomicBool,
    /// A live window being dragged by its title bar here: (stream, since).
    dragging: Mutex<Option<(u64, Instant)>>,
    /// This computer's desktop (to keep windows coming back on it).
    screen: Mutex<Option<crate::protocol::Rect>>,
    /// Every computer's state (this one too).
    stats: Mutex<BTreeMap<String, PcStatus>>,
    /// Focus (Do Not Disturb) on every computer.
    focus: AtomicBool,
    /// Do Not Disturb as we last set it ourselves (not to be sent back).
    dnd_expect: Mutex<Option<(bool, Instant)>>,
    /// The pointer is on this computer's screen.
    pointer_here: AtomicBool,
    /// A full-screen app is in front here.
    fullscreen: AtomicBool,
    /// We locked or woke this screen ourselves (not to be sent back).
    lock_expect: Mutex<Option<Instant>>,
    /// What was copied lately on every computer, newest first, and the
    /// content to copy again.
    history: Mutex<Vec<(ClipItem, ClipData)>>,
    /// Puts something on this computer's clipboard (set by the engine).
    clip_setter: Mutex<Option<Box<dyn Fn(ClipData) + Send>>>,
    /// Shelf items: this computer's own, and every other computer's.
    shelves: Mutex<BTreeMap<String, Vec<ShelfItem>>>,
    /// Installed apps on every computer.
    apps: Mutex<BTreeMap<String, Vec<AppEntry>>>,
    /// What's playing on every computer.
    media: Mutex<BTreeMap<String, media::NowPlaying>>,
    /// Media commands for this computer.
    media_tx: crossbeam_channel::Sender<media::MediaCmd>,
    /// The apps running on every computer.
    tasks: Mutex<BTreeMap<String, Vec<crate::protocol::RunningApp>>>,
    /// Quit requests for this computer's apps: (pids, force).
    quit_tx: crossbeam_channel::Sender<(Vec<u32>, bool)>,
    /// This hub, for work done on other threads.
    this: std::sync::Weak<Hub>,
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
        let (media_tx, media_rx) = crossbeam_channel::unbounded();
        let (quit_tx, quit_rx) = crossbeam_channel::unbounded();
        let hub = Arc::new_cyclic(|this| Hub {
            this: this.clone(),
            tasks: Mutex::new(BTreeMap::new()),
            quit_tx,
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
            focused: Mutex::new(None),
            resend: AtomicBool::new(true),
            whole_clicks: AtomicBool::new(false),
            dragging: Mutex::new(None),
            screen: Mutex::new(None),
            stats: Mutex::new(BTreeMap::new()),
            focus: AtomicBool::new(false),
            dnd_expect: Mutex::new(None),
            pointer_here: AtomicBool::new(true),
            fullscreen: AtomicBool::new(false),
            lock_expect: Mutex::new(None),
            history: Mutex::new(load_pins()),
            clip_setter: Mutex::new(None),
            shelves: Mutex::new(BTreeMap::new()),
            apps: Mutex::new(BTreeMap::new()),
            media: Mutex::new(BTreeMap::new()),
            media_tx,
            stop: Arc::new(AtomicBool::new(false)),
        });
        let h = hub.clone();
        let _ = std::thread::Builder::new().name("media".into()).spawn(move || h.media_loop(media_rx));
        let h = hub.clone();
        let _ = std::thread::Builder::new().name("tasks".into()).spawn(move || h.tasks_loop(quit_rx));
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

    /// How input from live views of this computer's windows gets in.
    /// `whole_clicks`: a press can't be injected on its own (on X11 the
    /// physical mouse is held by OpenHop's grab and a pressed button would
    /// keep the grab from coming back), so a click is injected in one go
    /// when its release arrives, as a short drag if the mouse moved.
    pub fn set_injector(&self, inject: Inject, whole_clicks: bool) {
        *self.inject.lock() = Some(inject);
        self.whole_clicks.store(whole_clicks, Ordering::SeqCst);
    }

    // ------------------------------------------------------ Control Center

    /// Every computer at a glance, this one first.
    pub fn computers(&self) -> Vec<Computer> {
        // Status arrives every couple of seconds (a window list can take longer).
        let stats = self.stats.lock();
        let mut v: Vec<Computer> = stats.iter().map(|(n, st)| Computer { name: n.clone(), this: *n == self.me, status: st.clone() }).collect();
        v.sort_by_key(|c| (!c.this, c.name.clone()));
        v
    }

    pub fn focus(&self) -> bool {
        self.focus.load(Ordering::SeqCst)
    }

    /// Focus (Do Not Disturb) on or off on every computer.
    pub fn set_focus(&self, on: bool) {
        self.apply_focus(on);
        self.send("*", Ext::Focus { on });
    }

    fn apply_focus(&self, on: bool) {
        if self.focus.swap(on, Ordering::SeqCst) == on {
            return;
        }
        log::info!("Focus turned {} on every computer", if on { "on" } else { "off" });
        *self.dnd_expect.lock() = Some((on, Instant::now()));
        std::thread::spawn(move || system::set_dnd(on));
        if let Some(st) = self.stats.lock().get_mut(&self.me) {
            st.focus = on;
        }
    }

    /// Lock every computer.
    pub fn lock_all(&self) {
        self.send("*", Ext::Lock);
        *self.lock_expect.lock() = Some(Instant::now());
        std::thread::spawn(system::lock_now);
    }

    /// Put every computer to sleep (this one last, so the message gets out).
    pub fn sleep_all(&self) {
        self.send("*", Ext::Sleep);
        std::thread::spawn(|| {
            std::thread::sleep(Duration::from_millis(800));
            system::sleep_now();
        });
    }

    /// Show where the pointer is, on whichever screen it's on.
    pub fn find_pointer(&self) {
        self.send("*", Ext::Locate);
        self.locate_here();
    }

    fn locate_here(&self) {
        if self.pointer_here.load(Ordering::SeqCst) {
            if let Some((x, y)) = crate::platform::cursor_pos() {
                self.ui(UiEvent::Locate { x, y });
            }
        }
    }

    /// Tell the user something on the screen they're using (this one or another).
    pub fn notice_everywhere(&self, title: &str, body: &str, icon: &str) {
        self.send("*", Ext::Notice { title: title.into(), body: body.into(), icon: icon.into() });
        self.notice_here(title, body, icon);
    }

    fn notice_here(&self, title: &str, body: &str, icon: &str) {
        if self.pointer_here.load(Ordering::SeqCst) {
            self.ui(UiEvent::Notice { title: title.into(), body: body.into(), icon: icon.into() });
        }
    }

    /// The engine says whether the pointer is on this screen.
    pub fn set_pointer_here(&self, here: bool) {
        self.pointer_here.store(here, Ordering::SeqCst);
    }

    pub fn pointer_here(&self) -> bool {
        self.pointer_here.load(Ordering::SeqCst)
    }

    // ------------------------------------------------------ clipboard history

    /// How the engine puts something on this computer's clipboard.
    pub fn set_clip_setter(&self, f: Box<dyn Fn(ClipData) + Send>) {
        *self.clip_setter.lock() = Some(f);
    }

    /// Something was copied (here, or on `from`).
    pub fn clip_seen(&self, from: &str, data: &ClipData) {
        let (kind, text, thumb) = match data {
            ClipData::Text(t) => {
                if t.trim().is_empty() {
                    return;
                }
                ("text", t.chars().take(400).collect::<String>(), None)
            }
            ClipData::Png(png) => ("image", String::new(), thumbnail(png)),
            ClipData::Files(paths) => {
                let names: Vec<String> = paths.iter().filter_map(|p| std::path::Path::new(p).file_name().map(|n| n.to_string_lossy().into_owned())).collect();
                ("files", names.join(", "), None)
            }
        };
        let mut h = self.history.lock();
        // The same thing again (copied once more, or arriving from another computer): move it up.
        if let Some(i) = h.iter().position(|(item, d)| item.kind == kind && same_clip(d, data)) {
            let (mut item, d) = h.remove(i);
            item.at = now_secs();
            h.insert(0, (item, d));
            return;
        }
        let item = ClipItem { id: crate::files::new_id() & ((1 << 52) - 1), kind, text, thumb, from: from.into(), at: now_secs(), pinned: false };
        h.insert(0, (item, data.clone()));
        // Keep 40 (pinned ones always), and the full pictures of the latest few only.
        let mut kept = 0;
        h.retain(|(item, _)| {
            if item.pinned {
                return true;
            }
            kept += 1;
            kept <= 40
        });
        let mut images = 0;
        for (item, d) in h.iter_mut() {
            if let ClipData::Png(_) = d {
                images += 1;
                if images > 8 && !item.pinned {
                    *d = ClipData::Text(String::new());
                }
            }
        }
        h.retain(|(item, d)| !(item.kind == "image" && matches!(d, ClipData::Text(t) if t.is_empty())));
    }

    pub fn clip_history(&self) -> Vec<ClipItem> {
        self.history.lock().iter().map(|(i, _)| i.clone()).collect()
    }

    /// Copy an item from the history again (on this computer).
    pub fn clip_use(&self, id: u64) {
        let data = self.history.lock().iter().find(|(i, _)| i.id == id).map(|(_, d)| d.clone());
        if let (Some(d), Some(set)) = (data, self.clip_setter.lock().as_ref()) {
            set(d);
        }
    }

    pub fn clip_pin(&self, id: u64, pinned: bool) {
        {
            let mut h = self.history.lock();
            if let Some((item, _)) = h.iter_mut().find(|(i, _)| i.id == id) {
                item.pinned = pinned;
            }
        }
        self.save_pins();
    }

    pub fn clip_forget(&self, id: u64) {
        self.history.lock().retain(|(i, _)| i.id != id);
        self.save_pins();
    }

    fn save_pins(&self) {
        let pins: Vec<(String, String)> = self
            .history
            .lock()
            .iter()
            .filter(|(i, _)| i.pinned)
            .filter_map(|(i, d)| match d {
                ClipData::Text(t) => Some((i.from.clone(), t.clone())),
                _ => None,
            })
            .collect();
        if let Some(path) = pins_path() {
            let _ = std::fs::create_dir_all(path.parent().unwrap_or(std::path::Path::new(".")));
            let text: String = pins.iter().map(|(from, t)| format!("{}\t{}\n", esc(from), esc(t))).collect();
            let _ = std::fs::write(path, text);
        }
    }

    // ------------------------------------------------------ shelf

    /// This computer put files on the shelf (served by offer `item.id`).
    pub fn shelf_add(&self, item: ShelfItem) {
        let list = {
            let mut sh = self.shelves.lock();
            let mine = sh.entry(self.me.clone()).or_default();
            mine.insert(0, item);
            mine.clone()
        };
        self.send("*", Ext::Shelf { items: list });
    }

    /// Take an item off the shelf (any computer's).
    pub fn shelf_remove(&self, origin: &str, id: u64) {
        if origin == self.me {
            let list = {
                let mut sh = self.shelves.lock();
                let mine = sh.entry(self.me.clone()).or_default();
                mine.retain(|i| i.id != id);
                mine.clone()
            };
            self.send("*", Ext::Shelf { items: list });
        } else {
            if let Some(l) = self.shelves.lock().get_mut(origin) {
                l.retain(|i| i.id != id);
            }
            self.send(origin, Ext::ShelfRemove { id });
        }
    }

    pub fn shelf(&self) -> Vec<ShelfEntry> {
        let sh = self.shelves.lock();
        let online: BTreeSet<String> = self.stats.lock().keys().cloned().collect();
        sh.iter()
            .filter(|(o, _)| **o == self.me || online.contains(*o))
            .flat_map(|(o, items)| items.iter().map(move |i| ShelfEntry { origin: o.clone(), item: i.clone() }))
            .collect()
    }

    pub fn shelf_item(&self, origin: &str, id: u64) -> Option<ShelfItem> {
        self.shelves.lock().get(origin).and_then(|l| l.iter().find(|i| i.id == id).cloned())
    }

    // ------------------------------------------------------ launcher

    pub fn apps(&self) -> Vec<AppsOf> {
        let mut v: Vec<AppsOf> = self.apps.lock().iter().map(|(n, a)| AppsOf { name: n.clone(), apps: a.clone() }).collect();
        v.sort_by_key(|a| (a.name != self.me, a.name.clone()));
        v
    }

    /// Open an app here; if it doesn't start, say so on the screen in use.
    fn launch_here(&self, id: &str) {
        let id = id.to_string();
        let name = self.apps.lock().get(&self.me).and_then(|l| l.iter().find(|a| a.id == id).map(|a| a.name.clone())).unwrap_or_else(|| id.clone());
        let this = self.this.clone();
        std::thread::spawn(move || {
            if let Err(e) = apps::launch(&id) {
                if let Some(h) = this.upgrade() {
                    h.notice_everywhere(&format!("{name} didn't open"), &format!("On {}: {e}", h.me), "info");
                }
            }
        });
    }

    /// The apps running on every computer (this one first).
    pub fn tasks(&self) -> Vec<(String, Vec<crate::protocol::RunningApp>)> {
        let mut v: Vec<_> = self.tasks.lock().iter().map(|(n, l)| (n.clone(), l.clone())).collect();
        v.sort_by_key(|(n, _)| (*n != self.me, n.clone()));
        v
    }

    /// Quit an app on computer `on`.
    pub fn quit(&self, on: &str, pids: Vec<u32>, force: bool) {
        if on == self.me {
            let _ = self.quit_tx.send((pids, force));
        } else {
            self.send(on, Ext::Quit { pids, force });
        }
    }

    /// Bring a window to the front on computer `on`.
    pub fn raise(&self, on: &str, window: u64) {
        if on == self.me {
            std::thread::spawn(move || wins::raise(window));
        } else {
            self.send(on, Ext::Raise { window });
        }
    }

    /// Keep the list of running apps, tell the others, and quit apps.
    fn tasks_loop(self: Arc<Self>, rx: crossbeam_channel::Receiver<(Vec<u32>, bool)>) {
        let mut t = tasks::Tasks::new();
        let mut last: Vec<crate::protocol::RunningApp> = vec![];
        let mut sent_at = Instant::now() - Duration::from_secs(60);
        let mut was_connected = false;
        while !self.stop.load(Ordering::SeqCst) {
            let quick = match rx.recv_timeout(Duration::from_secs(3)) {
                Ok((pids, force)) => {
                    // Only processes we listed ourselves.
                    let ours: BTreeSet<u32> = last.iter().flat_map(|a| a.pids.iter().copied()).collect();
                    let pids: Vec<u32> = pids.into_iter().filter(|p| ours.contains(p)).collect();
                    t.quit(&pids, force);
                    std::thread::sleep(Duration::from_millis(700));
                    true
                }
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => return,
                Err(_) => false,
            };
            let windows = self.lists.lock().get(&self.me).cloned().unwrap_or_default();
            let installed = self.apps.lock().get(&self.me).cloned().unwrap_or_default();
            if installed.is_empty() && windows.is_empty() {
                continue;
            }
            let now = t.list(&windows, &installed);
            let connected = self.out.read().is_some();
            // Changed: apps or windows came or went, memory moved a lot, or now and then.
            let shape = |v: &[crate::protocol::RunningApp]| v.iter().map(|a| (a.name.clone(), a.windows.clone(), a.pids.clone())).collect::<Vec<_>>();
            let mem_moved = now.iter().zip(last.iter()).any(|(a, b)| a.memory.abs_diff(b.memory) > b.memory / 10 + (16 << 20));
            let changed = quick || shape(&now) != shape(&last) || mem_moved || sent_at.elapsed() > Duration::from_secs(30);
            self.tasks.lock().insert(self.me.clone(), now.clone());
            if connected && (changed || !was_connected) {
                self.send("*", Ext::Tasks { list: now.clone() });
                sent_at = Instant::now();
            }
            if changed {
                last = now;
            }
            was_connected = connected;
        }
    }

    /// What's playing on every computer (this one first).
    pub fn media(&self) -> Vec<(String, media::NowPlaying)> {
        let mut v: Vec<(String, media::NowPlaying)> = self.media.lock().iter().map(|(n, m)| (n.clone(), m.clone())).collect();
        // Playing first, then this computer.
        v.sort_by_key(|(n, m)| (!m.playing, *n != self.me, n.clone()));
        v
    }

    /// Play/pause, skip… on computer `on`.
    pub fn media_cmd(&self, on: &str, cmd: media::MediaCmd) {
        if on == self.me {
            let _ = self.media_tx.send(cmd);
        } else {
            self.send(on, Ext::MediaCmd { cmd });
        }
    }

    /// Watch what's playing here, tell the others, and carry out commands.
    fn media_loop(self: Arc<Self>, rx: crossbeam_channel::Receiver<media::MediaCmd>) {
        let mut m = media::Media::new();
        let mut last: Option<media::NowPlaying> = None;
        let mut sent_at = Instant::now();
        let mut was_connected = false;
        let every = Duration::from_millis(if cfg!(target_os = "macos") { 2000 } else { 1000 });
        while !self.stop.load(Ordering::SeqCst) {
            match rx.recv_timeout(every) {
                Ok(cmd) => {
                    m.command(cmd);
                    std::thread::sleep(Duration::from_millis(250));
                }
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => return,
                Err(_) => {}
            }
            let now = m.now();
            let connected = self.out.read().is_some();
            // Changed: a different track or state, a jump in position, or now and then.
            let changed = match (&last, &now) {
                (None, None) => false,
                (Some(a), Some(b)) => {
                    let drift = match (a.position, b.position) {
                        (Some(pa), Some(pb)) => {
                            let expect = pa + if a.playing { (b.at.saturating_sub(a.at)) as f64 / 1000.0 } else { 0.0 };
                            (pb - expect).abs() > 2.5
                        }
                        _ => false,
                    };
                    !a.same_track_state(b) || drift || sent_at.elapsed() > Duration::from_secs(20)
                }
                _ => true,
            };
            if changed || (connected && !was_connected) {
                {
                    let mut map = self.media.lock();
                    match &now {
                        Some(n) => map.insert(self.me.clone(), n.clone()),
                        None => map.remove(&self.me),
                    };
                }
                if connected {
                    self.send("*", Ext::Media { now: now.clone() });
                }
                sent_at = Instant::now();
                last = now;
            }
            was_connected = connected;
        }
    }

    /// Open app `id` on computer `on`.
    pub fn launch(&self, on: &str, id: &str) {
        if on == self.me {
            self.launch_here(id);
        } else {
            self.send(on, Ext::Launch { id: id.into() });
        }
    }

    /// A full-screen app is in front on computer `name` (this one: "").
    pub fn fullscreen(&self, name: &str) -> bool {
        if name.is_empty() || name == self.me {
            return self.fullscreen.load(Ordering::SeqCst);
        }
        self.stats.lock().get(name).map(|s| s.fullscreen).unwrap_or(false)
    }

    /// This computer's desktop bounds.
    pub fn set_screen(&self, r: crate::protocol::Rect) {
        *self.screen.lock() = Some(r);
    }

    /// Send an [`Ext`] to another computer.
    pub fn tell(&self, to: &str, ext: Ext) {
        self.send(to, ext)
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
        // And how every computer is doing, its apps and its shelf.
        let stats = self.stats.lock().clone();
        for (origin, st) in stats {
            if origin != name {
                send_as(&origin, Ext::Status(st));
            }
        }
        let apps = self.apps.lock().clone();
        for (origin, list) in apps {
            if origin != name {
                send_as(&origin, Ext::Apps { list });
            }
        }
        let tasks = self.tasks.lock().clone();
        for (origin, list) in tasks {
            if origin != name {
                send_as(&origin, Ext::Tasks { list });
            }
        }
        let media = self.media.lock().clone();
        for (origin, now) in media {
            if origin != name {
                send_as(&origin, Ext::Media { now: Some(now) });
            }
        }
        let shelves = self.shelves.lock().clone();
        for (origin, items) in shelves {
            if origin != name && !items.is_empty() {
                send_as(&origin, Ext::Shelf { items });
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
        self.stats.lock().remove(name);
        self.apps.lock().remove(name);
        self.media.lock().remove(name);
        self.tasks.lock().remove(name);
        self.shelves.lock().remove(name);
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
        self.viewers.lock().insert(stream, Viewer { origin: origin.into(), window, queue: vec![], received: 0, paused: None, closed: false });
        self.send(origin, Ext::WinOpen { stream, window, os: Os::current() });
        self.ui(UiEvent::OpenViewer { stream, origin: origin.into(), title, w, h, at });
        log::info!("opening a live window from {origin}");
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
                        let (origin, last) = (v.origin.clone(), frames.last().map(|f| f.remote).unwrap_or(0));
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

    /// A live window shown here started being dragged by its title bar.
    pub fn viewer_drag(&self, stream: u64, on: bool) {
        let mut d = self.dragging.lock();
        if on {
            *d = Some((stream, Instant::now()));
        } else if d.map(|x| x.0) == Some(stream) {
            *d = None;
        }
    }

    /// The live window being dragged here right now. The caller checks the
    /// left button is still held (it knows best: on a computer being
    /// controlled, the button was pressed by OpenHop itself).
    pub fn dragged_viewer(&self) -> Option<u64> {
        let d = *self.dragging.lock();
        let (stream, since) = d?;
        let alive = self.viewers.lock().contains_key(&stream);
        (alive && since.elapsed() < Duration::from_secs(600)).then_some(stream)
    }

    /// The left button was let go: no live window is being dragged any more.
    pub fn drag_ended(&self) {
        if self.dragging.lock().take().is_some() {
            log::debug!("live window put down");
        }
    }

    /// A live window shown here was dragged onto `to`'s screen: move it
    /// there. If that's the computer it belongs to, it simply goes back.
    pub fn move_viewer(&self, stream: u64, to: &str) {
        let Some((origin, window)) = self.viewers.lock().get(&stream).map(|v| (v.origin.clone(), v.window)) else {
            return;
        };
        if to == origin {
            log::info!("live window dragged back to {origin}");
            self.send(&origin, Ext::WinReturn { stream });
        } else {
            log::info!("live window from {origin} dragged on to {to}");
            self.offer_window(to, &origin, window);
            self.send(&origin, Ext::WinClose { stream });
        }
        self.viewer_drag(stream, false);
        self.viewer_closed(stream);
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
            Ext::Status(st) => {
                self.stats.lock().insert(from.into(), st);
            }
            Ext::Focus { on } => self.apply_focus(on),
            Ext::Lock => {
                if system::locked() != Some(true) {
                    *self.lock_expect.lock() = Some(Instant::now());
                    std::thread::spawn(system::lock_now);
                }
            }
            Ext::Sleep => {
                std::thread::spawn(system::sleep_now);
            }
            Ext::Wake => {
                *self.lock_expect.lock() = Some(Instant::now());
                std::thread::spawn(system::wake_display);
            }
            Ext::Locate => self.locate_here(),
            Ext::Apps { list } => {
                self.apps.lock().insert(from.into(), list);
            }
            Ext::Launch { id } => {
                // Only apps from our own list.
                if self.apps.lock().get(&self.me).map(|l| l.iter().any(|a| a.id == id)).unwrap_or(false) {
                    self.launch_here(&id);
                }
            }
            Ext::Shelf { items } => {
                self.shelves.lock().insert(from.into(), items);
            }
            Ext::Media { now } => {
                let mut m = self.media.lock();
                match now {
                    Some(n) => m.insert(from.into(), n),
                    None => m.remove(from),
                };
            }
            Ext::MediaCmd { cmd } => {
                let _ = self.media_tx.send(cmd);
            }
            Ext::Tasks { list } => {
                self.tasks.lock().insert(from.into(), list);
            }
            Ext::Quit { pids, force } => {
                let _ = self.quit_tx.send((pids, force));
            }
            Ext::Raise { window } => {
                std::thread::spawn(move || wins::raise(window));
            }
            Ext::ShelfRemove { id } => self.shelf_remove(&self.me.clone(), id),
            Ext::Notice { title, body, icon } => self.notice_here(&title, &body, &icon),
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
            Ext::WinFrame { stream, seq, w, h, bar, title, patches } => {
                log::trace!("{} got update {seq}", ms());
                let known = {
                    let mut viewers = self.viewers.lock();
                    match viewers.get_mut(&stream) {
                        Some(v) => {
                            v.received += 1;
                            let f = Frame { seq: v.received, remote: seq, w, h, bar, title, patches: Arc::new(patches) };
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
            Ext::WinMoveTo { stream, to } => self.move_viewer(stream, &to),
            Ext::WinMaximize { stream, on } => {
                if self.viewers.lock().contains_key(&stream) {
                    self.ui(UiEvent::MaximizeViewer { stream, on });
                }
            }
            Ext::WinReturn { stream } => {
                if let Some(s) = self.sources.lock().remove(&stream) {
                    s.returning.store(true, Ordering::SeqCst);
                    s.stop.store(true, Ordering::SeqCst);
                    s.poke.notify_all();
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
            returning: AtomicBool::new(false),
            maxed: AtomicBool::new(false),
        });
        self.sources.lock().insert(stream, src.clone());
        // It has moved to the other screen: hide it here (it keeps running).
        wins::set_hidden(window, true);
        let hub = self.clone();
        let _ = std::thread::Builder::new().name("live-window".into()).spawn(move || {
            hub.stream_loop(stream, &src);
            hub.sources.lock().remove(&stream);
            if src.returning.load(Ordering::SeqCst) {
                hub.welcome_back(src.window);
            } else {
                // It may be moving on to another computer (opened again there
                // in a moment): wait before showing it here.
                std::thread::sleep(Duration::from_millis(1500));
                // Back on this screen, unless it's still open somewhere else.
                if !hub.sources.lock().values().any(|s| s.window == src.window) {
                    wins::set_hidden(src.window, false);
                }
            }
            let mut f = hub.focused.lock();
            if f.as_ref().map(|x| x.1) == Some(stream) {
                *f = None;
            }
        });
    }

    /// A window of ours was dragged back from another screen: show it under
    /// the pointer, still following the mouse while the button is held.
    fn welcome_back(&self, window: u64) {
        let Some(g) = wins::geometry(window) else {
            wins::set_hidden(window, false);
            return;
        };
        let bar = wins::bar_height(window).max(20);
        if let Some((px, py)) = crate::platform::cursor_pos() {
            let (mut x, mut y) = (px - g.w / 2, py - bar / 2);
            // Keep it on the screen (it arrives at the edge).
            if let Some(s) = *self.screen.lock() {
                x = x.clamp(s.x, (s.x + s.w - g.w).max(s.x));
                y = y.clamp(s.y, (s.y + s.h - bar).max(s.y));
            }
            wins::move_to(window, x, y);
        }
        wins::set_hidden(window, false);
        wins::activate(window);
        let held = crate::platform::dnd::left_button_down();
        log::info!("window back on this screen{}", if held { "; following the mouse" } else { "" });
        if held {
            std::thread::sleep(Duration::from_millis(60));
            if let Some((px, py)) = crate::platform::cursor_pos() {
                wins::begin_move(window, px, py);
            }
        }
    }

    fn stream_loop(&self, stream: u64, src: &Source) {
        let (fps, q, max_w, full_colour) = quality(&self.settings.read().quality);
        let mut bar = (wins::bar_height(src.window).max(0) as u32, Instant::now());
        let interval = Duration::from_millis(1000 / fps as u64);
        let mut seq = 0u64;
        // The last picture sent (after scaling), to find what changed.
        let mut prev: Option<wins::Picture> = None;
        let mut last_full = Instant::now();
        let mut sent_at = Instant::now();
        let mut paused_sent = false;
        let mut last_max_check = Instant::now();
        // Area sent at lower quality during a big change, to send again sharp.
        let mut rough: Option<(u32, u32, u32, u32)> = None;
        let mut rough_pass;
        while !src.stop.load(Ordering::SeqCst) && !self.stop.load(Ordering::SeqCst) {
            let pause = src.remote_pause.lock().clone();
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
            // Maximized on its own screen (its maximize button, a double click
            // on its title bar): maximize the view instead, on this screen
            // size, like an app opened here would.
            if last_max_check.elapsed() > Duration::from_millis(40) {
                last_max_check = Instant::now();
                if wins::is_maximized(src.window) {
                    wins::unmaximize(src.window);
                    let on = !src.maxed.fetch_xor(true, Ordering::SeqCst);
                    log::debug!("live window {} on the other screen", if on { "maximized" } else { "restored" });
                    self.send(&src.viewer, Ext::WinMaximize { stream, on });
                    continue;
                }
            }
            let mut rects = if full { vec![(0, 0, fw, fh)] } else { changed_rects(prev.as_ref().unwrap(), &pic) };
            if rects.is_empty() {
                // Things settled after a big change (scrolling) sent quickly at
                // lower quality: send those areas again, sharp.
                if let Some(r) = rough.filter(|_| sent_at.elapsed() > Duration::from_millis(180)) {
                    rough = None;
                    rects = vec![r];
                    rough_pass = false;
                } else {
                    // Right after input, look again soon: the app is about to redraw.
                    let wait = if src.busy() { Duration::from_millis(8) } else { interval };
                    src.nap(wait.saturating_sub(started.elapsed()).max(Duration::from_millis(4)));
                    continue;
                }
            } else {
                // A big part of the window changed at once (scrolling, a new
                // page): send it fast first, then sharp once it settles.
                let area: u64 = rects.iter().map(|r| r.2 as u64 * r.3 as u64).sum();
                rough_pass = !full && area * 3 > fw as u64 * fh as u64;
                if rough_pass {
                    for r in &rects {
                        rough = Some(match rough {
                            None => *r,
                            Some(u) => {
                                let (x0, y0) = (u.0.min(r.0), u.1.min(r.1));
                                let (x1, y1) = ((u.0 + u.2).max(r.0 + r.2), (u.1 + u.3).max(r.1 + r.3));
                                (x0, y0, x1 - x0, y1 - y0)
                            }
                        });
                    }
                }
            }
            let (pq, colour) = if rough_pass { (q.min(72), false) } else { (q, full_colour) };
            // Bands of at most 64 rows: small messages, so keys and clicks
            // going the other way never wait behind a big picture.
            let bands: Vec<(u32, u32, u32, u32)> =
                rects.into_iter().flat_map(|(x, y, w, h)| (0..h.div_ceil(64)).map(move |i| (x, y + i * 64, w, (h - i * 64).min(64)))).collect();
            let patches: Vec<Patch> =
                bands.into_iter().filter_map(|(x, y, w, h)| encode_patch(&pic, (x, y, w, h), pq, colour).map(|jpeg| Patch { x, y, w, h, jpeg })).collect();
            if patches.is_empty() {
                std::thread::sleep(interval);
                continue;
            }
            if full {
                last_full = Instant::now();
                rough = None;
            }
            *src.scale.lock() = (fw, fh, ow, oh);
            prev = Some(pic);
            seq += 1;
            let title = self.lists.lock().get(&self.me).and_then(|l| l.iter().find(|w| w.id == src.window).map(|w| w.title.clone())).unwrap_or_default();
            if bar.1.elapsed() > Duration::from_secs(3) {
                bar = (wins::bar_height(src.window).max(0) as u32, Instant::now());
            }
            let bar_px = (bar.0 as u64 * fh as u64 / oh.max(1) as u64) as u32;
            let len: usize = patches.iter().map(|p| p.jpeg.len()).sum();
            if seq == 1 || seq.is_multiple_of(200) {
                log::debug!(
                    "live window: update {seq} ({fw}x{fh}, {} patch(es), {} KB, {} ms) -> {}",
                    patches.len(),
                    len / 1024,
                    started.elapsed().as_millis(),
                    src.viewer
                );
            }
            log::trace!("{} send update {seq} ({} bytes, took {} ms)", ms(), len, started.elapsed().as_millis());
            // Split into messages of about 48 KB (same update number).
            let mut part: Vec<Patch> = vec![];
            let mut part_len = 0;
            let mut first = true;
            let n = patches.len();
            for (i, p) in patches.into_iter().enumerate() {
                part_len += p.jpeg.len();
                part.push(p);
                if part_len >= 48 * 1024 || i + 1 == n {
                    let t = if first { title.clone() } else { String::new() };
                    first = false;
                    self.send(&src.viewer, Ext::WinFrame { stream, seq, w: fw, h: fh, bar: bar_px, title: t, patches: std::mem::take(&mut part) });
                    part_len = 0;
                }
            }
            crate::files::pace(len);
            sent_at = Instant::now();
            let wait = if src.busy() { Duration::from_millis(8) } else { interval };
            src.nap(wait.saturating_sub(started.elapsed()));
        }
    }

    fn inject_for(&self, stream: u64, src: &Source, ev: WinEvent) {
        let Some(g) = wins::geometry(src.window) else {
            return;
        };
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
                let mut w = (w as i64 * ow as i64 / fw.max(1) as i64) as i32;
                let mut h = (h as i64 * oh as i64 / fh.max(1) as i64) as i32;
                // Bigger than this screen (a maximized view on a bigger screen):
                // the window can't be used beyond this screen's edges, so it
                // gets this screen's size in the same proportions, and the
                // view shows it a little larger.
                let screen = *self.screen.lock();
                if let Some(sc) = screen {
                    if w > sc.w || h > sc.h {
                        let k = (sc.w as f64 / w as f64).min(sc.h as f64 / h as f64);
                        w = (w as f64 * k) as i32;
                        h = (h as f64 * k) as i32;
                    }
                }
                if (w - g.w).abs() > 2 || (h - g.h).abs() > 2 {
                    wins::resize(src.window, w, h);
                    // Keep all of it on this screen (so every part can be clicked).
                    if let Some(sc) = screen {
                        let x = g.x.clamp(sc.x, (sc.x + sc.w - w).max(sc.x));
                        let y = g.y.clamp(sc.y, (sc.y + sc.h - h).max(sc.y));
                        if (x, y) != (g.x, g.y) {
                            wins::move_to(src.window, x, y);
                        }
                    }
                }
            }
            WinEvent::Blur => {
                let mut f = self.focused.lock();
                if f.as_ref().map(|x| x.1) == Some(stream) {
                    *f = None;
                }
            }
            WinEvent::Move { x, y } => {
                let (ax, ay) = map(x, y);
                st.at = (ax, ay);
                // Hover moves are thinned out; drags go through in full
                // (or, with whole clicks, at the release).
                let busy = st.buttons != 0;
                // With whole clicks, moving our pointer would mean letting go of
                // the mouse for a moment each time: only clicks and scrolls move it.
                let whole = self.whole_clicks.load(Ordering::SeqCst);
                if !whole && (busy || st.last_move.map(|t| t.elapsed() > Duration::from_millis(50)).unwrap_or(true)) {
                    ops.push(InjectOp::MoveTo(ax, ay));
                    st.last_move = Some(Instant::now());
                }
            }
            WinEvent::Button { button, down, x, y } => {
                if down {
                    activate(&mut st);
                    // Hidden windows are kept at the back while the mouse is
                    // used here: bring it up so the click lands on it.
                    wins::raise(src.window);
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
                st.at = (ax, ay);
                if self.whole_clicks.load(Ordering::SeqCst) {
                    if down {
                        if st.press.is_none() {
                            st.press = Some((button, (ax, ay)));
                        }
                    } else if let Some((b, (dx, dy))) = st.press.filter(|p| p.0 == button) {
                        st.press = None;
                        ops.push(InjectOp::MoveTo(dx, dy));
                        ops.push(InjectOp::Button(b, true));
                        if (ax - dx).abs() > 2 || (ay - dy).abs() > 2 {
                            // A drag (selecting text, moving a slider): replay it.
                            for i in 1..=6 {
                                ops.push(InjectOp::MoveTo(dx + (ax - dx) * i / 6, dy + (ay - dy) * i / 6));
                            }
                        }
                        ops.push(InjectOp::Button(b, false));
                    } else {
                        // Another button while one is held: just click it.
                        ops.push(InjectOp::MoveTo(ax, ay));
                        ops.push(InjectOp::Button(button, true));
                        ops.push(InjectOp::Button(button, false));
                    }
                } else {
                    ops.push(InjectOp::MoveTo(ax, ay));
                    ops.push(InjectOp::Button(button, down));
                }
            }
            WinEvent::Wheel { dx, dy, x, y } => {
                wins::raise(src.window);
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
        let mut status_sent = Instant::now() - Duration::from_secs(60);
        let mut apps_at = Instant::now() - Duration::from_secs(3600);
        let mut last_status: Option<PcStatus> = None;
        let mut disk = (system::disk(), Instant::now());
        // Battery warnings already given (thresholds), reset when plugged in.
        let mut warned: u8 = 101;
        let mut last_locked = system::locked();
        let mut last_dnd = system::dnd();
        while !self.stop.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(500));
            tick += 1;
            let connected = self.out.read().is_some();
            // Window list: every second, sent when it changes (and now and then).
            if tick.is_multiple_of(2) {
                let list = wins::list();
                self.lists.lock().insert(self.me.clone(), list.clone());
                let resend = self.resend.swap(false, Ordering::SeqCst);
                if resend || apps_at.elapsed() > Duration::from_secs(60) {
                    // Installed apps (for the launcher), now and then.
                    apps_at = Instant::now();
                    let mine = apps::list();
                    // Apps installed or removed since: tell the others.
                    let changed = self.apps.lock().insert(self.me.clone(), mine.clone()).as_ref() != Some(&mine);
                    if connected && (resend || changed) {
                        self.send("*", Ext::Apps { list: mine });
                        let shelf = self.shelves.lock().get(&self.me).cloned().unwrap_or_default();
                        self.send("*", Ext::Shelf { items: shelf });
                    }
                }
                if connected && (resend || last_list.as_ref() != Some(&list) || list_sent.elapsed() > Duration::from_secs(20)) {
                    self.send("*", Ext::Windows { list: list.clone() });
                    list_sent = Instant::now();
                    last_list = Some(list);
                }
            }
            // Full-screen app in front (games, videos): stay out of the way.
            if tick.is_multiple_of(2) {
                self.fullscreen.store(system::presenting(), Ordering::SeqCst);
            }
            if !tick.is_multiple_of(4) {
                continue;
            }
            // This computer at a glance (every 2 s, sent when it changes).
            if disk.1.elapsed() > Duration::from_secs(60) {
                disk = (system::disk(), Instant::now());
            }
            let locked = system::locked();
            let battery = system::battery();
            let st = PcStatus {
                battery,
                disk: disk.0,
                focus: self.focus(),
                locked: locked == Some(true),
                fullscreen: self.fullscreen.load(Ordering::SeqCst),
                os: Some(Os::current()),
            };
            self.stats.lock().insert(self.me.clone(), st.clone());
            let resend = last_status.as_ref() != Some(&st) || status_sent.elapsed() > Duration::from_secs(30);
            if connected && resend {
                self.send("*", Ext::Status(st.clone()));
                status_sent = Instant::now();
            }
            last_status = Some(st);
            // Running low: say so on whichever screen is in use.
            match battery {
                Some((pct, false)) => {
                    let step = [5u8, 10, 20].into_iter().find(|t| pct <= *t);
                    if let Some(t) = step {
                        if t < warned {
                            warned = t;
                            let body = if pct <= 5 { "Plug it in now, it's about to run out." } else { "Plug it in soon." };
                            self.notice_everywhere(&format!("{} is at {pct}%", self.me), body, "battery");
                        }
                    }
                }
                Some((_, true)) => warned = 101,
                None => {}
            }
            // Locked here: lock the others too. Unlocked: wake their displays.
            if locked.is_some() && locked != last_locked {
                let ours = self.lock_expect.lock().map(|t| t.elapsed() < Duration::from_secs(10)).unwrap_or(false);
                if !ours && connected {
                    if locked == Some(true) {
                        log::info!("this computer was locked; locking the others");
                        self.send("*", Ext::Lock);
                    } else {
                        self.send("*", Ext::Wake);
                    }
                }
                last_locked = locked;
            }
            // Do Not Disturb switched here by hand: Focus everywhere.
            let dnd_now = system::dnd();
            if dnd_now.is_some() && dnd_now != last_dnd {
                let ours = self.dnd_expect.lock().map(|(v, t)| Some(v) == dnd_now && t.elapsed() < Duration::from_secs(10)).unwrap_or(false);
                if !ours && dnd_now != Some(self.focus()) {
                    self.set_focus(dnd_now == Some(true));
                }
                last_dnd = dnd_now;
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
                // (Do Not Disturb itself is shared as Focus.)
                let quiet = self.fullscreen.load(Ordering::SeqCst);
                if quiet != last_quiet || (quiet && tick.is_multiple_of(40)) {
                    if connected {
                        self.send("*", Ext::Quiet { on: quiet });
                    }
                    last_quiet = quiet;
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

fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn same_clip(a: &ClipData, b: &ClipData) -> bool {
    match (a, b) {
        (ClipData::Text(x), ClipData::Text(y)) => x == y,
        (ClipData::Png(x), ClipData::Png(y)) => x.len() == y.len() && x == y,
        (ClipData::Files(x), ClipData::Files(y)) => x == y,
        _ => false,
    }
}

/// A small PNG of a copied picture (for the history list), as a data: URL.
fn thumbnail(png: &[u8]) -> Option<String> {
    let img = image::load_from_memory(png).ok()?;
    let small = img.thumbnail(160, 120);
    let mut out = std::io::Cursor::new(Vec::new());
    small.write_to(&mut out, image::ImageFormat::Png).ok()?;
    Some(format!("data:image/png;base64,{}", base64(&out.into_inner())))
}

fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        s.push(T[(n >> 18) as usize & 63] as char);
        s.push(T[(n >> 12) as usize & 63] as char);
        s.push(if c.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        s.push(if c.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    s
}

/// Pinned clipboard items survive restarts (text only).
fn pins_path() -> Option<std::path::PathBuf> {
    Some(dirs::config_dir()?.join("openhop").join("clipboard-pins.txt"))
}

fn esc(s: &str) -> String {
    s.replace('\\', "\\\\").replace('\t', "\\t").replace('\n', "\\n")
}

fn unesc(s: &str) -> String {
    let mut out = String::new();
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c == '\\' {
            match it.next() {
                Some('t') => out.push('\t'),
                Some('n') => out.push('\n'),
                Some(o) => out.push(o),
                None => {}
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn load_pins() -> Vec<(ClipItem, ClipData)> {
    let Some(text) = pins_path().and_then(|p| std::fs::read_to_string(p).ok()) else { return vec![] };
    text.lines()
        .filter_map(|l| l.split_once('\t'))
        .map(|(from, t)| {
            let t = unesc(t);
            let item = ClipItem {
                id: crate::files::new_id() & ((1 << 52) - 1),
                kind: "text",
                text: t.chars().take(400).collect(),
                thumb: None,
                from: unesc(from),
                at: now_secs(),
                pinned: true,
            };
            (item, ClipData::Text(t))
        })
        .collect()
}

/// Milliseconds clock for timing traces.
fn ms() -> u128 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() % 100_000).unwrap_or(0)
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
        hub.viewers.lock().insert(7, Viewer { origin: "pc".into(), window: 1, queue: vec![], received: 0, paused: None, closed: false });
        assert!(matches!(hub.frame(7, 0, Duration::from_millis(20)), FrameWait::Timeout));
        let h2 = hub.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(30));
            h2.handle(
                "pc",
                Ext::WinFrame {
                    stream: 7,
                    seq: 1,
                    w: 2,
                    h: 2,
                    bar: 0,
                    title: "t".into(),
                    patches: vec![Patch { x: 0, y: 0, w: 2, h: 2, jpeg: vec![1, 2, 3] }],
                },
            );
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
