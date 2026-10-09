//! X11 (and XWayland) windows.

use super::Picture;
use crate::protocol::{Rect, WinInfo};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::OnceLock;
use x11rb::connection::Connection;
use x11rb::protocol::composite::{self, ConnectionExt as _};
use x11rb::protocol::xproto::*;
use x11rb::rust_connection::RustConnection;
use x11rb::NONE;

struct X {
    conn: RustConnection,
    root: Window,
    atoms: HashMap<&'static str, Atom>,
    composite: bool,
    /// Windows we redirected for off-screen pictures.
    redirected: Vec<Window>,
    /// Shared memory for fast pictures (MIT-SHM), if the server offers it.
    shm: Option<Shm>,
    shm_ok: bool,
}

struct Shm {
    seg: u32,
    map: memmap2::MmapMut,
    size: usize,
}

fn x() -> Option<&'static Mutex<X>> {
    static X: OnceLock<Option<Mutex<X>>> = OnceLock::new();
    X.get_or_init(|| {
        let (conn, n) = x11rb::connect(None).ok()?;
        let root = conn.setup().roots[n].root;
        let names = [
            "_NET_CLIENT_LIST_STACKING",
            "_NET_CLIENT_LIST",
            "_NET_WM_NAME",
            "UTF8_STRING",
            "_NET_WM_PID",
            "_NET_WM_WINDOW_TYPE",
            "_NET_WM_WINDOW_TYPE_NORMAL",
            "_NET_WM_WINDOW_TYPE_DIALOG",
            "_NET_ACTIVE_WINDOW",
            "WM_STATE",
            "_NET_WM_STATE",
            "_NET_WM_STATE_HIDDEN",
            "_NET_WM_STATE_MAXIMIZED_VERT",
            "_NET_WM_STATE_MAXIMIZED_HORZ",
            "_NET_WM_STATE_SKIP_TASKBAR",
            "_GTK_FRAME_EXTENTS",
            "_NET_FRAME_EXTENTS",
            "_NET_WM_MOVERESIZE",
            "_NET_WM_WINDOW_OPACITY",
            "_OPENHOP_HIDDEN",
        ];
        let mut atoms = HashMap::new();
        for n in names {
            atoms.insert(n, conn.intern_atom(false, n.as_bytes()).ok()?.reply().ok()?.atom);
        }
        let composite =
            conn.composite_query_version(0, 4).ok().and_then(|c| c.reply().ok()).map(|r| r.major_version > 0 || r.minor_version >= 2).unwrap_or(false);
        let shm_ok = {
            use x11rb::protocol::shm::ConnectionExt as _;
            conn.shm_query_version().ok().and_then(|c| c.reply().ok()).map(|r| (r.major_version, r.minor_version) >= (1, 2)).unwrap_or(false)
        };
        Some(Mutex::new(X { conn, root, atoms, composite, redirected: vec![], shm: None, shm_ok }))
    })
    .as_ref()
}

impl X {
    fn a(&self, n: &str) -> Atom {
        self.atoms.get(n).copied().unwrap_or(NONE)
    }

    fn prop32(&self, w: Window, prop: &str, kind: impl Into<Atom>) -> Vec<u32> {
        self.conn
            .get_property(false, w, self.a(prop), kind, 0, 4096)
            .ok()
            .and_then(|c| c.reply().ok())
            .and_then(|r| r.value32().map(|i| i.collect()))
            .unwrap_or_default()
    }

    fn text(&self, w: Window, prop: Atom, kind: Atom) -> Option<String> {
        let r = self.conn.get_property(false, w, prop, kind, 0, 1024).ok()?.reply().ok()?;
        (!r.value.is_empty()).then(|| String::from_utf8_lossy(&r.value).trim_end_matches('\0').to_string())
    }

    fn title(&self, w: Window) -> String {
        self.text(w, self.a("_NET_WM_NAME"), self.a("UTF8_STRING")).or_else(|| self.text(w, AtomEnum::WM_NAME.into(), AtomEnum::ANY.into())).unwrap_or_default()
    }

    fn app(&self, w: Window) -> String {
        // WM_CLASS is "instance\0Class\0".
        self.text(w, AtomEnum::WM_CLASS.into(), AtomEnum::STRING.into())
            .and_then(|s| s.split('\0').filter(|p| !p.is_empty()).next_back().map(str::to_string))
            .unwrap_or_default()
    }

