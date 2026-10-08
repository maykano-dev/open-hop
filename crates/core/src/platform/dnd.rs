//! Detecting a file drag in progress, so files dragged off one screen can be
//! delivered to the computer they're dropped on.
//!
//! - Linux (X11): the drag source owns the `XdndSelection`; we ask it for `text/uri-list`.
//! - macOS: the system drag pasteboard holds the dragged file URLs.
//! - Windows: there is no global drag pasteboard, so a tiny invisible OLE drop
//!   target is placed under the cursor; the drag source hands it the file list.

use std::path::PathBuf;

/// Is the (primary) left mouse button physically down right now?
pub fn left_button_down() -> bool {
    imp::left_button_down()
}

/// Files being dragged right now, if any. May block for up to ~0.4 s.
pub fn drag_files() -> Option<Vec<PathBuf>> {
    imp::drag_files().filter(|v| !v.is_empty())
}

/// Files being dragged on a Wayland desktop, where only an X11 window under
/// the pointer gets to see the drag. `at` is where the pointer is; `nudge`
/// wiggles it so the desktop notices the window placed under it.
pub fn drag_files_at(at: (i32, i32), nudge: &mut dyn FnMut(i32)) -> Option<Vec<PathBuf>> {
    #[cfg(target_os = "linux")]
    if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        if let Some(f) = drag_files() {
            return Some(f);
        }
        return imp::drag_files_wayland(at, nudge).filter(|v| !v.is_empty());
    }
    let _ = (at, nudge);
    drag_files()
}

/// Drop `files` onto whatever is under the pointer, as if they had been
/// dragged there from a file manager. Blocks for up to a few seconds.
/// Returns false if nothing there accepted them.
pub fn drop_files(files: &[PathBuf]) -> bool {
    if files.is_empty() {
        return false;
    }
    imp::drop_files(files)
}

/// Cancel the local drag (sends Escape to the drag source).
pub fn cancel_drag() {
    imp::cancel_drag()
}

/// Remember that a mouse button went down (macOS uses this to tell a fresh
/// drag from stale pasteboard contents).
pub fn note_left_down() {
    #[cfg(target_os = "macos")]
    imp::note_left_down();
}

