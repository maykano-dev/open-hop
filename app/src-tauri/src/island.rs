//! The island: a small black pill at the top of the screen (around the
//! notch on a MacBook) that springs open into OpenHop's Control Center, and
//! shows short live activities (a computer running low, files arriving).

use crate::App;
use openhop_core::engine::{ComputerWindows, Note};
use openhop_core::extras::Computer;
use openhop_core::files::TransferInfo;
use serde::Serialize;
use std::collections::VecDeque;
use tauri::{AppHandle, Manager, PhysicalPosition, PhysicalSize, State, WebviewUrl, WebviewWindowBuilder};

/// The lane's height and the gap above the island in it (logical pixels).
pub const LANE: f64 = 34.0;
const LANE_TITLE: &str = "OpenHop lane";

/// Give the island a lane of its own across the top of the screen, which
/// maximized windows leave free. Checks the system really kept it free;
/// otherwise it goes back to resting as a thin line.
fn make_lane(app: &AppHandle, attempt: u32) {
    if cfg!(target_os = "macos") {
        return; // The menu bar is its lane.
    }
    let Some(m) = app.primary_monitor().ok().flatten() else { return };
    let scale = m.scale_factor();
    // GNOME on Wayland with OpenHop's Shell helper running: it makes the
    // lane. Otherwise (no helper yet, other desktops) the X11 way below
    // works through XWayland too.
    #[cfg(target_os = "linux")]
    if openhop_core::platform::linux_is_wayland() {
        // The helper may only start a little after OpenHop (both start at
        // login): keep asking for a while.
        let handle = app.clone();
        std::thread::spawn(move || {
            if attempt > 0 {
                return;
            }
            for _ in 0..240 {
                if openhop_core::wins::gnome::available() {
                    if let Some(top) = openhop_core::wins::gnome::lane(LANE as u32) {
                        log::info!("island lane under GNOME's top bar at {top}");
                        let h = handle.clone();
                        let _ = handle.run_on_main_thread(move || {
                            // The helper's lane replaces an XWayland one.
                            if let Some(w) = h.get_webview_window("lane") {
                                openhop_core::extras::lane::release(0, LANE_TITLE);
                                let _ = w.destroy();
                            }
                            let island = h.state::<Island>();
                            *island.lane_top.lock() = Some((top as f64 * scale).round() as i32);
                            island.fit.lock().lane = Some(LANE);
                            let g = *island.geo.lock();
                            if g.scale > 0.0 {
                                place(&h, g.w as f64 / g.scale, g.h as f64 / g.scale);
                            }
                        });
                        return;
                    }
                }
                std::thread::sleep(std::time::Duration::from_secs(5));
            }
        });
        if openhop_core::wins::gnome::available() {
            return;
        }
    }
    let area = m.work_area();
    let top = if attached() { m.position().y } else { area.position.y };
    let (x, width, height) = (m.position().x, m.size().width as i32, (LANE * scale).round() as i32);
    let Ok(lane) = WebviewWindowBuilder::new(app, "lane", WebviewUrl::App("lane.html".into()))
        .title(LANE_TITLE)
        .decorations(false)
        .transparent(true)
        .shadow(false)
        .skip_taskbar(true)
        // Resizable, or GTK makes it as tall as a web view likes to be.
        .resizable(true)
        .focused(false)
        .visible_on_all_workspaces(true)
        .inner_size(width as f64 / scale, LANE)
        .min_inner_size(20.0, 4.0)
        .position(x as f64 / scale, top as f64 / scale)
        .visible(false)
        .build()
    else {
        return;
    };
    // Linux: a dock window (window managers keep space free for docks).
    #[cfg(target_os = "linux")]
    if let Ok(g) = lane.gtk_window() {
        use gtk::prelude::GtkWindowExt;
        g.set_type_hint(gtk::gdk::WindowTypeHint::Dock);
    }
    let _ = lane.show();
    let _ = lane.set_ignore_cursor_events(true);
    #[cfg(windows)]
    let raw = lane.hwnd().map(|h| h.0 as isize).unwrap_or(0);
    #[cfg(not(windows))]
    let raw = 0isize;
    let handle = app.clone();
    std::thread::spawn(move || {
        // The window has to be on screen first.
        std::thread::sleep(std::time::Duration::from_millis(600));
        let reserved = openhop_core::extras::lane::reserve(raw, LANE_TITLE, x, top, width, height);
        // Did maximized windows really move down? (Give the system a moment.)
        let mut kept = false;
        for _ in 0..20 {
            std::thread::sleep(std::time::Duration::from_millis(200));
            if reserved && openhop_core::extras::lane::kept(top, height) {
                kept = true;
                break;
            }
        }
        log::info!("island lane: {}", if kept { "in use" } else { "not kept free by this system (yet)" });
        let h = handle.clone();
        let _ = handle.run_on_main_thread(move || {
            // The lane was shown after the island: some window managers put
            // it (and its frame, which takes the pointer) on top. The island
            // goes back above it.
            #[cfg(target_os = "linux")]
            if let Some(i) = h.get_webview_window("island") {
                use gtk::prelude::WidgetExt;
                if let Some(gw) = i.gtk_window().ok().and_then(|g| g.window()) {
                    gw.raise();
                }
            }
            let island = h.state::<Island>();
            if kept {
                *island.lane_top.lock() = Some(top);
                island.fit.lock().lane = Some(LANE);
            } else {
                openhop_core::extras::lane::release(raw, LANE_TITLE);
                if let Some(w) = h.get_webview_window("lane") {
                    let _ = w.destroy();
                }
                // Try again in a moment (an older copy of OpenHop may still
                // have been holding its lane while quitting).
                // Keep trying for a few minutes (the window manager may
                // still be starting, right after login).
                if attempt < 12 {
                    let h2 = h.clone();
                    std::thread::spawn(move || {
                        std::thread::sleep(std::time::Duration::from_secs(if attempt < 3 { 4 } else { 15 }));
                        let h3 = h2.clone();
                        let _ = h2.run_on_main_thread(move || make_lane(&h3, attempt + 1));
                    });
                }
            }
            let g = *island.geo.lock();
            if g.scale > 0.0 {
                place(&h, g.w as f64 / g.scale, g.h as f64 / g.scale);
            }
        });
    });
}

