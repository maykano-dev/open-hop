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
    /// Take over the mouse and keyboard. With `drag`, a file drag is being
    /// carried off this screen and its source may hold the pointer.
    Grab(Sender<bool>, bool),
    Release(i32, i32),
}

/// Carrying a file drag to another screen. The drag source (a file manager)
/// holds the pointer grab, so OpenHop can't grab it; instead it follows the
/// pointer by warping it back to the centre, over a transparent "shield"
/// window that refuses drops, so letting go doesn't drop anything here.
struct Carry {
    shield: Window,
    released: bool,
    last_try: std::time::Instant,
}

struct XdndAtoms {
    aware: u32,
    position: u32,
    status: u32,
    drop: u32,
    finished: u32,
}

fn make_shield(conn: &RustConnection, root: Window, (cx, cy): (i32, i32), a: &XdndAtoms) -> Result<Window> {
    use x11rb::protocol::xproto::{ColormapAlloc, CreateWindowAux, PropMode, WindowClass};
    use x11rb::wrapper::ConnectionExt as _;
    let screen = conn.setup().roots.iter().find(|s| s.root == root).context("no screen")?;
    // A see-through window needs a compositor; without one, a window with no
    // background simply leaves what was on screen in place.
    let n = conn.setup().roots.iter().position(|s| s.root == root).unwrap_or(0);
    let cm = conn.intern_atom(false, format!("_NET_WM_CM_S{n}").as_bytes())?.reply()?.atom;
    let composited = conn.get_selection_owner(cm)?.reply()?.owner != NONE;
    let argb = screen
        .allowed_depths
        .iter()
        .find(|d| d.depth == 32)
        .and_then(|d| d.visuals.first())
        .map(|v| v.visual_id)
        .filter(|_| composited);
    let win = conn.generate_id()?;
    const R: i32 = 250;
    let (x, y, w) = ((cx - R) as i16, (cy - R) as i16, (2 * R) as u16);
    match argb {
        Some(visual) => {
            let cmap = conn.generate_id()?;
            conn.create_colormap(ColormapAlloc::NONE, cmap, root, visual)?;
            let aux = CreateWindowAux::new().background_pixel(0).border_pixel(0).colormap(cmap).override_redirect(1);
            conn.create_window(32, win, root, x, y, w, w, 0, WindowClass::INPUT_OUTPUT, visual, &aux)?;
        }
        None => {
            let aux = CreateWindowAux::new().background_pixmap(NONE).override_redirect(1);
            conn.create_window(0, win, root, x, y, w, w, 0, WindowClass::INPUT_OUTPUT, 0, &aux)?;
        }
    }
    conn.change_property32(PropMode::REPLACE, win, a.aware, x11rb::protocol::xproto::AtomEnum::ATOM, &[5])?;
    conn.map_window(win)?;
    conn.flush()?;
    Ok(win)
}

fn left_down(conn: &RustConnection, root: Window) -> Result<bool> {
    let p = conn.query_pointer(root)?.reply()?;
    Ok(u16::from(p.mask) & u16::from(x11rb::protocol::xproto::KeyButMask::BUTTON1) != 0)
}

/// Answer a drag source talking to the shield: "no, you can't drop here".
fn refuse_drop(conn: &RustConnection, shield: Window, a: &XdndAtoms, ev: &x11rb::protocol::xproto::ClientMessageEvent) {
    use x11rb::protocol::xproto::ClientMessageEvent;
    let src = ev.data.as_data32()[0];
    let reply = if ev.type_ == a.position {
        Some(ClientMessageEvent::new(32, src, a.status, [shield, 0, 0, 0, 0]))
    } else if ev.type_ == a.drop {
        Some(ClientMessageEvent::new(32, src, a.finished, [shield, 0, 0, 0, 0]))
    } else {
        None
    };
    if let Some(r) = reply {
        let _ = conn.send_event(false, src, EventMask::NO_EVENT, r);
        let _ = conn.flush();
    }
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
    fn grab(&self) -> bool {
        let (tx, rx) = crossbeam_channel::bounded(1);
        if self.cmd.send(Cmd::Grab(tx, false)).is_err() {
            return false;
        }
        rx.recv_timeout(Duration::from_secs(2)).unwrap_or(false)
    }
    fn grab_carrying_drag(&self) -> bool {
        let (tx, rx) = crossbeam_channel::bounded(1);
        if self.cmd.send(Cmd::Grab(tx, true)).is_err() {
            return false;
        }
        rx.recv_timeout(Duration::from_secs(2)).unwrap_or(false)
    }
    fn release(&self, x: i32, y: i32) {
        let _ = self.cmd.send(Cmd::Release(x, y));
    }
    fn screen(&self) -> Rect {
        *self.screen.lock()
    }
}

