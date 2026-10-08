//! The X11 clipboard (also used under XWayland on Wayland desktops).
//!
//! OpenHop needs more than a generic clipboard library offers:
//! - **Writing** several formats at once, so a received image can be pasted as
//!   a picture into a chat or document *and* as a file into a folder.
//!   GNOME Files / Caja / Nemo only paste `x-special/gnome-copied-files`,
//!   Dolphin and most apps take `text/uri-list`, editors take `image/png`.
//! - **Reading** with the list of offered formats (`TARGETS`), so a copied file
//!   isn't mistaken for its path text, and images in any format are picked up.
//!
//! Large transfers use the INCR protocol in both directions.

use crossbeam_channel::{Receiver, Sender};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::{Duration, Instant};
use x11rb::connection::{Connection, RequestConnection};
use x11rb::protocol::xproto::*;
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;
use x11rb::{CURRENT_TIME, NONE};

/// What to put on the clipboard.
#[derive(Clone, Default, Debug, PartialEq)]
pub struct Contents {
    pub files: Vec<PathBuf>,
    pub png: Option<Vec<u8>>,
    pub text: Option<String>,
}

impl Contents {
    /// Bytes for a target name, if we have that format.
    fn bytes_for(&self, target: &str) -> Option<Vec<u8>> {
        let uris = || self.files.iter().map(|p| crate::clipboard::file_uri(p)).collect::<Vec<_>>();
        match target {
            "x-special/gnome-copied-files" | "x-special/mate-copied-files" if !self.files.is_empty() => {
                Some(format!("copy\n{}", uris().join("\n")).into_bytes())
            }
            "text/uri-list" if !self.files.is_empty() => Some(uris().iter().map(|u| format!("{u}\r\n")).collect::<String>().into_bytes()),
            "image/png" => self.png.clone(),
            "UTF8_STRING" | "text/plain;charset=utf-8" | "text/plain" | "STRING" | "TEXT" => self.text.as_ref().map(|t| t.clone().into_bytes()),
            _ => None,
        }
    }

    fn target_names(&self) -> Vec<&'static str> {
        let mut v = vec![];
        if !self.files.is_empty() {
            v.extend(["x-special/gnome-copied-files", "x-special/mate-copied-files", "text/uri-list"]);
        }
        if self.png.is_some() {
            v.push("image/png");
        }
        if self.text.is_some() {
            v.extend(["UTF8_STRING", "text/plain;charset=utf-8", "text/plain", "STRING", "TEXT"]);
        }
        v
    }
}

struct Atoms {
    clipboard: Atom,
    targets: Atom,
    timestamp: Atom,
    incr: Atom,
    prop: Atom,
}

fn intern(conn: &RustConnection, name: &str) -> Option<Atom> {
    conn.intern_atom(false, name.as_bytes()).ok()?.reply().ok().map(|r| r.atom)
}

fn atoms(conn: &RustConnection) -> Option<Atoms> {
    Some(Atoms {
        clipboard: intern(conn, "CLIPBOARD")?,
        targets: intern(conn, "TARGETS")?,
        timestamp: intern(conn, "TIMESTAMP")?,
        incr: intern(conn, "INCR")?,
        prop: intern(conn, "OPENHOP_CLIP")?,
    })
}

fn make_window(conn: &RustConnection, screen: usize) -> Option<Window> {
    let root = conn.setup().roots[screen].root;
    let win = conn.generate_id().ok()?;
    conn.create_window(0, win, root, -10, -10, 1, 1, 0, WindowClass::INPUT_ONLY, 0, &CreateWindowAux::new().event_mask(EventMask::PROPERTY_CHANGE)).ok()?;
    conn.flush().ok()?;
    Some(win)
}

// ---------------------------------------------------------------- owner

static OWNER_WIN: OnceLock<Option<Window>> = OnceLock::new();
static OWNER_TX: OnceLock<Sender<(u64, Contents)>> = OnceLock::new();
static GEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
/// The generation of the contents we currently own the clipboard with (0 = none).
static OWNED: Mutex<u64> = Mutex::new(0);

/// The window we own the clipboard with, if the owner thread is running.
pub fn owner_window() -> Option<Window> {
    OWNER_WIN.get().copied().flatten()
}