/// Give the lane back (quitting).
pub fn release_lane(handle: &AppHandle) {
    #[cfg(target_os = "linux")]
    if openhop_core::platform::linux_is_wayland() {
        openhop_core::wins::gnome::lane_off();
    }
    if let Some(w) = handle.get_webview_window("lane") {
        #[cfg(windows)]
        let raw = w.hwnd().map(|h| h.0 as isize).unwrap_or(0);
        #[cfg(not(windows))]
        let raw = 0isize;
        openhop_core::extras::lane::release(raw, LANE_TITLE);
        let _ = w.destroy();
    }
}

/// The island's window when OpenHop follows the pointer itself: big enough
/// for the island at its largest, so it never has to be resized or moved
/// (which shows as a jump). Clicks go through everywhere the island isn't.
const STAGE: (f64, f64) = (580.0, 780.0);
const ISLAND_TITLE: &str = "OpenHop Island";

/// Collapsed size (logical pixels).
pub const PILL: (f64, f64) = (210.0, 34.0);

/// Something to show briefly in the island.
#[derive(Clone, Serialize)]
pub struct Activity {
    pub title: String,
    pub body: String,
    pub icon: String,
    /// Shown briefly.
    pub short: bool,
}

/// What the island has to fit around at the top of the screen.
#[derive(Clone, Copy, Serialize, Default)]
pub struct Fit {
    /// A MacBook's camera notch (width, height in points).
    notch: Option<(f64, f64)>,
    /// The Mac menu bar's height.
    bar: Option<f64>,
    /// OpenHop follows the pointer itself (hover, click-through). Not on
    /// Wayland, which doesn't tell apps where the pointer is.
    watch: bool,
    /// The island's own lane at the top of the screen (its height), on
    /// Windows and Linux.
    lane: Option<f64>,
}

#[derive(Default)]
pub struct Island {
    pub activities: parking_lot::Mutex<VecDeque<Activity>>,
    fit: parking_lot::Mutex<Fit>,
    /// Top of the lane (physical pixels), while it's reserved.
    lane_top: parking_lot::Mutex<Option<i32>>,
    /// The window (physical pixels) and the part of it the island fills
    /// (logical pixels, centred at the top).
    geo: parking_lot::Mutex<Geo>,
}

