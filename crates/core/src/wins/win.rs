//! Windows windows.

use super::Picture;
use crate::protocol::{Rect, WinInfo};
use windows::core::BOOL;
use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS};
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC, ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS,
};
use windows::Win32::Storage::Xps::{PrintWindow, PRINT_WINDOW_FLAGS};
use windows::Win32::System::Threading::{OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION};
use windows::Win32::UI::Input::KeyboardAndMouse::{SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS, VIRTUAL_KEY};
use windows::Win32::UI::WindowsAndMessaging::*;

fn hwnd(id: u64) -> HWND {
    HWND(id as usize as *mut _)
}

fn title(h: HWND) -> String {
    unsafe {
        let n = GetWindowTextLengthW(h);
        if n <= 0 {
            return String::new();
        }
        let mut buf = vec![0u16; n as usize + 1];
        let got = GetWindowTextW(h, &mut buf);
        String::from_utf16_lossy(&buf[..got.max(0) as usize])
    }
}

fn app_name(h: HWND) -> String {
    unsafe {
        let mut pid = 0u32;
        GetWindowThreadProcessId(h, Some(&mut pid));
        let Ok(p) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
            return String::new();
        };
        let mut buf = vec![0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(p, PROCESS_NAME_WIN32, windows::core::PWSTR(buf.as_mut_ptr()), &mut len).is_ok();
        let _ = CloseHandle(p);
        if !ok {
            return String::new();
        }
        let path = String::from_utf16_lossy(&buf[..len as usize]);
        std::path::Path::new(&path).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
    }
}

