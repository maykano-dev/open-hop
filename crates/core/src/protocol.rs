//! Messages exchanged between the server (the computer with the physical
//! keyboard and mouse) and its clients.

use crate::layout::{Layout, Side};
use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u32 = 8;
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
    Hello {
        version: u32,
        name: String,
        os: Os,
        screen: Rect,
    },
    /// Server's reply.
    Welcome {
        version: u32,
        name: String,
        os: Os,
    },
    /// Client's desktop size changed (monitor plugged/unplugged).
    Screen(Rect),
    /// Cursor enters this client at (x, y), coordinates relative to the client's screen origin.
    Enter {
        x: i32,
        y: i32,
    },
    /// Cursor left this client; release anything held.
    Leave,
    /// Absolute cursor position, relative to the client's screen origin.
    Move {
        x: i32,
        y: i32,
    },
    Button {
        button: MouseButton,
        down: bool,
    },
    /// Scroll amount in 1/120ths of a notch (Windows WHEEL_DELTA units). Positive = up / right.
    Wheel {
        dx: i32,
        dy: i32,
    },
    /// USB HID usage id.
    Key {
        key: u16,
        down: bool,
    },
    /// Clipboard contents copied on `origin`.
    Clip {
        origin: String,
        data: ClipData,
    },
    /// Part of a large clipboard image (PNG), reassembled by the receiver.
    ClipPart {
        origin: String,
        id: u64,
        total: u64,
        data: Vec<u8>,
    },
    Ping,
    Pong,

    // ---- file transfer (relayed by the server between clients) ----
    /// `origin` has files available as `offer`.
    FileOffer {
        offer: u64,
        origin: String,
        kind: OfferKind,
        files: Vec<FileMeta>,
    },
    /// `requester` wants `offer` from `origin`.
    FileRequest {
        offer: u64,
        origin: String,
        requester: String,
    },
    /// Next chunk of file number `index` (files are streamed in order).
    FileData {
        offer: u64,
        dest: String,
        index: u32,
        data: Vec<u8>,
    },
    FileEnd {
        offer: u64,
        dest: String,
        error: Option<String>,
    },

    // ---- drag and drop ----
    /// Server -> client: the cursor is leaving you with the left button held.
    /// Reply with the files being dragged, if any.
    DragQuery {
        id: u64,
    },
    DragReply {
        id: u64,
        offer: Option<u64>,
        files: Vec<FileMeta>,
    },
    /// Server -> client: the drag was carried to another screen; cancel it locally.
    DragCancel,

    /// Client -> server: details used for Wake-on-LAN.
    Mac(String),
    /// Server -> client after pairing with a code: the key to use from now on.
    Paired {
        server_device: String,
        key: String,
    },
    /// Client -> server, answering a [`Msg::DragQuery`] when a *window* (not
    /// files) was being dragged by its title bar: it can be opened live on
    /// the computer it was dragged to.
    DragWindow {
        id: u64,
        window: u64,
    },
    /// Client -> server, answering a [`Msg::DragQuery`] when a *live window*
    /// (shown on the client) was being dragged by its title bar: it goes on
    /// to the computer it was dragged to (or home, if that's its own).
    DragViewer {
        id: u64,
        stream: u64,
    },

    // ---- every computer can drive the others ----
    /// Client -> server: this computer's own mouse reached its screen edge.
    EdgeHit {
        side: Side,
        frac: f64,
    },
    /// Server -> client: your keyboard and mouse now drive the others:
    /// capture them and send [`Msg::Drive`].
    DriveStart,
    /// Server -> client: the pointer came back to your screen at (x, y): let
    /// your keyboard and mouse work here again. (-1, -1): another computer's
    /// mouse took over; stop, and keep your pointer hidden.
    DriveStop {
        x: i32,
        y: i32,
    },
    /// Client -> server: input from this computer's own keyboard and mouse.
    Drive(DriveEv),
    /// Client -> server: this computer's own mouse moved the pointer (while
    /// another computer was using this screen), to (x, y) on its screen.
    LocalPos {
        x: i32,
        y: i32,
    },
    /// Server -> clients: the arrangement and how to add a computer, so
    /// every computer can show (and change) them.
    Group {
        layout: Layout,
        code: String,
        hub: String,
    },
    /// Client -> server: change the arrangement.
    SetLayout(Layout),

    /// Everything beyond keyboard, mouse, clipboard and files, addressed by
    /// computer name. `to` is "*" for everyone. The server forwards these.
    Ext {
        to: String,
        from: String,
        ext: Ext,
    },
}

