//! Windows backend: low-level hooks for capture, SendInput for injection.

use super::{Capture, InputEvent, Injector};
use crate::keys;
use crate::protocol::{MouseButton, Rect};
use anyhow::{anyhow, Result};
use crossbeam_channel::Sender;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, Ordering};
use std::sync::{Arc, OnceLock};
use windows::core::w;
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::SystemInformation::GetTickCount;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};
use windows::Win32::UI::HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYBD_EVENT_FLAGS, MOUSEINPUT,
    MOUSE_EVENT_FLAGS, VIRTUAL_KEY,
};
use windows::Win32::UI::WindowsAndMessaging::*;

// Window messages (raw values keep this independent of crate re-exports).
const M_MOUSEMOVE: u32 = 0x200;
const M_LDOWN: u32 = 0x201;
const M_LUP: u32 = 0x202;
const M_RDOWN: u32 = 0x204;
const M_RUP: u32 = 0x205;
const M_MDOWN: u32 = 0x207;
const M_MUP: u32 = 0x208;
const M_WHEEL: u32 = 0x20A;
const M_XDOWN: u32 = 0x20B;
const M_XUP: u32 = 0x20C;
const M_HWHEEL: u32 = 0x20E;
const M_KEYDOWN: u32 = 0x100;
const M_KEYUP: u32 = 0x101;
const M_SYSKEYDOWN: u32 = 0x104;
const M_SYSKEYUP: u32 = 0x105;
const APP_GRAB: u32 = 0x8000 + 1;
const APP_RELEASE: u32 = 0x8000 + 2;

const LLMHF_INJECTED: u32 = 0x1;
const LLKHF_EXTENDED: u32 = 0x1;
const LLKHF_INJECTED: u32 = 0x10;

static TX: OnceLock<Sender<InputEvent>> = OnceLock::new();
static GRABBED: AtomicBool = AtomicBool::new(false);
static CX: AtomicI32 = AtomicI32::new(0);
static CY: AtomicI32 = AtomicI32::new(0);
static HOOK_THREAD: AtomicU32 = AtomicU32::new(0);
/// GetTickCount() of the last hook callback. Windows silently removes
/// low-level hooks that are ever too slow; a watchdog reinstalls them.
static LAST_HOOK_TICK: AtomicU32 = AtomicU32::new(0);
/// Keys pressed since the grab began. A key-up for anything else was pressed
/// before crossing over, so we let it through to avoid a stuck local key.
static GRAB_KEYS: parking_lot::Mutex<Vec<u16>> = parking_lot::Mutex::new(Vec::new());

fn dpi_aware() {
    // Work in physical pixels so coordinates match across monitors.
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
}

pub fn virtual_screen() -> Rect {
    unsafe {
        Rect {
            x: GetSystemMetrics(SM_XVIRTUALSCREEN),
            y: GetSystemMetrics(SM_YVIRTUALSCREEN),
            w: GetSystemMetrics(SM_CXVIRTUALSCREEN).max(1),
            h: GetSystemMetrics(SM_CYVIRTUALSCREEN).max(1),
        }
    }
}

fn send(ev: InputEvent) {
    if let Some(tx) = TX.get() {
        let _ = tx.try_send(ev);
    }
}