#[derive(Default, Clone, Copy)]
struct Geo {
    x: i32,
    y: i32,
    w: u32,
    h: u32,
    scale: f64,
    /// The top of the screen it's on.
    screen_top: i32,
    vis_w: f64,
    vis_h: f64,
}

#[derive(Serialize)]
pub struct IslandState {
    running: bool,
    me: String,
    message: String,
    /// The computer the pointer is on ("" = this one).
    active: String,
    here: bool,
    focus: bool,
    connected: usize,
    computers: Vec<Computer>,
    windows: Vec<ComputerWindows>,
    transfers: Vec<TransferInfo>,
    notes: Vec<Note>,
    activities: Vec<Activity>,
    accent: String,
    dark: Option<bool>,
    fit: Fit,
    media: Vec<MediaOf>,
    /// What this computer's owner allows (switched-off ones aren't shown).
    features: Features,
    /// Who is seeing and using this computer with its owner's OK.
    helped_by: Vec<String>,
    /// Sitting at the very top edge (Windows, macOS) or under a top bar (Linux).
    attached: bool,
}

/// Where the island sits: centred at the top of the primary screen. On
/// Linux desktops the top bar is there, so just below it.
fn place(handle: &AppHandle, w: f64, h: f64) {
    let Some(win) = handle.get_webview_window("island") else { return };
    // Linux: always (the input shape lets clicks through around the island).
    let fixed = cfg!(target_os = "linux") || handle.state::<Island>().fit.lock().watch;
    let (w, h) = if fixed { STAGE } else { (w, h) };
    let Some(m) = win.primary_monitor().ok().flatten().or_else(|| win.current_monitor().ok().flatten()) else { return };
    let scale = m.scale_factor();
    let (pw, ph) = ((w * scale).round() as u32, (h * scale).round() as u32);
    let area = m.work_area();
    // Right at the top edge (macOS, Windows), or just under a top bar
    // (Linux desktops keep their clock and menus there).
    let lane_top = *handle.state::<Island>().lane_top.lock();
    let top = match lane_top {
        // In its lane: a little gap above it.
        // Hanging from the top of its lane, like a notch.
        Some(t) => t,
        None if attached() => m.position().y,
        None => area.position.y,
    };
    let x = m.position().x + (m.size().width as i32 - pw as i32) / 2;
    let island = handle.state::<Island>();
    let mut g = island.geo.lock();
    let same = g.x == x && g.y == top && g.w == pw && g.h == ph && g.scale == scale;
    if !same {
        // Both at once where the system allows: no jump in between.
        #[cfg(windows)]
        let raw = win.hwnd().map(|h| h.0 as isize).unwrap_or(0);
        #[cfg(not(windows))]
        let raw = 0isize;
        if !openhop_core::extras::lane::move_resize(raw, ISLAND_TITLE, x, top, pw, ph) {
            let _ = win.set_size(PhysicalSize::new(pw, ph));
            let _ = win.set_position(PhysicalPosition::new(x, top));
        }
    }
    *g = Geo { x, y: top, w: pw, h: ph, scale, screen_top: top, vis_w: g.vis_w, vis_h: g.vis_h };
}