/// Keyboard and mouse input from a client's own devices.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum DriveEv {
    Delta { dx: i32, dy: i32 },
    Button { button: MouseButton, down: bool },
    Wheel { dx: i32, dy: i32 },
    Key { key: u16, down: bool },
}

/// A window open on some computer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WinInfo {
    pub id: u64,
    pub title: String,
    pub app: String,
    pub w: i32,
    pub h: i32,
}

/// Input aimed at a live window, in the streamed picture's pixels.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum WinEvent {
    Move {
        x: i32,
        y: i32,
    },
    Button {
        button: MouseButton,
        down: bool,
        x: i32,
        y: i32,
    },
    Wheel {
        dx: i32,
        dy: i32,
        x: i32,
        y: i32,
    },
    Key {
        key: u16,
        down: bool,
    },
    /// The viewer window got the focus: bring the real window forward.
    Focus,
    /// The viewer window lost the focus.
    Blur,
    /// The viewer window was resized: resize the real one to match.
    Resize {
        w: i32,
        h: i32,
    },
}

/// Part of a live window's picture (JPEG) at (x, y). Only what changed is
/// sent, so typing a letter costs a few hundred bytes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Patch {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
    pub jpeg: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Ext {
    /// Dark mode was turned on or off on the sender.
    Theme {
        dark: bool,
    },
    /// The sender is in Do Not Disturb or presenting (or stopped being).
    Quiet {
        on: bool,
    },
    /// The windows open on the sender (sent when they change).
    Windows {
        list: Vec<WinInfo>,
    },
    /// Viewer -> owner: stream `window` to me as `stream`.
    WinOpen {
        stream: u64,
        window: u64,
        os: Os,
    },
    /// Ask a computer to open `origin`'s window here (a window dragged across).
    WinOffer {
        origin: String,
        window: u64,
    },
    /// Owner -> viewer: what changed in the window. `w`×`h` is the whole
    /// picture (the window with its own title bar); a patch covering all of
    /// it starts afresh. The top `bar` pixels are the title bar.
    WinFrame {
        stream: u64,
        seq: u64,
        w: u32,
        h: u32,
        bar: u32,
        title: String,
        patches: Vec<Patch>,
    },
    /// Viewer -> owner: picture received; send the next one.
    WinAck {
        stream: u64,
        seq: u64,
    },
    WinInput {
        stream: u64,
        ev: WinEvent,
    },
    /// Either side: the live window was closed.
    WinClose {
        stream: u64,
    },
    /// Either side: pause or resume the picture.
    WinPause {
        stream: u64,
        paused: bool,
        reason: String,
    },
    /// Server -> the computer showing live window `stream`: it was dragged
    /// onto computer `to`'s screen; move it there.
    WinMoveTo {
        stream: u64,
        to: String,
    },
    /// Viewer -> owner: the window was dragged back home. Stop streaming,
    /// show it again under the pointer (and keep it moving with the mouse
    /// if the button is still held).
    WinReturn {
        stream: u64,
    },
    /// Owner -> viewer: the window was maximized (or restored) with its own
    /// controls; maximize the view on the viewer's screen (or restore it).
    WinMaximize {
        stream: u64,
        on: bool,
    },
}

impl Msg {
    /// Bulk messages go through a separate, lower-priority queue so mouse and
    /// keyboard never wait behind a file or a big image.
    pub fn is_bulk(&self) -> bool {
        matches!(
            self,
            Msg::Clip { .. }
                | Msg::ClipPart { .. }
                | Msg::FileData { .. }
                | Msg::FileEnd { .. }
                | Msg::Ext { ext: Ext::WinFrame { .. } | Ext::Windows { .. }, .. }
        )
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
        let m = Msg::Hello { version: 1, name: "pc".into(), os: Os::Linux, screen: Rect { x: 0, y: 0, w: 1920, h: 1080 } };
        assert_eq!(decode(&encode(&m)).unwrap(), m);
        // Mouse moves are the hot path: keep them tiny.
        assert!(encode(&Msg::Move { x: 100, y: 200 }).len() <= 12);
    }
}