/// Put `contents` on the clipboard (all formats at once).
pub fn set(contents: Contents) -> Result<(), String> {
    if OWNER_TX.get().is_none() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let (ready_tx, ready_rx) = crossbeam_channel::bounded(1);
        std::thread::Builder::new().name("x11-clip-owner".into()).spawn(move || owner_thread(rx, ready_tx)).map_err(|e| e.to_string())?;
        let win = ready_rx.recv_timeout(Duration::from_secs(3)).ok().flatten();
        let _ = OWNER_WIN.set(win);
        if win.is_none() {
            return Err("can't connect to the X server".into());
        }
        let _ = OWNER_TX.set(tx);
    }
    let (Some(tx), Some(_)) = (OWNER_TX.get(), owner_window()) else {
        return Err("no X11 clipboard".into());
    };
    let gen = GEN.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
    tx.send((gen, contents)).map_err(|e| e.to_string())?;
    // Wait until the owner thread has taken the selection.
    let deadline = Instant::now() + Duration::from_millis(500);
    while Instant::now() < deadline {
        if *OWNED.lock() == gen {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    Err("timed out taking the clipboard".into())
}

struct Transfer {
    requestor: Window,
    prop: Atom,
    kind: Atom,
    data: Vec<u8>,
    offset: usize,
    started: Instant,
}

fn owner_thread(rx: Receiver<(u64, Contents)>, ready: Sender<Option<Window>>) {
    let Ok((conn, screen)) = x11rb::connect(None) else {
        let _ = ready.send(None);
        return;
    };
    let (Some(a), Some(win)) = (atoms(&conn), make_window(&conn, screen)) else {
        let _ = ready.send(None);
        return;
    };
    let _ = ready.send(Some(win));
    let chunk = (conn.maximum_request_bytes().saturating_sub(1024)).clamp(64 * 1024, 1024 * 1024);
    let mut names: HashMap<Atom, String> = HashMap::new();
    let mut ids: HashMap<&'static str, Atom> = HashMap::new();
    let mut contents: Option<Contents> = None;
    let mut since: u32 = CURRENT_TIME;
    let mut transfers: Vec<Transfer> = Vec::new();
    loop {
        match rx.recv_timeout(Duration::from_millis(5)) {
            Ok((gen, c)) => {
                for n in c.target_names() {
                    if !ids.contains_key(n) {
                        if let Some(id) = intern(&conn, n) {
                            ids.insert(n, id);
                            names.insert(id, n.to_string());
                        }
                    }
                }
                since = server_time(&conn, win, a.prop).unwrap_or(CURRENT_TIME);
                let _ = conn.set_selection_owner(win, a.clipboard, since);
                let _ = conn.flush();
                let ok = conn.get_selection_owner(a.clipboard).ok().and_then(|r| r.reply().ok()).map(|r| r.owner == win).unwrap_or(false);
                contents = ok.then_some(c);
                *OWNED.lock() = if ok { gen } else { 0 };
            }
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => return,
            Err(_) => {}
        }
        transfers.retain(|t| t.started.elapsed() < Duration::from_secs(30));
        while let Ok(Some(ev)) = conn.poll_for_event() {
            match ev {
                Event::SelectionClear(e) if e.selection == a.clipboard => {
                    contents = None;
                    *OWNED.lock() = 0;
                }
                Event::SelectionRequest(r) => {
                    let prop = if r.property == NONE { r.target } else { r.property };
                    let mut answered = NONE;
                    if r.selection == a.clipboard {
                        if let Some(c) = &contents {
                            if r.target == a.targets {
                                let mut list: Vec<Atom> = vec![a.targets, a.timestamp];
                                list.extend(c.target_names().iter().filter_map(|n| ids.get(n)));
                                let _ = conn.change_property32(PropMode::REPLACE, r.requestor, prop, AtomEnum::ATOM, &list);
                                answered = prop;
                            } else if r.target == a.timestamp {
                                let _ = conn.change_property32(PropMode::REPLACE, r.requestor, prop, AtomEnum::INTEGER, &[since]);
                                answered = prop;
                            } else if let Some(bytes) = names.get(&r.target).and_then(|n| c.bytes_for(n)) {
                                if bytes.len() > chunk {
                                    let _ =
                                        conn.change_window_attributes(r.requestor, &ChangeWindowAttributesAux::new().event_mask(EventMask::PROPERTY_CHANGE));
                                    let _ = conn.change_property32(PropMode::REPLACE, r.requestor, prop, a.incr, &[bytes.len() as u32]);
                                    transfers.push(Transfer { requestor: r.requestor, prop, kind: r.target, data: bytes, offset: 0, started: Instant::now() });
                                } else {
                                    let _ = conn.change_property8(PropMode::REPLACE, r.requestor, prop, r.target, &bytes);
                                }
                                answered = prop;
                            }
                        }
                    }
                    let notify = SelectionNotifyEvent {
                        response_type: SELECTION_NOTIFY_EVENT,
                        sequence: 0,
                        time: r.time,
                        requestor: r.requestor,
                        selection: r.selection,
                        target: r.target,
                        property: answered,
                    };
                    let _ = conn.send_event(false, r.requestor, EventMask::NO_EVENT, notify);
                    let _ = conn.flush();
                }
                Event::PropertyNotify(p) if p.state == Property::DELETE => {
                    if let Some(i) = transfers.iter().position(|t| t.requestor == p.window && t.prop == p.atom) {
                        let t = &mut transfers[i];
                        let end = (t.offset + chunk).min(t.data.len());
                        let _ = conn.change_property8(PropMode::REPLACE, t.requestor, t.prop, t.kind, &t.data[t.offset..end]);
                        if t.offset == t.data.len() {
                            // That was the empty end-of-data marker.
                            transfers.remove(i);
                        } else {
                            t.offset = end;
                        }
                        let _ = conn.flush();
                    }
                }
                _ => {}
            }
        }
    }
}

/// A real server timestamp (by touching a property on our window).
fn server_time(conn: &RustConnection, win: Window, prop: Atom) -> Option<u32> {
    conn.change_property8(PropMode::APPEND, win, prop, AtomEnum::STRING, &[]).ok()?;
    conn.flush().ok()?;
    let deadline = Instant::now() + Duration::from_millis(200);
    while Instant::now() < deadline {
        match conn.poll_for_event().ok()? {
            Some(Event::PropertyNotify(p)) if p.window == win => return Some(p.time),
            Some(_) => {}
            None => std::thread::sleep(Duration::from_millis(1)),
        }
    }
    None
}

// ---------------------------------------------------------------- reader

pub struct Reader {
    conn: RustConnection,
    win: Window,
    a: Atoms,
    names: HashMap<Atom, String>,
}

/// What another app put on the clipboard.
pub struct Offer<'a> {
    reader: &'a mut Reader,
    targets: Vec<(Atom, String)>,
}

