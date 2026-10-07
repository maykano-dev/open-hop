#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use openhop_core::engine::Note;
use openhop_core::layout::Layout;
use openhop_core::protocol::Os;
use openhop_core::{Config, Engine, Status};
use parking_lot::Mutex;
use serde::Serialize;
use std::collections::VecDeque;
use std::path::PathBuf;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, LogicalPosition, LogicalSize, Manager, State, WebviewUrl, WebviewWindowBuilder, WindowEvent};

const TOAST_W: f64 = 372.0;

struct App {
    path: PathBuf,
    config: Mutex<Config>,
    engine: Mutex<Option<Engine>>,
    last_error: Mutex<Option<String>>,
    toasts: Mutex<VecDeque<Note>>,
}

#[derive(Serialize)]
struct Snapshot {
    config: Config,
    status: Option<Status>,
    os: Os,
    error: Option<String>,
    permission: Option<String>,
    wayland: bool,
}

fn wayland() -> bool {
    #[cfg(target_os = "linux")]
    {
        return openhop_core::platform::linux_is_wayland();
    }
    #[allow(unreachable_code)]
    false
}

#[tauri::command]
fn snapshot(app: State<App>) -> Snapshot {
    let engine = app.engine.lock();
    let status = engine.as_ref().map(|e| e.status());
    let mut config = app.config.lock().clone();
    // The running server keeps the live layout (new computers get auto-placed).
    if let Some(s) = &status {
        if s.role == openhop_core::Role::Server {
            config.layout = s.layout.clone();
        }
    }
    Snapshot {
        config,
        status,
        os: Os::current(),
        error: app.last_error.lock().clone(),
        permission: openhop_core::platform::check_permissions(false),
        wayland: wayland(),
    }
}

fn start_engine(app: &App) -> Result<(), String> {
    let mut engine = app.engine.lock();
    if let Some(e) = engine.take() {
        e.stop();
    }
    let cfg = app.config.lock().clone();
    match Engine::start(cfg, Some(app.path.clone())) {
        Ok(e) => {
            *engine = Some(e);
            *app.last_error.lock() = None;
            Ok(())
        }
        Err(e) => {
            let msg = format!("{e:#}");
            *app.last_error.lock() = Some(msg.clone());
            Err(msg)
        }
    }
}

#[tauri::command]
fn save_config(app: State<App>, config: Config, restart: bool) -> Result<(), String> {
    config.save(&app.path).map_err(|e| e.to_string())?;
    let running = app.engine.lock().is_some();
    *app.config.lock() = config;
    if restart && running {
        start_engine(&app)?;
    }
    Ok(())
}

#[tauri::command]
fn start(app: State<App>) -> Result<(), String> {
    start_engine(&app)
}

#[tauri::command]
fn stop(app: State<App>) {
    if let Some(e) = app.engine.lock().take() {
        e.stop();
    }
}

#[tauri::command]
fn set_layout(app: State<App>, layout: Layout) -> Result<(), String> {
    {
        let mut cfg = app.config.lock();
        cfg.layout = layout.clone();
        cfg.save(&app.path).map_err(|e| e.to_string())?;
    }
    if let Some(e) = app.engine.lock().as_ref() {
        e.set_layout(layout);
    }
    Ok(())
}

#[tauri::command]
fn request_permission() -> Option<String> {
    openhop_core::platform::check_permissions(true)
}

/// Toast window: take the notifications waiting to be shown.
#[tauri::command]
fn toast_queue(app: State<App>) -> Vec<Note> {
    let v: Vec<Note> = app.toasts.lock().drain(..).collect();
    if !v.is_empty() {
        log::debug!("showing {} notification(s)", v.len());
    }
    v
}

/// Toast window: fit the window to its content (0 = hide).
#[tauri::command]
fn toast_layout(handle: AppHandle, height: f64) {
    let Some(w) = handle.get_webview_window("toast") else {
        log::warn!("toast window missing");
        return;
    };
    log::debug!("toast height {height}");
    if height <= 0.0 {
        let _ = w.hide();
        return;
    }
    let _ = w.set_size(LogicalSize::new(TOAST_W, height));
    if let Ok(Some(m)) = w.primary_monitor() {
        let scale = m.scale_factor();
        let size = m.size().to_logical::<f64>(scale);
        let pos = m.position().to_logical::<f64>(scale);
        // Windows shows notifications bottom-right; macOS and Linux top-right.
        let y = if cfg!(windows) { pos.y + size.height - height - 60.0 } else { pos.y + 36.0 };
        let _ = w.set_position(LogicalPosition::new(pos.x + size.width - TOAST_W - 12.0, y));
    }
    if !w.is_visible().unwrap_or(false) {
        let _ = w.show();
    }
}

#[tauri::command]
fn note_action(kind: String, target: String) -> Result<(), String> {
    openhop_core::engine::run_action(&kind, &target).map_err(|e| e.to_string())
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let path = Config::default_path();
    let config = Config::load(&path).unwrap_or_else(|e| {
        log::warn!("{e:#}; using defaults");
        Config::default()
    });
    let autostart = !config.passphrase.trim().is_empty();
    let state = App {
        path,
        config: Mutex::new(config),
        engine: Mutex::new(None),
        last_error: Mutex::new(None),
        toasts: Mutex::new(VecDeque::new()),
    };

    tauri::Builder::default()
        .manage(state)
        .invoke_handler(tauri::generate_handler![
            snapshot,
            save_config,
            start,
            stop,
            set_layout,
            request_permission,
            toast_queue,
            toast_layout,
            note_action
        ])
        .setup(move |app| {
            // Keep sharing in the background: closing the window hides it to the tray.
            let show = MenuItem::with_id(app, "show", "Open OpenHop", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show, &quit])?;
            TrayIconBuilder::with_id("tray")
                .icon(app.default_window_icon().cloned().expect("icon"))
                .tooltip("OpenHop")
                .menu(&menu)
                .on_menu_event(|app, ev| match ev.id().as_ref() {
                    "show" => {
                        if let Some(w) = app.get_webview_window("main") {
                            let _ = w.show();
                            let _ = w.set_focus();
                        }
                    }
                    "quit" => {
                        if let Some(e) = app.state::<App>().engine.lock().take() {
                            e.stop();
                        }
                        app.exit(0);
                    }
                    _ => {}
                })
                .build(app)?;
            // Small always-on-top window for notifications (received files, copied links).
            WebviewWindowBuilder::new(app, "toast", WebviewUrl::App("toast.html".into()))
                .title("OpenHop")
                .inner_size(TOAST_W, 100.0)
                .decorations(false)
                .transparent(true)
                .shadow(false)
                .always_on_top(true)
                .skip_taskbar(true)
                .resizable(false)
                .focused(false)
                .visible(false)
                .build()?;
            // Move notifications from the engine to the toast window.
            let handle = app.handle().clone();
            std::thread::spawn(move || loop {
                std::thread::sleep(std::time::Duration::from_millis(250));
                let state = handle.state::<App>();
                let notes = state.engine.lock().as_ref().map(|e| e.take_notes()).unwrap_or_default();
                if !notes.is_empty() {
                    let mut q = state.toasts.lock();
                    q.extend(notes);
                    while q.len() > 5 {
                        q.pop_front();
                    }
                }
            });
            if autostart {
                let _ = start_engine(&app.state::<App>());
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == "toast" {
                    let _ = window.hide();
                    api.prevent_close();
                    return;
                }
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running OpenHop");
}
