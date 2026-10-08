//! Linux X11 backend: capture via pointer/keyboard grabs, injection via XTEST.

use super::{Capture, Injector, InputEvent, WheelAccum};
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
    /// Ungrab for a moment so OpenHop's own XTEST input reaches windows.
    /// `true`: the mouse (for injected pointer input), `false`: just the keyboard.
    Suspend(Sender<bool>, bool),
    Resume,
    KeyboardPass(bool),
    IdleCursor(bool),
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
    let argb = screen.allowed_depths.iter().find(|d| d.depth == 32).and_then(|d| d.visuals.first()).map(|v| v.visual_id).filter(|_| composited);
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
            let _ = conn.xkb_per_client_flags(xkb::ID::USE_CORE_KBD.into(), flag, flag, 0u32.into(), 0u32.into(), 0u32.into())?.reply();
        }
        let (cmd_tx, cmd_rx) = crossbeam_channel::unbounded();
        let screen = Arc::new(Mutex::new(rect));
        std::thread::Builder::new().name("x11-capture".into()).spawn(move || {
            if let Err(e) = capture_loop(conn, root, rect, tx, cmd_rx) {
                log::error!("X11 capture stopped: {e:#}");
            }
        })?;
        Ok(X11Capture { cmd: cmd_tx, screen })
    }
}

