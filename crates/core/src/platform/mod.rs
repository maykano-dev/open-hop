//! Per-OS input capture (server side) and input injection (client side).

use crate::protocol::{MouseButton, Rect};
use anyhow::Result;
use crossbeam_channel::Sender;
use std::sync::Arc;

pub mod dnd;
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
    /// Returns false if the input couldn't be captured (then nothing changed).
    fn grab(&self) -> bool;
    /// Like `grab`, while a file drag is being carried off this screen (the
    /// drag source may be holding the pointer).
    fn grab_carrying_drag(&self) -> bool {
        self.grab()
    }
    /// Stop swallowing input, show the cursor and put it at (x, y) (native coords).
    fn release(&self, x: i32, y: i32);
    /// Let OpenHop's own synthetic input through for a moment (used to
    /// control a local window from a live view on another computer while the
    /// pointer is over there). `pointer`: mouse input is coming (else just
    /// keys). Returns whether it was capturing.
    fn suspend(&self, _pointer: bool) -> bool {
        false
    }
    /// Undo [`Capture::suspend`].
    fn resume(&self) {}
    /// While the pointer is on another computer, let the keyboard type
    /// straight into this computer's focused window (a live window of it is
    /// focused over there). Only needed where our own input can't get past
    /// the capture.
    fn keyboard_passthrough(&self, _on: bool) {}
    /// Our own injected mouse button can't be held down on its own while
    /// the mouse is captured: inject whole clicks (see `Hub::set_injector`).
    fn whole_clicks(&self) -> bool {
        false
    }
    /// Hide this computer's mouse pointer while the shared pointer is on
    /// another screen (so there's only ever one pointer). It comes back by
    /// itself as soon as this computer's own mouse or touchpad moves.
    fn idle_cursor(&self, _hidden: bool) {}
    /// Current local desktop bounds.
    fn screen(&self) -> Rect;
}

/// One step of synthetic input.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum InjectOp {
    MoveTo(i32, i32),
    Button(MouseButton, bool),
    Wheel(i32, i32),
    Key(u16, bool),
}

pub fn apply(inj: &mut dyn Injector, op: InjectOp) -> Result<()> {
    match op {
        InjectOp::MoveTo(x, y) => inj.move_to(x, y),
        InjectOp::Button(b, d) => inj.button(b, d),
        InjectOp::Wheel(dx, dy) => inj.wheel(dx, dy),
        InjectOp::Key(k, d) => inj.key(k, d),
    }
}

/// This computer's monitors (native desktop coordinates), refreshed every
/// few seconds. Empty when the system can't say.
pub fn monitors() -> Vec<Rect> {
    use parking_lot::Mutex;
    use std::time::{Duration, Instant};
    static CACHE: Mutex<Option<(Instant, Vec<Rect>)>> = Mutex::new(None);
    let mut c = CACHE.lock();
    if let Some((at, v)) = c.as_ref() {
        if at.elapsed() < Duration::from_secs(3) {
            return v.clone();
        }
    }
    let v = monitors_now();
    *c = Some((Instant::now(), v.clone()));
    v
}

fn monitors_now() -> Vec<Rect> {
    #[cfg(target_os = "linux")]
    {
        use x11rb::connection::Connection;
        use x11rb::protocol::randr::ConnectionExt as _;
        let Ok((conn, n)) = x11rb::connect(None) else { return vec![] };
        let root = conn.setup().roots[n].root;
        let Some(r) = conn.randr_get_monitors(root, true).ok().and_then(|c| c.reply().ok()) else { return vec![] };
        return r.monitors.iter().map(|m| Rect { x: m.x as i32, y: m.y as i32, w: m.width as i32, h: m.height as i32 }).collect();
    }
    #[cfg(windows)]
    {
        // (`::windows`: this module has a `windows` of its own.)
        use ::windows::core::BOOL;
        use ::windows::Win32::Foundation::{LPARAM, RECT};
        use ::windows::Win32::Graphics::Gdi::{EnumDisplayMonitors, HDC, HMONITOR};
        unsafe extern "system" fn each(_: HMONITOR, _: HDC, r: *mut RECT, data: LPARAM) -> BOOL {
            let v = &mut *(data.0 as *mut Vec<Rect>);
            let r = &*r;
            v.push(Rect { x: r.left, y: r.top, w: r.right - r.left, h: r.bottom - r.top });
            BOOL(1)
        }
        let mut v: Vec<Rect> = vec![];
        unsafe {
            let _ = EnumDisplayMonitors(None, None, Some(each), LPARAM(&mut v as *mut Vec<Rect> as isize));
        }
        return v;
    }
    #[cfg(target_os = "macos")]
    {
        use core_graphics::display::CGDisplay;
        return CGDisplay::active_displays()
            .unwrap_or_default()
            .into_iter()
            .map(|id| {
                let b = CGDisplay::new(id).bounds();
                Rect { x: b.origin.x as i32, y: b.origin.y as i32, w: b.size.width as i32, h: b.size.height as i32 }
            })
            .collect();
    }
    #[allow(unreachable_code)]
    Vec::new()
}

