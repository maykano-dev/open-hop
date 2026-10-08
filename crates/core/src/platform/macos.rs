//! macOS backend: CGEventTap for capture, CGEvent posting for injection.
//! Requires Accessibility permission (System Settings > Privacy & Security).

use super::{Capture, InputEvent, Injector};
use crate::keys;
use crate::protocol::{MouseButton, Rect};
use anyhow::{anyhow, Result};
use core_foundation::base::TCFType;
use core_foundation::boolean::CFBoolean;
use core_foundation::dictionary::CFDictionary;
use core_foundation::mach_port::CFMachPortRef;
use core_foundation::runloop::{kCFRunLoopCommonModes, CFRunLoop};
use core_foundation::string::{CFString, CFStringRef};
use core_graphics::display::CGDisplay;
use core_graphics::event::{
    CGEvent, CGEventFlags, CGEventTap, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement, CGEventType,
    CGMouseButton, CallbackResult, EventField, ScrollEventUnit,
};
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
use core_graphics::geometry::CGPoint;
use crossbeam_channel::Sender;
use parking_lot::Mutex;
use std::collections::HashSet;
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Instant;

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXIsProcessTrustedWithOptions(options: *const c_void) -> bool;
    static kAXTrustedCheckOptionPrompt: CFStringRef;
}

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGEventTapEnable(tap: CFMachPortRef, enable: bool);
    fn CGAssociateMouseAndMouseCursorPosition(connected: u32) -> i32;
    fn CGWarpMouseCursorPosition(point: CGPoint) -> i32;
    fn _CGSDefaultConnection() -> i32;
    fn CGSSetConnectionProperty(cid: i32, target: i32, key: CFStringRef, value: *const c_void) -> i32;
}

pub fn check_permissions(prompt: bool) -> Option<String> {
    let trusted = unsafe {
        let key = CFString::wrap_under_get_rule(kAXTrustedCheckOptionPrompt);
        let value = if prompt { CFBoolean::true_value() } else { CFBoolean::false_value() };
        let dict = CFDictionary::from_CFType_pairs(&[(key, value)]);
        AXIsProcessTrustedWithOptions(dict.as_concrete_TypeRef() as *const c_void)
    };
    if trusted {
        None
    } else {
        Some(
            "OpenHop needs Accessibility permission. Open System Settings > Privacy & Security > \
             Accessibility, enable OpenHop, then restart it."
                .into(),
        )
    }
}

pub fn desktop_bounds() -> Rect {
    let ids = CGDisplay::active_displays().unwrap_or_default();
    let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for id in ids {
        let b = CGDisplay::new(id).bounds();
        x0 = x0.min(b.origin.x);
        y0 = y0.min(b.origin.y);
        x1 = x1.max(b.origin.x + b.size.width);
        y1 = y1.max(b.origin.y + b.size.height);
    }
    if x0 > x1 {
        let b = CGDisplay::main().bounds();
        return Rect { x: 0, y: 0, w: b.size.width as i32, h: b.size.height as i32 };
    }
    Rect { x: x0 as i32, y: y0 as i32, w: (x1 - x0) as i32, h: (y1 - y0) as i32 }
}

static TX: OnceLock<Sender<InputEvent>> = OnceLock::new();
static GRABBED: AtomicBool = AtomicBool::new(false);
static TAP_PORT: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());
static HELD_MODS: Mutex<Vec<u16>> = Mutex::new(Vec::new());

fn send(ev: InputEvent) {
    if let Some(tx) = TX.get() {
        let _ = tx.try_send(ev);
    }
}

fn button_from_number(n: i64) -> Option<MouseButton> {
    match n {
        0 => Some(MouseButton::Left),
        1 => Some(MouseButton::Right),
        2 => Some(MouseButton::Middle),
        3 => Some(MouseButton::Back),
        4 => Some(MouseButton::Forward),
        _ => None,
    }
}

/// Letting OpenHop's own input through while grabbed (controlling a window).
static PASS: AtomicBool = AtomicBool::new(false);

