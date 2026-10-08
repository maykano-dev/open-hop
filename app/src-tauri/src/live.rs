//! Windows OpenHop opens besides its main one: live windows from other
//! computers, the window dock, and the arrival animation.

use crate::App;
use openhop_core::extras::{FrameWait, UiEvent};
use openhop_core::protocol::WinEvent;
use std::time::Duration;
use tauri::{AppHandle, Emitter, LogicalPosition, LogicalSize, Manager, PhysicalPosition, State, WebviewUrl, WebviewWindowBuilder};

pub const FX_SIZE: f64 = 220.0;

fn hub(app: &App) -> Option<std::sync::Arc<openhop_core::extras::Hub>> {
    app.engine.lock().as_ref().map(|e| e.hub())
}

fn parse(s: &str) -> Result<u64, String> {
    s.parse().map_err(|_| "bad id".to_string())
}

/// Native screen coordinates -> where Tauri wants window positions.
fn to_logical(handle: &AppHandle, x: i32, y: i32) -> (f64, f64) {
    if cfg!(target_os = "macos") {
        // macOS already works in points.
        return (x as f64, y as f64);
    }
    let scale = handle
        .monitor_from_point(x as f64, y as f64)
        .ok()
        .flatten()
        .or_else(|| handle.primary_monitor().ok().flatten())
        .map(|m| m.scale_factor())
        .unwrap_or(1.0);
    let p = PhysicalPosition::new(x, y).to_logical::<f64>(scale);
    (p.x, p.y)
}

fn monitor_logical(handle: &AppHandle) -> (f64, f64) {
    handle
        .primary_monitor()
        .ok()
        .flatten()
        .map(|m| {
            let s = m.size().to_logical::<f64>(m.scale_factor());
            (s.width, s.height)
        })
        .unwrap_or((1440.0, 900.0))
}