unsafe extern "system" fn mouse_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    LAST_HOOK_TICK.store(GetTickCount(), Ordering::Relaxed);
    if code >= 0 {
        let info = &*(lparam.0 as *const MSLLHOOKSTRUCT);
        if info.flags & LLMHF_INJECTED == 0 {
            let msg = wparam.0 as u32;
            if GRABBED.load(Ordering::Relaxed) {
                let wheel = ((info.mouseData >> 16) as u16) as i16 as i32;
                let xbtn = if (info.mouseData >> 16) & 0xFFFF == 2 { MouseButton::Forward } else { MouseButton::Back };
                let ev = match msg {
                    M_MOUSEMOVE => {
                        let dx = info.pt.x - CX.load(Ordering::Relaxed);
                        let dy = info.pt.y - CY.load(Ordering::Relaxed);
                        (dx != 0 || dy != 0).then_some(InputEvent::Delta { dx, dy })
                    }
                    M_LDOWN | M_LUP => Some(InputEvent::Button { button: MouseButton::Left, down: msg == M_LDOWN }),
                    M_RDOWN | M_RUP => Some(InputEvent::Button { button: MouseButton::Right, down: msg == M_RDOWN }),
                    M_MDOWN | M_MUP => Some(InputEvent::Button { button: MouseButton::Middle, down: msg == M_MDOWN }),
                    M_XDOWN | M_XUP => Some(InputEvent::Button { button: xbtn, down: msg == M_XDOWN }),
                    M_WHEEL => Some(InputEvent::Wheel { dx: 0, dy: wheel }),
                    M_HWHEEL => Some(InputEvent::Wheel { dx: wheel, dy: 0 }),
                    _ => None,
                };
                if let Some(ev) = ev {
                    send(ev);
                }
                return LRESULT(1); // swallow: the cursor stays parked locally
            } else if msg == M_MOUSEMOVE {
                send(InputEvent::LocalMove { x: info.pt.x, y: info.pt.y });
            }
        }
    }
    CallNextHookEx(None, code, wparam, lparam)
}

unsafe extern "system" fn keyboard_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    LAST_HOOK_TICK.store(GetTickCount(), Ordering::Relaxed);
    if code >= 0 && GRABBED.load(Ordering::Relaxed) {
        let info = &*(lparam.0 as *const KBDLLHOOKSTRUCT);
        let flags = info.flags.0;
        if flags & LLKHF_INJECTED == 0 {
            let msg = wparam.0 as u32;
            let down = matches!(msg, M_KEYDOWN | M_SYSKEYDOWN);
            if down || matches!(msg, M_KEYUP | M_SYSKEYUP) {
                let scan = (info.scanCode & 0xFF) as u16;
                let native = if flags & LLKHF_EXTENDED != 0 { 0xE000 | scan } else { scan };
                // AltGr's fake Ctrl has scanCode 0x21D; it maps to nothing and is dropped.
                let hid = if info.scanCode > 0xFF {
                    None
                } else {
                    keys::hid_from_win(native).or_else(|| keys::hid_from_win(scan))
                };
                if let Some(key) = hid {
                    let mut pressed = GRAB_KEYS.lock();
                    if down {
                        if !pressed.contains(&key) {
                            pressed.push(key);
                        }
                    } else if let Some(i) = pressed.iter().position(|&k| k == key) {
                        pressed.remove(i);
                    } else {
                        drop(pressed);
                        return CallNextHookEx(None, code, wparam, lparam);
                    }
                    drop(pressed);
                    send(InputEvent::Key { key, down });
                }
            }
            return LRESULT(1);
        }
    }
    CallNextHookEx(None, code, wparam, lparam)
}

unsafe extern "system" fn blank_wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if msg == WM_SETCURSOR {
        SetCursor(None);
        return LRESULT(1);
    }
    DefWindowProcW(hwnd, msg, wparam, lparam)
}

/// A nearly invisible full-desktop window with no cursor. Showing it while
/// grabbed hides the parked cursor on the server's screen.
unsafe fn create_blank_window(hinst: HINSTANCE) -> Option<HWND> {
    let class = w!("OpenHopBlank");
    let wc = WNDCLASSW { lpfnWndProc: Some(blank_wndproc), hInstance: hinst, lpszClassName: class, ..Default::default() };
    RegisterClassW(&wc);
    CreateWindowExW(
        WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
        class,
        w!("OpenHop"),
        WS_POPUP,
        0,
        0,
        1,
        1,
        None,
        None,
        Some(hinst),
        None,
    )
    .ok()
    .inspect(|&h| {
        let _ = SetLayeredWindowAttributes(h, windows::Win32::Foundation::COLORREF(0), 1, LWA_ALPHA);
    })
}

pub struct WinCapture;

