//! Linux X11 backend: capture via pointer/keyboard grabs, injection via XTEST.

use super::{Capture, InputEvent, Injector, WheelAccum};
use crate::keys;
use crate::protocol::{MouseButton, Rect};
use anyhow::{bail, Context, Result};
use crossbeam_channel::{Receiver, Sender};
use parking_lot::Mutex;
use std::sync::Arc;
use std::time::Duration;
use x11rb::connection::Connection;
use x11rb::protocol::xfixes::ConnectionExt as _;
use x11rb::protocol::xkb::{self, ConnectionExt as _};
use x11rb::protocol::xproto::{ConnectionExt as _, EventMask, GrabMode, GrabStatus, Window};
use x11rb::protocol::xtest::ConnectionExt as _;
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;
use x11rb::{CURRENT_TIME, NONE};

const KEY_PRESS: u8 = 2;
const KEY_RELEASE: u8 = 3;
const BUTTON_PRESS: u8 = 4;
const BUTTON_RELEASE: u8 = 5;
const MOTION_NOTIFY: u8 = 6;

/// Desktop size from the X root window (works under XWayland too).
pub fn root_rect() -> Option<Rect> {
    let (conn, n) = x11rb::connect(None).ok()?;
    let s = &conn.setup().roots[n];
    Some(Rect { x: 0, y: 0, w: s.width_in_pixels as i32, h: s.height_in_pixels as i32 })
}

fn x_button(b: MouseButton) -> u8 {
    match b {
        MouseButton::Left => 1,
        MouseButton::Middle => 2,
        MouseButton::Right => 3,
        MouseButton::Back => 8,
        MouseButton::Forward => 9,
    }
}

enum Cmd {
    Grab,
    Release(i32, i32),
}

pub struct X11Capture {
    cmd: Sender<Cmd>,
    screen: Arc<Mutex<Rect>>,
}

impl X11Capture {
    pub fn start(tx: Sender<InputEvent>, override_screen: Option<Rect>) -> Result<X11Capture> {
        let (conn, n) = x11rb::connect(None).context("cannot connect to the X server (is DISPLAY set?)")?;
        let root = conn.setup().roots[n].root;
        let s = &conn.setup().roots[n];
        let rect = override_screen.unwrap_or(Rect { x: 0, y: 0, w: s.width_in_pixels as i32, h: s.height_in_pixels as i32 });
        conn.xfixes_query_version(5, 0)?.reply().context("XFIXES extension missing")?;
        // Without this, X sends fake release/press pairs while a key auto-repeats.
        if conn.xkb_use_extension(1, 0)?.reply().map(|r| r.supported).unwrap_or(false) {
            let flag = xkb::PerClientFlag::DETECTABLE_AUTO_REPEAT;
            let _ = conn
                .xkb_per_client_flags(xkb::ID::USE_CORE_KBD.into(), flag, flag, 0u32.into(), 0u32.into(), 0u32.into())?
                .reply();
        }
        let (cmd_tx, cmd_rx) = crossbeam_channel::unbounded();
        let screen = Arc::new(Mutex::new(rect));
        std::thread::Builder::new()
            .name("x11-capture".into())
            .spawn(move || {
                if let Err(e) = capture_loop(conn, root, rect, tx, cmd_rx) {
                    log::error!("X11 capture stopped: {e:#}");
                }
            })?;
        Ok(X11Capture { cmd: cmd_tx, screen })
    }
}

impl Capture for X11Capture {
    fn grab(&self) {
        let _ = self.cmd.send(Cmd::Grab);
    }
    fn release(&self, x: i32, y: i32) {
        let _ = self.cmd.send(Cmd::Release(x, y));
    }
    fn screen(&self) -> Rect {
        *self.screen.lock()
    }
}

