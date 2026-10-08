//! The windows open on this computer: listing them, taking pictures of
//! them (for live windows on other computers), and bringing one forward.

use crate::protocol::{Rect, WinInfo};

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

/// Normal application windows, front-most first where the OS says.
pub fn list() -> Vec<WinInfo> {
    let mut v = imp::list();
    v.retain(|w| w.w >= 40 && w.h >= 40);
    v
}

/// A picture of the window's contents (even if other windows cover it, where
/// the OS allows). None if it's gone or minimized.
pub fn capture(id: u64) -> Option<Picture> {
    imp::capture(id).filter(|p| p.w > 0 && p.h > 0 && p.bgra.len() == (p.w * p.h * 4) as usize)
}

/// Where the window is on screen, title bar included (native coordinates).
/// That's also what's captured.
pub fn geometry(id: u64) -> Option<Rect> {
    imp::geometry(id)
}

/// Bring the window to the front and give it the keyboard.
pub fn activate(id: u64) {
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
    imp::set_hidden(id, hidden)
}

/// Put a hidden window behind the others, so it can't catch clicks here.
pub fn lower(id: u64) {
    imp::lower(id)
}

/// Resize the window's content area (from a live view being resized).
pub fn resize(id: u64, w: i32, h: i32) {
    if w >= 80 && h >= 60 {
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
    imp::bar_height(id)
}

/// Put the window above the others (without moving the keyboard focus).
pub fn raise(id: u64) {
    imp::raise(id)
}

/// Put the window's top-left corner (title bar included) at (x, y).
pub fn move_to(id: u64, x: i32, y: i32) {
    imp::move_to(id, x, y)
}

/// Maximized (filling its screen)?
pub fn is_maximized(id: u64) -> bool {
    imp::is_maximized(id)
}

/// Back to its size from before it was maximized.
pub fn unmaximize(id: u64) {
    imp::unmaximize(id)
}

/// The left button is held over the window's title bar at (x, y): the
/// window follows the mouse from now on, as if picked up there.
pub fn begin_move(id: u64, x: i32, y: i32) {
    imp::begin_move(id, x, y)
}
