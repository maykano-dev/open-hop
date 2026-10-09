//! Files sent from the file manager's right-click menu
//! (`openhop-app --send NAME FILES…`, `openhop-app --shelf FILES…`), and
//! keeping that menu up to date with the computers.

use crate::App;
use openhop_core::extras::menus;
use parking_lot::Mutex;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Manager};

/// Where a batch of files goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Dest {
    Computer(String),
    Shelf,
    Phone,
}

/// Requests waiting to go out. Explorer opens OpenHop once per selected
/// file, so requests that arrive close together are gathered up first.
#[derive(Default)]
pub struct Outgoing {
    queue: Mutex<Vec<(Dest, Vec<String>, Instant)>>,
}

/// `--send NAME FILES…` or `--shelf FILES…` in a command line (relative
/// file names are in `cwd`).
pub fn parse(args: &[String], cwd: &Path) -> Option<(Dest, Vec<String>)> {
    let i = args.iter().position(|a| a == "--send" || a == "--shelf" || a == "--phone")?;
    let (dest, rest) = match args[i].as_str() {
        "--shelf" => (Dest::Shelf, &args[i + 1..]),
        "--phone" => (Dest::Phone, &args[i + 1..]),
        _ => (Dest::Computer(args.get(i + 1)?.clone()), args.get(i + 2..).unwrap_or(&[])),
    };
    let files: Vec<String> = rest
        .iter()
        .filter(|a| !a.is_empty())
        .map(|a| {
            // Some file managers hand over file:// links.
            let a = a.strip_prefix("file://").map(unescape).unwrap_or_else(|| a.clone());
            let p = PathBuf::from(&a);
            if p.is_absolute() { p } else { cwd.join(p) }.to_string_lossy().into_owned()
        })
        .collect();
    Some((dest, files))
}

fn unescape(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Some(v) = std::str::from_utf8(&b[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Take a request from a command line.
pub fn queue(handle: &AppHandle, args: &[String], cwd: &Path) -> bool {
    let Some((dest, files)) = parse(args, cwd) else { return false };
    if files.is_empty() {
        return true;
    }
    log::info!("asked to send {} item(s) to {dest:?}", files.len());
    let out = handle.state::<Outgoing>();
    let mut q = out.queue.lock();
    match q.iter_mut().find(|(d, _, _)| *d == dest) {
        Some((_, f, at)) => {
            for x in files {
                if !f.contains(&x) {
                    f.push(x);
                }
            }
            *at = Instant::now();
        }
        None => q.push((dest, files, Instant::now())),
    }
    true
}

/// Send what's been gathered (called often). Waits a moment after the last
/// request, and for OpenHop to be on and the computer to be connected.
fn flush(handle: &AppHandle) {
    let out = handle.state::<Outgoing>();
    let app = handle.state::<App>();
    let mut q = out.queue.lock();
    if q.is_empty() {
        return;
    }
    // Phones don't need OpenHop's connections.
    q.retain(|(dest, files, at)| {
        if *dest != Dest::Phone || at.elapsed() < Duration::from_millis(350) {
            return true;
        }
        let paths: Vec<PathBuf> = files.iter().map(PathBuf::from).collect();
        crate::phone::link(handle).offer(&paths);
        false
    });
    let engine = app.engine.lock();
    q.retain(|(dest, files, at)| {
        let quiet = at.elapsed() > Duration::from_millis(350);
        let too_old = at.elapsed() > Duration::from_secs(45);
        let Some(e) = engine.as_ref() else {
            if too_old {
                crate::island::push(
                    handle,
                    crate::island::Activity { title: "OpenHop is off".into(), body: "Turn it on to send files.".into(), icon: "files".into(), short: false },
                );
            }
            return !too_old;
        };
        if !quiet {
            return true;
        }
        match dest {
            Dest::Phone => false,
            Dest::Shelf => {
                e.shelf_add(files.clone());
                false
            }
            Dest::Computer(name) => {
                let online = name == "*" || e.hub().computers().iter().any(|c| &c.name == name);
                // Just started: give the connection a few seconds.
                if online || too_old || at.elapsed() > Duration::from_secs(12) {
                    e.send_files(name.clone(), files.clone());
                    false
                } else {
                    true
                }
            }
        }
    });
}

fn known_path() -> PathBuf {
    openhop_core::Config::default_path().with_file_name("send-menu.txt")
}

/// Keep the right-click menu listing the computers (ones seen before stay,
/// so the menu doesn't change while one is asleep).
fn refresh_menu(handle: &AppHandle, installed: &mut Option<(Vec<String>, bool)>) {
    let Some(exe) = menus::exe() else { return };
    let app = handle.state::<App>();
    let (me, now): (String, Vec<String>) = match app.engine.lock().as_ref() {
        Some(e) => (e.status().name, e.hub().computers().into_iter().filter(|c| !c.this).map(|c| c.name).collect()),
        None => (app.config.lock().name.clone(), vec![]),
    };
    let saved: BTreeSet<String> = std::fs::read_to_string(known_path()).unwrap_or_default().lines().map(str::to_string).filter(|l| !l.is_empty()).collect();
    let mut all = saved.clone();
    all.extend(now);
    all.remove(&me);
    if all != saved {
        let _ = std::fs::write(known_path(), all.iter().cloned().collect::<Vec<_>>().join("\n"));
    }
    let names: Vec<String> = all.into_iter().collect();
    let phone = app.path.parent().map(|d| d.join("phone.json").is_file()).unwrap_or(false);
    let want = (names, phone);
    if installed.as_ref() != Some(&want) {
        menus::install(&exe, &want.0, want.1);
        *installed = Some(want);
    }
}

/// Forget computers that were removed (on "Forget").
pub fn forget_all() {
    let _ = std::fs::remove_file(known_path());
}

pub fn start(handle: AppHandle) {
    std::thread::spawn(move || {
        let mut installed = None;
        let mut last_menu = Instant::now() - Duration::from_secs(60);
        loop {
            std::thread::sleep(Duration::from_millis(150));
            flush(&handle);
            if last_menu.elapsed() > Duration::from_secs(5) {
                last_menu = Instant::now();
                refresh_menu(&handle, &mut installed);
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_command_lines() {
        let a = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let cwd = Path::new("/home/me/Pictures");
        assert_eq!(
            parse(&a(&["openhop-app", "--send", "Laptop", "a.png", "/tmp/b"]), cwd),
            Some((Dest::Computer("Laptop".into()), vec!["/home/me/Pictures/a.png".into(), "/tmp/b".into()]))
        );
        assert_eq!(parse(&a(&["x", "--shelf", "file:///tmp/a%20b.txt"]), cwd), Some((Dest::Shelf, vec!["/tmp/a b.txt".into()])));
        assert_eq!(parse(&a(&["x", "--phone", "c.pdf"]), cwd), Some((Dest::Phone, vec!["/home/me/Pictures/c.pdf".into()])));
        assert_eq!(parse(&a(&["x", "--hidden"]), cwd), None);
        assert_eq!(parse(&a(&["x", "--send"]), cwd), None);
    }
}