    /// Client windows, bottom to top.
    fn clients(&self) -> Vec<Window> {
        let mut v = self.prop32(self.root, "_NET_CLIENT_LIST_STACKING", AtomEnum::WINDOW);
        if v.is_empty() {
            v = self.prop32(self.root, "_NET_CLIENT_LIST", AtomEnum::WINDOW);
        }
        if v.is_empty() {
            // No window manager: top-level windows that are shown and named.
            if let Some(tree) = self.conn.query_tree(self.root).ok().and_then(|c| c.reply().ok()) {
                v = tree
                    .children
                    .into_iter()
                    .filter(|&w| {
                        self.conn
                            .get_window_attributes(w)
                            .ok()
                            .and_then(|c| c.reply().ok())
                            .map(|a| a.map_state == MapState::VIEWABLE && !a.override_redirect && a.class == WindowClass::INPUT_OUTPUT)
                            .unwrap_or(false)
                    })
                    .filter(|&w| !self.title(w).is_empty())
                    .collect();
            }
        }
        v
    }

    fn is_listed(&self, w: Window) -> bool {
        let me = std::process::id();
        if self.prop32(w, "_NET_WM_PID", AtomEnum::CARDINAL).first() == Some(&me) {
            return false;
        }
        let types = self.prop32(w, "_NET_WM_WINDOW_TYPE", AtomEnum::ATOM);
        if !types.is_empty() && !types.iter().any(|t| *t == self.a("_NET_WM_WINDOW_TYPE_NORMAL") || *t == self.a("_NET_WM_WINDOW_TYPE_DIALOG")) {
            return false;
        }
        let state = self.prop32(w, "_NET_WM_STATE", AtomEnum::ATOM);
        !state.contains(&self.a("_NET_WM_STATE_HIDDEN")) && !state.contains(&self.a("_NET_WM_STATE_SKIP_TASKBAR"))
    }

    /// The window's visible content area on screen (without GTK's shadow margins).
    fn rect(&self, w: Window) -> Option<Rect> {
        let g = self.conn.get_geometry(w).ok()?.reply().ok()?;
        let t = self.conn.translate_coordinates(w, self.root, 0, 0).ok()?.reply().ok()?;
        let mut r = Rect { x: t.dst_x as i32, y: t.dst_y as i32, w: g.width as i32, h: g.height as i32 };
        let ext = self.prop32(w, "_GTK_FRAME_EXTENTS", AtomEnum::CARDINAL);
        if ext.len() == 4 {
            let (l, rr, top, b) = (ext[0] as i32, ext[1] as i32, ext[2] as i32, ext[3] as i32);
            if l + rr < r.w && top + b < r.h {
                r = Rect { x: r.x + l, y: r.y + top, w: r.w - l - rr, h: r.h - top - b };
            }
        }
        Some(r)
    }

    /// The window manager's title bar and borders: (left, right, top, bottom).
    fn frame_extents(&self, w: Window) -> (i32, i32, i32, i32) {
        let e = self.prop32(w, "_NET_FRAME_EXTENTS", AtomEnum::CARDINAL);
        if e.len() == 4 && e.iter().all(|v| *v < 400) {
            (e[0] as i32, e[1] as i32, e[2] as i32, e[3] as i32)
        } else {
            (0, 0, 0, 0)
        }
    }

    /// The whole window as you see it: content plus the window manager's
    /// title bar and borders.
    fn full_rect(&self, w: Window) -> Option<Rect> {
        let r = self.rect(w)?;
        let (l, rr, t, b) = self.frame_extents(w);
        Some(Rect { x: r.x - l, y: r.y - t, w: r.w + l + rr, h: r.h + t + b })
    }

    /// Height of the title bar at the top of `full_rect`.
    fn bar(&self, w: Window) -> i32 {
        let (_, _, t, _) = self.frame_extents(w);
        if t > 0 {
            t
        } else if self.prop32(w, "_GTK_FRAME_EXTENTS", AtomEnum::CARDINAL).len() == 4 {
            // The app draws its own (GTK header bar).
            46
        } else {
            0
        }
    }