impl Reader {
    pub fn new() -> Option<Reader> {
        let (conn, screen) = x11rb::connect(None).ok()?;
        let a = atoms(&conn)?;
        let win = make_window(&conn, screen)?;
        Some(Reader { conn, win, a, names: HashMap::new() })
    }

    fn name(&mut self, atom: Atom) -> String {
        if let Some(n) = self.names.get(&atom) {
            return n.clone();
        }
        let n = self.conn.get_atom_name(atom).ok().and_then(|r| r.reply().ok()).map(|r| String::from_utf8_lossy(&r.name).into_owned()).unwrap_or_default();
        self.names.insert(atom, n.clone());
        n
    }

    /// The current clipboard's formats, or None if it's empty or ours.
    pub fn offer(&mut self) -> Option<Offer<'_>> {
        let owner = self.conn.get_selection_owner(self.a.clipboard).ok()?.reply().ok()?.owner;
        if owner == NONE || Some(owner) == owner_window() {
            return None;
        }
        let raw = self.fetch(self.a.targets, Duration::from_millis(1500))?;
        let atoms: Vec<Atom> = raw.chunks_exact(4).map(|c| u32::from_ne_bytes([c[0], c[1], c[2], c[3]])).collect();
        let targets = atoms.into_iter().map(|t| (t, self.name(t))).collect();
        Some(Offer { reader: self, targets })
    }

    /// Ask the owner for one format. Handles INCR.
    fn fetch(&mut self, target: Atom, wait: Duration) -> Option<Vec<u8>> {
        let (conn, win, prop, clip) = (&self.conn, self.win, self.a.prop, self.a.clipboard);
        while let Ok(Some(_)) = conn.poll_for_event() {}
        conn.delete_property(win, prop).ok()?;
        conn.convert_selection(win, clip, target, prop, CURRENT_TIME).ok()?;
        conn.flush().ok()?;
        let deadline = Instant::now() + wait;
        let notify = loop {
            if Instant::now() > deadline {
                return None;
            }
            match conn.poll_for_event().ok()? {
                Some(Event::SelectionNotify(e)) if e.requestor == win => break e,
                Some(_) => {}
                None => std::thread::sleep(Duration::from_millis(2)),
            }
        };
        if notify.property == NONE {
            return None;
        }
        let r = conn.get_property(false, win, prop, AtomEnum::ANY, 0, u32::MAX / 4).ok()?.reply().ok()?;
        if r.type_ != self.a.incr {
            let _ = conn.delete_property(win, prop);
            let _ = conn.flush();
            return Some(r.value);
        }
        // INCR: deleting the property asks for the first chunk.
        let mut out = Vec::new();
        conn.delete_property(win, prop).ok()?;
        conn.flush().ok()?;
        let mut last = Instant::now();
        loop {
            if last.elapsed() > Duration::from_secs(3) {
                return None;
            }
            match conn.poll_for_event().ok()? {
                Some(Event::PropertyNotify(p)) if p.window == win && p.atom == prop && p.state == Property::NEW_VALUE => {
                    let r = conn.get_property(true, win, prop, AtomEnum::ANY, 0, u32::MAX / 4).ok()?.reply().ok()?;
                    conn.flush().ok()?;
                    if r.value.is_empty() {
                        return Some(out);
                    }
                    out.extend_from_slice(&r.value);
                    if out.len() > crate::net::MAX_MSG {
                        return None;
                    }
                    last = Instant::now();
                }
                Some(_) => {}
                None => std::thread::sleep(Duration::from_millis(1)),
            }
        }
    }
}