pub fn parse_uri_list(text: &str) -> Vec<PathBuf> {
    // Some apps (libfm/PCManFM) end the list with a NUL byte.
    text.split(['\n', '\r', '\0'])
        .map(str::trim)
        .filter_map(|l| l.strip_prefix("file://"))
        .map(|rest| if rest.starts_with('/') { rest } else { rest.find('/').map(|i| &rest[i..]).unwrap_or(rest) })
        .map(|p| PathBuf::from(percent_decode(p)))
        .filter(|p| p.exists())
        .collect()
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
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

// ------------------------------------------------------------------ Linux / X11
#[cfg(target_os = "linux")]
mod imp {
    use super::*;
    use parking_lot::Mutex;
    use std::sync::OnceLock;
    use std::time::{Duration, Instant};
    use x11rb::connection::Connection;
    use x11rb::protocol::xfixes::{ConnectionExt as _, SelectionEventMask};
    use x11rb::protocol::xproto::{
        AtomEnum, ClientMessageEvent, ConnectionExt as _, CreateWindowAux, EventMask, KeyButMask, PropMode, SelectionNotifyEvent, WindowClass,
        SELECTION_NOTIFY_EVENT,
    };
    use x11rb::wrapper::ConnectionExt as _;
    use x11rb::protocol::xtest::ConnectionExt as _;
    use x11rb::protocol::Event;
    use x11rb::rust_connection::RustConnection;
    use x11rb::{CURRENT_TIME, NONE};

    struct X {
        conn: RustConnection,
        root: u32,
        win: u32,
        xdnd: u32,
        uri_list: u32,
        prop: u32,
        /// When a drag last started (XdndSelection changed hands).
        last_drag: Option<Instant>,
    }

    fn x() -> Option<&'static Mutex<X>> {
        static X: OnceLock<Option<Mutex<X>>> = OnceLock::new();
        X.get_or_init(|| {
            let (conn, n) = x11rb::connect(None).ok()?;
            let root = conn.setup().roots[n].root;
            let atom = |name: &[u8]| conn.intern_atom(false, name).ok()?.reply().ok().map(|r| r.atom);
            let xdnd = atom(b"XdndSelection")?;
            let uri_list = atom(b"text/uri-list")?;
            let prop = atom(b"OPENHOP_DND")?;
            let win = conn.generate_id().ok()?;
            conn.create_window(0, win, root, -10, -10, 1, 1, 0, WindowClass::INPUT_ONLY, 0, &CreateWindowAux::new().event_mask(EventMask::PROPERTY_CHANGE))
                .ok()?;
            conn.xfixes_query_version(5, 0).ok()?.reply().ok()?;
            conn.xfixes_select_selection_input(win, xdnd, SelectionEventMask::SET_SELECTION_OWNER).ok()?;
            conn.flush().ok()?;
            Some(Mutex::new(X { conn, root, win, xdnd, uri_list, prop, last_drag: None }))
        })
        .as_ref()
    }

    impl X {
        fn pump(&mut self) {
            while let Ok(Some(ev)) = self.conn.poll_for_event() {
                if let Event::XfixesSelectionNotify(e) = ev {
                    if e.owner != NONE {
                        self.last_drag = Some(Instant::now());
                    }
                }
            }
        }
    }

    pub fn start_watch() {
        // Record when drags start, as they happen.
        static STARTED: std::sync::Once = std::sync::Once::new();
        STARTED.call_once(|| {
            if x().is_some() {
                let _ = std::thread::Builder::new().name("xdnd-watch".into()).spawn(|| loop {
                    if let Some(x) = x() {
                        x.lock().pump();
                    }
                    std::thread::sleep(Duration::from_millis(100));
                });
            }
        });
    }

    pub fn left_button_down() -> bool {
        let Some(x) = x() else { return false };
        let x = x.lock();
        x.conn
            .query_pointer(x.root)
            .ok()
            .and_then(|c| c.reply().ok())
            .map(|r| u16::from(r.mask) & u16::from(KeyButMask::BUTTON1) != 0)
            .unwrap_or(false)
    }

    pub fn drag_files() -> Option<Vec<PathBuf>> {
        let x = x()?;
        let mut x = x.lock();
        x.pump();
        let owner = x.conn.get_selection_owner(x.xdnd).ok()?.reply().ok()?.owner;
        // Only trust a drag that started recently (the owner may linger afterwards).
        if owner == NONE || x.last_drag.map(|t| t.elapsed() > Duration::from_secs(120)).unwrap_or(true) {
            return None;
        }
        x.conn.convert_selection(x.win, x.xdnd, x.uri_list, x.prop, CURRENT_TIME).ok()?;
        x.conn.flush().ok()?;
        let deadline = Instant::now() + Duration::from_millis(400);
        while Instant::now() < deadline {
            match x.conn.poll_for_event() {
                Ok(Some(Event::SelectionNotify(e))) if e.selection == x.xdnd => {
                    if e.property == NONE {
                        return None;
                    }
                    let r = x.conn.get_property(true, x.win, x.prop, AtomEnum::ANY, 0, 16 * 1024 * 1024).ok()?.reply().ok()?;
                    return Some(parse_uri_list(&String::from_utf8_lossy(&r.value)));
                }
                Ok(Some(Event::XfixesSelectionNotify(_))) => x.last_drag = Some(Instant::now()),
                Ok(Some(_)) => {}
                Ok(None) => std::thread::sleep(Duration::from_millis(5)),
                Err(_) => return None,
            }
        }
        None
    }


    fn atoms(conn: &RustConnection, names: &[&str]) -> Option<Vec<u32>> {
        let cookies: Vec<_> = names.iter().map(|n| conn.intern_atom(false, n.as_bytes())).collect::<Result<_, _>>().ok()?;
        cookies.into_iter().map(|c| c.reply().ok().map(|r| r.atom)).collect()
    }

    fn client_message(conn: &RustConnection, dest: u32, window: u32, kind: u32, data: [u32; 5]) {
        let ev = ClientMessageEvent::new(32, window, kind, data);
        let _ = conn.send_event(false, dest, EventMask::NO_EVENT, ev);
        let _ = conn.flush();
    }

    /// The XdndAware window under the pointer: (window, where to send, version).
    fn xdnd_target(conn: &RustConnection, root: u32, aware: u32, proxy: u32) -> Option<(u32, u32, u32)> {
        let mut w = root;
        for _ in 0..32 {
            if w != root {
                let r = conn.get_property(false, w, aware, AtomEnum::ATOM, 0, 1).ok()?.reply().ok()?;
                if let Some(v) = r.value32().and_then(|mut i| i.next()) {
                    let dest = conn
                        .get_property(false, w, proxy, AtomEnum::WINDOW, 0, 1)
                        .ok()
                        .and_then(|c| c.reply().ok())
                        .and_then(|r| r.value32().and_then(|mut i| i.next()))
                        .unwrap_or(w);
                    return Some((w, dest, v.min(5)));
                }
            }
            let child = conn.query_pointer(w).ok()?.reply().ok()?.child;
            if child == NONE {
                return None;
            }
            w = child;
        }
        None
    }

    pub fn drop_files(files: &[PathBuf]) -> bool {
        let Ok((conn, n)) = x11rb::connect(None) else { return false };
        let root = conn.setup().roots[n].root;
        let names = [
            "XdndAware", "XdndProxy", "XdndSelection", "XdndEnter", "XdndPosition", "XdndStatus", "XdndDrop", "XdndFinished",
            "XdndLeave", "XdndActionCopy", "text/uri-list", "TARGETS", "x-special/gnome-copied-files",
        ];
        let Some(a) = atoms(&conn, &names) else { return false };
        let [aware, proxy, selection, enter, position, status, drop, finished, leave, copy, uri_list, targets, gnome] = a[..] else { return false };
        let Ok(win) = conn.generate_id() else { return false };
        if conn.create_window(0, win, root, -10, -10, 1, 1, 0, WindowClass::INPUT_ONLY, 0, &CreateWindowAux::new()).is_err() {
            return false;
        }
        let _ = conn.set_selection_owner(win, selection, CURRENT_TIME);
        let _ = conn.flush();
        let done = |ok: bool| {
            let _ = conn.destroy_window(win);
            let _ = conn.flush();
            ok
        };
        let Some((target, dest, version)) = xdnd_target(&conn, root, aware, proxy) else {
            log::debug!("no drop target under the pointer");
            return done(false);
        };
        let Some(p) = conn.query_pointer(root).ok().and_then(|c| c.reply().ok()) else { return done(false) };
        let (x, y) = (p.root_x as u32 & 0xffff, p.root_y as u32 & 0xffff);
        let uris: String = files.iter().map(|f| format!("{}\r\n", crate::clipboard::file_uri(f))).collect();
        log::debug!("XDND drop on window {target:#x} (version {version})");
        client_message(&conn, dest, target, enter, [win, version << 24, uri_list, gnome, 0]);
        client_message(&conn, dest, target, position, [win, 0, (x << 16) | y, CURRENT_TIME, copy]);
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut dropped = false;
        let mut tries = 0;
        while Instant::now() < deadline {
            let ev = match conn.poll_for_event() {
                Ok(Some(ev)) => ev,
                Ok(None) => {
                    std::thread::sleep(Duration::from_millis(3));
                    continue;
                }
                Err(_) => return false,
            };
            match &ev {
                Event::SelectionRequest(r) => log::trace!(
                    "xdnd: request for {:?}",
                    conn.get_atom_name(r.target).ok().and_then(|c| c.reply().ok()).map(|r| String::from_utf8_lossy(&r.name).into_owned())
                ),
                Event::ClientMessage(m) => log::trace!("xdnd: message {} {:?}", m.type_, m.data.as_data32()),
                _ => {}
            }
            match ev {
                Event::SelectionRequest(r) if r.selection == selection => {
                    let prop = if r.property == NONE { r.target } else { r.property };
                    let mut answered = prop;
                    if r.target == targets {
                        let _ = conn.change_property32(PropMode::REPLACE, r.requestor, prop, AtomEnum::ATOM, &[targets, uri_list, gnome]);
                    } else if r.target == uri_list {
                        let _ = conn.change_property8(PropMode::REPLACE, r.requestor, prop, uri_list, uris.as_bytes());
                    } else if r.target == gnome {
                        let body = format!("copy\n{}", uris.trim_end().replace("\r\n", "\n"));
                        let _ = conn.change_property8(PropMode::REPLACE, r.requestor, prop, gnome, body.as_bytes());
                    } else {
                        answered = NONE;
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
                Event::ClientMessage(m) if m.type_ == status && !dropped => {
                    let d = m.data.as_data32();
                    if d[1] & 1 == 1 {
                        client_message(&conn, dest, target, drop, [win, 0, CURRENT_TIME, 0, 0]);
                        dropped = true;
                    } else if tries >= 12 {
                        client_message(&conn, dest, target, leave, [win, 0, 0, 0, 0]);
                        return done(false);
                    } else {
                        // Targets often say "not yet" until they've looked at the
                        // data; ask again like a moving pointer would.
                        tries += 1;
                        std::thread::sleep(Duration::from_millis(100));
                        client_message(&conn, dest, target, position, [win, 0, (x << 16) | y, CURRENT_TIME, copy]);
                    }
                }
                Event::ClientMessage(m) if m.type_ == finished => {
                    let d = m.data.as_data32();
                    // Version 5 says whether the drop worked; older ones don't.
                    return done(version < 5 || d[1] & 1 == 1);
                }
                _ => {}
            }
        }
        if !dropped {
            client_message(&conn, dest, target, leave, [win, 0, 0, 0, 0]);
        }
        done(dropped)
    }

    /// On Wayland, a drag only shows up in X11 once it passes over an X11
    /// window. Put a small one under the pointer and read the drag from it.
    pub fn drag_files_wayland((px, py): (i32, i32), nudge: &mut dyn FnMut(i32)) -> Option<Vec<PathBuf>> {
        let (conn, n) = x11rb::connect(None).ok()?;
        let screen = &conn.setup().roots[n];
        let root = screen.root;
        let a = atoms(&conn, &["XdndAware", "XdndSelection", "XdndPosition", "XdndStatus", "text/uri-list", "OPENHOP_DND"])?;
        let [aware, selection, position, status, uri_list, prop] = a[..] else { return None };
        let win = conn.generate_id().ok()?;
        let size = 24u16;
        conn.create_window(
            0,
            win,
            root,
            (px - size as i32 / 2) as i16,
            (py - size as i32 / 2) as i16,
            size,
            size,
            0,
            WindowClass::INPUT_OUTPUT,
            0,
            &CreateWindowAux::new().override_redirect(1).background_pixel(screen.white_pixel).event_mask(EventMask::PROPERTY_CHANGE),
        )
        .ok()?;
        conn.change_property32(PropMode::REPLACE, win, aware, AtomEnum::ATOM, &[5]).ok()?;
        conn.map_window(win).ok()?;
        conn.flush().ok()?;
        let cleanup = || {
            let _ = conn.destroy_window(win);
            let _ = conn.flush();
        };
        let deadline = Instant::now() + Duration::from_millis(600);
        let mut asked = false;
        let mut out = None;
        let mut wiggles = 0;
        let mut next_wiggle = Instant::now() + Duration::from_millis(30);
        while Instant::now() < deadline {
            if !asked && Instant::now() >= next_wiggle {
                nudge(if wiggles % 2 == 0 { 1 } else { -1 });
                wiggles += 1;
                next_wiggle = Instant::now() + Duration::from_millis(40);
            }
            let Ok(ev) = conn.poll_for_event() else { break };
            let Some(ev) = ev else {
                std::thread::sleep(Duration::from_millis(5));
                continue;
            };
            match ev {
                Event::ClientMessage(m) if m.type_ == position => {
                    let src = m.data.as_data32()[0];
                    // Politely refuse; we only want to look.
                    client_message(&conn, src, src, status, [win, 0, 0, 0, 0]);
                    if !asked {
                        let _ = conn.convert_selection(win, selection, uri_list, prop, CURRENT_TIME);
                        let _ = conn.flush();
                        asked = true;
                    }
                }
                Event::SelectionNotify(e) if e.requestor == win => {
                    if e.property != NONE {
                        if let Ok(Ok(r)) = conn.get_property(true, win, prop, AtomEnum::ANY, 0, 4 * 1024 * 1024).map(|c| c.reply()) {
                            out = Some(parse_uri_list(&String::from_utf8_lossy(&r.value)));
                        }
                    }
                    break;
                }
                _ => {}
            }
        }
        cleanup();
        if wiggles % 2 == 1 {
            nudge(-1);
        }
        out
    }

    pub fn cancel_drag() {
        let Some(x) = x() else { return };
        let x = x.lock();
        const ESC: u8 = 9; // evdev KEY_ESC (1) + 8
        let _ = x.conn.xtest_fake_input(2, ESC, CURRENT_TIME, x.root, 0, 0, 0);
        let _ = x.conn.xtest_fake_input(3, ESC, CURRENT_TIME, x.root, 0, 0, 0);
        let _ = x.conn.flush();
        drop(x);
        // Give the drag source a moment to drop its pointer grab.
        std::thread::sleep(Duration::from_millis(80));
    }
}

// ------------------------------------------------------------------ macOS
#[cfg(target_os = "macos")]
mod imp {
    use super::*;
    use core_graphics::event::{CGEvent, CGEventTapLocation};
    use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
    use objc2_app_kit::{NSPasteboard, NSPasteboardNameDrag, NSPasteboardTypeFileURL};
    use std::sync::atomic::{AtomicIsize, Ordering};

    static DOWN_COUNT: AtomicIsize = AtomicIsize::new(-1);

    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGEventSourceButtonState(state: i32, button: u32) -> bool;
    }

    fn drag_pb() -> objc2::rc::Retained<NSPasteboard> {
        NSPasteboard::pasteboardWithName(unsafe { NSPasteboardNameDrag })
    }

    pub fn note_left_down() {
        DOWN_COUNT.store(drag_pb().changeCount(), Ordering::SeqCst);
    }

    pub fn left_button_down() -> bool {
        // kCGEventSourceStateCombinedSessionState = 0, kCGMouseButtonLeft = 0
        unsafe { CGEventSourceButtonState(0, 0) }
    }

    pub fn drag_files() -> Option<Vec<PathBuf>> {
        let pb = drag_pb();
        // The drag pasteboard keeps the last drag's contents: only use it if a
        // new drag has started since the button went down.
        if pb.changeCount() == DOWN_COUNT.load(Ordering::SeqCst) {
            return None;
        }
        let items = pb.pasteboardItems()?;
        let mut out = Vec::new();
        for item in items.iter() {
            if let Some(s) = item.stringForType(unsafe { NSPasteboardTypeFileURL }) {
                out.extend(parse_uri_list(&s.to_string()));
            }
        }
        Some(out)
    }

    /// Not yet on macOS: the files land in Downloads › OpenHop and on the clipboard.
    pub fn drop_files(_: &[PathBuf]) -> bool {
        false
    }

    pub fn cancel_drag() {
        if let Ok(src) = CGEventSource::new(CGEventSourceStateID::HIDSystemState) {
            for down in [true, false] {
                if let Ok(ev) = CGEvent::new_keyboard_event(src.clone(), 0x35, down) {
                    ev.post(CGEventTapLocation::HID);
                }
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(80));
    }
}

// ------------------------------------------------------------------ Windows
#[cfg(windows)]
mod imp {
    pub use super::win_drop::drop_files;
    use super::*;
    use parking_lot::Mutex;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::OnceLock;
    use std::time::{Duration, Instant};
    use windows::core::{implement, w, Ref};
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, POINTL, WPARAM};
    use windows::Win32::System::Com::{IDataObject, DVASPECT_CONTENT, FORMATETC, TYMED_HGLOBAL};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::System::Ole::{IDropTarget, IDropTarget_Impl, OleInitialize, RegisterDragDrop, ReleaseStgMedium, DROPEFFECT, DROPEFFECT_NONE};
    use windows::Win32::System::SystemServices::MODIFIERKEYS_FLAGS;
    use windows::Win32::System::Threading::GetCurrentThreadId;
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        GetAsyncKeyState, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYBD_EVENT_FLAGS, MOUSEINPUT,
        MOUSE_EVENT_FLAGS, VIRTUAL_KEY,
    };
    use windows::Win32::UI::Shell::{DragQueryFileW, HDROP};
    use windows::Win32::UI::WindowsAndMessaging::*;

    const CF_HDROP: u16 = 15;
    const APP_SHOW: u32 = 0x8000 + 10;
    const APP_HIDE: u32 = 0x8000 + 11;

    static THREAD: AtomicU32 = AtomicU32::new(0);
    static FILES: Mutex<Option<Vec<PathBuf>>> = Mutex::new(None);

    fn files_from(obj: &IDataObject) -> Option<Vec<PathBuf>> {
        let fmt = FORMATETC { cfFormat: CF_HDROP, ptd: std::ptr::null_mut(), dwAspect: DVASPECT_CONTENT.0, lindex: -1, tymed: TYMED_HGLOBAL.0 as u32 };
        unsafe {
            let mut medium = obj.GetData(&fmt).ok()?;
            let hdrop = HDROP(medium.u.hGlobal.0);
            let n = DragQueryFileW(hdrop, u32::MAX, None);
            let mut out = Vec::new();
            for i in 0..n {
                let len = DragQueryFileW(hdrop, i, None) as usize;
                let mut buf = vec![0u16; len + 1];
                DragQueryFileW(hdrop, i, Some(&mut buf));
                out.push(PathBuf::from(String::from_utf16_lossy(&buf[..len])));
            }
            ReleaseStgMedium(&mut medium);
            Some(out)
        }
    }

    #[implement(IDropTarget)]
    struct Target;

    impl IDropTarget_Impl for Target_Impl {
        fn DragEnter(&self, obj: Ref<IDataObject>, _keys: MODIFIERKEYS_FLAGS, _pt: &POINTL, effect: *mut DROPEFFECT) -> windows::core::Result<()> {
            if let Some(obj) = obj.as_ref() {
                *FILES.lock() = files_from(obj);
            }
            unsafe { *effect = DROPEFFECT_NONE };
            Ok(())
        }
        fn DragOver(&self, _keys: MODIFIERKEYS_FLAGS, _pt: &POINTL, effect: *mut DROPEFFECT) -> windows::core::Result<()> {
            unsafe { *effect = DROPEFFECT_NONE };
            Ok(())
        }
        fn DragLeave(&self) -> windows::core::Result<()> {
            Ok(())
        }
        fn Drop(&self, _obj: Ref<IDataObject>, _keys: MODIFIERKEYS_FLAGS, _pt: &POINTL, effect: *mut DROPEFFECT) -> windows::core::Result<()> {
            unsafe { *effect = DROPEFFECT_NONE };
            Ok(())
        }
    }

    unsafe extern "system" fn wndproc(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
        DefWindowProcW(h, m, w, l)
    }

    /// Thread owning an OLE drop-target window that can be shown under the cursor.
    fn thread() -> Option<u32> {
        static INIT: OnceLock<Option<u32>> = OnceLock::new();
        *INIT.get_or_init(|| {
            let (tx, rx) = crossbeam_channel::bounded(1);
            std::thread::Builder::new()
                .name("ole-dnd".into())
                .spawn(move || unsafe {
                    if OleInitialize(None).is_err() {
                        let _ = tx.send(None);
                        return;
                    }
                    let hinst = GetModuleHandleW(None).map(|m| windows::Win32::Foundation::HINSTANCE(m.0)).unwrap_or_default();
                    let class = w!("OpenHopDropTarget");
                    let wc = WNDCLASSW { lpfnWndProc: Some(wndproc), hInstance: hinst, lpszClassName: class, ..Default::default() };
                    RegisterClassW(&wc);
                    let Ok(hwnd) = CreateWindowExW(
                        WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                        class,
                        w!("OpenHop drop"),
                        WS_POPUP,
                        0, 0, 1, 1, None, None, Some(hinst), None,
                    ) else {
                        let _ = tx.send(None);
                        return;
                    };
                    let _ = SetLayeredWindowAttributes(hwnd, windows::Win32::Foundation::COLORREF(0), 1, LWA_ALPHA);
                    let target: IDropTarget = Target.into();
                    if RegisterDragDrop(hwnd, &target).is_err() {
                        let _ = tx.send(None);
                        return;
                    }
                    let _ = tx.send(Some(GetCurrentThreadId()));
                    let mut msg = MSG::default();
                    while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                        match msg.message {
                            APP_SHOW => {
                                let (x, y) = (msg.wParam.0 as i32, msg.lParam.0 as i32);
                                let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), x - 24, y - 24, 48, 48, SWP_NOACTIVATE | SWP_SHOWWINDOW);
                            }
                            APP_HIDE => {
                                let _ = ShowWindow(hwnd, SW_HIDE);
                            }
                            _ => {
                                let _ = TranslateMessage(&msg);
                                DispatchMessageW(&msg);
                            }
                        }
                    }
                })
                .ok()?;
            let id = rx.recv().ok().flatten();
            if let Some(id) = id {
                THREAD.store(id, Ordering::SeqCst);
            }
            id
        })
    }

    pub fn left_button_down() -> bool {
        unsafe { GetAsyncKeyState(0x01) as u16 & 0x8000 != 0 }
    }

    fn send(inputs: &[INPUT]) {
        unsafe {
            SendInput(inputs, std::mem::size_of::<INPUT>() as i32);
        }
    }

    pub fn drag_files() -> Option<Vec<PathBuf>> {
        let tid = thread()?;
        let mut p = POINT::default();
        unsafe { GetCursorPos(&mut p).ok()? };
        *FILES.lock() = None;
        unsafe {
            PostThreadMessageW(tid, APP_SHOW, WPARAM(p.x as isize as usize), LPARAM(p.y as isize)).ok()?;
        }
        std::thread::sleep(Duration::from_millis(30));
        // Nudge the pointer so the drag loop notices the window under it.
        let nudge = |dx: i32| INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 { mi: MOUSEINPUT { dx, dy: 0, mouseData: 0, dwFlags: MOUSE_EVENT_FLAGS(0x0001), time: 0, dwExtraInfo: 0 } },
        };
        let deadline = Instant::now() + Duration::from_millis(400);
        let mut found = None;
        while Instant::now() < deadline {
            send(&[nudge(1), nudge(-1)]);
            std::thread::sleep(Duration::from_millis(40));
            if let Some(f) = FILES.lock().take() {
                found = Some(f);
                break;
            }
        }
        unsafe {
            let _ = PostThreadMessageW(tid, APP_HIDE, WPARAM(0), LPARAM(0));
        }
        found
    }

    pub fn cancel_drag() {
        let key = |up: bool| INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT { wVk: VIRTUAL_KEY(0x1B), wScan: 0x01, dwFlags: KEYBD_EVENT_FLAGS(if up { 0x2 } else { 0 }), time: 0, dwExtraInfo: 0 },
            },
        };
        send(&[key(false), key(true)]);
        std::thread::sleep(Duration::from_millis(80));
    }
}