    fn picture(&mut self, client: Window) -> Option<Picture> {
        let r = self.full_rect(client)?;
        // With a window manager frame, take the picture of the frame (it
        // holds the title bar and the app's window).
        let decorated = self.frame_extents(client) != (0, 0, 0, 0);
        let w = if decorated { self.toplevel(client) } else { client };
        let full = self.conn.get_geometry(w).ok()?.reply().ok()?;
        let t = self.conn.translate_coordinates(w, self.root, 0, 0).ok()?.reply().ok()?;
        let (ox, oy) = ((r.x - t.dst_x as i32).max(0) as i16, (r.y - t.dst_y as i32).max(0) as i16);
        let r = Rect { x: r.x, y: r.y, w: r.w.min(full.width as i32 - ox as i32), h: r.h.min(full.height as i32 - oy as i32) };
        if r.w <= 0 || r.h <= 0 {
            return None;
        }
        let depth = full.depth;
        // With Composite the picture is right even if other windows cover it.
        let mut source: Drawable = w;
        let mut pixmap = None;
        if self.composite {
            if !self.redirected.contains(&w) && self.conn.composite_redirect_window(w, composite::Redirect::AUTOMATIC).is_ok() {
                self.redirected.push(w);
            }
            if let Ok(p) = self.conn.generate_id() {
                if self.conn.composite_name_window_pixmap(w, p).is_ok() {
                    pixmap = Some(p);
                    source = p;
                }
            }
        }
        let (pw, ph) = (r.w as u32, r.h as u32);
        let need = (pw * ph * 4) as usize;
        if depth >= 24 && self.shm_ok {
            if let Some(bgra) = self.shm_picture(source, ox, oy, pw as u16, ph as u16, need) {
                if let Some(p) = pixmap {
                    let _ = self.conn.free_pixmap(p);
                }
                return Some(Picture { w: pw, h: ph, bgra });
            }
        }
        let img = self.conn.get_image(ImageFormat::Z_PIXMAP, source, ox, oy, r.w as u16, r.h as u16, !0).ok()?.reply();
        if let Some(p) = pixmap {
            let _ = self.conn.free_pixmap(p);
        }
        let img = match img {
            Ok(i) => i,
            // Fall back to the visible pixels.
            Err(_) => self.conn.get_image(ImageFormat::Z_PIXMAP, w, ox, oy, r.w as u16, r.h as u16, !0).ok()?.reply().ok()?,
        };
        if depth < 24 || img.data.len() < need {
            return None;
        }
        // 32 bits per pixel, BGRX little-endian.
        let mut bgra = img.data;
        bgra.truncate(need);
        Some(Picture { w: pw, h: ph, bgra })
    }

    /// Picture through shared memory: no copying over the X connection.
    fn shm_picture(&mut self, d: Drawable, x: i16, y: i16, w: u16, h: u16, need: usize) -> Option<Vec<u8>> {
        use x11rb::protocol::shm::ConnectionExt as _;
        if self.shm.as_ref().map(|s| s.size < need).unwrap_or(true) {
            if let Some(old) = self.shm.take() {
                let _ = self.conn.shm_detach(old.seg);
            }
            let size = need.max(8 << 20).next_power_of_two();
            let seg = self.conn.generate_id().ok()?;
            let made = self.conn.shm_create_segment(seg, size as u32, false).ok().and_then(|c| c.reply().ok());
            let Some(reply) = made else {
                self.shm_ok = false;
                return None;
            };
            let file = std::fs::File::from(reply.shm_fd);
            let map = unsafe { memmap2::MmapOptions::new().len(size).map_mut(&file).ok()? };
            self.shm = Some(Shm { seg, map, size });
        }
        let seg = self.shm.as_ref()?.seg;
        let r = self.conn.shm_get_image(d, x, y, w, h, !0, ImageFormat::Z_PIXMAP.into(), seg, 0).ok()?.reply().ok()?;
        if (r.size as usize) < need {
            return None;
        }
        Some(self.shm.as_ref()?.map[..need].to_vec())
    }

    /// The window manager's frame around a client window (or the window itself).
    fn toplevel(&self, w: Window) -> Window {
        let mut cur = w;
        for _ in 0..16 {
            let Some(t) = self.conn.query_tree(cur).ok().and_then(|c| c.reply().ok()) else {
                break;
            };
            if t.parent == self.root || t.parent == NONE {
                return cur;
            }
            cur = t.parent;
        }
        w
    }
}

