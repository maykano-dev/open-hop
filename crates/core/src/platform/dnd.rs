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
    text.lines()
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
    use x11rb::protocol::xproto::{AtomEnum, ConnectionExt as _, CreateWindowAux, EventMask, KeyButMask, WindowClass};
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

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
mod imp {
    use super::*;
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