/// Follow the pointer: open on hover, and let clicks through everywhere
/// the island isn't (the window is often bigger than the island while it
/// changes shape, and the resting island is only a thin lip).
fn watch(handle: AppHandle) {
    std::thread::spawn(move || {
        let mut probe = openhop_core::platform::PointerProbe::new();
        let mut hover = false;
        let mut through: Option<bool> = None;
        let mut dwell: Option<std::time::Instant> = None;
        let mut checked = std::time::Instant::now();
        let mut sure = std::time::Instant::now() - std::time::Duration::from_secs(60);
        let mut can = true;
        loop {
            std::thread::sleep(std::time::Duration::from_millis(30));
            // Wayland: only with OpenHop's GNOME helper (it may start later).
            #[cfg(target_os = "linux")]
            if openhop_core::platform::linux_is_wayland() && sure.elapsed() > std::time::Duration::from_secs(3) {
                sure = std::time::Instant::now();
                let now = openhop_core::wins::gnome::available();
                if now != can {
                    can = now;
                    let island = handle.state::<Island>();
                    island.fit.lock().watch = now;
                    log::info!("island follows the pointer itself: {now}");
                    if let Some(w) = handle.get_webview_window("island") {
                        let _ = w.set_ignore_cursor_events(false);
                    }
                    through = None;
                    let g = *island.geo.lock();
                    let h = handle.clone();
                    let _ = handle.run_on_main_thread(move || {
                        if g.scale > 0.0 {
                            place(&h, g.vis_w.max(20.0), g.vis_h.max(4.0));
                        }
                    });
                }
            }
            if !can {
                continue;
            }
            let Some(win) = handle.get_webview_window("island") else { continue };
            let Some((mut px, mut py, button)) = probe.read() else { continue };
            let g = *handle.state::<Island>().geo.lock();
            if g.scale <= 0.0 {
                continue;
            }
            if cfg!(target_os = "macos") {
                // Points to pixels.
                px = (px as f64 * g.scale) as i32;
                py = (py as f64 * g.scale) as i32;
            }
            // Where the window really is (a window manager may have moved
            // it): hit-test against that, never against where we asked.
            if checked.elapsed() > std::time::Duration::from_millis(500) {
                checked = std::time::Instant::now();
                if let Ok(p) = win.outer_position() {
                    let st = handle.state::<Island>();
                    let mut gg = st.geo.lock();
                    if (p.x - gg.x).abs() > 2 || (p.y - gg.y).abs() > 2 {
                        log::debug!("island is at {},{} (asked for {},{})", p.x, p.y, gg.x, gg.y);
                        gg.x = p.x;
                        gg.y = p.y;
                    }
                }
            }
            let g = *handle.state::<Island>().geo.lock();
            let vw = g.vis_w * g.scale;
            let vh = g.vis_h * g.scale;
            let left = g.x as f64 + (g.w as f64 - vw) / 2.0;
            // Resting outside a lane (a thin line, or a small pill under a
            // top bar): clicks go through until the pointer rests on it.
            let lip = handle.state::<Island>().fit.lock().lane.is_none() && !cfg!(target_os = "macos") && g.vis_h <= 30.0;
            // A thin lip is easy to miss: count the strip above it, and a bit to the sides.
            let pad = if lip { 14.0 * g.scale } else { 2.0 };
            let inside = (px as f64) >= left - pad
                && (px as f64) <= left + vw + pad
                && py >= g.screen_top.min(g.y) - 1
                && (py as f64) <= g.y as f64 + vh + if lip { 4.0 * g.scale } else { 2.0 };
            let want = if !inside {
                dwell = None;
                false
            } else if hover || button {
                // Dragging files to it opens it straight away.
                true
            } else {
                // Resting: only after the pointer stays a moment, so a quick
                // click on a browser tab underneath doesn't open it.
                let since = *dwell.get_or_insert_with(std::time::Instant::now);
                since.elapsed() > std::time::Duration::from_millis(if lip { 220 } else { 90 })
            };
            if want != hover {
                hover = want;
                let _ = tauri::Emitter::emit_to(&win, "island", "hover", hover);
            }
            // The resting lip never takes a click (a browser tab may be
            // right under it); it only opens after the pointer rests on it.
            let pass = !(hover || (inside && !lip));
            if through != Some(pass) {
                through = Some(pass);
                let _ = win.set_ignore_cursor_events(pass);
            }
        }
    });
}

fn attached() -> bool {
    !cfg!(target_os = "linux")
}

