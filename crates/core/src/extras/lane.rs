//! A lane of its own for the island: a strip across the top of the screen
//! that maximized windows leave free, like a second, thin menu bar.
//!
//! Linux (X11 and XWayland): a dock window with a strut. Windows: an app bar.
//! macOS doesn't need one (the island lives in the menu bar).

/// Reserve `height` pixels at `top` (screen pixels) across `x .. x + width`
/// for `window` (Windows: its HWND; Linux: found by `title` and our pid).
/// Returns false when this system can't.
pub fn reserve(window: isize, title: &str, x: i32, top: i32, width: i32, height: i32) -> bool {
    let ok = imp::reserve(window, title, x, top, width, height);
    log::info!("island lane {}x{} at {x},{top}: {}", width, height, if ok { "reserved" } else { "not available" });
    ok
}

/// Whether the system now keeps the strip free (the work area starts below it).
pub fn kept(top: i32, height: i32) -> bool {
    imp::work_top().map(|t| t >= top + height - 2).unwrap_or(false)
}

/// Give the strip back.
pub fn release(window: isize, title: &str) {
    imp::release(window, title);
}

#[cfg(target_os = "linux")]
mod imp {
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{AtomEnum, ConnectionExt as _, PropMode, Window};
    use x11rb::wrapper::ConnectionExt as _;

    fn atom(c: &impl Connection, name: &str) -> Option<u32> {
        Some(c.intern_atom(false, name.as_bytes()).ok()?.reply().ok()?.atom)
    }

    /// Our shown window with this title and width (top-level, or one level
    /// down under a frame; GTK also keeps hidden helper windows).
    fn find(c: &impl Connection, root: Window, title: &str, width: i32) -> Option<Window> {
        let pid_atom = atom(c, "_NET_WM_PID")?;
        let me = std::process::id();
        let mut stack = vec![(root, 0)];
        while let Some((w, depth)) = stack.pop() {
            let tree = c.query_tree(w).ok()?.reply().ok()?;
            for &ch in &tree.children {
                let name = c
                    .get_property(false, ch, AtomEnum::WM_NAME, AtomEnum::ANY, 0, 256)
                    .ok()
                    .and_then(|r| r.reply().ok())
                    .map(|r| String::from_utf8_lossy(&r.value).into_owned());
                let pid = c
                    .get_property(false, ch, pid_atom, AtomEnum::CARDINAL, 0, 1)
                    .ok()
                    .and_then(|r| r.reply().ok())
                    .and_then(|r| r.value32().and_then(|mut v| v.next()));
                if name.as_deref() == Some(title) && pid == Some(me) {
                    let shown = c
                        .get_window_attributes(ch)
                        .ok()
                        .and_then(|r| r.reply().ok())
                        .map(|a| a.map_state == x11rb::protocol::xproto::MapState::VIEWABLE)
                        .unwrap_or(false);
                    let wide = c.get_geometry(ch).ok().and_then(|r| r.reply().ok()).map(|g| (g.width as i32 - width).abs() <= 4).unwrap_or(false);
                    if shown && (wide || width <= 0) {
                        return Some(ch);
                    }
                }
                if depth < 2 {
                    stack.push((ch, depth + 1));
                }
            }
        }
        None
    }

    #[cfg(test)]
    pub fn reserve_any(title: &str, x: i32, top: i32, width: i32, height: i32) -> String {
        let Ok((c, n)) = x11rb::connect(None) else { return "no X".into() };
        let root = c.setup().roots[n].root;
        match find_any(&c, root, title) {
            Some(w) => format!("{w:#x}: {}", set(&c, w, x, top, width, height)),
            None => "not found".into(),
        }
    }

    #[cfg(test)]
    fn find_any(c: &impl Connection, root: Window, title: &str) -> Option<Window> {
        let mut stack = vec![(root, 0)];
        while let Some((w, depth)) = stack.pop() {
            for &ch in &c.query_tree(w).ok()?.reply().ok()?.children {
                let name = c
                    .get_property(false, ch, AtomEnum::WM_NAME, AtomEnum::ANY, 0, 256)
                    .ok()
                    .and_then(|r| r.reply().ok())
                    .map(|r| String::from_utf8_lossy(&r.value).into_owned());
                if name.as_deref() == Some(title) {
                    return Some(ch);
                }
                if depth < 2 {
                    stack.push((ch, depth + 1));
                }
            }
        }
        None
    }

    pub fn reserve(_: isize, title: &str, x: i32, top: i32, width: i32, height: i32) -> bool {
        let Ok((c, n)) = x11rb::connect(None) else { return false };
        let root = c.setup().roots[n].root;
        let Some(w) = find(&c, root, title, width) else { return false };
        log::info!("lane window {w:#x}");
        set(&c, w, x, top, width, height)
    }

