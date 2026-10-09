//! The windows open on this computer: listing them, taking pictures of
//! them (for live windows on other computers), and bringing one forward.

use crate::protocol::{Rect, WinInfo};

#[cfg(target_os = "linux")]
pub mod gnome;
#[cfg(target_os = "linux")]
mod x11;
#[cfg(target_os = "linux")]
use x11 as imp;
#[cfg(windows)]
mod win;
#[cfg(windows)]
use win as imp;
#[cfg(target_os = "macos")]
mod mac;
#[cfg(target_os = "macos")]
use mac as imp;

#[cfg(not(any(target_os = "linux", windows, target_os = "macos")))]
mod imp {
    use super::*;
    pub fn list() -> Vec<WinInfo> {
        vec![]
    }
    pub fn taskbar_pids() -> Vec<u32> {
        vec![]
    }
    pub fn capture(_: u64) -> Option<Picture> {
        None
    }
    pub fn geometry(_: u64) -> Option<Rect> {
        None
    }
    pub fn activate(_: u64) {}
    pub fn titlebar_window_at(_: i32, _: i32) -> Option<u64> {
        None
    }
    pub fn set_hidden(_: u64, _: bool) {}
    pub fn lower(_: u64) {}
    pub fn resize(_: u64, _: i32, _: i32) {}
    pub fn restore_all() {}
    pub fn bar_height(_: u64) -> i32 {
        0
    }
    pub fn raise(_: u64) {}
    pub fn move_to(_: u64, _: i32, _: i32) {}
    pub fn begin_move(_: u64, _: i32, _: i32) {}
    pub fn is_maximized(_: u64) -> bool {
        false
    }
    pub fn unmaximize(_: u64) {}
}

/// A picture of a window: BGRA (what every OS hands out), top row first.
pub struct Picture {
    pub w: u32,
    pub h: u32,
    pub bgra: Vec<u8>,
}

/// GNOME on Wayland with OpenHop's helper running: windows come from it
/// (Wayland doesn't let apps see other apps' windows).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn via_gnome() -> bool {
    #[cfg(target_os = "linux")]
    {
        return crate::platform::linux_is_wayland() && gnome::available();
    }
    #[allow(unreachable_code)]
    false
}

/// Windows here can be shown live elsewhere (not on Wayland).
pub fn can_stream() -> bool {
    #[cfg(target_os = "linux")]
    {
        return !crate::platform::linux_is_wayland();
    }
    #[allow(unreachable_code)]
    true
}

/// Processes that have a button in the taskbar or dock (minimized windows
/// count).
pub fn taskbar_pids() -> Vec<u32> {
    #[cfg(target_os = "linux")]
    if via_gnome() {
        let mut v = gnome::taskbar_pids();
        v.extend(imp::taskbar_pids());
        return v;
    }
    imp::taskbar_pids()
}

/// Normal application windows, front-most first where the OS says.
pub fn list() -> Vec<WinInfo> {
    #[cfg(target_os = "linux")]
    let mut v = if via_gnome() { gnome::list() } else { imp::list() };
    #[cfg(not(target_os = "linux"))]
    let mut v = imp::list();
    v.retain(|w| w.w >= 40 && w.h >= 40);
    v
}

/// "Window" number for the whole screen (fits in a JavaScript number, and is
/// no real window's).
pub const SCREEN: u64 = (1 << 52) - 2;

/// The whole desktop's rectangle (every monitor).
pub fn screen_rect() -> Option<Rect> {
    let m = crate::platform::monitors();
    if m.is_empty() {
        return None;
    }
    let (x0, y0) = (m.iter().map(|r| r.x).min()?, m.iter().map(|r| r.y).min()?);
    let (x1, y1) = (m.iter().map(|r| r.x + r.w).max()?, m.iter().map(|r| r.y + r.h).max()?);
    Some(Rect { x: x0, y: y0, w: x1 - x0, h: y1 - y0 })
}

/// A picture of the window's contents (even if other windows cover it, where
/// the OS allows). None if it's gone or minimized.
pub fn capture(id: u64) -> Option<Picture> {
    if id == SCREEN {
        #[cfg(any(target_os = "linux", windows, target_os = "macos"))]
        return imp::capture_screen().filter(|p| p.w > 0 && p.h > 0 && p.bgra.len() == (p.w * p.h * 4) as usize);
        #[cfg(not(any(target_os = "linux", windows, target_os = "macos")))]
        return None;
    }
    imp::capture(id).filter(|p| p.w > 0 && p.h > 0 && p.bgra.len() == (p.w * p.h * 4) as usize)
}

/// Where the window is on screen, title bar included (native coordinates).
/// That's also what's captured.
pub fn geometry(id: u64) -> Option<Rect> {
    if id == SCREEN {
        return screen_rect();
    }
    imp::geometry(id)
}

/// Bring the window to the front and give it the keyboard.
pub fn activate(id: u64) {
    if id == SCREEN {
        return;
    }
    imp::activate(id)
}

/// The window whose title bar is at (x, y), if any: with the button held
/// there, that window is being dragged.
pub fn titlebar_window_at(x: i32, y: i32) -> Option<u64> {
    imp::titlebar_window_at(x, y)
}

/// Hide the window on this screen (it's open live on another computer) or
/// show it again. It stays where it is, so it can still be controlled.
pub fn set_hidden(id: u64, hidden: bool) {
    if id == SCREEN {
        return;
    }
    imp::set_hidden(id, hidden)
}

/// Put a hidden window behind the others, so it can't catch clicks here.
pub fn lower(id: u64) {
    if id == SCREEN {
        return;
    }
    imp::lower(id)
}

/// Resize the window's content area (from a live view being resized).
pub fn resize(id: u64, w: i32, h: i32) {
    if id != SCREEN && w >= 80 && h >= 60 {
        imp::resize(id, w, h)
    }
}

/// Show any windows a previous run left hidden (e.g. after a crash).
pub fn restore_all() {
    imp::restore_all()
}

/// Height of the window's title bar (pixels at the top of its picture),
/// 0 if unknown.
pub fn bar_height(id: u64) -> i32 {
    if id == SCREEN {
        return 0;
    }
    imp::bar_height(id)
}

/// Put the window above the others (without moving the keyboard focus).
pub fn raise(id: u64) {
    if id == SCREEN {
        return;
    }
    #[cfg(target_os = "linux")]
    if via_gnome() {
        return gnome::raise(id);
    }
    imp::raise(id)
}

/// Put the window's top-left corner (title bar included) at (x, y).
pub fn move_to(id: u64, x: i32, y: i32) {
    imp::move_to(id, x, y)
}

/// Maximized (filling its screen)?
pub fn is_maximized(id: u64) -> bool {
    if id == SCREEN {
        return false;
    }
    imp::is_maximized(id)
}

/// Back to its size from before it was maximized.
pub fn unmaximize(id: u64) {
    if id == SCREEN {
        return;
    }
    imp::unmaximize(id)
}

/// The left button is held over the window's title bar at (x, y): the
/// window follows the mouse from now on, as if picked up there.
pub fn begin_move(id: u64, x: i32, y: i32) {
    imp::begin_move(id, x, y)
}