fn listed(h: HWND) -> bool {
    unsafe {
        if !IsWindowVisible(h).as_bool() || IsIconic(h).as_bool() {
            return false;
        }
        if GetWindow(h, GW_OWNER).map(|o| !o.is_invalid()).unwrap_or(false) {
            return false;
        }
        let ex = GetWindowLongW(h, GWL_EXSTYLE) as u32;
        if ex & WS_EX_TOOLWINDOW.0 != 0 {
            return false;
        }
        let mut cloaked = 0u32;
        let _ = DwmGetWindowAttribute(h, DWMWA_CLOAKED, &mut cloaked as *mut u32 as *mut _, 4);
        if cloaked != 0 {
            return false;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(h, Some(&mut pid));
        pid != std::process::id() && !title(h).is_empty()
    }
}

unsafe extern "system" fn collect(h: HWND, l: LPARAM) -> BOOL {
    let v = &mut *(l.0 as *mut Vec<HWND>);
    if listed(h) {
        v.push(h);
    }
    BOOL(1)
}

pub fn list() -> Vec<WinInfo> {
    let mut hs: Vec<HWND> = Vec::new();
    unsafe {
        let _ = EnumWindows(Some(collect), LPARAM(&mut hs as *mut Vec<HWND> as isize));
    }
    // EnumWindows goes front to back already.
    hs.into_iter()
        .filter_map(|h| {
            let r = geometry(h.0 as usize as u64)?;
            Some(WinInfo { id: h.0 as usize as u64, title: title(h), app: app_name(h), w: r.w, h: r.h })
        })
        .collect()
}

/// (whole window rectangle including invisible resize borders, the part you see).
fn rects(h: HWND) -> Option<(RECT, RECT)> {
    unsafe {
        if !IsWindow(Some(h)).as_bool() {
            return None;
        }
        let mut wr = RECT::default();
        GetWindowRect(h, &mut wr).ok()?;
        let mut vis = RECT::default();
        let ok = DwmGetWindowAttribute(h, DWMWA_EXTENDED_FRAME_BOUNDS, &mut vis as *mut RECT as *mut _, std::mem::size_of::<RECT>() as u32).is_ok();
        if !ok || vis.right <= vis.left || vis.bottom <= vis.top {
            vis = wr;
        }
        Some((wr, vis))
    }
}

/// The window as you see it (title bar included), on screen. That's what's
/// captured and what input maps onto.
pub fn geometry(id: u64) -> Option<Rect> {
    let (_, v) = rects(hwnd(id))?;
    Some(Rect { x: v.left, y: v.top, w: v.right - v.left, h: v.bottom - v.top })
}

/// The title bar's height: ask the window what's at each height down its middle.
pub fn bar_height(id: u64) -> i32 {
    let Some(g) = geometry(id) else { return 0 };
    let h = hwnd(id);
    let x = g.x + g.w / 2;
    let mut last = -1;
    let mut y = 1;
    while y < 120.min(g.h) {
        let (px, py) = (x, g.y + y);
        let lp = LPARAM((((py as i16) as u16 as isize) << 16) | ((px as i16) as u16 as isize));
        let mut hit = 0usize;
        unsafe {
            let _ = SendMessageTimeoutW(h, WM_NCHITTEST, WPARAM(0), lp, SMTO_ABORTIFHUNG, 30, Some(&mut hit));
        }
        // HTCAPTION, or the window's top resize border.
        if hit == 2 || hit == 12 {
            last = y;
        } else if last >= 0 {
            break;
        }
        y += 2;
    }
    if last >= 0 {
        last + 2
    } else {
        0
    }
}

pub fn raise(id: u64) {
    unsafe {
        let _ = SetWindowPos(hwnd(id), Some(HWND_TOP), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
    }
}

pub fn move_to(id: u64, x: i32, y: i32) {
    let h = hwnd(id);
    let Some((wr, v)) = rects(h) else { return };
    // The invisible resize border sits outside what you see.
    let (dx, dy) = (v.left - wr.left, v.top - wr.top);
    unsafe {
        let _ = SetWindowPos(h, None, x - dx, y - dy, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
    }
}

pub fn is_maximized(id: u64) -> bool {
    unsafe { IsZoomed(hwnd(id)).as_bool() }
}

pub fn unmaximize(id: u64) {
    unsafe {
        let _ = ShowWindow(hwnd(id), SW_SHOWNOACTIVATE);
        let _ = ShowWindow(hwnd(id), SW_RESTORE);
    }
}

pub fn begin_move(id: u64, _x: i32, _y: i32) {
    unsafe {
        // SC_MOVE | HTCAPTION: the window follows the mouse while the button is held.
        let h = hwnd(id);
        let _ = SetForegroundWindow(h);
        let _ = PostMessageW(Some(h), WM_SYSCOMMAND, WPARAM(0xF012), LPARAM(0));
    }
}

pub fn capture(id: u64) -> Option<Picture> {
    unsafe {
        let h = hwnd(id);
        if IsIconic(h).as_bool() {
            return None;
        }
        let (wr, v) = rects(h)?;
        // The whole window is drawn (title bar and invisible borders
        // included), then cut to the part you see.
        let (w, hgt) = (wr.right - wr.left, wr.bottom - wr.top);
        if w <= 0 || hgt <= 0 {
            return None;
        }
        let (cx, cy) = ((v.left - wr.left).clamp(0, w - 1), (v.top - wr.top).clamp(0, hgt - 1));
        let (cw, ch) = ((v.right - v.left).min(w - cx), (v.bottom - v.top).min(hgt - cy));
        let screen = GetDC(None);
        let dc = CreateCompatibleDC(Some(screen));
        let mut bi = BITMAPINFO::default();
        bi.bmiHeader = BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w,
            biHeight: -hgt, // top-down
            biPlanes: 1,
            biBitCount: 32,
            ..Default::default()
        };
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let bmp = CreateDIBSection(Some(dc), &bi, DIB_RGB_COLORS, &mut bits, None, 0);
        let out = match bmp {
            Ok(bmp) if !bits.is_null() => {
                let old = SelectObject(dc, bmp.into());
                // PW_RENDERFULLCONTENT: works for covered windows and for
                // apps drawn with DirectX (browsers, Electron).
                let ok = PrintWindow(h, dc, PRINT_WINDOW_FLAGS(2)).as_bool();
                let px = std::slice::from_raw_parts(bits as *const u8, (w * hgt * 4) as usize);
                let mut bgra: Vec<u8> = Vec::with_capacity((cw * ch * 4) as usize);
                for row in cy..cy + ch {
                    let start = ((row * w + cx) * 4) as usize;
                    bgra.extend_from_slice(&px[start..start + (cw * 4) as usize]);
                }
                SelectObject(dc, old);
                let _ = DeleteObject(bmp.into());
                ok.then_some(Picture { w: cw as u32, h: ch as u32, bgra })
            }
            _ => None,
        };
        let _ = DeleteDC(dc);
        ReleaseDC(None, screen);
        out
    }
}

pub fn activate(id: u64) {
    unsafe {
        let h = hwnd(id);
        if IsIconic(h).as_bool() {
            let _ = ShowWindow(h, SW_RESTORE);
        }
        // Windows only lets the app that last got input switch windows. A
        // tap of Alt counts as input, which allows the switch.
        let alt = |up: bool| INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT { wVk: VIRTUAL_KEY(0x12), wScan: 0, dwFlags: KEYBD_EVENT_FLAGS(if up { 2 } else { 0 }), time: 0, dwExtraInfo: 0 },
            },
        };
        SendInput(&[alt(false), alt(true)], std::mem::size_of::<INPUT>() as i32);
        let _ = SetForegroundWindow(h);
        let _ = BringWindowToTop(h);
    }
}

