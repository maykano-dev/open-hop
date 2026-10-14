//! AirDrop from Linux (through omdrop, see openhop_core::extras::airdrop):
//! "AirDrop" in the file manager's Send with OpenHop menu, and "Let iPhones
//! Send Here" in Settings.

use crate::island::{poke, push, Activity};
use crate::App;
use openhop_core::engine::{Note, NoteAction};
use openhop_core::extras::airdrop;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use tauri::{AppHandle, Manager};

static NOTE_ID: AtomicU64 = AtomicU64::new((u64::MAX / 4) * 3);

/// Files waiting for the person to pick which device gets them.
#[derive(Default)]
pub struct Waiting(Mutex<HashMap<u64, Vec<PathBuf>>>);

fn note(handle: &AppHandle, title: String, body: String, actions: Vec<NoteAction>) {
    handle.state::<App>().toasts.lock().push_back(Note { id: NOTE_ID.fetch_add(1, Ordering::Relaxed), title, body, actions });
    poke(handle);
}

fn what(files: &[PathBuf]) -> String {
    match files {
        [one] => crate::phone::short(&one.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()),
        many => format!("{} items", many.len()),
    }
}

/// Send `files` to an Apple device nearby: straight away when only one is
/// around, otherwise the island asks which.
pub fn send(handle: &AppHandle, files: Vec<PathBuf>) {
    let handle = handle.clone();
    std::thread::spawn(move || {
        if !airdrop::available() {
            note(
                &handle,
                "AirDrop isn't set up on this computer".into(),
                airdrop::needs().into(),
                vec![NoteAction { label: "Learn How".into(), kind: "open_url".into(), target: airdrop::INFO_URL.into() }],
            );
            return;
        }
        push(
            &handle,
            Activity {
                title: "Looking for iPhones and Macs…".into(),
                body: "Unlock it and keep AirDrop open on it.".into(),
                icon: "files".into(),
                short: true,
            },
        );
        let peers = match airdrop::peers() {
            Ok(p) => p,
            Err(e) => {
                note(&handle, "AirDrop didn't work".into(), e, vec![]);
                return;
            }
        };
        let named: Vec<String> = {
            let mut v: Vec<String> = peers.into_iter().filter_map(|p| p.name).collect();
            v.dedup();
            v
        };
        match named.as_slice() {
            [] => note(
                &handle,
                "No iPhone or Mac nearby".into(),
                "On it: unlock, open Control Center, and set AirDrop to Everyone for 10 Minutes. Then send again.".into(),
                vec![],
            ),
            [one] => deliver(&handle, files, one.clone()),
            many => {
                let id = NOTE_ID.fetch_add(1, Ordering::Relaxed);
                handle.state::<Waiting>().0.lock().insert(id, files.clone());
                let actions = many.iter().take(4).map(|n| NoteAction { label: n.clone(), kind: "airdrop_to".into(), target: format!("{id}\n{n}") }).collect();
                handle.state::<App>().toasts.lock().push_back(Note { id, title: format!("AirDrop {} to…", what(&files)), body: String::new(), actions });
                poke(&handle);
            }
        }
    });
}

/// The person picked a device for files waiting.
pub fn picked(handle: &AppHandle, target: &str) {
    let Some((id, name)) = target.split_once('\n') else { return };
    let Some(files) = id.parse().ok().and_then(|id: u64| handle.state::<Waiting>().0.lock().remove(&id)) else { return };
    let (handle, name) = (handle.clone(), name.to_string());
    std::thread::spawn(move || deliver(&handle, files, name));
}

fn deliver(handle: &AppHandle, files: Vec<PathBuf>, to: String) {
    push(handle, Activity { title: format!("AirDropping to {to}"), body: format!("{} · accept it there", what(&files)), icon: "files".into(), short: false });
    match airdrop::send(&files, &to) {
        Ok(()) => push(handle, Activity { title: format!("Sent to {to}"), body: what(&files), icon: "copied".into(), short: true }),
        Err(e) => note(handle, format!("{to} didn't take it"), e, vec![]),
    }
}

#[derive(serde::Serialize)]
pub struct AirDropState {
    /// This kind of computer could (Linux, Windows).
    possible: bool,
    /// Windows (set up by OpenHop itself).
    windows: bool,
    installed: bool,
    /// Being visible turns this computer's Wi‑Fi off (so not always on).
    costs_wifi: bool,
    status: Option<airdrop::Status>,
    url: &'static str,
}

#[tauri::command]
pub async fn airdrop_state() -> AirDropState {
    tauri::async_runtime::spawn_blocking(|| {
        let installed = airdrop::available();
        AirDropState {
            possible: airdrop::possible(),
            windows: cfg!(windows),
            installed,
            costs_wifi: installed && airdrop::costs_wifi(),
            status: if installed { airdrop::status() } else { None },
            url: airdrop::INFO_URL,
        }
    })
    .await
    .unwrap_or(AirDropState { possible: false, windows: false, installed: false, costs_wifi: false, status: None, url: airdrop::INFO_URL })
}

/// Visible to Apple devices nearby for a while (0: stop).
#[tauri::command]
pub async fn airdrop_receive(minutes: u32) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || if minutes == 0 { airdrop::stop_receiving() } else { airdrop::receive(minutes) })
        .await
        .map_err(|e| e.to_string())?
}

/// Windows: start the guided set-up (a window that asks as administrator).
#[tauri::command]
pub fn airdrop_setup() -> Result<(), String> {
    airdrop::setup()
}

/// Cheap: is AirDrop set up here? (For the island's drop targets.)
#[tauri::command]
pub fn airdrop_ready() -> bool {
    airdrop::available()
}

/// Files dropped on AirDrop in the island.
#[tauri::command]
pub fn airdrop_send(handle: AppHandle, paths: Vec<String>) {
    send(&handle, paths.into_iter().map(PathBuf::from).collect());
}

/// AirDrop always on: while it's set up and the setting is on, keep this
/// computer visible to iPhones and Macs nearby (checked every minute: after
/// a restart, after Linux under WSL stopped, after the adapter came back).
pub fn keep_on(handle: AppHandle) {
    std::thread::spawn(move || {
        let mut was = false;
        let mut failed = 0u32;
        loop {
            let want = handle.state::<App>().config.lock().airdrop_always && !airdrop::costs_wifi();
            if airdrop::available() {
                if want {
                    let on = airdrop::status().map(|s| s.receiving).unwrap_or(false);
                    if !on {
                        match airdrop::receive_always() {
                            Ok(()) => {
                                failed = 0;
                                if !was {
                                    log::info!("AirDrop: visible to iPhones and Macs nearby");
                                }
                            }
                            Err(e) => {
                                failed += 1;
                                if failed == 1 || failed % 30 == 0 {
                                    log::info!("AirDrop: couldn't switch on: {e}");
                                }
                            }
                        }
                    }
                    was = true;
                } else if was {
                    let _ = airdrop::stop_receiving();
                    was = false;
                }
            }
            // Sooner after a change of the setting.
            for _ in 0..60 {
                std::thread::sleep(std::time::Duration::from_secs(1));
                if handle.state::<App>().config.lock().airdrop_always != want && !airdrop::costs_wifi() {
                    break;
                }
            }
        }
    });
}
