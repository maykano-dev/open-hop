//! Clipboard history, the shelf, the launcher and sending files: the parts
//! of the island that reach every computer.

use crate::App;
use openhop_core::extras::{AppsOf, ClipItem, ShelfEntry};
use serde::Serialize;
use tauri::State;

fn with_engine<T>(app: &App, f: impl FnOnce(&openhop_core::Engine) -> T) -> Option<T> {
    app.engine.lock().as_ref().map(f)
}

#[derive(Serialize)]
pub struct Tools {
    me: String,
    clips: Vec<ClipItem>,
    shelf: Vec<ShelfEntry>,
    apps: Vec<AppsOf>,
    computers: Vec<String>,
}

/// Everything the island's Clipboard, Shelf and Open tabs show.
#[tauri::command]
pub fn tools_state(app: State<App>) -> Tools {
    with_engine(&app, |e| {
        let hub = e.hub();
        let me = e.status().name;
        let computers = hub.computers().into_iter().filter(|c| !c.this).map(|c| c.name).collect();
        Tools { me, clips: hub.clip_history(), shelf: hub.shelf(), apps: hub.apps(), computers }
    })
    .unwrap_or_else(|| Tools { me: app.config.lock().name.clone(), clips: vec![], shelf: vec![], apps: vec![], computers: vec![] })
}

#[tauri::command]
pub fn clip_use(app: State<App>, id: u64) {
    with_engine(&app, |e| e.hub().clip_use(id));
}

#[tauri::command]
pub fn clip_pin(app: State<App>, id: u64, pinned: bool) {
    with_engine(&app, |e| e.hub().clip_pin(id, pinned));
}

#[tauri::command]
pub fn clip_forget(app: State<App>, id: u64) {
    with_engine(&app, |e| e.hub().clip_forget(id));
}

#[tauri::command]
pub fn shelf_put(app: State<App>, paths: Vec<String>) {
    if !paths.is_empty() {
        with_engine(&app, |e| e.shelf_add(paths));
    }
}

#[tauri::command]
pub fn shelf_take(app: State<App>, origin: String, id: u64) {
    with_engine(&app, |e| e.shelf_take(origin, id));
}

#[tauri::command]
pub fn shelf_remove(app: State<App>, origin: String, id: u64) {
    with_engine(&app, |e| e.hub().shelf_remove(&origin, id));
}

/// Open the app `id` on computer `on`; the pointer follows when it's another computer.
#[tauri::command]
pub fn app_launch(app: State<App>, on: String, id: String) {
    with_engine(&app, |e| {
        let me = e.status().name;
        e.hub().launch(&on, &id);
        if on != me {
            e.go_to(on);
        }
    });
}

#[tauri::command]
pub fn go_to(app: State<App>, name: String) {
    with_engine(&app, |e| e.go_to(name));
}

/// Send files to a computer ("*" = all of them).
#[tauri::command]
pub fn send_paths(app: State<App>, to: String, paths: Vec<String>) {
    if !paths.is_empty() {
        with_engine(&app, |e| e.send_files(to, paths));
    }
}

/// Arrange the screens by pushing the pointer toward each computer in turn.
#[tauri::command]
pub fn learn_layout(app: State<App>) -> Result<(), String> {
    with_engine(&app, |e| {
        let names: Vec<String> = e.hub().computers().into_iter().filter(|c| !c.this).map(|c| c.name).collect();
        if names.is_empty() {
            return Err("Connect another computer first.".to_string());
        }
        e.learn(names);
        Ok(())
    })
    .unwrap_or_else(|| Err("Turn OpenHop on first.".into()))
}