// ------------------------------------------------------------------ Windows drop
#[cfg(windows)]
mod win_drop {
    //! Drop received files where the pointer is, with a real OLE drag:
    //! the same data object Explorer uses, and a drop source that lets go
    //! after the target under the pointer has had a look.
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::time::{Duration, Instant};
    use windows::core::{implement, BOOL, HRESULT, HSTRING};
    use windows::Win32::Foundation::{DRAGDROP_S_CANCEL, DRAGDROP_S_DROP, DRAGDROP_S_USEDEFAULTCURSORS, S_OK};
    use windows::Win32::System::Com::IDataObject;
    use windows::Win32::System::Ole::{DoDragDrop, IDropSource, IDropSource_Impl, OleInitialize, OleUninitialize, DROPEFFECT, DROPEFFECT_COPY, DROPEFFECT_NONE};
    use windows::Win32::System::SystemServices::MODIFIERKEYS_FLAGS;
    use windows::Win32::UI::Input::KeyboardAndMouse::{SendInput, INPUT, INPUT_0, INPUT_MOUSE, MOUSEINPUT, MOUSE_EVENT_FLAGS};
    use windows::Win32::UI::Shell::Common::ITEMIDLIST;
    use windows::Win32::UI::Shell::{ILClone, ILCreateFromPathW, ILFindLastID, ILFree, ILRemoveLastID, SHCreateDataObject};