/// Where the pointer is now (native coordinates).
pub fn cursor_pos() -> Option<(i32, i32)> {
    #[cfg(target_os = "linux")]
    {
        use x11rb::connection::Connection;
        use x11rb::protocol::xproto::ConnectionExt as _;
        let (conn, n) = x11rb::connect(None).ok()?;
        let root = conn.setup().roots[n].root;
        let p = conn.query_pointer(root).ok()?.reply().ok()?;
        return Some((p.root_x as i32, p.root_y as i32));
    }
    #[cfg(windows)]
    {
        let mut p = ::windows::Win32::Foundation::POINT::default();
        unsafe { ::windows::Win32::UI::WindowsAndMessaging::GetCursorPos(&mut p).ok()? };
        return Some((p.x, p.y));
    }
    #[cfg(target_os = "macos")]
    {
        use core_graphics::event::CGEvent;
        use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
        let src = CGEventSource::new(CGEventSourceStateID::CombinedSessionState).ok()?;
        let p = CGEvent::new(src).ok()?.location();
        return Some((p.x.round() as i32, p.y.round() as i32));
    }
    #[allow(unreachable_code)]
    None
}

/// Reads the pointer position and whether the main button is held, many
/// times a second (keeps its connection open).
#[derive(Default)]
pub struct PointerProbe {
    #[cfg(target_os = "linux")]
    x: Option<(x11rb::rust_connection::RustConnection, u32)>,
}

impl PointerProbe {
    pub fn new() -> Self {
        Self::default()
    }

    /// (x, y, main button down). macOS: in points, not pixels.
    pub fn read(&mut self) -> Option<(i32, i32, bool)> {
        #[cfg(target_os = "linux")]
        if linux_is_wayland() {
            // Only GNOME (with OpenHop's helper) says where the pointer is.
            return crate::wins::gnome::pointer();
        }
        #[cfg(target_os = "linux")]
        {
            use x11rb::connection::Connection;
            use x11rb::protocol::xproto::{ConnectionExt as _, KeyButMask};
            if self.x.is_none() {
                let (conn, n) = x11rb::connect(None).ok()?;
                let root = conn.setup().roots[n].root;
                self.x = Some((conn, root));
            }
            let (conn, root) = self.x.as_ref()?;
            let p = match conn.query_pointer(*root).ok().and_then(|c| c.reply().ok()) {
                Some(p) => p,
                None => {
                    self.x = None;
                    return None;
                }
            };
            return Some((p.root_x as i32, p.root_y as i32, p.mask.contains(KeyButMask::BUTTON1)));
        }
        #[cfg(windows)]
        {
            use ::windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON};
            let (x, y) = cursor_pos()?;
            let down = unsafe { GetAsyncKeyState(VK_LBUTTON.0 as i32) } as u16 & 0x8000 != 0;
            return Some((x, y, down));
        }
        #[cfg(target_os = "macos")]
        {
            #[link(name = "CoreGraphics", kind = "framework")]
            extern "C" {
                fn CGEventSourceButtonState(state: i32, button: u32) -> bool;
            }
            let (x, y) = cursor_pos()?;
            // kCGEventSourceStateCombinedSessionState, left button.
            let down = unsafe { CGEventSourceButtonState(0, 0) };
            return Some((x, y, down));
        }
        #[allow(unreachable_code)]
        None
    }
}

/// Client-side synthetic input.
pub trait Injector: Send {
    /// Absolute position in native desktop coordinates.
    fn move_to(&mut self, x: i32, y: i32) -> Result<()>;
    fn button(&mut self, button: MouseButton, down: bool) -> Result<()>;
    fn wheel(&mut self, dx: i32, dy: i32) -> Result<()>;
    fn key(&mut self, hid: u16, down: bool) -> Result<()>;
    fn screen(&self) -> Rect;
    /// Wait until the OS has processed everything injected so far.
    fn sync(&mut self) {}
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
    std::env::var("XDG_SESSION_TYPE").map(|s| s == "wayland").unwrap_or(false) || std::env::var_os("WAYLAND_DISPLAY").is_some()
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