fn tap_callback(etype: CGEventType, event: &CGEvent) -> CallbackResult {
    use CGEventType::*;
    if PASS.load(Ordering::Relaxed) && !matches!(etype, TapDisabledByTimeout | TapDisabledByUserInput) {
        return CallbackResult::Keep;
    }
    let grabbed = GRABBED.load(Ordering::Relaxed);
    if !grabbed && matches!(etype, CGEventType::LeftMouseDown) {
        super::dnd::note_left_down();
    }
    match etype {
        TapDisabledByTimeout | TapDisabledByUserInput => {
            let port = TAP_PORT.load(Ordering::SeqCst);
            if !port.is_null() {
                unsafe { CGEventTapEnable(port as CFMachPortRef, true) };
            }
            CallbackResult::Keep
        }
        MouseMoved | LeftMouseDragged | RightMouseDragged | OtherMouseDragged => {
            if grabbed {
                let dx = event.get_integer_value_field(EventField::MOUSE_EVENT_DELTA_X) as i32;
                let dy = event.get_integer_value_field(EventField::MOUSE_EVENT_DELTA_Y) as i32;
                if dx != 0 || dy != 0 {
                    send(InputEvent::Delta { dx, dy });
                }
                CallbackResult::Drop
            } else {
                let p = event.location();
                send(InputEvent::LocalMove { x: p.x.round() as i32, y: p.y.round() as i32 });
                CallbackResult::Keep
            }
        }
        LeftMouseDown | LeftMouseUp | RightMouseDown | RightMouseUp | OtherMouseDown | OtherMouseUp if grabbed => {
            let down = matches!(etype, LeftMouseDown | RightMouseDown | OtherMouseDown);
            let n = event.get_integer_value_field(EventField::MOUSE_EVENT_BUTTON_NUMBER);
            if let Some(button) = button_from_number(n) {
                send(InputEvent::Button { button, down });
            }
            CallbackResult::Drop
        }
        ScrollWheel if grabbed => {
            let v = event.get_double_value_field(EventField::SCROLL_WHEEL_EVENT_FIXED_POINT_DELTA_AXIS_1);
            let h = event.get_double_value_field(EventField::SCROLL_WHEEL_EVENT_FIXED_POINT_DELTA_AXIS_2);
            let (dx, dy) = ((-h * 120.0).round() as i32, (v * 120.0).round() as i32);
            if dx != 0 || dy != 0 {
                send(InputEvent::Wheel { dx, dy });
            }
            CallbackResult::Drop
        }
        KeyDown | KeyUp if grabbed => {
            let repeat = event.get_integer_value_field(EventField::KEYBOARD_EVENT_AUTOREPEAT) != 0;
            if !repeat {
                let code = event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE) as u16;
                if let Some(key) = keys::hid_from_mac(code) {
                    send(InputEvent::Key { key, down: matches!(etype, KeyDown) });
                }
            }
            CallbackResult::Drop
        }
        FlagsChanged if grabbed => {
            let code = event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE) as u16;
            if let Some(key) = keys::hid_from_mac(code) {
                if key == keys::HID_CAPS {
                    // macOS reports a toggle, not press/release.
                    send(InputEvent::Key { key, down: true });
                    send(InputEvent::Key { key, down: false });
                } else {
                    let mut held = HELD_MODS.lock();
                    let down = !held.contains(&key);
                    if down {
                        held.push(key);
                    } else {
                        held.retain(|&k| k != key);
                    }
                    send(InputEvent::Key { key, down });
                }
            }
            CallbackResult::Drop
        }
        _ => CallbackResult::Keep,
    }
}

fn set_cursor_hidden(hidden: bool) {
    unsafe {
        // Lets a background app hide the cursor (same trick Barrier/Deskflow use).
        let cid = _CGSDefaultConnection();
        let key = CFString::new("SetsCursorInBackground");
        CGSSetConnectionProperty(cid, cid, key.as_concrete_TypeRef(), CFBoolean::true_value().as_CFTypeRef());
    }
    let d = CGDisplay::main();
    let _ = if hidden { d.hide_cursor() } else { d.show_cursor() };
}

pub struct MacCapture {
    hidden: AtomicBool,
}