impl WinCapture {
    pub fn start(tx: Sender<InputEvent>) -> Result<Arc<WinCapture>> {
        dpi_aware();
        TX.set(tx).map_err(|_| anyhow!("capture already started"))?;
        let (ready_tx, ready_rx) = crossbeam_channel::bounded::<Result<(), String>>(1);
        std::thread::Builder::new().name("win-hooks".into()).spawn(move || unsafe {
            let hinst: HINSTANCE = GetModuleHandleW(None).map(|m| HINSTANCE(m.0)).unwrap_or_default();
            let mut mouse = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_proc), Some(hinst), 0);
            let mut kbd = SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_proc), Some(hinst), 0);
            if let (Err(e), _) | (_, Err(e)) = (&mouse, &kbd) {
                let _ = ready_tx.try_send(Err(format!("installing input hooks failed: {e}")));
                return;
            }
            LAST_HOOK_TICK.store(GetTickCount(), Ordering::Relaxed);
            // Watchdog: every 2 s, if there was user input the hooks never saw, reinstall them.
            let _ = SetTimer(None, 0, 2000, None);
            let blank = create_blank_window(hinst);
            HOOK_THREAD.store(GetCurrentThreadId(), Ordering::SeqCst);
            let _ = ready_tx.try_send(Ok(()));
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                match msg.message {
                    APP_GRAB => {
                        GRAB_KEYS.lock().clear();
                        let r = virtual_screen();
                        let (cx, cy) = r.center();
                        if let Some(h) = blank {
                            let _ = SetWindowPos(h, Some(HWND_TOPMOST), r.x, r.y, r.w, r.h, SWP_NOACTIVATE | SWP_SHOWWINDOW);
                        }
                        CX.store(cx, Ordering::Relaxed);
                        CY.store(cy, Ordering::Relaxed);
                        let _ = SetCursorPos(cx, cy);
                        GRABBED.store(true, Ordering::SeqCst);
                    }
                    APP_RELEASE => {
                        GRABBED.store(false, Ordering::SeqCst);
                        if let Some(h) = blank {
                            let _ = ShowWindow(h, SW_HIDE);
                        }
                        let _ = SetCursorPos(msg.wParam.0 as i32, msg.lParam.0 as i32);
                    }
                    WM_TIMER => {
                        let mut lii = LASTINPUTINFO { cbSize: std::mem::size_of::<LASTINPUTINFO>() as u32, dwTime: 0 };
                        if GetLastInputInfo(&mut lii).as_bool() {
                            let seen = LAST_HOOK_TICK.load(Ordering::Relaxed);
                            if lii.dwTime.wrapping_sub(seen) as i32 > 1500 {
                                log::warn!("input hooks went silent; reinstalling");
                                if let Ok(h) = &mouse {
                                    let _ = UnhookWindowsHookEx(*h);
                                }
                                if let Ok(h) = &kbd {
                                    let _ = UnhookWindowsHookEx(*h);
                                }
                                mouse = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_proc), Some(hinst), 0);
                                kbd = SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_proc), Some(hinst), 0);
                                LAST_HOOK_TICK.store(GetTickCount(), Ordering::Relaxed);
                            }
                        }
                    }
                    _ => {
                        let _ = TranslateMessage(&msg);
                        DispatchMessageW(&msg);
                    }
                }
            }
        })?;
        match ready_rx.recv() {
            Ok(Ok(())) => Ok(Arc::new(WinCapture)),
            Ok(Err(e)) => Err(anyhow!(e)),
            Err(_) => Err(anyhow!("hook thread exited")),
        }
    }

    fn post(&self, msg: u32, w: usize, l: isize) -> bool {
        let tid = HOOK_THREAD.load(Ordering::SeqCst);
        unsafe { PostThreadMessageW(tid, msg, WPARAM(w), LPARAM(l)).is_ok() }
    }
}

impl Capture for WinCapture {
    fn grab(&self) -> bool {
        self.post(APP_GRAB, 0, 0)
    }
    fn release(&self, x: i32, y: i32) {
        let _ = self.post(APP_RELEASE, x as isize as usize, y as isize);
    }
    fn screen(&self) -> Rect {
        virtual_screen()
    }
}

pub struct WinInjector;

impl WinInjector {
    pub fn new() -> Result<WinInjector> {
        dpi_aware();
        Ok(WinInjector)
    }
}

fn send_inputs(inputs: &[INPUT]) -> Result<()> {
    let n = unsafe { SendInput(inputs, std::mem::size_of::<INPUT>() as i32) };
    if n as usize != inputs.len() {
        // Usually UIPI: the focused window belongs to an elevated app.
        log::debug!("SendInput injected {n}/{} events", inputs.len());
    }
    Ok(())
}