    fn set(c: &impl Connection, w: Window, x: i32, top: i32, width: i32, height: i32) -> bool {
        let c = c;
        let (Some(ty), Some(dock), Some(strut), Some(partial)) =
            (atom(c, "_NET_WM_WINDOW_TYPE"), atom(c, "_NET_WM_WINDOW_TYPE_DOCK"), atom(c, "_NET_WM_STRUT"), atom(c, "_NET_WM_STRUT_PARTIAL"))
        else {
            return false;
        };
        let bottom = (top + height).max(0) as u32;
        let (x0, x1) = (x.max(0) as u32, (x + width - 1).max(0) as u32);
        // (The app makes it a dock window before showing it: window managers
        // read the type then. Setting it again here doesn't hurt.)
        let _ = c.change_property32(PropMode::REPLACE, w, ty, AtomEnum::ATOM, &[dock]);
        let _ = c.change_property32(PropMode::REPLACE, w, strut, AtomEnum::CARDINAL, &[0, 0, bottom, 0]);
        let _ = c.change_property32(PropMode::REPLACE, w, partial, AtomEnum::CARDINAL, &[0, 0, bottom, 0, 0, 0, 0, 0, x0, x1, 0, 0]);
        let _ = c.configure_window(w, &x11rb::protocol::xproto::ConfigureWindowAux::new().x(x).y(top).width(width as u32).height(height as u32));
        c.flush().is_ok()
    }

    pub fn work_top() -> Option<i32> {
        let (c, n) = x11rb::connect(None).ok()?;
        let root = c.setup().roots[n].root;
        let a = atom(&c, "_NET_WORKAREA")?;
        let r = c.get_property(false, root, a, AtomEnum::CARDINAL, 0, 4).ok()?.reply().ok()?;
        let v: Vec<u32> = r.value32()?.collect();
        v.get(1).map(|y| *y as i32)
    }

    pub fn release(_: isize, title: &str) {
        let Ok((c, n)) = x11rb::connect(None) else { return };
        let root = c.setup().roots[n].root;
        if let Some(w) = find(&c, root, title, 0) {
            for name in ["_NET_WM_STRUT", "_NET_WM_STRUT_PARTIAL"] {
                if let Some(a) = atom(&c, name) {
                    let _ = c.delete_property(w, a);
                }
            }
            let _ = c.flush();
        }
    }
}

#[cfg(windows)]
mod imp {
    use windows::Win32::Foundation::{HWND, RECT};
    use windows::Win32::UI::Shell::{SHAppBarMessage, ABE_TOP, ABM_NEW, ABM_QUERYPOS, ABM_REMOVE, ABM_SETPOS, APPBARDATA};
    use windows::Win32::UI::WindowsAndMessaging::{MoveWindow, WM_USER};

    fn data(window: isize) -> APPBARDATA {
        APPBARDATA { cbSize: std::mem::size_of::<APPBARDATA>() as u32, hWnd: HWND(window as *mut _), uCallbackMessage: WM_USER + 77, ..Default::default() }
    }

    pub fn reserve(window: isize, _: &str, x: i32, top: i32, width: i32, height: i32) -> bool {
        unsafe {
            let mut d = data(window);
            if SHAppBarMessage(ABM_NEW, &mut d) == 0 {
                return false;
            }
            d.uEdge = ABE_TOP;
            d.rc = RECT { left: x, top, right: x + width, bottom: top + height };
            SHAppBarMessage(ABM_QUERYPOS, &mut d);
            // The shell may move it below another bar: keep our height.
            d.rc.bottom = d.rc.top + height;
            SHAppBarMessage(ABM_SETPOS, &mut d);
            let _ = MoveWindow(HWND(window as *mut _), d.rc.left, d.rc.top, d.rc.right - d.rc.left, height, true);
        }
        true
    }

    pub fn work_top() -> Option<i32> {
        use windows::Win32::UI::WindowsAndMessaging::{SystemParametersInfoW, SPI_GETWORKAREA, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS};
        let mut rc = RECT::default();
        unsafe { SystemParametersInfoW(SPI_GETWORKAREA, 0, Some(&mut rc as *mut RECT as *mut _), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0)).ok()? };
        Some(rc.top)
    }

    pub fn release(window: isize, _: &str) {
        unsafe {
            let mut d = data(window);
            SHAppBarMessage(ABM_REMOVE, &mut d);
        }
    }
}

#[cfg(not(any(target_os = "linux", windows)))]
mod imp {
    pub fn reserve(_: isize, _: &str, _: i32, _: i32, _: i32, _: i32) -> bool {
        false
    }
    pub fn release(_: isize, _: &str) {}
    pub fn work_top() -> Option<i32> {
        None
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    /// Manual check against a live X server: OPENHOP_LANE_TEST=1 with a
    /// window titled "OpenHop lane" from this process... (skipped normally).
    #[test]
    fn reserve_live() {
        if std::env::var_os("OPENHOP_LANE_TEST").is_none() {
            return;
        }
        let title = std::env::var("OPENHOP_LANE_TITLE").unwrap();
        println!("{}", super::imp::reserve_any(&title, 0, 0, 1920, 34));
    }
}