fn try_grab(conn: &RustConnection, root: Window) -> Result<()> {
    let mask = EventMask::POINTER_MOTION | EventMask::BUTTON_PRESS | EventMask::BUTTON_RELEASE;
    // Another client (an open menu, a screensaver) may hold a grab briefly.
    for _ in 0..20 {
        let p = conn
            .grab_pointer(false, root, mask, GrabMode::ASYNC, GrabMode::ASYNC, NONE, NONE, CURRENT_TIME)?
            .reply()?;
        if p.status == GrabStatus::SUCCESS {
            let k = conn.grab_keyboard(false, root, CURRENT_TIME, GrabMode::ASYNC, GrabMode::ASYNC)?.reply()?;
            if k.status == GrabStatus::SUCCESS {
                return Ok(());
            }
            conn.ungrab_pointer(CURRENT_TIME)?;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    bail!("could not grab pointer/keyboard")
}

fn capture_loop(conn: RustConnection, root: Window, rect: Rect, tx: Sender<InputEvent>, cmd_rx: Receiver<Cmd>) -> Result<()> {
    let (cx, cy) = rect.center();
    let mut grabbed = false;
    let mut last = (i32::MIN, i32::MIN);
    let mut held: std::collections::HashSet<u8> = Default::default();
    loop {
        // Commands from the engine.
        loop {
            match cmd_rx.try_recv() {
                Ok(Cmd::Grab) if !grabbed => match try_grab(&conn, root) {
                    Ok(()) => {
                        grabbed = true;
                        held.clear();
                        conn.xfixes_hide_cursor(root)?;
                        conn.warp_pointer(NONE, root, 0, 0, 0, 0, cx as i16, cy as i16)?;
                        conn.flush()?;
                    }
                    Err(e) => log::warn!("{e}"),
                },
                Ok(Cmd::Grab) => {}
                Ok(Cmd::Release(x, y)) => {
                    if grabbed {
                        conn.ungrab_keyboard(CURRENT_TIME)?;
                        conn.ungrab_pointer(CURRENT_TIME)?;
                        conn.xfixes_show_cursor(root)?;
                        grabbed = false;
                    }
                    conn.warp_pointer(NONE, root, 0, 0, 0, 0, x as i16, y as i16)?;
                    conn.flush()?;
                    last = (x, y);
                    // Drop events generated before the release.
                    while conn.poll_for_event()?.is_some() {}
                }
                Err(crossbeam_channel::TryRecvError::Empty) => break,
                Err(crossbeam_channel::TryRecvError::Disconnected) => {
                    if grabbed {
                        let _ = conn.ungrab_keyboard(CURRENT_TIME);
                        let _ = conn.ungrab_pointer(CURRENT_TIME);
                        let _ = conn.xfixes_show_cursor(root);
                        let _ = conn.flush();
                    }
                    return Ok(());
                }
            }
        }

        if !grabbed {
            // Edge detection: poll the pointer ~250 times a second.
            let p = conn.query_pointer(root)?.reply()?;
            let pos = (p.root_x as i32, p.root_y as i32);
            if pos != last {
                last = pos;
                let _ = tx.try_send(InputEvent::LocalMove { x: pos.0, y: pos.1 });
            }
            while conn.poll_for_event()?.is_some() {}
            std::thread::sleep(Duration::from_millis(4));
            continue;
        }

        // Grabbed: drain events. Motion is measured from the screen centre,
        // and we warp back to the centre after each batch.
        let mut motion: Option<(i32, i32)> = None;
        let mut any = false;
        while let Some(ev) = conn.poll_for_event()? {
            any = true;
            match ev {
                Event::MotionNotify(e) => motion = Some((e.root_x as i32, e.root_y as i32)),
                Event::ButtonPress(e) | Event::ButtonRelease(e) => {
                    let down = matches!(ev, Event::ButtonPress(_));
                    // Flush pending motion first so clicks land where expected.
                    if let Some((x, y)) = motion.take() {
                        send_delta(&tx, x - cx, y - cy);
                        conn.warp_pointer(NONE, root, 0, 0, 0, 0, cx as i16, cy as i16)?;
                    }
                    let ev = match e.detail {
                        1 => Some(InputEvent::Button { button: MouseButton::Left, down }),
                        2 => Some(InputEvent::Button { button: MouseButton::Middle, down }),
                        3 => Some(InputEvent::Button { button: MouseButton::Right, down }),
                        8 => Some(InputEvent::Button { button: MouseButton::Back, down }),
                        9 => Some(InputEvent::Button { button: MouseButton::Forward, down }),
                        4 if down => Some(InputEvent::Wheel { dx: 0, dy: 120 }),
                        5 if down => Some(InputEvent::Wheel { dx: 0, dy: -120 }),
                        6 if down => Some(InputEvent::Wheel { dx: -120, dy: 0 }),
                        7 if down => Some(InputEvent::Wheel { dx: 120, dy: 0 }),
                        _ => None,
                    };
                    if let Some(ev) = ev {
                        let _ = tx.try_send(ev);
                    }
                }
                Event::KeyPress(e) | Event::KeyRelease(e) => {
                    let down = matches!(ev, Event::KeyPress(_));
                    if down && !held.insert(e.detail) {
                        continue; // auto-repeat: the remote OS repeats on its own
                    }
                    if !down {
                        held.remove(&e.detail);
                    }
                    match keys::hid_from_evdev(e.detail.saturating_sub(8) as u16) {
                        Some(key) => {
                            let _ = tx.try_send(InputEvent::Key { key, down });
                        }
                        None => log::debug!("unmapped X keycode {}", e.detail),
                    }
                }
                _ => {}
            }
        }
        if let Some((x, y)) = motion {
            if (x, y) != (cx, cy) {
                send_delta(&tx, x - cx, y - cy);
                conn.warp_pointer(NONE, root, 0, 0, 0, 0, cx as i16, cy as i16)?;
                conn.flush()?;
            }
        }
        if !any {
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}

fn send_delta(tx: &Sender<InputEvent>, dx: i32, dy: i32) {
    if dx != 0 || dy != 0 {
        let _ = tx.try_send(InputEvent::Delta { dx, dy });
    }
}

pub struct X11Injector {
    conn: RustConnection,
    root: Window,
    rect: Rect,
    wheel: WheelAccum,
}

impl X11Injector {
    pub fn new(override_screen: Option<Rect>) -> Result<X11Injector> {
        let (conn, n) = x11rb::connect(None).context("cannot connect to the X server (is DISPLAY set?)")?;
        conn.xtest_get_version(2, 2)?.reply().context("XTEST extension missing")?;
        let s = &conn.setup().roots[n];
        let root = s.root;
        let rect = override_screen.unwrap_or(Rect { x: 0, y: 0, w: s.width_in_pixels as i32, h: s.height_in_pixels as i32 });
        Ok(X11Injector { conn, root, rect, wheel: WheelAccum::default() })
    }

    fn fake(&self, kind: u8, detail: u8, x: i16, y: i16) -> Result<()> {
        self.conn.xtest_fake_input(kind, detail, CURRENT_TIME, self.root, x, y, 0)?;
        Ok(())
    }

    fn click(&self, button: u8, times: i32) -> Result<()> {
        for _ in 0..times {
            self.fake(BUTTON_PRESS, button, 0, 0)?;
            self.fake(BUTTON_RELEASE, button, 0, 0)?;
        }
        Ok(())
    }
}

impl Injector for X11Injector {
    fn move_to(&mut self, x: i32, y: i32) -> Result<()> {
        self.fake(MOTION_NOTIFY, 0, x as i16, y as i16)?;
        self.conn.flush()?;
        Ok(())
    }
    fn button(&mut self, button: MouseButton, down: bool) -> Result<()> {
        self.fake(if down { BUTTON_PRESS } else { BUTTON_RELEASE }, x_button(button), 0, 0)?;
        self.conn.flush()?;
        Ok(())
    }
    fn wheel(&mut self, dx: i32, dy: i32) -> Result<()> {
        let (nx, ny) = self.wheel.add(dx, dy);
        if ny > 0 {
            self.click(4, ny)?;
        } else if ny < 0 {
            self.click(5, -ny)?;
        }
        if nx > 0 {
            self.click(7, nx)?;
        } else if nx < 0 {
            self.click(6, -nx)?;
        }
        self.conn.flush()?;
        Ok(())
    }
    fn key(&mut self, hid: u16, down: bool) -> Result<()> {
        let Some(code) = keys::evdev_from_hid(hid) else { return Ok(()) };
        self.fake(if down { KEY_PRESS } else { KEY_RELEASE }, (code + 8) as u8, 0, 0)?;
        self.conn.flush()?;
        Ok(())
    }
    fn screen(&self) -> Rect {
        self.rect
    }
}
