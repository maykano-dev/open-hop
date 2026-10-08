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

#[derive(Default)]
pub struct Island {
    pub activities: parking_lot::Mutex<VecDeque<Activity>>,
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
    let top = if attached() { m.position().y } else { area.position.y + (6.0 * scale) as i32 };
    let x = m.position().x + (m.size().width as i32 - pw as i32) / 2;
    let _ = win.set_size(PhysicalSize::new(pw, ph));
    let _ = win.set_position(PhysicalPosition::new(x, top));
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
        .min_inner_size(60.0, 20.0)
        .focused(false)
        .build()?;
    #[cfg(target_os = "macos")]
    above_menu_bar(&win);
    let _ = win;
    place(app.handle(), PILL.0, PILL.1);
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
        },
    }
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

/// The page resized the island (collapsed, an activity, or open).
#[tauri::command]
pub fn island_size(handle: AppHandle, w: f64, h: f64) {
    place(&handle, w.clamp(80.0, 900.0), h.clamp(20.0, 900.0));
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