/// Processes with windows in the taskbar or dock (minimized ones too).
pub fn taskbar_pids() -> Vec<u32> {
    let Some(x) = x() else { return vec![] };
    let x = x.lock();
    let me = std::process::id();
    x.prop32(x.root, "_NET_CLIENT_LIST", AtomEnum::WINDOW)
        .into_iter()
        .filter(|&w| {
            let types = x.prop32(w, "_NET_WM_WINDOW_TYPE", AtomEnum::ATOM);
            let normal = types.is_empty() || types.iter().any(|t| *t == x.a("_NET_WM_WINDOW_TYPE_NORMAL") || *t == x.a("_NET_WM_WINDOW_TYPE_DIALOG"));
            normal && !x.prop32(w, "_NET_WM_STATE", AtomEnum::ATOM).contains(&x.a("_NET_WM_STATE_SKIP_TASKBAR"))
        })
        .filter_map(|w| x.prop32(w, "_NET_WM_PID", AtomEnum::CARDINAL).first().copied())
        .filter(|&p| p != me)
        .collect()
}

pub fn list() -> Vec<WinInfo> {
    let Some(x) = x() else { return vec![] };
    let x = x.lock();
    let mut out: Vec<WinInfo> = x
        .clients()
        .into_iter()
        .filter(|&w| x.is_listed(w))
        .filter_map(|w| {
            let r = x.full_rect(w)?;
            let pid = x.prop32(w, "_NET_WM_PID", AtomEnum::CARDINAL).first().copied().unwrap_or(0);
            Some(WinInfo { id: w as u64, title: x.title(w), app: x.app(w), w: r.w, h: r.h, pid })
        })
        .filter(|w| !w.title.is_empty())
        .collect();
    out.reverse(); // front-most first
    out
}

pub fn capture(id: u64) -> Option<Picture> {
    x()?.lock().picture(id as Window)
}

pub fn geometry(id: u64) -> Option<Rect> {
    x()?.lock().full_rect(id as Window)
}

pub fn bar_height(id: u64) -> i32 {
    x().map(|x| x.lock().bar(id as Window)).unwrap_or(0)
}

pub fn raise(id: u64) {
    let Some(x) = x() else { return };
    let x = x.lock();
    let top = x.toplevel(id as Window);
    let _ = x.conn.configure_window(top, &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE));
    let _ = x.conn.flush();
}

/// Put the window's top-left corner (title bar included) at (px, py).
pub fn move_to(id: u64, px: i32, py: i32) {
    let Some(x) = x() else { return };
    let x = x.lock();
    let w = id as Window;
    // Positions given to the window manager are the frame's corner
    // (NorthWest gravity); GTK windows also have invisible shadow margins.
    let ext = x.prop32(w, "_GTK_FRAME_EXTENTS", AtomEnum::CARDINAL);
    let (sl, st) = if ext.len() == 4 { (ext[0] as i32, ext[2] as i32) } else { (0, 0) };
    let _ = x.conn.configure_window(w, &ConfigureWindowAux::new().x(px - sl).y(py - st));
    let _ = x.conn.flush();
}

pub fn is_maximized(id: u64) -> bool {
    let Some(x) = x() else { return false };
    let x = x.lock();
    let st = x.prop32(id as Window, "_NET_WM_STATE", AtomEnum::ATOM);
    st.contains(&x.a("_NET_WM_STATE_MAXIMIZED_VERT")) && st.contains(&x.a("_NET_WM_STATE_MAXIMIZED_HORZ"))
}

pub fn unmaximize(id: u64) {
    let Some(x) = x() else { return };
    let x = x.lock();
    // _NET_WM_STATE_REMOVE both maximized states.
    let ev =
        ClientMessageEvent::new(32, id as Window, x.a("_NET_WM_STATE"), [0, x.a("_NET_WM_STATE_MAXIMIZED_VERT"), x.a("_NET_WM_STATE_MAXIMIZED_HORZ"), 1, 0]);
    let _ = x.conn.send_event(false, x.root, EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY, ev);
    let _ = x.conn.flush();
}