fn try_grab(conn: &RustConnection, root: Window) -> Result<()> {
    try_grab_for(conn, root, 20)
}

fn try_grab_for(conn: &RustConnection, root: Window, tries: u32) -> Result<()> {
    let mask = EventMask::POINTER_MOTION | EventMask::BUTTON_PRESS | EventMask::BUTTON_RELEASE;
    // Another client (an open menu, a screensaver) may hold a grab briefly.
    for _ in 0..tries {
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
    let mut carry: Option<Carry> = None;
    let mut last = (i32::MIN, i32::MIN);
    let mut held: std::collections::HashSet<u8> = Default::default();
    let atom = |n: &[u8]| -> Result<u32> { Ok(conn.intern_atom(false, n)?.reply()?.atom) };
    let xa = XdndAtoms {
        aware: atom(b"XdndAware")?,
        position: atom(b"XdndPosition")?,
        status: atom(b"XdndStatus")?,
        drop: atom(b"XdndDrop")?,
        finished: atom(b"XdndFinished")?,
    };
    loop {
        // Commands from the engine.
        loop {
            match cmd_rx.try_recv() {
                Ok(Cmd::Grab(reply, drag)) if !grabbed && carry.is_none() => match try_grab(&conn, root) {
                    Ok(()) => {
                        grabbed = true;
                        held.clear();
                        conn.xfixes_hide_cursor(root)?;
                        conn.warp_pointer(NONE, root, 0, 0, 0, 0, cx as i16, cy as i16)?;
                        conn.flush()?;
                        let _ = reply.send(true);
                    }
                    Err(_) if drag && left_down(&conn, root).unwrap_or(false) => match make_shield(&conn, root, (cx, cy), &xa) {
                        Ok(shield) => {
                            log::info!("the drag source holds the pointer; following it until the drop");
                            held.clear();
                            conn.xfixes_hide_cursor(root)?;
                            conn.warp_pointer(NONE, root, 0, 0, 0, 0, cx as i16, cy as i16)?;
                            conn.flush()?;
                            carry = Some(Carry { shield, released: false, last_try: std::time::Instant::now() });
                            let _ = reply.send(true);
                        }
                        Err(e) => {
                            log::warn!("could not carry the drag: {e:#}");
                            let _ = reply.send(false);
                        }
                    },
                    Err(e) => {
                        log::warn!("{e}");
                        let _ = reply.send(false);
                    }
                },
                Ok(Cmd::Grab(reply, _)) => {
                    let _ = reply.send(true);
                }
                Ok(Cmd::Release(x, y)) => {
                    if let Some(c) = carry.take() {
                        let _ = conn.destroy_window(c.shield);
                        conn.xfixes_show_cursor(root)?;
                    }
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
                    if let Some(c) = carry.take() {
                        let _ = conn.destroy_window(c.shield);
                        let _ = conn.xfixes_show_cursor(root);
                    }
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

        if let Some(c) = carry.as_mut() {
            let p = conn.query_pointer(root)?.reply()?;
            let (x, y) = (p.root_x as i32, p.root_y as i32);
            if (x, y) != (cx, cy) {
                send_delta(&tx, x - cx, y - cy);
                conn.warp_pointer(NONE, root, 0, 0, 0, 0, cx as i16, cy as i16)?;
                conn.flush()?;
            }
            let down = u16::from(p.mask) & u16::from(x11rb::protocol::xproto::KeyButMask::BUTTON1) != 0;
            if !down && !c.released {
                // Let go: that's the drop, on the other computer.
                c.released = true;
                let _ = tx.try_send(InputEvent::Button { button: MouseButton::Left, down: false });
            }
            while let Some(ev) = conn.poll_for_event()? {
                if let Event::ClientMessage(m) = &ev {
                    refuse_drop(&conn, c.shield, &xa, m);
                }
            }
            // Once the drag source lets go of the pointer, take over properly.
            if c.last_try.elapsed() > Duration::from_millis(50) {
                c.last_try = std::time::Instant::now();
                if try_grab_for(&conn, root, 1).is_ok() {
                    let _ = conn.destroy_window(c.shield);
                    conn.flush()?;
                    log::debug!("drag over; mouse and keyboard captured");
                    // Still held: the release will come as a normal event.
                    if !c.released {
                        held.clear();
                    }
                    carry = None;
                    grabbed = true;
                }
            }
            std::thread::sleep(Duration::from_millis(4));
            continue;
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
