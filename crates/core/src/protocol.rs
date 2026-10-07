//! Messages exchanged between the server (the computer with the physical
//! keyboard and mouse) and its clients.

use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u32 = 2;
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
    /// Local only: files copied in a file manager (absolute paths). Never sent
    /// as-is; the engine turns it into a [`Msg::FileOffer`].
    Files(Vec<String>),
}

/// One file inside an offer. `path` is relative ("folder/clip.mp4", '/'-separated).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FileMeta {
    pub path: String,
    pub size: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OfferKind {
    /// Copied to the clipboard: put the files on the receiver's clipboard.
    Clipboard,
    /// Dragged across screens and dropped: save and offer to open.
    Drop,
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
    /// Clipboard contents copied on `origin`.
    Clip { origin: String, data: ClipData },
    /// Part of a large clipboard image (PNG), reassembled by the receiver.
    ClipPart { origin: String, id: u64, total: u64, data: Vec<u8> },
    Ping,
    Pong,

    // ---- file transfer (relayed by the server between clients) ----
    /// `origin` has files available as `offer`.
    FileOffer { offer: u64, origin: String, kind: OfferKind, files: Vec<FileMeta> },
    /// `requester` wants `offer` from `origin`.
    FileRequest { offer: u64, origin: String, requester: String },
    /// Next chunk of file number `index` (files are streamed in order).
    FileData { offer: u64, dest: String, index: u32, data: Vec<u8> },
    FileEnd { offer: u64, dest: String, error: Option<String> },

    // ---- drag and drop ----
    /// Server -> client: the cursor is leaving you with the left button held.
    /// Reply with the files being dragged, if any.
    DragQuery { id: u64 },
    DragReply { id: u64, offer: Option<u64>, files: Vec<FileMeta> },
    /// Server -> client: the drag was carried to another screen; cancel it locally.
    DragCancel,

    /// Client -> server: details used for Wake-on-LAN.
    Mac(String),
}

impl Msg {
    /// Bulk messages go through a separate, lower-priority queue so mouse and
    /// keyboard never wait behind a file or a big image.
    pub fn is_bulk(&self) -> bool {
        matches!(self, Msg::Clip { .. } | Msg::ClipPart { .. } | Msg::FileData { .. } | Msg::FileEnd { .. })
    }
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
