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
            "_NET_WM_STATE_SKIP_TASKBAR",
            "_GTK_FRAME_EXTENTS",
        ];
        let mut atoms = HashMap::new();
        for n in names {
            atoms.insert(n, conn.intern_atom(false, n.as_bytes()).ok()?.reply().ok()?.atom);
        }
        let composite = conn
            .composite_query_version(0, 4)
            .ok()
            .and_then(|c| c.reply().ok())
            .map(|r| r.major_version > 0 || r.minor_version >= 2)
            .unwrap_or(false);
        Some(Mutex::new(X { conn, root, atoms, composite, redirected: vec![] }))
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
        self.text(w, self.a("_NET_WM_NAME"), self.a("UTF8_STRING"))
            .or_else(|| self.text(w, AtomEnum::WM_NAME.into(), AtomEnum::ANY.into()))
            .unwrap_or_default()
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

    fn picture(&mut self, w: Window) -> Option<Picture> {
        let full = self.conn.get_geometry(w).ok()?.reply().ok()?;
        let r = self.rect(w)?;
        let t = self.conn.translate_coordinates(w, self.root, 0, 0).ok()?.reply().ok()?;
        let (ox, oy) = ((r.x - t.dst_x as i32) as i16, (r.y - t.dst_y as i32) as i16);
        let depth = full.depth;
        // With Composite the picture is right even if other windows cover it.
        let mut source: Drawable = w;
        let mut pixmap = None;
        if self.composite {
            if !self.redirected.contains(&w)
                && self.conn.composite_redirect_window(w, composite::Redirect::AUTOMATIC).is_ok() {
                    self.redirected.push(w);
                }
            if let Ok(p) = self.conn.generate_id() {
                if self.conn.composite_name_window_pixmap(w, p).is_ok() {
                    pixmap = Some(p);
                    source = p;
                }
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
        let (pw, ph) = (r.w as u32, r.h as u32);
        if depth < 24 || img.data.len() < (pw * ph * 4) as usize {
            return None;
        }
        // 32 bits per pixel, BGRX little-endian.
        let mut rgba = Vec::with_capacity((pw * ph * 4) as usize);
        for px in img.data.chunks_exact(4).take((pw * ph) as usize) {
            rgba.extend_from_slice(&[px[2], px[1], px[0], 255]);
        }
        Some(Picture { w: pw, h: ph, rgba })
    }
}

pub fn list() -> Vec<WinInfo> {
    let Some(x) = x() else { return vec![] };
    let x = x.lock();
    let mut out: Vec<WinInfo> = x
        .clients()
        .into_iter()
        .filter(|&w| x.is_listed(w))
        .filter_map(|w| {
            let r = x.rect(w)?;
            Some(WinInfo { id: w as u64, title: x.title(w), app: x.app(w), w: r.w, h: r.h })
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
    x()?.lock().rect(id as Window)
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
        let Some(r) = x.rect(w) else { continue };
        // The title bar is drawn by the window manager just above the window,
        // or by the app itself (GTK header bars) in its top 50 pixels.
        if px >= r.x - 2 && px <= r.x + r.w + 2 && py >= r.y - 48 && py <= r.y + r.h {
            if py < r.y + 50 {
                return Some(w as u64);
            }
            return None;
        }
    }
    None
}
