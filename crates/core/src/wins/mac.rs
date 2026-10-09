//! macOS windows (Quartz window list). Window titles and pictures need the
//! Screen Recording permission (System Settings › Privacy & Security).

use super::Picture;
use crate::protocol::{Rect, WinInfo};
use core_foundation::base::{CFType, TCFType};
use core_foundation::dictionary::{CFDictionary, CFDictionaryRef};
use core_foundation::number::CFNumber;
use core_foundation::string::CFString;
use core_graphics::geometry::{CGPoint, CGRect, CGSize};
use core_graphics::window::*;

struct Info {
    id: u32,
    pid: i32,
    layer: i64,
    title: String,
    app: String,
    bounds: Rect,
}

fn num(d: &CFDictionary<CFString, CFType>, key: &str) -> Option<f64> {
    d.find(CFString::new(key)).and_then(|v| v.downcast::<CFNumber>()).and_then(|n| n.to_f64())
}

fn text(d: &CFDictionary<CFString, CFType>, key: &str) -> String {
    d.find(CFString::new(key)).and_then(|v| v.downcast::<CFString>()).map(|s| s.to_string()).unwrap_or_default()
}

fn windows(option: CGWindowListOption, rel: u32) -> Vec<Info> {
    let Some(arr) = copy_window_info(option, rel) else {
        return vec![];
    };
    let mut out = vec![];
    for item in arr.iter() {
        let d: CFDictionary<CFString, CFType> = unsafe { CFDictionary::wrap_under_get_rule(*item as CFDictionaryRef) };
        let bounds = d
            .find(CFString::new("kCGWindowBounds"))
            .and_then(|v| v.downcast::<CFDictionary>())
            .map(|b| {
                let b: CFDictionary<CFString, CFType> = unsafe { CFDictionary::wrap_under_get_rule(b.as_concrete_TypeRef()) };
                Rect {
                    x: num(&b, "X").unwrap_or(0.0) as i32,
                    y: num(&b, "Y").unwrap_or(0.0) as i32,
                    w: num(&b, "Width").unwrap_or(0.0) as i32,
                    h: num(&b, "Height").unwrap_or(0.0) as i32,
                }
            })
            .unwrap_or(Rect { x: 0, y: 0, w: 0, h: 0 });
        out.push(Info {
            id: num(&d, "kCGWindowNumber").unwrap_or(0.0) as u32,
            pid: num(&d, "kCGWindowOwnerPID").unwrap_or(0.0) as i32,
            layer: num(&d, "kCGWindowLayer").unwrap_or(0.0) as i64,
            title: text(&d, "kCGWindowName"),
            app: text(&d, "kCGWindowOwnerName"),
            bounds,
        });
    }
    out
}

fn on_screen() -> Vec<Info> {
    let me = std::process::id() as i32;
    windows(kCGWindowListOptionOnScreenOnly | kCGWindowListExcludeDesktopElements, 0)
        .into_iter()
        .filter(|w| w.layer == 0 && w.pid != me && w.bounds.w > 0)
        .collect()
}

/// Apps in the Dock (regular apps that are running).
pub fn taskbar_pids() -> Vec<u32> {
    use objc2_app_kit::{NSApplicationActivationPolicy, NSWorkspace};
    let ws = NSWorkspace::sharedWorkspace();
    let me = std::process::id() as i32;
    ws.runningApplications()
        .iter()
        .filter(|a| a.activationPolicy() == NSApplicationActivationPolicy::Regular)
        .map(|a| a.processIdentifier())
        .filter(|&p| p > 0 && p != me)
        .map(|p| p as u32)
        .collect()
}

pub fn list() -> Vec<WinInfo> {
    on_screen()
        .into_iter()
        .map(|w| {
            // Without Screen Recording permission titles are empty: use the app.
            let title = if w.title.is_empty() { w.app.clone() } else { w.title.clone() };
            WinInfo { id: w.id as u64, title, app: w.app, w: w.bounds.w, h: w.bounds.h, pid: w.pid.max(0) as u32 }
        })
        .collect()
}

pub fn geometry(id: u64) -> Option<Rect> {
    windows(kCGWindowListOptionIncludingWindow, id as u32).into_iter().find(|w| w.id as u64 == id).map(|w| w.bounds)
}

pub fn capture(id: u64) -> Option<Picture> {
    let null = CGRect::new(&CGPoint::new(f64::INFINITY, f64::INFINITY), &CGSize::new(0.0, 0.0));
    let img = create_image(null, kCGWindowListOptionIncludingWindow, id as u32, kCGWindowImageBoundsIgnoreFraming | kCGWindowImageNominalResolution)?;
    let (w, h, stride) = (img.width(), img.height(), img.bytes_per_row());
    if img.bits_per_pixel() != 32 || w == 0 || h == 0 {
        return None;
    }
    let data = img.data();
    let bytes = data.bytes();
    // BGRA (little-endian premultiplied-first); rows may be padded.
    let mut bgra = Vec::with_capacity(w * h * 4);
    for row in 0..h {
        bgra.extend_from_slice(bytes.get(row * stride..row * stride + w * 4)?);
    }
    // Pictures come at the window's size in points (nominal resolution).
    Some(Picture { w: w as u32, h: h as u32, bgra })
}

pub fn activate(id: u64) {
    use objc2_app_kit::{NSApplicationActivationOptions, NSRunningApplication};
    let Some(w) = windows(kCGWindowListOptionIncludingWindow, id as u32).into_iter().find(|w| w.id as u64 == id) else {
        return;
    };
    if let Some(app) = NSRunningApplication::runningApplicationWithProcessIdentifier(w.pid) {
        #[allow(deprecated)]
        app.activateWithOptions(NSApplicationActivationOptions::ActivateIgnoringOtherApps);
    }
}

pub fn titlebar_window_at(x: i32, y: i32) -> Option<u64> {
    // Front to back: the first window containing the point is the one under it.
    let w = on_screen().into_iter().find(|w| {
        let b = w.bounds;
        x >= b.x && x < b.x + b.w && y >= b.y && y < b.y + b.h
    })?;
    (y < w.bounds.y + 30).then_some(w.id as u64)
}

// macOS doesn't let one app change another app's windows (without private
// APIs), so the original stays visible there.
pub fn set_hidden(_: u64, _: bool) {}
pub fn lower(_: u64) {}
pub fn resize(_: u64, _: i32, _: i32) {}
pub fn restore_all() {}
pub fn raise(_: u64) {}
pub fn move_to(_: u64, _: i32, _: i32) {}
pub fn begin_move(_: u64, _: i32, _: i32) {}
pub fn is_maximized(_: u64) -> bool {
    false
}
pub fn unmaximize(_: u64) {}

/// Pictures include the window's title bar (28 points on current macOS).
pub fn bar_height(_: u64) -> i32 {
    28
}