fn enc(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// Open a viewer window for a live window from another computer.
fn open_viewer(handle: &AppHandle, stream: u64, origin: &str, title: &str, w: i32, h: i32, at: Option<(i32, i32)>) {
    let label = format!("view-{stream}");
    if handle.get_webview_window(&label).is_some() {
        return;
    }
    let (mw, mh) = monitor_logical(handle);
    // Same size as on the other computer, but never bigger than 85% of this screen.
    let (mut vw, mut vh) = (w.max(320) as f64, h.max(200) as f64);
    let k = (mw * 0.85 / vw).min(mh * 0.85 / vh).min(1.0);
    vw *= k;
    vh *= k;
    let url = format!("viewer.html?s={stream}&o={}&t={}", enc(origin), enc(title));
    let mut b = WebviewWindowBuilder::new(handle, &label, WebviewUrl::App(url.into()))
        .title(format!("{title} — on {origin}"))
        .inner_size(vw, vh)
        .min_inner_size(240.0, 160.0)
        .focused(true);
    b = match at {
        // Dragged across: put its title bar under the pointer, as if still held.
        Some((x, y)) => {
            let (lx, ly) = to_logical(handle, x, y);
            let x = (lx - vw / 2.0).clamp(0.0, (mw - vw).max(0.0));
            let y = (ly - 16.0).clamp(0.0, (mh - vh).max(0.0));
            b.position(x, y)
        }
        None => b.center(),
    };
    if let Err(e) = b.build() {
        log::warn!("couldn't open a live window: {e}");
    }
}

fn show_fx(handle: &AppHandle, x: i32, y: i32, payload: serde_json::Value) {
    let Some(w) = handle.get_webview_window("fx") else { return };
    let (lx, ly) = to_logical(handle, x, y);
    let _ = w.set_position(LogicalPosition::new(lx - FX_SIZE / 2.0, ly - FX_SIZE / 2.0));
    let _ = w.show();
    let _ = w.set_ignore_cursor_events(true);
    let _ = w.emit_to("fx", "fx", payload);
}

/// Act on what the engine wants shown. Called a few times a second.
pub fn pump(handle: &AppHandle) {
    let app = handle.state::<App>();
    let Some(hub) = hub(&app) else { return };
    for ev in hub.take_ui() {
        match ev {
            UiEvent::OpenViewer { stream, origin, title, w, h, at } => open_viewer(handle, stream, &origin, &title, w, h, at),
            UiEvent::CloseViewer { stream } => {
                if let Some(w) = handle.get_webview_window(&format!("view-{stream}")) {
                    let _ = w.destroy();
                }
            }
            UiEvent::Incoming { x, y, label, from } => {
                show_fx(handle, x, y, serde_json::json!({ "kind": "incoming", "label": label, "from": from }));
            }
            UiEvent::Landed { x, y, label } => {
                show_fx(handle, x, y, serde_json::json!({ "kind": "landed", "label": label }));
            }
        }
    }
}

/// Create the (hidden) arrival-animation window.
pub fn create_fx(app: &tauri::App) -> tauri::Result<()> {
    let w = WebviewWindowBuilder::new(app, "fx", WebviewUrl::App("fx.html".into()))
        .title("OpenHop")
        .inner_size(FX_SIZE, FX_SIZE)
        .decorations(false)
        .transparent(true)
        .shadow(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .resizable(false)
        .focused(false)
        .visible(false)
        .build()?;
    // Click-through is set once it's shown (GTK can't do it on a hidden window).
    let _ = w;
    Ok(())
}

pub fn toggle_dock(handle: &AppHandle) {
    if let Some(w) = handle.get_webview_window("dock") {
        log::debug!("dock toggle (visible: {:?})", w.is_visible());
        if w.is_visible().unwrap_or(false) {
            let _ = w.hide();
        } else {
            place_dock(handle, &w);
            let _ = w.show();
            let _ = w.set_focus();
            let _ = w.emit_to("dock", "shown", ());
        }
        return;
    }
    let (mw, _) = monitor_logical(handle);
    let width = (mw * 0.9).min(1100.0);
    match WebviewWindowBuilder::new(handle, "dock", WebviewUrl::App("dock.html".into()))
        .title("OpenHop Windows")
        .inner_size(width, 168.0)
        .decorations(false)
        .transparent(true)
        .shadow(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .resizable(false)
        .focused(true)
        .build()
    {
        Ok(w) => place_dock(handle, &w),
        Err(e) => log::warn!("couldn't open the dock: {e}"),
    }
}

fn place_dock(handle: &AppHandle, w: &tauri::WebviewWindow) {
    let (mw, mh) = monitor_logical(handle);
    let width = (mw * 0.9).min(1100.0);
    let _ = w.set_size(LogicalSize::new(width, 168.0));
    let _ = w.set_position(LogicalPosition::new((mw - width) / 2.0, mh - 168.0 - 64.0));
}

// ------------------------------------------------------------------ commands

#[tauri::command]
pub fn open_window(app: State<App>, origin: String, window: String) -> Result<(), String> {
    let hub = hub(&app).ok_or("Turn OpenHop on first.")?;
    hub.open(&origin, parse(&window)?, None);
    Ok(())
}

#[tauri::command]
pub fn close_window(app: State<App>, stream: String) {
    if let (Some(hub), Ok(s)) = (hub(&app), parse(&stream)) {
        hub.close(s);
    }
}

#[tauri::command]
pub fn win_input(app: State<App>, stream: String, ev: WinEvent) {
    if let (Some(hub), Ok(s)) = (hub(&app), parse(&stream)) {
        hub.input(s, ev);
    }
}

/// The next picture of a live window, as bytes:
/// kind (0 picture, 1 paused, 2 closed, 3 nothing new), seq u64, w u32, h u32,
/// text length u16, text (title, or pause reason), JPEG.
#[tauri::command]
pub async fn win_frame(app: State<'_, App>, stream: String, after: String) -> Result<tauri::ipc::Response, String> {
    let hub = hub(&app).ok_or("not running")?;
    let (stream, after) = (parse(&stream)?, parse(&after).unwrap_or(0));
    let r = tauri::async_runtime::spawn_blocking(move || hub.frame(stream, after, Duration::from_secs(2))).await.map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    let mut head = |kind: u8, seq: u64, w: u32, h: u32, text: &str| {
        out.push(kind);
        out.extend_from_slice(&seq.to_le_bytes());
        out.extend_from_slice(&w.to_le_bytes());
        out.extend_from_slice(&h.to_le_bytes());
        let t = text.as_bytes();
        let t = &t[..t.len().min(1000)];
        out.extend_from_slice(&(t.len() as u16).to_le_bytes());
        out.extend_from_slice(t);
    };
    match r {
        FrameWait::Frame(f) => {
            head(0, f.seq, f.w, f.h, &f.title);
            out.extend_from_slice(&f.jpeg);
        }
        FrameWait::Paused(reason) => head(1, 0, 0, 0, &reason),
        FrameWait::Closed => head(2, 0, 0, 0, ""),
        FrameWait::Timeout => head(3, 0, 0, 0, ""),
    }
    Ok(tauri::ipc::Response::new(out))
}

#[tauri::command]
pub fn fx_done(handle: AppHandle) {
    if let Some(w) = handle.get_webview_window("fx") {
        let _ = w.hide();
    }
}

#[tauri::command]
pub fn dock_toggle(handle: AppHandle) {
    toggle_dock(&handle);
}

#[tauri::command]
pub fn dock_hide(handle: AppHandle) {
    if let Some(w) = handle.get_webview_window("dock") {
        let _ = w.hide();
    }
}

#[tauri::command]
pub fn play_sound(kind: String, volume: u32) {
    openhop_core::extras::sound::play(&kind, volume);
}