    #[implement(IDropSource)]
    struct Source {
        started: Instant,
        release: Arc<AtomicBool>,
    }

    impl IDropSource_Impl for Source_Impl {
        fn QueryContinueDrag(&self, escape: BOOL, _keys: MODIFIERKEYS_FLAGS) -> HRESULT {
            if escape.as_bool() || self.started.elapsed() > Duration::from_secs(4) {
                DRAGDROP_S_CANCEL
            } else if self.release.load(Ordering::SeqCst) {
                DRAGDROP_S_DROP
            } else {
                S_OK
            }
        }
        fn GiveFeedback(&self, _effect: DROPEFFECT) -> HRESULT {
            DRAGDROP_S_USEDEFAULTCURSORS
        }
    }

    fn nudge(dx: i32) {
        let i = INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 { mi: MOUSEINPUT { dx, dy: 0, mouseData: 0, dwFlags: MOUSE_EVENT_FLAGS(0x0001), time: 0, dwExtraInfo: 0 } },
        };
        unsafe {
            SendInput(&[i], std::mem::size_of::<INPUT>() as i32);
        }
    }

    unsafe fn data_object(files: &[PathBuf]) -> Option<IDataObject> {
        let full: Vec<*mut ITEMIDLIST> = files
            .iter()
            .map(|f| ILCreateFromPathW(&HSTRING::from(f.canonicalize().unwrap_or_else(|_| f.clone()).to_string_lossy().trim_start_matches(r"\\?\"))))
            .filter(|p| !p.is_null())
            .collect();
        if full.is_empty() {
            return None;
        }
        // All received files share a folder (Downloads › OpenHop).
        let folder = ILClone(full[0]);
        let _ = ILRemoveLastID(Some(folder));
        let children: Vec<*const ITEMIDLIST> = full.iter().map(|p| ILFindLastID(*p) as *const _).collect();
        let obj = SHCreateDataObject::<_, IDataObject>(Some(folder), Some(&children), None::<&IDataObject>).ok();
        ILFree(Some(folder));
        for p in full {
            ILFree(Some(p));
        }
        obj
    }

    fn run(files: Vec<PathBuf>) -> bool {
        unsafe {
            if OleInitialize(None).is_err() {
                return false;
            }
            let ok = (|| {
                let obj = data_object(&files)?;
                let release = Arc::new(AtomicBool::new(false));
                let source: IDropSource = Source { started: Instant::now(), release: release.clone() }.into();
                // Wiggle the pointer so the drag loop finds the window under it,
                // then let go.
                let r = release.clone();
                std::thread::spawn(move || {
                    for i in 0..10 {
                        std::thread::sleep(Duration::from_millis(30));
                        nudge(if i % 2 == 0 { 1 } else { -1 });
                    }
                    r.store(true, Ordering::SeqCst);
                    for i in 0..6 {
                        std::thread::sleep(Duration::from_millis(30));
                        nudge(if i % 2 == 0 { 1 } else { -1 });
                    }
                });
                let mut effect = DROPEFFECT_NONE;
                let hr = DoDragDrop(&obj, &source, DROPEFFECT_COPY, &mut effect);
                Some(hr == DRAGDROP_S_DROP && effect != DROPEFFECT_NONE)
            })()
            .unwrap_or(false);
            OleUninitialize();
            ok
        }
    }

    pub fn drop_files(files: &[PathBuf]) -> bool {
        let files = files.to_vec();
        let (tx, rx) = crossbeam_channel::bounded(1);
        if std::thread::Builder::new().name("ole-drop".into()).spawn(move || {
            let _ = tx.send(run(files));
        }).is_err() {
            return false;
        }
        rx.recv_timeout(Duration::from_secs(8)).unwrap_or(false)
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
mod imp {
    use super::*;
    pub fn drop_files(_: &[PathBuf]) -> bool {
        false
    }
    pub fn left_button_down() -> bool {
        false
    }
    pub fn drag_files() -> Option<Vec<PathBuf>> {
        None
    }
    pub fn cancel_drag() {}
}

/// Start background tracking needed for drag detection (Linux).
pub fn init() {
    #[cfg(target_os = "linux")]
    imp::start_watch();
}

#[cfg(test)]
mod tests {
    #[test]
    fn uri_list() {
        let tmp = std::env::temp_dir();
        let s = format!("file://{}\r\n# comment\nhttp://x\n", tmp.display());
        assert_eq!(super::parse_uri_list(&s), vec![tmp]);
    }
}