impl Capture for X11Capture {
    fn suspend(&self, pointer: bool) -> bool {
        let (tx, rx) = crossbeam_channel::bounded(1);
        if self.cmd.send(Cmd::Suspend(tx, pointer)).is_err() {
            return false;
        }
        rx.recv_timeout(Duration::from_secs(1)).unwrap_or(false)
    }
    fn resume(&self) {
        let _ = self.cmd.send(Cmd::Resume);
    }
    fn keyboard_passthrough(&self, on: bool) {
        let _ = self.cmd.send(Cmd::KeyboardPass(on));
    }
    fn idle_cursor(&self, hidden: bool) {
        let _ = self.cmd.send(Cmd::IdleCursor(hidden));
    }
    fn whole_clicks(&self) -> bool {
        // The core grab is shared by the physical mouse and our own (XTEST)
        // one; with XInput 2 per-device grabs they're separate.
        !xi2_requested()
    }
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

/// Holding devices one by one with XInput 2 is opt-in: some X servers crash
/// when such a grab ends while the device is in use.
fn xi2_requested() -> bool {
    std::env::var_os("OPENHOP_XI2").is_some()
}

fn try_grab(conn: &RustConnection, root: Window) -> Result<()> {
    try_grab_for(conn, root, 20)
}

fn try_grab_for(conn: &RustConnection, root: Window, tries: u32) -> Result<()> {
    let mask = EventMask::POINTER_MOTION | EventMask::BUTTON_PRESS | EventMask::BUTTON_RELEASE;
    // Another client (an open menu, a screensaver) may hold a grab briefly.
    for _ in 0..tries {
        let p = conn.grab_pointer(false, root, mask, GrabMode::ASYNC, GrabMode::ASYNC, NONE, NONE, CURRENT_TIME)?.reply()?;
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
    // Let go for a moment so OpenHop's own input reaches a window here
    // (a live window of this computer, used from another one).
    let mut suspended: Option<bool> = None; // Some(pointer?)
    let mut buttons_down: std::collections::HashSet<u8> = Default::default();
    // The keyboard is left to this computer's focused window (see keyboard_passthrough).
    let mut key_pass = false;
    // Devices held with XInput 2 (empty: the core grab is used).
    let mut xi: Vec<(u16, bool)> = vec![];
    // Opt-in: hold the physical devices with XInput 2 instead.
    let xi_ok = xi2_requested() && {
        use x11rb::protocol::xinput::ConnectionExt as _;
        conn.xinput_xi_query_version(2, 2).ok().and_then(|c| c.reply().ok()).map(|r| r.major_version >= 2).unwrap_or(false)
    };
    let mut last = (i32::MIN, i32::MIN);
    // The pointer is hidden because the shared pointer is on another screen.
    let mut idle_hidden = false;
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
            let next = cmd_rx.try_recv();
            if matches!(next, Ok(Cmd::Grab(..))) && idle_hidden {
                // The grab hides the pointer itself.
                conn.xfixes_show_cursor(root)?;
                idle_hidden = false;
            }
            match next {
                Ok(Cmd::Grab(reply, drag))
                    if !grabbed && carry.is_none() && !drag && xi_ok && {
                        xi = xi_grab(&conn, root, (cx, cy)).unwrap_or_default();
                        !xi.is_empty()
                    } =>
                {
                    grabbed = true;
                    held.clear();
                    conn.xfixes_hide_cursor(root)?;
                    conn.warp_pointer(NONE, root, 0, 0, 0, 0, cx as i16, cy as i16)?;
                    conn.flush()?;
                    let _ = reply.send(true);
                }
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
                Ok(Cmd::IdleCursor(hide)) => {
                    if !grabbed && carry.is_none() && hide != idle_hidden {
                        if hide {
                            conn.xfixes_hide_cursor(root)?;
                            // Moves from here on are the user's own.
                            last = conn
                                .query_pointer(root)
                                .map(|c| c.reply().map(|p| (p.root_x as i32, p.root_y as i32)))
                                .ok()
                                .and_then(|r| r.ok())
                                .unwrap_or(last);
                        } else {
                            conn.xfixes_show_cursor(root)?;
                        }
                        conn.flush()?;
                        idle_hidden = hide;
                    }
                }
                Ok(Cmd::KeyboardPass(on)) => {
                    if grabbed && xi.is_empty() && on != key_pass {
                        if on {
                            conn.ungrab_keyboard(CURRENT_TIME)?;
                        } else {
                            for _ in 0..20 {
                                if conn.grab_keyboard(false, root, CURRENT_TIME, GrabMode::ASYNC, GrabMode::ASYNC)?.reply()?.status == GrabStatus::SUCCESS {
                                    break;
                                }
                                std::thread::sleep(Duration::from_millis(5));
                            }
                        }
                        conn.flush()?;
                        key_pass = on;
                        log::debug!("keyboard {}", if on { "typing straight into a window here" } else { "captured again" });
                    }
                }
                Ok(Cmd::Suspend(reply, pointer)) => {
                    // With XInput 2 our own input already gets through, and so
                    // do keys while the keyboard is passed through.
                    let ok = grabbed && xi.is_empty() && carry.is_none() && (pointer || !key_pass);
                    if ok && suspended.is_none() {
                        if pointer {
                            conn.ungrab_pointer(CURRENT_TIME)?;
                        } else {
                            conn.ungrab_keyboard(CURRENT_TIME)?;
                        }
                        conn.flush()?;
                        let _ = conn.get_input_focus()?.reply();
                        suspended = Some(pointer);
                    }
                    let _ = reply.send(ok);
                }
                Ok(Cmd::Resume) => {
                    if let Some(pointer) = suspended.take() {
                        // The injected events must land before we grab again
                        // (the injector already waited for the X server).
                        std::thread::sleep(Duration::from_millis(2));
                        let _ = conn.get_input_focus()?.reply();
                        if pointer {
                            // Our own clicks are whole (pressed and released
                            // together), so nothing of ours holds the mouse:
                            // take it straight back, even while the physical
                            // button is held (a drag going on over there).
                            retake_pointer(&conn, root, (cx, cy))?;
                            // Buttons let go meanwhile went past us: tell the engine.
                            let mask = u16::from(conn.query_pointer(root)?.reply()?.mask);
                            for b in buttons_down.iter().copied().filter(|b| (1..=3).contains(b) && mask & (0x80 << b) == 0).collect::<Vec<_>>() {
                                buttons_down.remove(&b);
                                if let Some(ev) = button_event(b as u32, false) {
                                    let _ = tx.try_send(ev);
                                }
                            }
                        } else {
                            for _ in 0..20 {
                                if conn.grab_keyboard(false, root, CURRENT_TIME, GrabMode::ASYNC, GrabMode::ASYNC)?.reply()?.status == GrabStatus::SUCCESS {
                                    break;
                                }
                                std::thread::sleep(Duration::from_millis(5));
                            }
                            // Keys let go meanwhile went straight to the window: tell the engine.
                            let keymap = conn.query_keymap()?.reply()?.keys;
                            let gone: Vec<u8> = held.iter().copied().filter(|k| keymap[(*k / 8) as usize] & (1 << (*k % 8)) == 0).collect();
                            for k in gone {
                                key_event(&mut held, &tx, k, false);
                            }
                        }
                        conn.flush()?;
                    }
                }
                Ok(Cmd::Release(x, y)) => {
                    suspended = None;
                    key_pass = false;
                    buttons_down.clear();
                    if let Some(c) = carry.take() {
                        let _ = conn.destroy_window(c.shield);
                        conn.xfixes_show_cursor(root)?;
                    }
                    if grabbed {
                        if xi.is_empty() {
                            conn.ungrab_keyboard(CURRENT_TIME)?;
                            conn.ungrab_pointer(CURRENT_TIME)?;
                        } else {
                            xi_ungrab(&conn, &xi);
                            xi.clear();
                        }
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
                        xi_ungrab(&conn, &xi);
                        let _ = conn.ungrab_keyboard(CURRENT_TIME);
                        let _ = conn.ungrab_pointer(CURRENT_TIME);
                        let _ = conn.xfixes_show_cursor(root);
                        let _ = conn.flush();
                    }
                    return Ok(());
                }
            }
        }

        if suspended.is_some() {
            std::thread::sleep(Duration::from_millis(1));
            continue;
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
                if idle_hidden {
                    // This computer's own mouse moved: show its pointer again.
                    conn.xfixes_show_cursor(root)?;
                    conn.flush()?;
                    idle_hidden = false;
                }
                let _ = tx.try_send(InputEvent::LocalMove { x: pos.0, y: pos.1 });
            }
            while conn.poll_for_event()?.is_some() {}
            std::thread::sleep(Duration::from_millis(4));
            continue;
        }