impl MacCapture {
    pub fn start(tx: Sender<InputEvent>) -> Result<Arc<MacCapture>> {
        if let Some(msg) = check_permissions(true) {
            return Err(anyhow!(msg));
        }
        TX.set(tx).map_err(|_| anyhow!("capture already started"))?;
        let (ready_tx, ready_rx) = crossbeam_channel::bounded::<Result<(), String>>(1);
        std::thread::Builder::new().name("mac-event-tap".into()).spawn(move || {
            use CGEventType::*;
            let events = vec![
                MouseMoved, LeftMouseDragged, RightMouseDragged, OtherMouseDragged, LeftMouseDown, LeftMouseUp,
                RightMouseDown, RightMouseUp, OtherMouseDown, OtherMouseUp, ScrollWheel, KeyDown, KeyUp, FlagsChanged,
            ];
            let tap = CGEventTap::new(
                CGEventTapLocation::HID,
                CGEventTapPlacement::HeadInsertEventTap,
                CGEventTapOptions::Default,
                events,
                |_proxy, etype, event| tap_callback(etype, event),
            );
            let tap = match tap {
                Ok(t) => t,
                Err(()) => {
                    let _ = ready_tx.try_send(Err("could not create event tap (check Accessibility and Input Monitoring permissions)".into()));
                    return;
                }
            };
            TAP_PORT.store(tap.mach_port().as_concrete_TypeRef() as *mut c_void, Ordering::SeqCst);
            let source = tap.mach_port().create_runloop_source(0).expect("runloop source");
            CFRunLoop::get_current().add_source(&source, unsafe { kCFRunLoopCommonModes });
            tap.enable();
            let _ = ready_tx.try_send(Ok(()));
            CFRunLoop::run_current();
            drop(tap);
        })?;
        match ready_rx.recv() {
            Ok(Ok(())) => Ok(Arc::new(MacCapture { hidden: AtomicBool::new(false) })),
            Ok(Err(e)) => Err(anyhow!(e)),
            Err(_) => Err(anyhow!("event tap thread exited")),
        }
    }
}

impl Capture for MacCapture {
    fn suspend(&self, _pointer: bool) -> bool {
        if !GRABBED.load(Ordering::SeqCst) {
            return false;
        }
        PASS.store(true, Ordering::SeqCst);
        true
    }
    fn resume(&self) {
        std::thread::sleep(std::time::Duration::from_millis(10));
        PASS.store(false, Ordering::SeqCst);
    }
    fn grab(&self) -> bool {
        HELD_MODS.lock().clear();
        GRABBED.store(true, Ordering::SeqCst);
        unsafe { CGAssociateMouseAndMouseCursorPosition(0) };
        if !self.hidden.swap(true, Ordering::SeqCst) {
            set_cursor_hidden(true);
        }
        true
    }
    fn release(&self, x: i32, y: i32) {
        GRABBED.store(false, Ordering::SeqCst);
        unsafe {
            CGWarpMouseCursorPosition(CGPoint::new(x as f64, y as f64));
            CGAssociateMouseAndMouseCursorPosition(1);
        }
        if self.hidden.swap(false, Ordering::SeqCst) {
            set_cursor_hidden(false);
        }
    }
    fn screen(&self) -> Rect {
        desktop_bounds()
    }
}

pub struct MacInjector {
    pos: (f64, f64),
    buttons: HashSet<MouseButton>,
    mods: Vec<u16>,
    last_click: Option<(MouseButton, Instant, i64)>,
}

impl MacInjector {
    pub fn new() -> Result<MacInjector> {
        if let Some(msg) = check_permissions(true) {
            return Err(anyhow!(msg));
        }
        Ok(MacInjector { pos: (0.0, 0.0), buttons: HashSet::new(), mods: Vec::new(), last_click: None })
    }

    fn source() -> Result<CGEventSource> {
        CGEventSource::new(CGEventSourceStateID::HIDSystemState).map_err(|_| anyhow!("CGEventSource failed"))
    }

    fn flags(&self) -> CGEventFlags {
        let mut f = CGEventFlags::CGEventFlagNull;
        for &m in &self.mods {
            f |= match m {
                0xE0 | 0xE4 => CGEventFlags::CGEventFlagControl,
                0xE1 | 0xE5 => CGEventFlags::CGEventFlagShift,
                0xE2 | 0xE6 => CGEventFlags::CGEventFlagAlternate,
                0xE3 | 0xE7 => CGEventFlags::CGEventFlagCommand,
                _ => CGEventFlags::CGEventFlagNull,
            };
        }
        f
    }
}

fn cg_button(b: MouseButton) -> (CGMouseButton, i64) {
    match b {
        MouseButton::Left => (CGMouseButton::Left, 0),
        MouseButton::Right => (CGMouseButton::Right, 1),
        MouseButton::Middle => (CGMouseButton::Center, 2),
        MouseButton::Back => (CGMouseButton::Center, 3),
        MouseButton::Forward => (CGMouseButton::Center, 4),
    }
}

