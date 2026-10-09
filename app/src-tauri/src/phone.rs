//! Phones in the island: a QR code that opens a page on the phone for
//! sending files both ways (see `openhop_core::extras::phone`).

use crate::island::{push, Activity};
use crate::App;
use openhop_core::engine::{Note, NoteAction};
use openhop_core::extras::phone::{PhoneEvent, PhoneServer, PhoneState};
use openhop_core::protocol::ClipData;
use parking_lot::Mutex;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use tauri::{AppHandle, Manager};

#[derive(Default)]
pub struct Phone(Mutex<Option<PhoneServer>>);

static NOTE_ID: AtomicU64 = AtomicU64::new(u64::MAX / 2);

fn on_event(handle: &AppHandle, ev: PhoneEvent) {
    match ev {
        PhoneEvent::Received { name, path, .. } => {
            let p = path.to_string_lossy().into_owned();
            handle.state::<App>().toasts.lock().push_back(Note {
                id: NOTE_ID.fetch_add(1, Ordering::Relaxed),
                title: format!("Received {name}"),
                body: "From your phone, in Downloads › OpenHop.".into(),
                actions: vec![
                    NoteAction { label: "Open".into(), kind: "open_path".into(), target: p.clone() },
                    NoteAction { label: "Show in Folder".into(), kind: "reveal".into(), target: p },
                ],
            });
        }
        PhoneEvent::Text(t) => {
            let app = handle.state::<App>();
            let hub = app.engine.lock().as_ref().map(|e| e.hub());
            let set = hub.as_ref().map(|h| h.set_clipboard(ClipData::Text(t.clone()))).unwrap_or(false);
            if let Some(h) = hub {
                h.clip_seen("Phone", &ClipData::Text(t.clone()));
            }
            let short: String = t.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(70).collect();
            push(
                handle,
                Activity { title: if set { "Copied from your phone".into() } else { "Text from your phone".into() }, body: short, icon: "copied".into() },
            );
        }
        PhoneEvent::Sent { name } => push(handle, Activity { title: "On your phone".into(), body: name, icon: "phone".into() }),
        PhoneEvent::Connected { device } => push(
            handle,
            Activity { title: format!("{device} connected"), body: "Send from the page, or drop files on Phone here.".into(), icon: "phone".into() },
        ),
    }
}

fn server(handle: &AppHandle) -> Result<PhoneServer, String> {
    let phone = handle.state::<Phone>();
    let mut g = phone.0.lock();
    if let Some(s) = g.as_ref() {
        return Ok(s.clone());
    }
    let app = handle.state::<App>();
    let (dir, name, cfg_dir) = {
        let c = app.config.lock();
        let dir = c.download_dir.as_ref().map(PathBuf::from).unwrap_or_else(openhop_core::files::download_root);
        let cfg_dir = app.path.parent().map(PathBuf::from).unwrap_or_default();
        (dir, c.name.clone(), cfg_dir)
    };
    let h = handle.clone();
    let s = PhoneServer::start(&cfg_dir, dir, &name, move |ev| on_event(&h, ev)).map_err(|e| format!("Couldn't start the phone page: {e}"))?;
    *g = Some(s.clone());
    Ok(s)
}

/// The QR code and whether a phone is there. Starts the page the first time.
#[tauri::command]
pub fn phone_state(handle: AppHandle) -> Result<PhoneState, String> {
    Ok(server(&handle)?.state())
}

/// Put files on the phone page.
#[tauri::command]
pub fn phone_send(handle: AppHandle, paths: Vec<String>) -> Result<usize, String> {
    let paths: Vec<PathBuf> = paths.into_iter().map(PathBuf::from).collect();
    Ok(server(&handle)?.offer(&paths))
}