fn mouse_input(dx: i32, dy: i32, data: i32, flags: u32) -> INPUT {
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT { dx, dy, mouseData: data as u32, dwFlags: MOUSE_EVENT_FLAGS(flags), time: 0, dwExtraInfo: 0 },
        },
    }
}

fn key_input(vk: u16, scan: u16, flags: u32) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT { wVk: VIRTUAL_KEY(vk), wScan: scan, dwFlags: KEYBD_EVENT_FLAGS(flags), time: 0, dwExtraInfo: 0 },
        },
    }
}

const MOUSEEVENTF_MOVE: u32 = 0x0001;
const MOUSEEVENTF_ABSOLUTE: u32 = 0x8000;
const MOUSEEVENTF_VIRTUALDESK: u32 = 0x4000;
const MOUSEEVENTF_WHEEL: u32 = 0x0800;
const MOUSEEVENTF_HWHEEL: u32 = 0x1000;
const KEYEVENTF_EXTENDEDKEY: u32 = 0x1;
const KEYEVENTF_KEYUP: u32 = 0x2;
const KEYEVENTF_SCANCODE: u32 = 0x8;

impl Injector for WinInjector {
    fn move_to(&mut self, x: i32, y: i32) -> Result<()> {
        let r = virtual_screen();
        let nx = (((x - r.x) as i64 * 65535 + (r.w as i64 - 1) / 2) / (r.w as i64 - 1).max(1)) as i32;
        let ny = (((y - r.y) as i64 * 65535 + (r.h as i64 - 1) / 2) / (r.h as i64 - 1).max(1)) as i32;
        send_inputs(&[mouse_input(nx, ny, 0, MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK)])
    }
    fn button(&mut self, button: MouseButton, down: bool) -> Result<()> {
        let (flags, data) = match (button, down) {
            (MouseButton::Left, true) => (0x0002, 0),
            (MouseButton::Left, false) => (0x0004, 0),
            (MouseButton::Right, true) => (0x0008, 0),
            (MouseButton::Right, false) => (0x0010, 0),
            (MouseButton::Middle, true) => (0x0020, 0),
            (MouseButton::Middle, false) => (0x0040, 0),
            (MouseButton::Back, true) => (0x0080, 1),
            (MouseButton::Back, false) => (0x0100, 1),
            (MouseButton::Forward, true) => (0x0080, 2),
            (MouseButton::Forward, false) => (0x0100, 2),
        };
        send_inputs(&[mouse_input(0, 0, data, flags)])
    }
    fn wheel(&mut self, dx: i32, dy: i32) -> Result<()> {
        let mut v = Vec::new();
        if dy != 0 {
            v.push(mouse_input(0, 0, dy, MOUSEEVENTF_WHEEL));
        }
        if dx != 0 {
            v.push(mouse_input(0, 0, dx, MOUSEEVENTF_HWHEEL));
        }
        if v.is_empty() {
            return Ok(());
        }
        send_inputs(&v)
    }
    fn key(&mut self, hid: u16, down: bool) -> Result<()> {
        let up = if down { 0 } else { KEYEVENTF_KEYUP };
        // Pause and Num Lock share scancode 0x45 and need virtual keys.
        let input = match hid {
            0x48 => key_input(0x13, 0, up),
            0x53 => key_input(0x90, 0, up | KEYEVENTF_EXTENDEDKEY),
            _ => {
                let Some(code) = keys::win_from_hid(hid) else { return Ok(()) };
                let ext = if code & 0xFF00 == 0xE000 { KEYEVENTF_EXTENDEDKEY } else { 0 };
                key_input(0, code & 0xFF, KEYEVENTF_SCANCODE | ext | up)
            }
        };
        send_inputs(&[input])
    }
    fn screen(&self) -> Rect {
        virtual_screen()
    }
}

#[allow(dead_code)]
fn cursor_pos() -> Option<(i32, i32)> {
    let mut p = POINT::default();
    unsafe { GetCursorPos(&mut p).ok().map(|_| (p.x, p.y)) }
}
