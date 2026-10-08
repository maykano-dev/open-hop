#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod live;
mod updater;

use openhop_core::engine::{Note, NoteAction};
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
    version: &'static str,
    update: updater::UpdateState,
}

/// Close other OpenHop app processes. The single-instance check only knows
/// about this version, so an older copy left running in the tray (e.g. after
/// installing an update over it) would otherwise keep the network port.
fn close_older_copies() {
    let me = std::process::id();
    let name = if cfg!(windows) { "openhop-app.exe" } else { "openhop-app" };
    let mut others: Vec<u32> = Vec::new();
    #[cfg(target_os = "linux")]
    if let Ok(dir) = std::fs::read_dir("/proc") {
        let uid = std::fs::metadata("/proc/self").map(|m| std::os::unix::fs::MetadataExt::uid(&m)).ok();
        for e in dir.flatten() {
            let Some(pid) = e.file_name().to_str().and_then(|s| s.parse::<u32>().ok()) else { continue };
            let comm = std::fs::read_to_string(e.path().join("comm")).unwrap_or_default();
            let same_user = std::fs::metadata(e.path()).map(|m| Some(std::os::unix::fs::MetadataExt::uid(&m)) == uid).unwrap_or(false);
            if pid != me && comm.trim() == name && same_user {
                others.push(pid);
            }
        }
    }
    #[cfg(target_os = "macos")]
    if let Ok(out) = std::process::Command::new("pgrep").args(["-x", "-U", &std::env::var("USER").unwrap_or_default(), name]).output() {
        others.extend(String::from_utf8_lossy(&out.stdout).lines().filter_map(|l| l.trim().parse::<u32>().ok()).filter(|&p| p != me));
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        if let Ok(out) = std::process::Command::new("tasklist")
            .args(["/FI", &format!("IMAGENAME eq {name}"), "/FO", "CSV", "/NH"])
            .creation_flags(0x08000000)
            .output()
        {
            for line in String::from_utf8_lossy(&out.stdout).lines() {
                if let Some(pid) = line.split(',').nth(1).and_then(|p| p.trim_matches('"').parse::<u32>().ok()) {
                    if pid != me {
                        others.push(pid);
                    }
                }
            }
        }
    }
    for pid in &others {
        log::info!("closing an older copy of OpenHop (pid {pid})");
        #[cfg(unix)]
        let _ = std::process::Command::new("kill").arg(pid.to_string()).status();
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            let _ = std::process::Command::new("taskkill").args(["/PID", &pid.to_string(), "/F"]).creation_flags(0x08000000).status();
        }
    }
    if !others.is_empty() {
        // Give them a moment to release the port.
        std::thread::sleep(std::time::Duration::from_millis(800));
    }
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
        version: env!("CARGO_PKG_VERSION"),
        update: updater::state(),
    }
}