const IMAGE_TYPES: [&str; 6] = ["image/png", "image/jpeg", "image/bmp", "image/gif", "image/webp", "image/tiff"];
const TEXT_TYPES: [&str; 5] = ["UTF8_STRING", "text/plain;charset=utf-8", "text/plain", "STRING", "TEXT"];

impl Offer<'_> {
    pub fn names(&self) -> Vec<String> {
        self.targets.iter().map(|(_, n)| n.clone()).collect()
    }

    fn has(&self, name: &str) -> Option<Atom> {
        self.targets.iter().find(|(_, n)| n == name).map(|(a, _)| *a)
    }

    fn get(&mut self, name: &str, wait: Duration) -> Option<Vec<u8>> {
        let atom = self.has(name)?;
        self.reader.fetch(atom, wait)
    }

    /// Whether a file manager put this here.
    pub fn from_file_manager(&self) -> bool {
        self.has("x-special/gnome-copied-files").is_some() || self.has("text/uri-list").is_some()
    }

    pub fn files(&mut self) -> Vec<PathBuf> {
        let wait = Duration::from_millis(1500);
        for t in ["x-special/gnome-copied-files", "x-special/mate-copied-files", "text/uri-list"] {
            if let Some(b) = self.get(t, wait) {
                let files = crate::platform::dnd::parse_uri_list(&String::from_utf8_lossy(&b));
                if !files.is_empty() {
                    return files;
                }
            }
        }
        vec![]
    }

    pub fn has_image(&self) -> bool {
        IMAGE_TYPES.iter().any(|t| self.has(t).is_some())
    }

    /// The image as PNG.
    pub fn image_png(&mut self) -> Option<Vec<u8>> {
        for t in IMAGE_TYPES {
            if self.has(t).is_none() {
                continue;
            }
            if let Some(b) = self.get(t, Duration::from_secs(3)) {
                if let Some(png) = crate::clipboard::to_png(&b) {
                    return Some(png);
                }
            }
        }
        None
    }

    pub fn text(&mut self) -> Option<String> {
        for t in TEXT_TYPES {
            if let Some(b) = self.get(t, Duration::from_millis(1500)) {
                return Some(String::from_utf8_lossy(&b).trim_end_matches('\0').to_string());
            }
        }
        None
    }
}