        // Grabbed: drain events. Motion is measured from the screen centre,
        // and we warp back to the centre after each batch.
        let mut motion: Option<(i32, i32)> = None;
        let mut xi_motion: std::collections::HashMap<u16, (i32, i32)> = Default::default();
        let mut any = false;
        while let Some(ev) = conn.poll_for_event()? {
            any = true;
            let xi_down = matches!(ev, Event::XinputButtonPress(_) | Event::XinputKeyPress(_));
            if !xi.is_empty() {
                log::trace!("grabbed event: {:?}", std::mem::discriminant(&ev));
            }
            match ev {
                Event::MotionNotify(e) => motion = Some((e.root_x as i32, e.root_y as i32)),
                Event::ButtonPress(e) | Event::ButtonRelease(e) => {
                    let down = matches!(ev, Event::ButtonPress(_));
                    // Flush pending motion first so clicks land where expected.
                    if let Some((x, y)) = motion.take() {
                        send_delta(&tx, x - cx, y - cy);
                        conn.warp_pointer(NONE, root, 0, 0, 0, 0, cx as i16, cy as i16)?;
                    }
                    if (1..=3).contains(&e.detail) || (8..=9).contains(&e.detail) {
                        if down {
                            buttons_down.insert(e.detail);
                        } else {
                            buttons_down.remove(&e.detail);
                        }
                    }
                    if let Some(ev) = button_event(e.detail as u32, down) {
                        let _ = tx.try_send(ev);
                    }
                }
                Event::KeyPress(e) | Event::KeyRelease(e) => {
                    key_event(&mut held, &tx, e.detail, matches!(ev, Event::KeyPress(_)));
                }
                Event::XinputMotion(e) => {
                    xi_motion.insert(e.deviceid, ((e.root_x >> 16), (e.root_y >> 16)));
                }
                Event::XinputButtonPress(e) | Event::XinputButtonRelease(e) => {
                    let down = xi_down;
                    for (dev, (x, y)) in xi_motion.drain() {
                        send_delta(&tx, x - cx, y - cy);
                        xi_warp(&conn, root, dev, (cx, cy));
                    }
                    if let Some(ev) = button_event(e.detail, down) {
                        let _ = tx.try_send(ev);
                    }
                }
                Event::XinputKeyPress(e) | Event::XinputKeyRelease(e) => {
                    key_event(&mut held, &tx, e.detail as u8, xi_down);
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
        for (dev, (x, y)) in xi_motion {
            if (x, y) != (cx, cy) {
                send_delta(&tx, x - cx, y - cy);
                xi_warp(&conn, root, dev, (cx, cy));
                conn.flush()?;
            }
        }
        if !any {
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}

/// Grab the mouse again (after letting our own input through) and park it.
fn retake_pointer(conn: &RustConnection, root: Window, (cx, cy): (i32, i32)) -> Result<()> {
    let mask = EventMask::POINTER_MOTION | EventMask::BUTTON_PRESS | EventMask::BUTTON_RELEASE;
    for _ in 0..40 {
        let r = conn.grab_pointer(false, root, mask, GrabMode::ASYNC, GrabMode::ASYNC, NONE, NONE, CURRENT_TIME)?.reply()?;
        if r.status == GrabStatus::SUCCESS {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    conn.warp_pointer(NONE, root, 0, 0, 0, 0, cx as i16, cy as i16)?;
    conn.flush()?;
    // Our own injected motion isn't the user moving the mouse.
    let _ = conn.get_input_focus()?.reply();
    while conn.poll_for_event()?.is_some() {}
    Ok(())
}

fn button_event(detail: u32, down: bool) -> Option<InputEvent> {
    match detail {
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
    }
}

fn key_event(held: &mut std::collections::HashSet<u8>, tx: &Sender<InputEvent>, detail: u8, down: bool) {
    if down && !held.insert(detail) {
        return; // auto-repeat: the remote OS repeats on its own
    }
    if !down {
        held.remove(&detail);
    }
    match keys::hid_from_evdev(detail.saturating_sub(8) as u16) {
        Some(key) => {
            let _ = tx.try_send(InputEvent::Key { key, down });
        }
        None => log::debug!("unmapped X keycode {detail}"),
    }
}

// ------------------------------------------------------------- XInput 2
//
// Grabbing the physical mice and keyboards one by one (instead of the
// core pointer and keyboard) detaches them while OpenHop holds them, so
// OpenHop's own synthetic input (XTEST) still reaches windows. That's what
// lets you click and type into a live window of this computer shown on
// another one, using this computer's keyboard and mouse.

const XI_MASK: u32 = (1 << 2) | (1 << 3) | (1 << 4) | (1 << 5) | (1 << 6); // key, button, motion

/// Physical (non-synthetic) mice and keyboards: (device id, is a pointer).
fn physical_devices(conn: &RustConnection) -> Vec<(u16, bool)> {
    use x11rb::protocol::xinput::{ConnectionExt as _, DeviceType};
    let Ok(Ok(r)) = conn.xinput_xi_query_device(0u16).map(|c| c.reply()) else {
        return vec![];
    };
    r.infos
        .into_iter()
        .filter(|d| d.enabled && (d.type_ == DeviceType::SLAVE_POINTER || d.type_ == DeviceType::SLAVE_KEYBOARD))
        .filter(|d| {
            let name = String::from_utf8_lossy(&d.name);
            // Leave our own synthetic devices and the system buttons alone.
            !name.starts_with("Virtual core XTEST") && !["Power Button", "Sleep Button", "Video Bus", "Lid Switch"].iter().any(|n| name.contains(n))
        })
        .map(|d| (d.deviceid, d.type_ == DeviceType::SLAVE_POINTER))
        .collect()
}

fn xi_ungrab(conn: &RustConnection, devs: &[(u16, bool)]) {
    use x11rb::protocol::xinput::ConnectionExt as _;
    for (id, _) in devs {
        let _ = conn.xinput_xi_ungrab_device(CURRENT_TIME, *id);
    }
    let _ = conn.flush();
}

fn xi_warp(conn: &RustConnection, root: Window, dev: u16, (x, y): (i32, i32)) {
    use x11rb::protocol::xinput::ConnectionExt as _;
    let _ = conn.xinput_xi_warp_pointer(NONE, root, 0, 0, 0, 0, x << 16, y << 16, dev);
}

/// Grab every physical device. None if that isn't possible (then the core grab is used).
fn xi_grab(conn: &RustConnection, root: Window, center: (i32, i32)) -> Option<Vec<(u16, bool)>> {
    use x11rb::protocol::xinput::{ConnectionExt as _, GrabOwner};
    let devs = physical_devices(conn);
    if !devs.iter().any(|d| d.1) || !devs.iter().any(|d| !d.1) {
        return None;
    }
    let mut done = vec![];
    for &(id, ptr) in &devs {
        let ok = conn
            .xinput_xi_grab_device(root, CURRENT_TIME, NONE, id, GrabMode::ASYNC, GrabMode::ASYNC, GrabOwner::NO_OWNER, &[XI_MASK])
            .ok()
            .and_then(|c| c.reply().ok())
            .map(|r| r.status == GrabStatus::SUCCESS)
            .unwrap_or(false);
        if ok {
            done.push((id, ptr));
        } else if ptr {
            // A mouse we can't hold (another app has it): give up on this way.
            xi_ungrab(conn, &done);
            return None;
        }
    }
    for &(id, ptr) in &done {
        if ptr {
            xi_warp(conn, root, id, center);
        }
    }
    let _ = conn.flush();
    log::debug!("holding {} input device(s) with XInput 2", done.len());
    Some(done)
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
    fn sync(&mut self) {
        if let Ok(c) = self.conn.get_input_focus() {
            let _ = c.reply();
        }
    }
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
        let Some(code) = keys::evdev_from_hid(hid) else {
            return Ok(());
        };
        self.fake(if down { KEY_PRESS } else { KEY_RELEASE }, (code + 8) as u8, 0, 0)?;
        self.conn.flush()?;
        Ok(())
    }
    fn screen(&self) -> Rect {
        self.rect
    }
}
