//! Per-OS input capture (server side) and input injection (client side).

use crate::protocol::{MouseButton, Rect};
use anyhow::Result;
use crossbeam_channel::Sender;
use std::sync::Arc;

#[cfg(target_os = "linux")]
mod linux_uinput;
#[cfg(target_os = "linux")]
mod linux_x11;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod windows;

/// Events produced by a capture backend.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum InputEvent {
    /// Cursor moved on the local screen (only while not grabbed). Native coordinates.
    LocalMove { x: i32, y: i32 },
    /// Relative motion while grabbed (cursor is on a remote screen).
    Delta { dx: i32, dy: i32 },
    /// Only while grabbed.
    Button { button: MouseButton, down: bool },
    /// Only while grabbed. 1/120 notch units, positive = up/right.
    Wheel { dx: i32, dy: i32 },
    /// Only while grabbed. HID usage id.
    Key { key: u16, down: bool },
}

/// Server-side capture of the physical mouse and keyboard.
pub trait Capture: Send + Sync {
    /// Start swallowing local input and reporting it as events; hide the cursor.
    fn grab(&self);
    /// Stop swallowing input, show the cursor and put it at (x, y) (native coords).
    fn release(&self, x: i32, y: i32);
    /// Current local desktop bounds.
    fn screen(&self) -> Rect;
}

/// Client-side synthetic input.
pub trait Injector: Send {
    /// Absolute position in native desktop coordinates.
    fn move_to(&mut self, x: i32, y: i32) -> Result<()>;
    fn button(&mut self, button: MouseButton, down: bool) -> Result<()>;
    fn wheel(&mut self, dx: i32, dy: i32) -> Result<()>;
    fn key(&mut self, hid: u16, down: bool) -> Result<()>;
    fn screen(&self) -> Rect;
}

/// Ask for OS permissions where needed (macOS Accessibility). Returns a
/// human-readable problem if input control isn't allowed yet.
pub fn check_permissions(prompt: bool) -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        return macos::check_permissions(prompt);
    }
    #[allow(unreachable_code)]
    {
        let _ = prompt;
        None
    }
}

pub fn start_capture(tx: Sender<InputEvent>, override_screen: Option<Rect>) -> Result<Arc<dyn Capture>> {
    #[cfg(target_os = "linux")]
    {
        if linux_is_wayland() && std::env::var_os("DISPLAY").is_none() {
            anyhow::bail!(
                "This Linux computer is running Wayland without X11 support, so it can't share its \
                 mouse and keyboard yet. Make it a client, or log in with an X11 (Xorg) session."
            );
        }
        if linux_is_wayland() {
            log::warn!(
                "Wayland session detected: capturing through XWayland only sees the cursor over X11 \
                 windows. For reliable sharing use an Xorg session or make this computer a client."
            );
        }
        return Ok(Arc::new(linux_x11::X11Capture::start(tx, override_screen)?));
    }
    #[cfg(windows)]
    {
        let _ = override_screen;
        return Ok(windows::WinCapture::start(tx)?);
    }
    #[cfg(target_os = "macos")]
    {
        let _ = override_screen;
        return Ok(macos::MacCapture::start(tx)?);
    }
    #[allow(unreachable_code)]
    {
        let _ = (tx, override_screen);
        anyhow::bail!("unsupported platform")
    }
}

pub fn create_injector(override_screen: Option<Rect>, linux_backend: &str) -> Result<Box<dyn Injector>> {
    #[cfg(target_os = "linux")]
    {
        let use_uinput = match linux_backend {
            "uinput" => true,
            "x11" => false,
            _ => linux_is_wayland() || std::env::var_os("DISPLAY").is_none(),
        };
        if use_uinput {
            let screen = override_screen.or_else(linux_x11::root_rect).unwrap_or(Rect { x: 0, y: 0, w: 1920, h: 1080 });
            return Ok(Box::new(linux_uinput::UinputInjector::new(screen)?));
        }
        return Ok(Box::new(linux_x11::X11Injector::new(override_screen)?));
    }
    #[cfg(windows)]
    {
        let _ = (override_screen, linux_backend);
        return Ok(Box::new(windows::WinInjector::new()?));
    }
    #[cfg(target_os = "macos")]
    {
        let _ = (override_screen, linux_backend);
        return Ok(Box::new(macos::MacInjector::new()?));
    }
    #[allow(unreachable_code)]
    {
        let _ = (override_screen, linux_backend);
        anyhow::bail!("unsupported platform")
    }
}

#[cfg(target_os = "linux")]
pub fn linux_is_wayland() -> bool {
    std::env::var("XDG_SESSION_TYPE").map(|s| s == "wayland").unwrap_or(false)
        || std::env::var_os("WAYLAND_DISPLAY").is_some()
}

/// Accumulates fractional wheel movement into whole notches for platforms
/// that only understand notches.
#[derive(Default)]
pub struct WheelAccum {
    x: i32,
    y: i32,
}

impl WheelAccum {
    /// Returns whole notches (x, y) to emit.
    pub fn add(&mut self, dx: i32, dy: i32) -> (i32, i32) {
        self.x += dx;
        self.y += dy;
        let nx = self.x / 120;
        let ny = self.y / 120;
        self.x -= nx * 120;
        self.y -= ny * 120;
        (nx, ny)
    }
}