pub fn create(app: &tauri::App) -> tauri::Result<()> {
    let win = WebviewWindowBuilder::new(app, "island", WebviewUrl::App("island.html".into()))
        .title(ISLAND_TITLE)
        .inner_size(PILL.0, PILL.1)
        .decorations(false)
        .transparent(true)
        .shadow(false)
        .always_on_top(true)
        .visible_on_all_workspaces(true)
        .skip_taskbar(true)
        // Resizable (it has no frame to drag): a fixed size would also fix
        // the smallest size GTK allows, and the island changes shape.
        .resizable(true)
        .min_inner_size(20.0, 4.0)
        .focused(false)
        .visible(false)
        .build()?;
    // Linux: a dock window, like a panel. Window managers then leave it where
    // it's put (normal windows get pushed below the top bar and the lane)
    // and keep it above other windows.
    #[cfg(target_os = "linux")]
    if let Ok(g) = win.gtk_window() {
        use gtk::prelude::GtkWindowExt;
        g.set_type_hint(gtk::gdk::WindowTypeHint::Dock);
    }
    let _ = win.show();
    #[cfg(target_os = "macos")]
    above_menu_bar(&win);
    let _ = win;
    // Linux: the window's input shape is the island itself, so the page's own
    // pointer events are exact (and work on Wayland too). Elsewhere OpenHop
    // follows the pointer and lets clicks through around the island.
    let watch_pointer = !cfg!(target_os = "linux");
    {
        let island = app.state::<Island>();
        let mut fit = island.fit.lock();
        fit.watch = watch_pointer;
        #[cfg(target_os = "macos")]
        {
            fit.notch = openhop_core::extras::system::notch();
            fit.bar = openhop_core::extras::system::menu_bar_height();
            log::info!("notch: {:?}, menu bar: {:?}", fit.notch, fit.bar);
        }
    }
    place(app.handle(), PILL.0, PILL.1);
    // After older copies of OpenHop have quit (they may hold a lane).
    let handle = app.handle().clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(2500));
        let h = handle.clone();
        let _ = handle.run_on_main_thread(move || make_lane(&h, 0));
    });
    if watch_pointer {
        watch(app.handle().clone());
    } else {
        #[cfg(target_os = "linux")]
        backstop(app.handle().clone());
    }
    Ok(())
}

/// Linux: the page sees the pointer come and go itself, but some systems
/// don't tell it when the pointer leaves (it stayed open). Where OpenHop can
/// see the pointer reliably (X11, or GNOME's helper on Wayland), it says
/// whether the pointer is on the island, as a second opinion.
#[cfg(target_os = "linux")]
fn backstop(handle: AppHandle) {
    std::thread::spawn(move || {
        let mut probe = openhop_core::platform::PointerProbe::new();
        let mut last: Option<bool> = None;
        let mut out_count = 0;
        let mut reliable = (false, std::time::Instant::now() - std::time::Duration::from_secs(60));
        loop {
            std::thread::sleep(std::time::Duration::from_millis(120));
            if reliable.1.elapsed() > std::time::Duration::from_secs(3) {
                let ok = !openhop_core::platform::linux_is_wayland() || openhop_core::wins::gnome::available();
                reliable = (ok, std::time::Instant::now());
            }
            if !reliable.0 {
                continue;
            }
            let g = *handle.state::<Island>().geo.lock();
            if g.scale <= 0.0 {
                continue;
            }
            let Some((px, py, _)) = probe.read() else { continue };
            let vw = g.vis_w * g.scale;
            let vh = g.vis_h * g.scale;
            let left = g.x as f64 + (g.w as f64 - vw) / 2.0;
            let m = 6.0 * g.scale;
            let inside = (px as f64) >= left - m && (px as f64) <= left + vw + m && (py as f64) >= g.y as f64 - m - 40.0 && (py as f64) <= g.y as f64 + vh + m;
            // Out twice in a row (a quick wobble at the edge isn't leaving).
            out_count = if inside { 0 } else { out_count + 1 };
            let now = if inside { Some(true) } else if out_count >= 2 { Some(false) } else { last };
            if now != last {
                last = now;
                if let (Some(v), Some(w)) = (now, handle.get_webview_window("island")) {
                    let _ = tauri::Emitter::emit_to(&w, "island", "pointer_in", v);
                }
            }
        }
    });
}

/// macOS: float over the menu bar (around the notch) on every Space, even
/// next to full-screen apps.
#[cfg(target_os = "macos")]
fn above_menu_bar(win: &tauri::WebviewWindow) {
    use objc2::runtime::AnyObject;
    let Ok(ns) = win.ns_window() else { return };
    let ns = ns as *mut AnyObject;
    if ns.is_null() {
        return;
    }
    unsafe {
        // NSStatusWindowLevel; canJoinAllSpaces | stationary | fullScreenAuxiliary.
        let _: () = objc2::msg_send![ns, setLevel: 25isize];
        let _: () = objc2::msg_send![ns, setCollectionBehavior: (1usize | 16 | 256)];
        let _: () = objc2::msg_send![ns, setHasShadow: objc2::runtime::Bool::NO];
    }
}