fn start_engine(app: &App) -> Result<(), String> {
    let mut engine = app.engine.lock();
    if let Some(e) = engine.take() {
        e.stop();
    }
    let cfg = merge_engine_fields(app.config.lock().clone(), &app.path);
    *app.config.lock() = cfg.clone();
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

/// Fields the engine writes while running (pairing keys, arrangement, Wake-on-LAN
/// details). Take them from disk so saving the settings form never undoes them.
fn merge_engine_fields(mut config: Config, path: &PathBuf) -> Config {
    if let Ok(disk) = Config::load(path) {
        config.device_id = disk.device_id;
        config.trusted = disk.trusted;
        config.server_device = disk.server_device;
        config.layout = disk.layout;
        config.macs = disk.macs;
        config.last_ips = disk.last_ips;
    }
    config
}

#[tauri::command]
fn save_config(app: State<App>, config: Config, restart: bool) -> Result<(), String> {
    let config = merge_engine_fields(config, &app.path);
    config.save(&app.path).map_err(|e| e.to_string())?;
    let running = app.engine.lock().is_some();
    *app.config.lock() = config;
    if restart && running {
        start_engine(&app)?;
    }
    Ok(())
}

/// Settings that apply straight away (no restart): sync, sounds, live
/// windows, look, open at login. `prefs` holds just the changed fields.
#[tauri::command]
fn update_prefs(handle: AppHandle, app: State<App>, prefs: serde_json::Value) -> Result<(), String> {
    let cfg = {
        let mut cfg = app.config.lock();
        let mut v = serde_json::to_value(&*cfg).map_err(|e| e.to_string())?;
        if let (Some(obj), Some(p)) = (v.as_object_mut(), prefs.as_object()) {
            for k in ["theme_sync", "dnd_sync", "sound", "sound_volume", "battery_saver", "stream_quality", "window_drag", "ui_appearance", "ui_accent", "open_at_login"] {
                if let Some(x) = p.get(k) {
                    obj.insert(k.into(), x.clone());
                }
            }
        }
        let new: Config = serde_json::from_value(v).map_err(|e| e.to_string())?;
        let merged = merge_engine_fields(new, &app.path);
        merged.save(&app.path).map_err(|e| e.to_string())?;
        *cfg = merged.clone();
        merged
    };
    if let Some(e) = app.engine.lock().as_ref() {
        e.hub().set_settings(openhop_core::extras::Settings::from_config(&cfg));
    }
    apply_login(&handle, cfg.open_at_login);
    Ok(())
}

fn apply_login(handle: &AppHandle, on: bool) {
    use tauri_plugin_autostart::ManagerExt;
    let al = handle.autolaunch();
    let now = al.is_enabled().unwrap_or(false);
    if on && !now {
        if let Err(e) = al.enable() {
            log::warn!("couldn't turn on open at login: {e}");
        }
    } else if !on && now {
        let _ = al.disable();
    }
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
        let mut merged = merge_engine_fields(cfg.clone(), &app.path);
        merged.layout = layout.clone();
        merged.save(&app.path).map_err(|e| e.to_string())?;
        *cfg = merged;
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
fn note_action(handle: AppHandle, kind: String, target: String) -> Result<(), String> {
    if kind == "install_update" {
        return update_install(handle);
    }
    openhop_core::engine::run_action(&kind, &target).map_err(|e| e.to_string())
}

/// Client: pair with a server using the code it shows.
#[tauri::command]
fn pair(app: State<App>, device: String, code: String) -> Result<(), String> {
    if app.engine.lock().is_none() {
        start_engine(&app)?;
    }
    match app.engine.lock().as_ref() {
        Some(e) => {
            e.pair(device, code);
            Ok(())
        }
        None => Err("OpenHop isn't running".into()),
    }
}

/// Client: pair with a server by its address (when it doesn't show up nearby).
#[tauri::command]
fn pair_addr(app: State<App>, addr: String, code: String) -> Result<(), String> {
    if app.engine.lock().is_none() {
        start_engine(&app)?;
    }
    match app.engine.lock().as_ref() {
        Some(e) => {
            e.pair_addr(addr, code);
            Ok(())
        }
        None => Err("OpenHop isn't running".into()),
    }
}

#[tauri::command]
fn forget(app: State<App>, device: String) -> Result<(), String> {
    match app.engine.lock().as_ref() {
        Some(e) => e.forget(device.clone()),
        None => {
            let mut cfg = merge_engine_fields(app.config.lock().clone(), &app.path);
            cfg.trusted.remove(&device);
            if cfg.server_device.as_deref() == Some(device.as_str()) {
                cfg.server_device = None;
            }
            cfg.save(&app.path).map_err(|e| e.to_string())?;
            *app.config.lock() = cfg;
        }
    }
    Ok(())
}

#[tauri::command]
fn update_state() -> updater::UpdateState {
    updater::state()
}

#[tauri::command]
fn update_check() {
    std::thread::spawn(updater::check);
}

#[tauri::command]
fn update_install(handle: AppHandle) -> Result<(), String> {
    std::thread::spawn(move || {
        if updater::install().is_ok() {
            // The installer takes over; stop sharing and quit so files can be replaced.
            if let Some(e) = handle.state::<App>().engine.lock().take() {
                e.stop();
            }
            std::thread::sleep(std::time::Duration::from_millis(300));
            handle.exit(0);
        }
    });
    Ok(())
}

/// `openhop-app --clipboard`: report what's on the clipboard (troubleshooting).
fn clipboard_report() {
    let report = openhop_core::clipboard::inspect();
    print!("{report}");
    let dir = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).map(std::path::PathBuf::from).unwrap_or_else(std::env::temp_dir);
    let dir = if dir.join("Desktop").is_dir() { dir.join("Desktop") } else { dir };
    let file = dir.join("OpenHop clipboard report.txt");
    if std::fs::write(&file, &report).is_ok() {
        println!("Saved to {}", file.display());
        #[cfg(windows)]
        let _ = std::process::Command::new("notepad").arg(&file).spawn();
        #[cfg(target_os = "macos")]
        let _ = std::process::Command::new("open").arg("-t").arg(&file).spawn();
    }
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    if std::env::args().any(|a| a == "--clipboard") {
        clipboard_report();
        return;
    }
    let path = Config::default_path();
    let config = Config::load(&path).unwrap_or_else(|e| {
        log::warn!("{e:#}; using defaults");
        Config::default()
    });
    // Always start: the host shows its pairing code and others appear right away.
    let autostart = true;
    let state = App {
        path,
        config: Mutex::new(config),
        engine: Mutex::new(None),
        last_error: Mutex::new(None),
        toasts: Mutex::new(VecDeque::new()),
    };

    tauri::Builder::default()
        // Only one copy may run (it owns the network port); opening OpenHop
        // again just brings the existing window forward.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.show();
                let _ = w.unminimize();
                let _ = w.set_focus();
            }
        }))
        .plugin(tauri_plugin_autostart::init(tauri_plugin_autostart::MacosLauncher::LaunchAgent, Some(vec!["--hidden"])))
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, _shortcut, event| {
                    log::debug!("shortcut {:?}", event.state());
                    if event.state() == tauri_plugin_global_shortcut::ShortcutState::Pressed {
                        live::toggle_dock(app);
                    }
                })
                .build(),
        )
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
            note_action,
            pair,
            pair_addr,
            forget,
            update_state,
            update_check,
            update_install,
            update_prefs,
            live::open_window,
            live::close_window,
            live::win_input,
            live::win_frame,
            live::fx_done,
            live::dock_toggle,
            live::dock_hide,
            live::play_sound,
            live::viewer_fit,
            live::viewer_title
        ])
        .setup(move |app| {
            // Keep sharing in the background: closing the window hides it to the tray.
            let show = MenuItem::with_id(app, "show", "Open OpenHop", true, None::<&str>)?;
            let dock = MenuItem::with_id(app, "dock", "Windows on All Computers (Ctrl+Alt+Space)", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show, &dock, &quit])?;
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
                    "dock" => live::toggle_dock(app),
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
            live::create_fx(app)?;
            {
                use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut};
                let sc = Shortcut::new(Some(Modifiers::CONTROL | Modifiers::ALT), Code::Space);
                if let Err(e) = app.global_shortcut().register(sc) {
                    log::info!("window dock shortcut unavailable: {e}");
                }
            }
            // Live windows and arrival animations.
            let handle = app.handle().clone();
            std::thread::spawn(move || loop {
                std::thread::sleep(std::time::Duration::from_millis(80));
                let h = handle.clone();
                let _ = handle.run_on_main_thread(move || live::pump(&h));
            });
            // Started at login: stay in the tray.
            if std::env::args().any(|a| a == "--hidden") {
                if let Some(w) = app.get_webview_window("main") {
                    let _ = w.hide();
                }
            }
            apply_login(app.handle(), app.state::<App>().config.lock().open_at_login);
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
            // Check for a new version shortly after launch, then once a day.
            let handle = app.handle().clone();
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_secs(20));
                loop {
                    let st = updater::check();
                    if st.phase == "available" {
                        let latest = st.latest.clone().unwrap_or_default();
                        handle.state::<App>().toasts.lock().push_back(Note {
                            id: u64::MAX - 1,
                            title: format!("OpenHop {latest} is available"),
                            body: format!("You have {}. Update every computer to the same version.", st.current),
                            actions: vec![NoteAction { label: "Update Now".into(), kind: "install_update".into(), target: String::new() }],
                        });
                    }
                    std::thread::sleep(std::time::Duration::from_secs(24 * 3600));
                }
            });
            // We're the one running copy now (single-instance passed); clear out older versions.
            close_older_copies();
            if autostart {
                let _ = start_engine(&app.state::<App>());
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            let label = window.label().to_string();
            if let Some(stream) = label.strip_prefix("view-") {
                if let WindowEvent::Destroyed = event {
                    if let Ok(s) = stream.parse::<u64>() {
                        if let Some(e) = window.state::<App>().engine.lock().as_ref() {
                            e.hub().close(s);
                        }
                    }
                }
                return;
            }
            if label == "dock" {
                if let WindowEvent::Focused(false) = event {
                    let _ = window.hide();
                }
                if let WindowEvent::CloseRequested { api, .. } = event {
                    api.prevent_close();
                    let _ = window.hide();
                }
                return;
            }
            if label == "fx" {
                if let WindowEvent::CloseRequested { api, .. } = event {
                    api.prevent_close();
                }
                return;
            }
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
