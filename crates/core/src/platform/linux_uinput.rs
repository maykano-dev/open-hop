//! Linux injection through /dev/uinput. Works on Wayland (GNOME, KDE, wlroots)
//! and X11 alike because the compositor sees an ordinary input device.
//!
//! Needs write access to /dev/uinput; see packaging/linux/60-openhop-uinput.rules.

use super::{Injector, WheelAccum};
use crate::keys;
use crate::protocol::{MouseButton, Rect};
use anyhow::{Context, Result};
use evdev::uinput::VirtualDevice;
use evdev::{AbsInfo, AbsoluteAxisCode, AttributeSet, EventType, InputEvent, KeyCode, RelativeAxisCode, UinputAbsSetup};

const BTN_LEFT: u16 = 0x110;
const BTN_RIGHT: u16 = 0x111;
const BTN_MIDDLE: u16 = 0x112;
const BTN_SIDE: u16 = 0x113;
const BTN_EXTRA: u16 = 0x114;

pub struct UinputInjector {
    keyboard: VirtualDevice,
    pointer: VirtualDevice,
    rect: Rect,
    wheel: WheelAccum,
}

impl UinputInjector {
    pub fn new(rect: Rect) -> Result<UinputInjector> {
        let hint = "cannot open /dev/uinput. Install the udev rule from packaging/linux and add \
                    yourself to the 'input' group (see README), then log out and back in";
        let mut kb_keys = AttributeSet::<KeyCode>::new();
        for row in keys::TABLE {
            kb_keys.insert(KeyCode::new(row.1));
        }
        let keyboard = VirtualDevice::builder()
            .context(hint)?
            .name("OpenHop virtual keyboard")
            .with_keys(&kb_keys)?
            .build()
            .context(hint)?;

        let mut btns = AttributeSet::<KeyCode>::new();
        for b in [BTN_LEFT, BTN_RIGHT, BTN_MIDDLE, BTN_SIDE, BTN_EXTRA] {
            btns.insert(KeyCode::new(b));
        }
        let mut rel = AttributeSet::<RelativeAxisCode>::new();
        for r in [
            RelativeAxisCode::REL_WHEEL,
            RelativeAxisCode::REL_HWHEEL,
            RelativeAxisCode::REL_WHEEL_HI_RES,
            RelativeAxisCode::REL_HWHEEL_HI_RES,
        ] {
            rel.insert(r);
        }
        // An absolute pointer (like a VM "tablet"): the compositor maps the
        // axis range onto the whole desktop, so positions are exact.
        let abs_x = UinputAbsSetup::new(AbsoluteAxisCode::ABS_X, AbsInfo::new(0, 0, rect.w.max(1) - 1, 0, 0, 1));
        let abs_y = UinputAbsSetup::new(AbsoluteAxisCode::ABS_Y, AbsInfo::new(0, 0, rect.h.max(1) - 1, 0, 0, 1));
        let pointer = VirtualDevice::builder()?
            .name("OpenHop virtual pointer")
            .with_keys(&btns)?
            .with_relative_axes(&rel)?
            .with_absolute_axis(&abs_x)?
            .with_absolute_axis(&abs_y)?
            .build()
            .context(hint)?;
        // Give the compositor a moment to pick up the new devices.
        std::thread::sleep(std::time::Duration::from_millis(300));
        Ok(UinputInjector { keyboard, pointer, rect, wheel: WheelAccum::default() })
    }
}

fn ev(t: EventType, code: u16, value: i32) -> InputEvent {
    InputEvent::new(t.0, code, value)
}

impl Injector for UinputInjector {
    fn move_to(&mut self, x: i32, y: i32) -> Result<()> {
        let x = (x - self.rect.x).clamp(0, self.rect.w - 1);
        let y = (y - self.rect.y).clamp(0, self.rect.h - 1);
        self.pointer.emit(&[
            ev(EventType::ABSOLUTE, AbsoluteAxisCode::ABS_X.0, x),
            ev(EventType::ABSOLUTE, AbsoluteAxisCode::ABS_Y.0, y),
        ])?;
        Ok(())
    }
    fn button(&mut self, button: MouseButton, down: bool) -> Result<()> {
        let code = match button {
            MouseButton::Left => BTN_LEFT,
            MouseButton::Right => BTN_RIGHT,
            MouseButton::Middle => BTN_MIDDLE,
            MouseButton::Back => BTN_SIDE,
            MouseButton::Forward => BTN_EXTRA,
        };
        self.pointer.emit(&[ev(EventType::KEY, code, down as i32)])?;
        Ok(())
    }
    fn wheel(&mut self, dx: i32, dy: i32) -> Result<()> {
        // Hi-res wheel uses the same 1/120 notch units we use on the wire.
        // Legacy notch events are sent too, for apps that ignore hi-res.
        let (nx, ny) = self.wheel.add(dx, dy);
        let mut evs = Vec::new();
        if dy != 0 {
            evs.push(ev(EventType::RELATIVE, RelativeAxisCode::REL_WHEEL_HI_RES.0, dy));
        }
        if ny != 0 {
            evs.push(ev(EventType::RELATIVE, RelativeAxisCode::REL_WHEEL.0, ny));
        }
        if dx != 0 {
            evs.push(ev(EventType::RELATIVE, RelativeAxisCode::REL_HWHEEL_HI_RES.0, dx));
        }
        if nx != 0 {
            evs.push(ev(EventType::RELATIVE, RelativeAxisCode::REL_HWHEEL.0, nx));
        }
        if !evs.is_empty() {
            self.pointer.emit(&evs)?;
        }
        Ok(())
    }
    fn key(&mut self, hid: u16, down: bool) -> Result<()> {
        if let Some(code) = keys::evdev_from_hid(hid) {
            self.keyboard.emit(&[ev(EventType::KEY, code, down as i32)])?;
        }
        Ok(())
    }
    fn screen(&self) -> Rect {
        self.rect
    }
}