/// Keep the island out of the way of full-screen games and videos.
pub fn follow_fullscreen(handle: &AppHandle, fullscreen: bool) {
    if let Some(w) = handle.get_webview_window("island") {
        let visible = w.is_visible().unwrap_or(true);
        if fullscreen && visible {
            let _ = w.hide();
        } else if !fullscreen && !visible {
            let _ = w.show();
        }
    }
}

/// Open the island (Ctrl+Alt+Space, the tray), or close it.
pub fn toggle(handle: &AppHandle) {
    if let Some(w) = handle.get_webview_window("island") {
        let _ = w.show();
        let _ = w.set_focus();
        let _ = tauri::Emitter::emit_to(&w, "island", "toggle", ());
    }
}

/// Open the island on one of its tabs ("clips", "open"), or close it.
pub fn show_tab(handle: &AppHandle, tab: &str) {
    if let Some(w) = handle.get_webview_window("island") {
        let _ = w.show();
        let _ = w.set_focus();
        // Keyboard focus inside the page too, so typing goes to its search.
        let _ = AsRef::<tauri::Webview>::as_ref(&w).set_focus();
        let _ = tauri::Emitter::emit_to(&w, "island", "tab", tab);
    }
}

pub fn push(handle: &AppHandle, a: Activity) {
    {
        let island = handle.state::<Island>();
        let mut q = island.activities.lock();
        q.push_back(a);
        while q.len() > 6 {
            q.pop_front();
        }
    }
    poke(handle);
}

/// Tell the island something changed, so it shows it now rather than at its
/// next look.
pub fn poke(handle: &AppHandle) {
    if let Some(w) = handle.get_webview_window("island") {
        let _ = tauri::Emitter::emit_to(&w, "island", "poke", ());
    }
}

// ------------------------------------------------------------------ commands

#[tauri::command]
pub fn island_state(handle: AppHandle, app: State<App>, island: State<Island>) -> IslandState {
    let status = app.engine.lock().as_ref().map(|e| (e.status(), e.hub()));
    let fit = *island.fit.lock();
    let media: Vec<MediaOf> = status.as_ref().map(|(_, hub)| hub.media().into_iter().map(|(name, now)| MediaOf { name, now }).collect()).unwrap_or_default();
    let cfg = app.config.lock().clone();
    let features = Features { control: cfg.allow_control, share: cfg.share_input, focus: cfg.allow_focus, lock: cfg.allow_lock, sleep: cfg.allow_sleep };
    let notes: Vec<Note> = app.toasts.lock().drain(..).collect();
    let activities: Vec<Activity> = island.activities.lock().drain(..).collect();
    let dark = crate::system_dark();
    match status {
        Some((st, hub)) => IslandState {
            running: st.running,
            me: st.name.clone(),
            message: st.message.clone(),
            active: st.active.clone(),
            here: hub.pointer_here(),
            focus: hub.focus(),
            connected: st.peers.len(),
            computers: hub.computers(),
            windows: st.windows.clone(),
            transfers: st.transfers.into_iter().chain(crate::phone::transfers(&handle)).filter(|t| !t.finished).collect(),
            notes,
            activities,
            accent: cfg.ui_accent,
            dark,
            attached: attached(),
            fit,
            media: media.clone(),
            features,
            helped_by: hub.helped_by(),
        },
        None => IslandState {
            running: false,
            me: cfg.name.clone(),
            message: "OpenHop is off".into(),
            active: String::new(),
            here: true,
            focus: false,
            connected: 0,
            computers: vec![],
            windows: vec![],
            transfers: crate::phone::transfers(&handle),
            notes,
            activities,
            accent: cfg.ui_accent,
            dark,
            attached: attached(),
            fit,
            media: media.clone(),
            features,
            helped_by: vec![],
        },
    }
}

#[derive(Serialize, Clone, Copy)]
pub struct Features {
    control: bool,
    share: bool,
    focus: bool,
    lock: bool,
    sleep: bool,
}

#[derive(Serialize, Clone)]
pub struct MediaOf {
    name: String,
    now: openhop_core::extras::media::NowPlaying,
}

#[derive(Serialize)]
pub struct Overview {
    focus: bool,
    me: String,
    active: String,
    computers: Vec<Computer>,
}

