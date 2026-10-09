//! Screen arrangement and edge-crossing maths.
//!
//! Screens sit on a grid. The server is always at (0, 0); a client at (1, 0)
//! is to its right, (0, -1) above it, and so on. Moving off an edge goes to
//! whichever screen occupies the neighbouring cell.

use crate::protocol::Rect;
use serde::{Deserialize, Serialize};

/// Name used for the server's own screen in layout lookups.
pub const SERVER: &str = "@server";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Side {
    Left,
    Right,
    Top,
    Bottom,
}

impl Side {
    pub fn opposite(self) -> Side {
        match self {
            Side::Left => Side::Right,
            Side::Right => Side::Left,
            Side::Top => Side::Bottom,
            Side::Bottom => Side::Top,
        }
    }
    fn offset(self) -> (i32, i32) {
        match self {
            Side::Left => (-1, 0),
            Side::Right => (1, 0),
            Side::Top => (0, -1),
            Side::Bottom => (0, 1),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScreenPos {
    pub name: String,
    pub x: i32,
    pub y: i32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Layout {
    #[serde(default)]
    pub screens: Vec<ScreenPos>,
}

impl Layout {
    pub fn position(&self, name: &str) -> Option<(i32, i32)> {
        if name == SERVER {
            return Some((0, 0));
        }
        self.screens.iter().find(|s| s.name == name).map(|s| (s.x, s.y))
    }

    pub fn at(&self, x: i32, y: i32) -> Option<&str> {
        if (x, y) == (0, 0) {
            return Some(SERVER);
        }
        self.screens.iter().find(|s| s.x == x && s.y == y).map(|s| s.name.as_str())
    }

    /// Put `name` at (x, y). Anything already in that cell is moved to the
    /// cell `name` came from (so dragging onto a neighbour swaps them).
    pub fn place(&mut self, name: &str, x: i32, y: i32) {
        if name == SERVER || (x, y) == (0, 0) {
            return;
        }
        let old = self.position(name);
        self.screens.retain(|s| s.name != name);
        let displaced = self.screens.iter().position(|s| s.x == x && s.y == y).map(|i| self.screens.remove(i));
        self.screens.push(ScreenPos { name: name.into(), x, y });
        if let Some(d) = displaced {
            match old {
                Some((ox, oy)) => self.screens.push(ScreenPos { name: d.name, x: ox, y: oy }),
                None => {
                    self.auto_place(&d.name);
                }
            }
        }
    }

    pub fn remove(&mut self, name: &str) {
        self.screens.retain(|s| s.name != name);
    }

    /// Place a newly seen screen in the first free cell: right of the
    /// server, then left, then further right, etc. Returns its position.
    pub fn auto_place(&mut self, name: &str) -> (i32, i32) {
        if let Some(p) = self.position(name) {
            return p;
        }
        for dist in 1..64 {
            for (x, y) in [(dist, 0), (-dist, 0), (0, -dist), (0, dist)] {
                if self.at(x, y).is_none() {
                    self.screens.push(ScreenPos { name: name.into(), x, y });
                    return (x, y);
                }
            }
        }
        unreachable!("layout full")
    }

    pub fn neighbor(&self, name: &str, side: Side) -> Option<&str> {
        let (x, y) = self.position(name)?;
        let (dx, dy) = side.offset();
        self.at(x + dx, y + dy)
    }
}

/// Which edge (if any) a local cursor position is touching.
pub fn touching_edge(r: &Rect, x: i32, y: i32) -> Option<Side> {
    if x <= r.x {
        Some(Side::Left)
    } else if x >= r.right() {
        Some(Side::Right)
    } else if y <= r.y {
        Some(Side::Top)
    } else if y >= r.bottom() {
        Some(Side::Bottom)
    } else {
        None
    }
}

/// Is this point on one of the monitors?
fn covered(mons: &[Rect], x: i32, y: i32) -> bool {
    mons.iter().any(|m| x >= m.x && x <= m.right() && y >= m.y && y <= m.bottom())
}

/// Like [`touching_edge`], but also at a monitor's own edge where there's no
/// other monitor beyond it (a smaller monitor beside a bigger one, or two
/// monitors offset from each other): those count as the desktop's edge too.
pub fn touching_edge_mon(r: &Rect, mons: &[Rect], x: i32, y: i32) -> Option<Side> {
    if let Some(s) = touching_edge(r, x, y) {
        return Some(s);
    }
    let m = mons.iter().find(|m| x >= m.x && x <= m.right() && y >= m.y && y <= m.bottom())?;
    if x <= m.x && !covered(mons, x - 1, y) {
        Some(Side::Left)
    } else if x >= m.right() && !covered(mons, x + 1, y) {
        Some(Side::Right)
    } else if y <= m.y && !covered(mons, x, y - 1) {
        Some(Side::Top)
    } else if y >= m.bottom() && !covered(mons, x, y + 1) {
        Some(Side::Bottom)
    } else {
        None
    }
}

/// [`apply_delta`] on a computer with several monitors (`mons`, relative to
/// its desktop's top-left): moving off a monitor where no other monitor
/// continues leaves through that side; otherwise the pointer stays on the
/// monitor it was on (as the system itself would keep it).
pub fn apply_delta_mon(w: i32, h: i32, mons: &[Rect], x: i32, y: i32, dx: i32, dy: i32) -> Result<(i32, i32), (Side, f64)> {
    let r = apply_delta(w, h, x, y, dx, dy)?;
    if mons.len() < 2 || covered(mons, r.0, r.1) {
        return Ok(r);
    }
    let Some(m) = mons.iter().find(|m| covered(std::slice::from_ref(m), x, y)) else { return Ok(r) };
    let (nx, ny) = r;
    let fy = |v: i32| (v.clamp(0, h - 1)) as f64 / (h.max(2) - 1) as f64;
    let fx = |v: i32| (v.clamp(0, w - 1)) as f64 / (w.max(2) - 1) as f64;
    let (cx, cy) = (nx.clamp(m.x, m.right()), ny.clamp(m.y, m.bottom()));
    if nx < m.x && !covered(mons, m.x - 1, cy) {
        return Err((Side::Left, fy(cy)));
    }
    if nx > m.right() && !covered(mons, m.right() + 1, cy) {
        return Err((Side::Right, fy(cy)));
    }
    if ny < m.y && !covered(mons, cx, m.y - 1) {
        return Err((Side::Top, fx(cx)));
    }
    if ny > m.bottom() && !covered(mons, cx, m.bottom() + 1) {
        return Err((Side::Bottom, fx(cx)));
    }
    Ok((cx, cy))
}

/// Fraction (0..1) along the edge that was crossed.
pub fn edge_fraction(r: &Rect, side: Side, x: i32, y: i32) -> f64 {
    let f = match side {
        Side::Left | Side::Right => (y - r.y) as f64 / (r.h.max(2) - 1) as f64,
        Side::Top | Side::Bottom => (x - r.x) as f64 / (r.w.max(2) - 1) as f64,
    };
    f.clamp(0.0, 1.0)
}

/// Where to put the cursor on `target` (coordinates relative to its origin)
/// after leaving the previous screen through `exit_side` at `frac`.
/// `inset` keeps the cursor a few pixels away from the edge it entered by.
pub fn entry_point(target_w: i32, target_h: i32, exit_side: Side, frac: f64, inset: i32) -> (i32, i32) {
    let along_x = (frac * (target_w - 1) as f64).round() as i32;
    let along_y = (frac * (target_h - 1) as f64).round() as i32;
    match exit_side {
        // Left the previous screen through its right edge -> enter on our left edge.
        Side::Right => (inset, along_y),
        Side::Left => (target_w - 1 - inset, along_y),
        Side::Bottom => (along_x, inset),
        Side::Top => (along_x, target_h - 1 - inset),
    }
}

/// Apply a delta to a virtual cursor on a remote screen of size w x h.
/// Returns the new position, or the side crossed (with fraction) if the
/// movement leaves the screen.
pub fn apply_delta(w: i32, h: i32, x: i32, y: i32, dx: i32, dy: i32) -> Result<(i32, i32), (Side, f64)> {
    let nx = x + dx;
    let ny = y + dy;
    let fy = |v: i32| (v.clamp(0, h - 1)) as f64 / (h.max(2) - 1) as f64;
    let fx = |v: i32| (v.clamp(0, w - 1)) as f64 / (w.max(2) - 1) as f64;
    if nx < 0 {
        Err((Side::Left, fy(ny)))
    } else if nx > w - 1 {
        Err((Side::Right, fy(ny)))
    } else if ny < 0 {
        Err((Side::Top, fx(nx)))
    } else if ny > h - 1 {
        Err((Side::Bottom, fx(nx)))
    } else {
        Ok((nx, ny))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn monitor_edges() {
        // A 1920x1080 monitor, and a 1280x720 one to its right, top-aligned.
        let mons = [Rect { x: 0, y: 0, w: 1920, h: 1080 }, Rect { x: 1920, y: 0, w: 1280, h: 720 }];
        let desk = Rect { x: 0, y: 0, w: 3200, h: 1080 };
        // Right edge of the small one: the desktop's edge.
        assert_eq!(touching_edge_mon(&desk, &mons, 3199, 100), Some(Side::Right));
        // Bottom of the small one, nothing below: an edge now.
        assert_eq!(touching_edge_mon(&desk, &mons, 2500, 719), Some(Side::Bottom));
        // Between the two monitors: not an edge.
        assert_eq!(touching_edge_mon(&desk, &mons, 1919, 300), None);
        // Right edge of the big one below the small one: an edge.
        assert_eq!(touching_edge_mon(&desk, &mons, 1919, 900), Some(Side::Right));
        // One monitor: as before.
        assert_eq!(touching_edge_mon(&desk, &mons[..1], 1000, 500), None);

        // Driving a remote pointer: from the big monitor rightwards below the small one.
        assert_eq!(apply_delta_mon(3200, 1080, &mons, 1900, 900, 40, 0).unwrap_err().0, Side::Right);
        // Into the small one: fine.
        assert_eq!(apply_delta_mon(3200, 1080, &mons, 1900, 300, 40, 0), Ok((1940, 300)));
        // Down off the small one: leaves through the bottom.
        assert_eq!(apply_delta_mon(3200, 1080, &mons, 2500, 715, 0, 20).unwrap_err().0, Side::Bottom);
    }

    #[test]
    fn auto_place_and_neighbors() {
        let mut l = Layout::default();
        assert_eq!(l.auto_place("mac"), (1, 0));
        assert_eq!(l.auto_place("linux"), (-1, 0));
        assert_eq!(l.auto_place("mac"), (1, 0));
        assert_eq!(l.neighbor(SERVER, Side::Right), Some("mac"));
        assert_eq!(l.neighbor("mac", Side::Left), Some(SERVER));
        assert_eq!(l.neighbor("mac", Side::Right), None);
        assert_eq!(l.neighbor(SERVER, Side::Left), Some("linux"));
    }

    #[test]
    fn place_swaps() {
        let mut l = Layout::default();
        l.auto_place("a"); // (1,0)
        l.auto_place("b"); // (-1,0)
        l.place("a", -1, 0);
        assert_eq!(l.position("a"), Some((-1, 0)));
        assert_eq!(l.position("b"), Some((1, 0)));
        l.place("c", 1, 0); // new screen onto occupied cell -> b moves elsewhere
        assert_eq!(l.position("c"), Some((1, 0)));
        assert!(l.position("b").is_some());
        assert_ne!(l.position("b"), Some((1, 0)));
    }

    #[test]
    fn edges() {
        let r = Rect { x: 0, y: 0, w: 1920, h: 1080 };
        assert_eq!(touching_edge(&r, 1919, 500), Some(Side::Right));
        assert_eq!(touching_edge(&r, 0, 500), Some(Side::Left));
        assert_eq!(touching_edge(&r, 10, 10), None);
        let f = edge_fraction(&r, Side::Right, 1919, 1079);
        assert!((f - 1.0).abs() < 1e-9);
        // Exit server's right edge halfway down -> enter client's left edge halfway down.
        let (x, y) = entry_point(2560, 1440, Side::Right, 0.5, 1);
        assert_eq!(x, 1);
        assert_eq!(y, 720);
    }

    #[test]
    fn deltas() {
        assert_eq!(apply_delta(100, 100, 50, 50, 10, -5), Ok((60, 45)));
        assert_eq!(apply_delta(100, 100, 2, 50, -5, 0), Err((Side::Left, 50.0 / 99.0)));
        assert!(matches!(apply_delta(100, 100, 50, 98, 0, 5), Err((Side::Bottom, _))));
    }
}
