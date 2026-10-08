//! Windows windows.

use super::Picture;
use crate::protocol::{Rect, WinInfo};
use windows::core::BOOL;
use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED};
use windows::Win32::Graphics::Gdi::{
    ClientToScreen, CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC, ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER,
    DIB_RGB_COLORS,
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
        let Ok(p) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else { return String::new() };
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

/// The client area (what's captured and what input maps onto), on screen.
pub fn geometry(id: u64) -> Option<Rect> {
    unsafe {
        let h = hwnd(id);
        if !IsWindow(Some(h)).as_bool() {
            return None;
        }
        let mut r = RECT::default();
        GetClientRect(h, &mut r).ok()?;
        let mut p = POINT { x: 0, y: 0 };
        let _ = ClientToScreen(h, &mut p);
        Some(Rect { x: p.x, y: p.y, w: r.right - r.left, h: r.bottom - r.top })
    }
}

pub fn capture(id: u64) -> Option<Picture> {
    unsafe {
        let h = hwnd(id);
        if IsIconic(h).as_bool() {
            return None;
        }
        let g = geometry(id)?;
        let (w, hgt) = (g.w, g.h);
        if w <= 0 || hgt <= 0 {
            return None;
        }
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
                // PW_CLIENTONLY | PW_RENDERFULLCONTENT: works for covered windows
                // and for apps drawn with DirectX (browsers, Electron).
                let ok = PrintWindow(h, dc, PRINT_WINDOW_FLAGS(1 | 2)).as_bool();
                let px = std::slice::from_raw_parts(bits as *const u8, (w * hgt * 4) as usize);
                let rgba: Vec<u8> = px.chunks_exact(4).flat_map(|p| [p[2], p[1], p[0], 255]).collect();
                SelectObject(dc, old);
                let _ = DeleteObject(bmp.into());
                ok.then_some(Picture { w: w as u32, h: hgt as u32, rgba })
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
            Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: VIRTUAL_KEY(0x12), wScan: 0, dwFlags: KEYBD_EVENT_FLAGS(if up { 2 } else { 0 }), time: 0, dwExtraInfo: 0 } },
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