/// The mouse button is held over the title bar: let the window manager
/// carry the window with the mouse from here.
pub fn begin_move(id: u64, px: i32, py: i32) {
    let Some(x) = x() else { return };
    let x = x.lock();
    let w = id as Window;
    // _NET_WM_MOVERESIZE_MOVE with button 1, from a normal application.
    let _ = x.conn.ungrab_pointer(x11rb::CURRENT_TIME);
    let ev = ClientMessageEvent::new(32, w, x.a("_NET_WM_MOVERESIZE"), [px as u32, py as u32, 8, 1, 1]);
    let _ = x.conn.send_event(false, x.root, EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY, ev);
    let _ = x.conn.flush();
}

pub fn activate(id: u64) {
    let Some(x) = x() else { return };
    let x = x.lock();
    let w = id as Window;
    let ev = ClientMessageEvent::new(32, w, x.a("_NET_ACTIVE_WINDOW"), [2, x11rb::CURRENT_TIME, 0, 0, 0]);
    let _ = x.conn.send_event(false, x.root, EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY, ev);
    let _ = x.conn.configure_window(w, &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE));
    let _ = x.conn.set_input_focus(InputFocus::PARENT, w, x11rb::CURRENT_TIME);
    let _ = x.conn.flush();
}

pub fn titlebar_window_at(px: i32, py: i32) -> Option<u64> {
    let x = x()?;
    let x = x.lock();
    // Front-most listed window containing the point.
    let clients: Vec<Window> = x.clients().into_iter().rev().filter(|&w| x.is_listed(w)).collect();
    for w in clients {
        let Some(r) = x.full_rect(w) else { continue };
        // The title bar is drawn by the window manager at the top of the
        // window, or by the app itself (GTK header bars).
        if px >= r.x - 2 && px <= r.x + r.w + 2 && py >= r.y - 2 && py <= r.y + r.h {
            let bar = match x.bar(w) {
                0 => 50,
                b => b + 4,
            };
            if py < r.y + bar {
                return Some(w as u64);
            }
            return None;
        }
    }
    None
}

pub fn set_hidden(id: u64, hidden: bool) {
    use x11rb::wrapper::ConnectionExt as _;
    let Some(x) = x() else { return };
    let x = x.lock();
    let w = id as Window;
    let top = x.toplevel(w);
    let (op, mark) = (x.a("_NET_WM_WINDOW_OPACITY"), x.a("_OPENHOP_HIDDEN"));
    for win in [w, top] {
        if hidden {
            // Fully transparent (with a compositor, as on GNOME and KDE).
            let _ = x.conn.change_property32(PropMode::REPLACE, win, op, AtomEnum::CARDINAL, &[0]);
        } else {
            let _ = x.conn.delete_property(win, op);
        }
    }
    if hidden {
        let _ = x.conn.change_property32(PropMode::REPLACE, w, mark, AtomEnum::CARDINAL, &[1]);
    } else {
        let _ = x.conn.delete_property(w, mark);
    }
    let _ = x.conn.flush();
}

pub fn lower(id: u64) {
    let Some(x) = x() else { return };
    let x = x.lock();
    let top = x.toplevel(id as Window);
    let _ = x.conn.configure_window(top, &ConfigureWindowAux::new().stack_mode(StackMode::BELOW));
    let _ = x.conn.flush();
}

pub fn resize(id: u64, w: i32, h: i32) {
    let Some(x) = x() else { return };
    let x = x.lock();
    let win = id as Window;
    // `w`×`h` includes the title bar and borders; GTK windows also have
    // shadows around them that aren't part of what's shown.
    let ext = x.prop32(win, "_GTK_FRAME_EXTENTS", AtomEnum::CARDINAL);
    let (ew, eh) = if ext.len() == 4 { ((ext[0] + ext[1]) as i32, (ext[2] + ext[3]) as i32) } else { (0, 0) };
    let (l, r, t, b) = x.frame_extents(win);
    let (cw, ch) = (w - l - r + ew, h - t - b + eh);
    if cw > 0 && ch > 0 {
        let _ = x.conn.configure_window(win, &ConfigureWindowAux::new().width(cw as u32).height(ch as u32));
    }
    let _ = x.conn.flush();
}

pub fn restore_all() {
    let Some(xm) = x() else { return };
    let hidden: Vec<Window> = {
        let x = xm.lock();
        x.clients().into_iter().filter(|&w| !x.prop32(w, "_OPENHOP_HIDDEN", AtomEnum::CARDINAL).is_empty()).collect()
    };
    for w in hidden {
        log::info!("showing a window a previous run had hidden");
        set_hidden(w as u64, false);
    }
}
