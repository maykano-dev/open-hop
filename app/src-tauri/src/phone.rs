//! Phones in the island: the QR code that pairs a phone, and what happens
//! when files and text come and go (see `openhop_core::extras::phone`).

use crate::island::{poke, push, Activity};
use crate::App;
use openhop_core::engine::{Note, NoteAction};
use openhop_core::extras::phone::{Phone as Link, PhoneEvent, PhoneHost, PhoneState};
use openhop_core::extras::Hub;
use openhop_core::files::TransferInfo;
use parking_lot::Mutex;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;
use tauri::{AppHandle, Manager};

#[derive(Default)]
pub struct Phone(Mutex<Option<Arc<Link>>>);

static NOTE_ID: AtomicU64 = AtomicU64::new(u64::MAX / 2);

struct Host {
    handle: AppHandle,
    last_poke: Mutex<Instant>,
}

impl Host {
    fn with_engine<T>(&self, f: impl FnOnce(&openhop_core::Engine) -> T) -> Option<T> {
        self.handle.state::<App>().engine.lock().as_ref().map(f)
    }
}

impl PhoneHost for Host {
    fn hub(&self) -> Option<Arc<Hub>> {
        self.with_engine(|e| e.hub())
    }
    fn send_files(&self, to: &str, paths: Vec<String>) {
        self.with_engine(|e| e.send_files(to.to_string(), paths));
    }
    fn shelf_add(&self, paths: Vec<String>) {
        self.with_engine(|e| e.shelf_add(paths));
    }
    fn shelf_take(&self, origin: &str, id: u64) {
        self.with_engine(|e| e.shelf_take(origin.to_string(), id));
    }
    fn local_files(&self, offer: u64) -> Option<Vec<(PathBuf, String)>> {
        self.with_engine(|e| e.local_files(offer)).flatten()
    }
    fn transfers(&self) -> Vec<TransferInfo> {
        self.with_engine(|e| e.status().transfers).unwrap_or_default()
    }
    fn name(&self) -> String {
        self.handle.state::<App>().config.lock().name.clone()
    }
    fn event(&self, ev: PhoneEvent) {
        let handle = &self.handle;
        match ev {
            PhoneEvent::Received { name, path, device, to, .. } => {
                let p = path.to_string_lossy().into_owned();
                let me = self.name();
                let onward = match to.as_str() {
                    "" | "here" => String::new(),
                    "shelf" => " Now on the shelf too.".into(),
                    "*" => " Sending it to all your computers.".into(),
                    t if t == me => String::new(),
                    t => format!(" Sending it to {t}."),
                };
                handle.state::<App>().toasts.lock().push_back(Note {
                    id: NOTE_ID.fetch_add(1, Ordering::Relaxed),
                    title: format!("Received {name}"),
                    body: format!("From your {device}, in Downloads › OpenHop.{onward}"),
                    actions: vec![
                        NoteAction { label: "Open".into(), kind: "open_path".into(), target: p.clone() },
                        NoteAction { label: "Show in Folder".into(), kind: "reveal".into(), target: p },
                    ],
                });
                poke(handle);
            }
            PhoneEvent::Text { text, device } => {
                let short: String = text.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(70).collect();
                push(handle, Activity { title: format!("Copied from your {device}"), body: short, icon: "copied".into(), short: true });
            }
            // The island's live pill already showed it going.
            PhoneEvent::Sent { .. } => poke(handle),
            PhoneEvent::Connected { device, how } => push(
                handle,
                Activity {
                    title: format!("{device} connected"),
                    body: if how == "direct" { "Encrypted end to end, on any network.".into() } else { "On this Wi‑Fi.".into() },
                    icon: "phone".into(),
                    short: true,
                },
            ),
            PhoneEvent::Disconnected { .. } => poke(handle),
            PhoneEvent::Progress => {
                // A few times a second at most.
                let mut last = self.last_poke.lock();
                if last.elapsed().as_millis() > 150 {
                    *last = Instant::now();
                    poke(handle);
                }
            }
        }
    }
}

fn config_dir(app: &App) -> PathBuf {
    app.path.parent().map(PathBuf::from).unwrap_or_default()
}

/// The phone link, started the first time it's needed.
pub fn link(handle: &AppHandle) -> Arc<Link> {
    let phone = handle.state::<Phone>();
    let mut g = phone.0.lock();
    if let Some(p) = g.as_ref() {
        return p.clone();
    }
    let app = handle.state::<App>();
    let (dir, relays, app_url) = {
        let c = app.config.lock();
        let dir = c.download_dir.as_ref().map(PathBuf::from).unwrap_or_else(openhop_core::files::download_root);
        let relays = if c.phone_relays.is_empty() {
            openhop_core::extras::phone::signal::DEFAULT_RELAYS.iter().map(|s| s.to_string()).collect()
        } else {
            c.phone_relays.clone()
        };
        (dir, relays, c.phone_app_url.clone())
    };
    let host = Arc::new(Host { handle: handle.clone(), last_poke: Mutex::new(Instant::now()) });
    let p = Link::start(host, &config_dir(&app), dir, relays, app_url);
    *g = Some(p.clone());
    p
}

/// At launch: phones that were paired before can reach this computer.
pub fn start_if_paired(handle: &AppHandle) {
    let dir = config_dir(&handle.state::<App>());
    if dir.join("phone.json").is_file() {
        link(handle);
    }
}

/// Files moving to and from phones (for the island's live pill).
pub fn transfers(handle: &AppHandle) -> Vec<TransferInfo> {
    handle.state::<Phone>().0.lock().as_ref().map(|p| p.transfers()).unwrap_or_default()
}

/// The QR code and whether a phone is there.
#[tauri::command]
pub fn phone_state(handle: AppHandle) -> PhoneState {
    link(&handle).state()
}

/// Files for the phone.
#[tauri::command]
pub fn phone_send(handle: AppHandle, paths: Vec<String>) -> usize {
    let paths: Vec<PathBuf> = paths.into_iter().map(PathBuf::from).collect();
    link(&handle).offer(&paths)
}

/// A new code: phones paired before can't connect anymore.
#[tauri::command]
pub fn phone_renew(handle: AppHandle) -> PhoneState {
    let l = link(&handle);
    l.renew();
    l.state()
}