impl Injector for MacInjector {
    fn move_to(&mut self, x: i32, y: i32) -> Result<()> {
        let (nx, ny) = (x as f64, y as f64);
        let (etype, btn) = if self.buttons.contains(&MouseButton::Left) {
            (CGEventType::LeftMouseDragged, MouseButton::Left)
        } else if self.buttons.contains(&MouseButton::Right) {
            (CGEventType::RightMouseDragged, MouseButton::Right)
        } else if let Some(&b) = self.buttons.iter().next() {
            (CGEventType::OtherMouseDragged, b)
        } else {
            (CGEventType::MouseMoved, MouseButton::Left)
        };
        let (cgb, num) = cg_button(btn);
        let ev = CGEvent::new_mouse_event(Self::source()?, etype, CGPoint::new(nx, ny), cgb)
            .map_err(|_| anyhow!("mouse event"))?;
        ev.set_integer_value_field(EventField::MOUSE_EVENT_DELTA_X, (nx - self.pos.0) as i64);
        ev.set_integer_value_field(EventField::MOUSE_EVENT_DELTA_Y, (ny - self.pos.1) as i64);
        if matches!(etype, CGEventType::OtherMouseDragged) {
            ev.set_integer_value_field(EventField::MOUSE_EVENT_BUTTON_NUMBER, num);
        }
        ev.set_flags(self.flags());
        ev.post(CGEventTapLocation::HID);
        self.pos = (nx, ny);
        Ok(())
    }

    fn button(&mut self, button: MouseButton, down: bool) -> Result<()> {
        let (cgb, num) = cg_button(button);
        let etype = match (cgb, down) {
            (CGMouseButton::Left, true) => CGEventType::LeftMouseDown,
            (CGMouseButton::Left, false) => CGEventType::LeftMouseUp,
            (CGMouseButton::Right, true) => CGEventType::RightMouseDown,
            (CGMouseButton::Right, false) => CGEventType::RightMouseUp,
            (_, true) => CGEventType::OtherMouseDown,
            (_, false) => CGEventType::OtherMouseUp,
        };
        // Double/triple clicks need an explicit click count on macOS.
        let clicks = if down {
            let n = match self.last_click {
                Some((b, t, n)) if b == button && t.elapsed().as_millis() < 500 => n + 1,
                _ => 1,
            };
            self.last_click = Some((button, Instant::now(), n));
            self.buttons.insert(button);
            n
        } else {
            self.buttons.remove(&button);
            self.last_click.map(|(_, _, n)| n).unwrap_or(1)
        };
        let ev = CGEvent::new_mouse_event(Self::source()?, etype, CGPoint::new(self.pos.0, self.pos.1), cgb)
            .map_err(|_| anyhow!("mouse event"))?;
        ev.set_integer_value_field(EventField::MOUSE_EVENT_CLICK_STATE, clicks);
        ev.set_integer_value_field(EventField::MOUSE_EVENT_BUTTON_NUMBER, num);
        ev.set_flags(self.flags());
        ev.post(CGEventTapLocation::HID);
        Ok(())
    }

    fn wheel(&mut self, dx: i32, dy: i32) -> Result<()> {
        // One notch (120) ~ 40 px keeps both notched wheels and smooth trackpads natural.
        let px = |v: i32| (v as f64 * 40.0 / 120.0).round() as i32;
        let (v, h) = (px(dy), -px(dx));
        if v == 0 && h == 0 {
            return Ok(());
        }
        let ev = CGEvent::new_scroll_event(Self::source()?, ScrollEventUnit::PIXEL, 2, v, h, 0)
            .map_err(|_| anyhow!("scroll event"))?;
        ev.post(CGEventTapLocation::HID);
        Ok(())
    }

    fn key(&mut self, hid: u16, down: bool) -> Result<()> {
        let Some(code) = keys::mac_from_hid(hid) else { return Ok(()) };
        if keys::is_modifier(hid) {
            if down {
                if !self.mods.contains(&hid) {
                    self.mods.push(hid);
                }
            } else {
                self.mods.retain(|&m| m != hid);
            }
        }
        let ev = CGEvent::new_keyboard_event(Self::source()?, code, down).map_err(|_| anyhow!("key event"))?;
        ev.set_flags(self.flags());
        ev.post(CGEventTapLocation::HID);
        Ok(())
    }

    fn screen(&self) -> Rect {
        desktop_bounds()
    }
}
