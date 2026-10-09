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

/// Collapsed size (logical pixels).
pub const PILL: (f64, f64) = (210.0, 34.0);

/// Something to show briefly in the island.
#[derive(Clone, Serialize)]
pub struct Activity {
    pub title: String,
    pub body: String,
    pub icon: String,
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
}

#[derive(Default)]
pub struct Island {
    pub activities: parking_lot::Mutex<VecDeque<Activity>>,
    fit: parking_lot::Mutex<Fit>,
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
    /// Sitting at the very top edge (Windows, macOS) or under a top bar (Linux).
    attached: bool,
}

/// Where the island sits: centred at the top of the primary screen. On
/// Linux desktops the top bar is there, so just below it.
fn place(handle: &AppHandle, w: f64, h: f64) {
    let Some(win) = handle.get_webview_window("island") else { return };
    let Some(m) = win.primary_monitor().ok().flatten().or_else(|| win.current_monitor().ok().flatten()) else { return };
    let scale = m.scale_factor();
    let (pw, ph) = ((w * scale).round() as u32, (h * scale).round() as u32);
    let area = m.work_area();
    // Right at the top edge (macOS, Windows), or just under a top bar
    // (Linux desktops keep their clock and menus there).
    let top = if attached() { m.position().y } else { area.position.y };
    let x = m.position().x + (m.size().width as i32 - pw as i32) / 2;
    let _ = win.set_size(PhysicalSize::new(pw, ph));
    let _ = win.set_position(PhysicalPosition::new(x, top));
    let island = handle.state::<Island>();
    let mut g = island.geo.lock();
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
        loop {
            std::thread::sleep(std::time::Duration::from_millis(30));
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
            // Now and then make sure the window is still where it belongs
            // (screens change, and some systems move new windows).
            if checked.elapsed() > std::time::Duration::from_secs(2) {
                checked = std::time::Instant::now();
                if let Ok(p) = win.outer_position() {
                    if (p.x - g.x).abs() > 2 || (p.y - g.y).abs() > 2 {
                        let h = handle.clone();
                        let (w, hh) = (g.w as f64 / g.scale, g.h as f64 / g.scale);
                        let _ = handle.run_on_main_thread(move || place(&h, w, hh));
                    }
                }
            }
            let vw = g.vis_w * g.scale;
            let vh = g.vis_h * g.scale;
            let left = g.x as f64 + (g.w as f64 - vw) / 2.0;
            let lip = g.vis_h <= 10.0;
            // A thin lip is easy to miss: count the strip above it, and a bit to the sides.
            let pad = if lip { 14.0 * g.scale } else { 2.0 };
            let inside = (px as f64) >= left - pad
                && (px as f64) <= left + vw + pad
                && py >= g.screen_top.min(g.y) - 1
                && (py as f64) <= g.y as f64 + vh + if lip { 4.0 * g.scale } else { 2.0 };
            let want = if !inside {
                dwell = None;
                false
            } else if hover || !lip || button {
                // Dragging files to it opens it straight away.
                true
            } else {
                // Resting: only after the pointer stays a moment, so a quick
                // click on a browser tab underneath doesn't open it.
                let since = *dwell.get_or_insert_with(std::time::Instant::now);
                since.elapsed() > std::time::Duration::from_millis(220)
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
        .title("OpenHop")
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
        .build()?;
    #[cfg(target_os = "macos")]
    above_menu_bar(&win);
    let _ = win;
    let watch_pointer = !(cfg!(target_os = "linux") && openhop_core::platform::linux_is_wayland());
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
    if watch_pointer {
        watch(app.handle().clone());
    }
    Ok(())
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
    let island = handle.state::<Island>();
    let mut q = island.activities.lock();
    q.push_back(a);
    while q.len() > 6 {
        q.pop_front();
    }
}

// ------------------------------------------------------------------ commands

#[tauri::command]
pub fn island_state(app: State<App>, island: State<Island>) -> IslandState {
    let status = app.engine.lock().as_ref().map(|e| (e.status(), e.hub()));
    let fit = *island.fit.lock();
    let media: Vec<MediaOf> = status.as_ref().map(|(_, hub)| hub.media().into_iter().map(|(name, now)| MediaOf { name, now }).collect()).unwrap_or_default();
    let cfg = app.config.lock().clone();
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
            transfers: st.transfers.into_iter().filter(|t| !t.finished).collect(),
            notes,
            activities,
            accent: cfg.ui_accent,
            dark,
            attached: attached(),
            fit,
            media: media.clone(),
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
            transfers: vec![],
            notes,
            activities,
            accent: cfg.ui_accent,
            dark,
            attached: attached(),
            fit,
            media: media.clone(),
        },
    }
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