/// Every computer at a glance, for the main window (without taking the
/// island's notifications).
#[tauri::command]
pub fn overview(app: State<App>) -> Overview {
    match app.engine.lock().as_ref() {
        Some(e) => {
            let hub = e.hub();
            let st = e.status();
            Overview { focus: hub.focus(), me: st.name, active: st.active, computers: hub.computers() }
        }
        None => Overview { focus: false, me: app.config.lock().name.clone(), active: String::new(), computers: vec![] },
    }
}

/// The page resized the island (collapsed, an activity, or open). `vw` and
/// `vh`: the part the island fills now (the rest lets clicks through).
#[tauri::command]
pub fn island_size(handle: AppHandle, island: State<Island>, w: f64, h: f64, vw: Option<f64>, vh: Option<f64>) {
    {
        let mut g = island.geo.lock();
        g.vis_w = vw.unwrap_or(w);
        g.vis_h = vh.unwrap_or(h);
    }
    if w > 0.0 && h > 0.0 {
        place(&handle, w.clamp(20.0, 900.0), h.clamp(4.0, 900.0));
    }
    // Linux: not while the island is shrinking. Changing a window's input
    // shape makes WebKitGTK drop the running animation (the island snapped
    // shut instead of folding up); the page asks again once it's done.
    #[cfg(target_os = "linux")]
    if w > 0.0 && h > 0.0 {
        let (vw, vh) = (vw.unwrap_or(w), vh.unwrap_or(h));
        let h2 = handle.clone();
        let _ = handle.run_on_main_thread(move || input_shape(&h2, vw, vh));
    }
}

/// Linux: only the island itself takes the pointer; everywhere else in its
/// (fixed-size, see-through) window, clicks and hovering reach the windows
/// underneath. The window never has to move or resize, so nothing jumps.
#[cfg(target_os = "linux")]
fn input_shape(handle: &AppHandle, vw: f64, vh: f64) {
    use gtk::prelude::{GtkWindowExt, WidgetExt};
    let Some(win) = handle.get_webview_window("island") else { return };
    let Ok(gw) = win.gtk_window() else { return };
    let Some(gdk) = gw.window() else { return };
    let (ww, _) = gw.size();
    let w = vw.round().max(1.0) as i32;
    let h = vh.round().max(1.0) as i32;
    let x = ((ww - w) / 2).max(0);
    let region = gtk::cairo::Region::create_rectangle(&gtk::cairo::RectangleInt::new(x, 0, w, h));
    gdk.input_shape_combine_region(&region, 0, 0);
}

/// Play/pause, skip… on a computer ("" = this one).
#[tauri::command]
pub fn media_cmd(app: State<App>, on: String, cmd: String, at: Option<f64>) {
    use openhop_core::extras::media::MediaCmd;
    let cmd = match cmd.as_str() {
        "toggle" => MediaCmd::PlayPause,
        "next" => MediaCmd::Next,
        "previous" => MediaCmd::Previous,
        "shuffle" => MediaCmd::Shuffle,
        "seek" => MediaCmd::Seek(at.unwrap_or(0.0).max(0.0)),
        _ => return,
    };
    if let Some(e) = app.engine.lock().as_ref() {
        let hub = e.hub();
        let on = if on.is_empty() { hub.me().to_string() } else { on };
        hub.media_cmd(&on, cmd);
    }
}

/// Typing in the island's search: ask for the keyboard (a dock window
/// only gets it when it asks).
#[tauri::command]
pub fn island_take_focus(handle: AppHandle) {
    if let Some(w) = handle.get_webview_window("island") {
        let _ = w.set_focus();
        let _ = AsRef::<tauri::Webview>::as_ref(&w).set_focus();
    }
}

#[tauri::command]
pub fn island_focus(app: State<App>, on: bool) {
    if let Some(e) = app.engine.lock().as_ref() {
        e.hub().set_focus(on);
    }
}

#[tauri::command]
pub fn island_lock_all(app: State<App>) {
    if let Some(e) = app.engine.lock().as_ref() {
        e.hub().lock_all();
    }
}

#[tauri::command]
pub fn island_sleep_all(app: State<App>) {
    if let Some(e) = app.engine.lock().as_ref() {
        e.hub().sleep_all();
    }
}

#[tauri::command]
pub fn island_find_pointer(app: State<App>) {
    if let Some(e) = app.engine.lock().as_ref() {
        e.hub().find_pointer();
    }
}

#[tauri::command]
pub fn island_open_app(handle: AppHandle) {
    if let Some(w) = handle.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}
