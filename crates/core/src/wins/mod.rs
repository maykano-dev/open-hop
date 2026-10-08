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
}

/// A picture of a window, RGBA, top row first.
pub struct Picture {
    pub w: u32,
    pub h: u32,
    pub rgba: Vec<u8>,
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
    imp::capture(id).filter(|p| p.w > 0 && p.h > 0 && p.rgba.len() == (p.w * p.h * 4) as usize)
}

/// Where the captured area is on screen (native coordinates).
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
