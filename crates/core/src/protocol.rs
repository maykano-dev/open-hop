//! Messages exchanged between the server (the computer with the physical
//! keyboard and mouse) and its clients.

use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u32 = 1;
pub const DEFAULT_PORT: u16 = 24850;
pub const DISCOVERY_PORT: u16 = 24851;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Os {
    Windows,
    MacOs,
    Linux,
    Other,
}

impl Os {
    pub fn current() -> Os {
        if cfg!(windows) {
            Os::Windows
        } else if cfg!(target_os = "macos") {
            Os::MacOs
        } else if cfg!(target_os = "linux") {
            Os::Linux
        } else {
            Os::Other
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Os::Windows => "Windows",
            Os::MacOs => "macOS",
            Os::Linux => "Linux",
            Os::Other => "Other",
        }
    }
}

/// A desktop's bounding box in native coordinates (all monitors combined).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub fn right(&self) -> i32 {
        self.x + self.w - 1
    }
    pub fn bottom(&self) -> i32 {
        self.y + self.h - 1
    }
    pub fn center(&self) -> (i32, i32) {
        (self.x + self.w / 2, self.y + self.h / 2)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
    Back,
    Forward,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ClipData {
    Text(String),
    /// PNG-encoded image.
    Png(Vec<u8>),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Msg {
    /// First message from a client after the encrypted handshake.
    Hello { version: u32, name: String, os: Os, screen: Rect },
    /// Server's reply.
    Welcome { version: u32, name: String, os: Os },
    /// Client's desktop size changed (monitor plugged/unplugged).
    Screen(Rect),
    /// Cursor enters this client at (x, y), coordinates relative to the client's screen origin.
    Enter { x: i32, y: i32 },
    /// Cursor left this client; release anything held.
    Leave,
    /// Absolute cursor position, relative to the client's screen origin.
    Move { x: i32, y: i32 },
    Button { button: MouseButton, down: bool },
    /// Scroll amount in 1/120ths of a notch (Windows WHEEL_DELTA units). Positive = up / right.
    Wheel { dx: i32, dy: i32 },
    /// USB HID usage id.
    Key { key: u16, down: bool },
    Clipboard(ClipData),
    Ping,
    Pong,
}

pub fn encode(msg: &Msg) -> Vec<u8> {
    bincode::serialize(msg).expect("serialize")
}

pub fn decode(bytes: &[u8]) -> anyhow::Result<Msg> {
    Ok(bincode::deserialize(bytes)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn roundtrip() {
        let m = Msg::Hello {
            version: 1,
            name: "pc".into(),
            os: Os::Linux,
            screen: Rect { x: 0, y: 0, w: 1920, h: 1080 },
        };
        assert_eq!(decode(&encode(&m)).unwrap(), m);
        // Mouse moves are the hot path: keep them tiny.
        assert!(encode(&Msg::Move { x: 100, y: 200 }).len() <= 12);
    }
}