pub fn titlebar_window_at(x: i32, y: i32) -> Option<u64> {
    unsafe {
        let h = WindowFromPoint(POINT { x, y });
        if h.is_invalid() {
            return None;
        }
        let top = GetAncestor(h, GA_ROOT);
        if top.is_invalid() || !listed(top) {
            return None;
        }
        let lp = LPARAM(((y as u16 as isize) << 16) | (x as u16 as isize));
        let mut hit = 0usize;
        let _ = SendMessageTimeoutW(top, WM_NCHITTEST, WPARAM(0), lp, SMTO_ABORTIFHUNG, 50, Some(&mut hit));
        // HTCAPTION
        (hit == 2).then_some(top.0 as usize as u64)
    }
}

const HIDDEN_PROP: windows::core::PCWSTR = windows::core::w!("OpenHopHidden");

pub fn set_hidden(id: u64, hidden: bool) {
    use windows::Win32::Foundation::{COLORREF, HANDLE};
    unsafe {
        let h = hwnd(id);
        if !IsWindow(Some(h)).as_bool() {
            return;
        }
        let saved = GetPropW(h, HIDDEN_PROP);
        if hidden {
            if saved.is_invalid() || saved.0.is_null() {
                let ex = GetWindowLongW(h, GWL_EXSTYLE);
                // Remember the original style (+1 so it's never zero).
                let _ = SetPropW(h, HIDDEN_PROP, Some(HANDLE((ex as u32 as usize + 1) as *mut _)));
                SetWindowLongW(h, GWL_EXSTYLE, ex | WS_EX_LAYERED.0 as i32);
            }
            // Nearly transparent, but still a normal window that takes input.
            let _ = SetLayeredWindowAttributes(h, COLORREF(0), 1, LWA_ALPHA);
        } else if !saved.0.is_null() {
            let ex = (saved.0 as usize - 1) as u32 as i32;
            if ex & WS_EX_LAYERED.0 as i32 != 0 {
                let _ = SetLayeredWindowAttributes(h, COLORREF(0), 255, LWA_ALPHA);
            }
            SetWindowLongW(h, GWL_EXSTYLE, ex);
            let _ = RemovePropW(h, HIDDEN_PROP);
            let _ = SetWindowPos(h, None, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED);
        }
    }
}

pub fn lower(id: u64) {
    unsafe {
        let _ = SetWindowPos(hwnd(id), Some(HWND_BOTTOM), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
    }
}

pub fn resize(id: u64, w: i32, h: i32) {
    unsafe {
        let hw = hwnd(id);
        let Some((wr, v)) = rects(hw) else { return };
        // `w`×`h` is what you see; add the invisible resize borders.
        let dw = (wr.right - wr.left) - (v.right - v.left);
        let dh = (wr.bottom - wr.top) - (v.bottom - v.top);
        let _ = SetWindowPos(hw, None, 0, 0, w + dw, h + dh, SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE);
    }
}

unsafe extern "system" fn restore_one(h: HWND, _: LPARAM) -> BOOL {
    if !GetPropW(h, HIDDEN_PROP).0.is_null() {
        set_hidden(h.0 as usize as u64, false);
    }
    BOOL(1)
}

pub fn restore_all() {
    unsafe {
        let _ = EnumWindows(Some(restore_one), LPARAM(0));
    }
}
