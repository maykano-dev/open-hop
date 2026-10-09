//! GNOME on Wayland: apps can't list windows or see the pointer there, so
//! OpenHop installs a tiny GNOME Shell extension that answers on D-Bus
//! (`dev.openhop.Shell`). Active after the next login.

use crate::protocol::WinInfo;
use std::sync::Mutex;
use std::time::{Duration, Instant};

const NAME: &str = "dev.openhop.Shell";
const PATH: &str = "/dev/openhop/Shell";

fn conn() -> Option<zbus::blocking::Connection> {
    static C: Mutex<Option<(Option<zbus::blocking::Connection>, Instant)>> = Mutex::new(None);
    let mut g = C.lock().ok()?;
    // Retry a failed connection now and then, not on every call.
    if let Some((c, at)) = g.as_ref() {
        if c.is_some() || at.elapsed() < Duration::from_secs(30) {
            return c.clone();
        }
    }
    let c = zbus::blocking::Connection::session().ok();
    *g = Some((c.clone(), Instant::now()));
    c
}

fn call<B: serde::de::DeserializeOwned + zbus::zvariant::Type>(method: &str, body: &(impl serde::Serialize + zbus::zvariant::DynamicType)) -> Option<B> {
    let c = conn()?;
    let r = c.call_method(Some(NAME), PATH, Some(NAME), method, body).ok()?;
    r.body().deserialize().ok()
}

/// The extension is running (GNOME on Wayland, after a login).
pub fn available() -> bool {
    static LAST: Mutex<Option<(bool, Instant)>> = Mutex::new(None);
    let mut g = match LAST.lock() {
        Ok(g) => g,
        Err(_) => return false,
    };
    if let Some((v, at)) = *g {
        if at.elapsed() < Duration::from_secs(if v { 60 } else { 15 }) {
            return v;
        }
    }
    let v = call::<String>("Windows", &()).is_some();
    *g = Some((v, Instant::now()));
    v
}

#[derive(serde::Deserialize)]
#[allow(dead_code)]
struct W {
    id: u64,
    title: String,
    app: String,
    pid: u32,
    w: i32,
    h: i32,
    minimized: bool,
    skip: bool,
}

fn windows() -> Vec<W> {
    call::<String>("Windows", &()).and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

pub fn list() -> Vec<WinInfo> {
    let me = std::process::id();
    windows()
        .into_iter()
        .filter(|w| !w.minimized && !w.skip && w.pid != me && !w.title.is_empty())
        .map(|w| WinInfo { id: w.id, title: w.title, app: w.app, w: w.w, h: w.h, pid: w.pid })
        .collect()
}

pub fn taskbar_pids() -> Vec<u32> {
    let me = std::process::id();
    windows().into_iter().filter(|w| !w.skip && w.pid != me).map(|w| w.pid).collect()
}

pub fn raise(id: u64) {
    let _ = call::<()>("Activate", &(id,));
}

/// Reserve the island's lane under GNOME's top bar; where it starts.
pub fn lane(height: u32) -> Option<i32> {
    call::<i32>("Lane", &(height,))
}

pub fn lane_off() {
    let _ = call::<()>("LaneOff", &());
}

/// Pointer position (screen pixels) and whether the main button is held.
pub fn pointer() -> Option<(i32, i32, bool)> {
    let (x, y, mods): (i32, i32, u32) = call("Pointer", &())?;
    Some((x, y, mods & (1 << 8) != 0))
}

#[cfg(test)]
mod tests {
    #[test]
    fn reads_window_json() {
        let s = r#"[{"id":12,"title":"Doc \"1\" – é","app":"org.gnome.TextEditor","pid":44,"x":0,"y":0,"w":800,"h":600,"minimized":false,"skip":false},{"id":13,"title":"Hidden","app":"x","pid":45,"x":0,"y":0,"w":1,"h":1,"minimized":true,"skip":false}]"#;
        let v: Vec<super::W> = serde_json::from_str(s).unwrap();
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].title, "Doc \"1\" – é");
        assert_eq!((v[0].id, v[0].pid, v[0].w, v[1].minimized), (12, 44, 800, true));
    }
}
