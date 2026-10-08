//! Clipboard sync: watches the local clipboard and applies remote updates.

use crate::protocol::ClipData;
use crossbeam_channel::{Receiver, Sender};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::Duration;

const POLL: Duration = Duration::from_millis(400);
/// Don't sync images bigger than this (raw RGBA bytes).
const MAX_IMAGE_BYTES: usize = 48 * 1024 * 1024;
const MAX_TEXT_BYTES: usize = 8 * 1024 * 1024;

fn hash_of(data: &ClipData) -> u64 {
    let mut h = DefaultHasher::new();
    match data {
        ClipData::Text(t) => {
            0u8.hash(&mut h);
            t.hash(&mut h)
        }
        ClipData::Png(p) => {
            1u8.hash(&mut h);
            p.hash(&mut h)
        }
        ClipData::Files(f) => {
            2u8.hash(&mut h);
            f.hash(&mut h)
        }
    }
    h.finish()
}

pub fn encode_png(width: usize, height: usize, rgba: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, width as u32, height as u32);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.set_compression(png::Compression::Fast);
        let mut w = enc.write_header().ok()?;
        w.write_image_data(rgba).ok()?;
    }
    Some(out)
}

pub fn decode_png(data: &[u8]) -> Option<(usize, usize, Vec<u8>)> {
    let mut dec = png::Decoder::new(std::io::Cursor::new(data));
    dec.set_transformations(png::Transformations::EXPAND | png::Transformations::ALPHA);
    let mut reader = dec.read_info().ok()?;
    let mut buf = vec![0; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut buf).ok()?;
    buf.truncate(info.buffer_size());
    let (w, h) = (info.width as usize, info.height as usize);
    let rgba = match info.color_type {
        png::ColorType::Rgba => buf,
        png::ColorType::Rgb => buf.chunks(3).flat_map(|c| [c[0], c[1], c[2], 255]).collect(),
        png::ColorType::GrayscaleAlpha => buf.chunks(2).flat_map(|c| [c[0], c[0], c[0], c[1]]).collect(),
        png::ColorType::Grayscale => buf.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        _ => return None,
    };
    Some((w, h, rgba))
}

/// Spawn the clipboard thread. Local changes come out of the returned
/// receiver; send remote clipboard contents into the returned sender.
pub fn start() -> (Sender<ClipData>, Receiver<ClipData>) {
    let (set_tx, set_rx) = crossbeam_channel::unbounded::<ClipData>();
    let (changed_tx, changed_rx) = crossbeam_channel::unbounded::<ClipData>();
    std::thread::Builder::new().name("clipboard".into()).spawn(move || run(set_rx, changed_tx)).expect("spawn clipboard");
    (set_tx, changed_rx)
}

/// A `file://` URI for a path.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn file_uri(p: &std::path::Path) -> String {
    let mut out = String::from("file://");
    for b in p.to_string_lossy().bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Is this text just a pointer to something (a URL or a file path), as apps
/// add next to a copied image? Then the image is what the user meant.
fn is_reference(t: &str) -> bool {
    let t = t.trim();
    if t.is_empty() {
        return true;
    }
    if t.contains('\n') || t.len() > 4096 {
        return false;
    }
    let lower = t.to_ascii_lowercase();
    ["http://", "https://", "file://", "data:", "blob:"].iter().any(|p| lower.starts_with(p))
        || t.starts_with('/')
        || t.starts_with("\\\\")
        || (t.len() > 2 && t.as_bytes()[1] == b':' && (t.as_bytes()[2] == b'\\' || t.as_bytes()[2] == b'/'))
}

/// Decide what a copy means. Files beat everything; an image beats text
/// that only names it (its URL or path); otherwise text wins, so copying
/// spreadsheet cells (which also offer a picture) still gives text.
fn choose(files: Vec<PathBuf>, has_image: bool, image: impl FnOnce() -> Option<Vec<u8>>, text: Option<String>) -> Option<ClipData> {
    let files: Vec<String> = files.into_iter().filter(|p| p.exists()).map(|p| p.to_string_lossy().into_owned()).collect();
    if !files.is_empty() {
        return Some(ClipData::Files(files));
    }
    if let Some(t) = &text {
        if looks_like_file_uris(t) {
            let files = uris_to_paths(t);
            if !files.is_empty() {
                return Some(ClipData::Files(files));
            }
        }
    }
    let text = text.filter(|t| !t.is_empty() && t.len() <= MAX_TEXT_BYTES);
    if has_image && text.as_deref().map(is_reference).unwrap_or(true) {
        if let Some(png) = image() {
            return Some(ClipData::Png(png));
        }
    }
    text.map(ClipData::Text)
}

/// Text that is nothing but absolute paths of existing files.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn existing_paths(t: &str) -> Vec<PathBuf> {
    let lines: Vec<&str> = t.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    let paths: Vec<PathBuf> = lines.iter().map(PathBuf::from).filter(|p| p.is_absolute() && p.exists()).collect();
    if !paths.is_empty() && paths.len() == lines.len() {
        paths
    } else {
        vec![]
    }
}

/// Image bytes in any common format -> PNG.
pub fn to_png(bytes: &[u8]) -> Option<Vec<u8>> {
    const SIG: &[u8] = b"\x89PNG\r\n\x1a\n";
    if bytes.starts_with(SIG) {
        return Some(bytes.to_vec());
    }
    let img = image::load_from_memory(bytes).ok()?.to_rgba8();
    if img.as_raw().len() > MAX_IMAGE_BYTES {
        return None;
    }
    encode_png(img.width() as usize, img.height() as usize, img.as_raw())
}

/// If `path` is a picture, the picture as PNG (so a received image file can
/// also be pasted as an image).
pub fn file_png(path: &Path) -> Option<Vec<u8>> {
    let ext = path.extension()?.to_string_lossy().to_ascii_lowercase();
    if !["png", "jpg", "jpeg", "gif", "bmp", "webp", "tif", "tiff"].contains(&ext.as_str()) {
        return None;
    }
    if std::fs::metadata(path).ok()?.len() > 64 * 1024 * 1024 {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    let png = to_png(&bytes)?;
    // Check a PNG passed through untouched really decodes.
    decode_png(&png).map(|_| png)
}

/// Platform clipboard access.
struct Clip {
    #[cfg(target_os = "linux")]
    x11: Option<crate::clip_x11::Reader>,
    cb: Option<arboard::Clipboard>,
}

impl Clip {
    fn new() -> Clip {
        #[cfg(target_os = "linux")]
        {
            let x11 = crate::clip_x11::Reader::new();
            if x11.is_some() {
                return Clip { x11, cb: None };
            }
        }
        let cb = loop {
            match arboard::Clipboard::new() {
                Ok(c) => break c,
                Err(e) => {
                    log::warn!("clipboard unavailable: {e}; retrying");
                    std::thread::sleep(Duration::from_secs(5));
                }
            }
        };
        Clip {
            #[cfg(target_os = "linux")]
            x11: None,
            cb: Some(cb),
        }
    }

    fn read(&mut self) -> Option<ClipData> {
        #[cfg(target_os = "linux")]
        if let Some(r) = self.x11.as_mut() {
            let mut offer = r.offer()?;
            let mut files = offer.files();
            let from_fm = offer.from_file_manager();
            let has_image = offer.has_image();
            let text = offer.text();
            if files.is_empty() && from_fm {
                // Some file managers' URI lists can't be read across XWayland;
                // their plain-text paths still name the files.
                files = text.as_deref().map(existing_paths).unwrap_or_default();
            }
            return choose(files, has_image, || offer.image_png(), text);
        }
        let cb = self.cb.as_mut()?;
        let files = cb.get().file_list().unwrap_or_default();
        let text = if files.is_empty() { cb.get_text().ok() } else { None };
        let mut image = None;
        let has_image = files.is_empty() && {
            image = cb.get_image().ok();
            image.is_some()
        };
        choose(
            files,
            has_image,
            || {
                let img = image?;
                if img.bytes.len() <= MAX_IMAGE_BYTES && img.width > 0 && img.height > 0 {
                    encode_png(img.width, img.height, &img.bytes)
                } else {
                    None
                }
            },
            text,
        )
    }

    /// Cheap text-only poll for systems without change notifications.
    fn quick_text(&mut self) -> Option<String> {
        self.cb.as_mut()?.get_text().ok()
    }

    fn write(&mut self, data: &ClipData) -> Result<(), String> {
        // Received files that are pictures also go on as the picture itself.
        let (files, png, text): (Vec<PathBuf>, Option<Vec<u8>>, Option<String>) = match data {
            ClipData::Files(f) => {
                let files: Vec<PathBuf> = f.iter().map(PathBuf::from).collect();
                let png = if files.len() == 1 { file_png(&files[0]) } else { None };
                (files, png, None)
            }
            ClipData::Png(p) => (vec![], Some(p.clone()), None),
            ClipData::Text(t) => (vec![], None, Some(t.clone())),
        };
        #[cfg(target_os = "linux")]
        if self.x11.is_some() {
            return crate::clip_x11::set(crate::clip_x11::Contents { files, png, text });
        }
        if !files.is_empty() {
            #[cfg(any(windows, target_os = "macos"))]
            return native::set_files(&files, png.as_deref());
        }
        let cb = self.cb.as_mut().ok_or("no clipboard")?;
        if let Some(png) = png {
            let (width, height, rgba) = decode_png(&png).ok_or("bad png")?;
            return cb.set_image(arboard::ImageData { width, height, bytes: rgba.into() }).map_err(|e| e.to_string());
        }
        if !files.is_empty() {
            return cb.set().file_list(&files).map_err(|e| e.to_string());
        }
        cb.set_text(text.unwrap_or_default()).map_err(|e| e.to_string())
    }
}

/// Some file managers only publish copied files as `file://` text.
fn looks_like_file_uris(t: &str) -> bool {
    let lines: Vec<&str> = t.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')).collect();
    !lines.is_empty() && lines.len() < 10_000 && lines.iter().all(|l| l.starts_with("file://"))
}

fn uris_to_paths(t: &str) -> Vec<String> {
    crate::platform::dnd::parse_uri_list(t).into_iter().map(|p| p.to_string_lossy().into_owned()).collect()
}

#[cfg(windows)]
mod native {
    //! Files plus (for a picture) the image, in one clipboard update.
    use std::path::PathBuf;
    use windows::core::w;
    use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL};
    use windows::Win32::System::DataExchange::{CloseClipboard, EmptyClipboard, OpenClipboard, RegisterClipboardFormatW, SetClipboardData};
    use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};

    const CF_DIB: u32 = 8;
    const CF_HDROP: u32 = 15;

    unsafe fn global(bytes: &[u8]) -> Option<HGLOBAL> {
        let h = GlobalAlloc(GMEM_MOVEABLE, bytes.len().max(1)).ok()?;
        let p = GlobalLock(h) as *mut u8;
        if p.is_null() {
            let _ = GlobalFree(Some(h));
            return None;
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), p, bytes.len());
        let _ = GlobalUnlock(h);
        Some(h)
    }

    unsafe fn put(format: u32, bytes: &[u8]) {
        if let Some(h) = global(bytes) {
            if SetClipboardData(format, Some(HANDLE(h.0))).is_err() {
                let _ = GlobalFree(Some(h));
            }
        }
    }

    fn dropfiles(files: &[PathBuf]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&20u32.to_le_bytes()); // pFiles
        out.extend_from_slice(&[0u8; 8]); // pt
        out.extend_from_slice(&0i32.to_le_bytes()); // fNC
        out.extend_from_slice(&1i32.to_le_bytes()); // fWide
        for f in files {
            let p = f.canonicalize().unwrap_or_else(|_| f.clone());
            let s = p.to_string_lossy();
            let s = s.strip_prefix(r"\\?\").unwrap_or(&s);
            for u in s.encode_utf16().chain(std::iter::once(0)) {
                out.extend_from_slice(&u.to_le_bytes());
            }
        }
        out.extend_from_slice(&[0, 0]);
        out
    }

    fn dib(png: &[u8]) -> Option<Vec<u8>> {
        let (w, h, rgba) = super::decode_png(png)?;
        let mut out = Vec::with_capacity(40 + w * h * 4);
        out.extend_from_slice(&40u32.to_le_bytes());
        out.extend_from_slice(&(w as i32).to_le_bytes());
        out.extend_from_slice(&(h as i32).to_le_bytes()); // positive: bottom-up rows
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&32u16.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes()); // BI_RGB
        out.extend_from_slice(&((w * h * 4) as u32).to_le_bytes());
        out.extend_from_slice(&[0u8; 16]);
        for row in (0..h).rev() {
            for px in rgba[row * w * 4..(row + 1) * w * 4].chunks_exact(4) {
                out.extend_from_slice(&[px[2], px[1], px[0], px[3]]);
            }
        }
        Some(out)
    }

    pub fn set_files(files: &[PathBuf], png: Option<&[u8]>) -> Result<(), String> {
        unsafe {
            let mut opened = false;
            for _ in 0..20 {
                if OpenClipboard(None).is_ok() {
                    opened = true;
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
            if !opened {
                return Err("the clipboard is busy".into());
            }
            let _ = EmptyClipboard();
            put(CF_HDROP, &dropfiles(files));
            // Tell Explorer it's a copy, not a move.
            put(RegisterClipboardFormatW(w!("Preferred DropEffect")), &1u32.to_le_bytes());
            if let Some(png) = png {
                put(RegisterClipboardFormatW(w!("PNG")), png);
                if let Some(d) = dib(png) {
                    put(CF_DIB, &d);
                }
            }
            let _ = CloseClipboard();
        }
        Ok(())
    }
}

#[cfg(target_os = "macos")]
mod native {
    //! Files plus (for a picture) the image, in one pasteboard update.
    use objc2::runtime::ProtocolObject;
    use objc2_app_kit::{NSPasteboard, NSPasteboardItem, NSPasteboardTypeFileURL, NSPasteboardTypePNG, NSPasteboardWriting};
    use objc2_foundation::{NSArray, NSData, NSString, NSURL};
    use std::path::PathBuf;

    pub fn set_files(files: &[PathBuf], png: Option<&[u8]>) -> Result<(), String> {
        let pb = NSPasteboard::generalPasteboard();
        let mut items: Vec<objc2::rc::Retained<ProtocolObject<dyn NSPasteboardWriting>>> = Vec::new();
        for (i, f) in files.iter().enumerate() {
            let p = f.canonicalize().unwrap_or_else(|_| f.clone());
            let url = NSURL::fileURLWithPath(&NSString::from_str(&p.to_string_lossy()));
            let Some(s) = url.absoluteString() else {
                continue;
            };
            let item = NSPasteboardItem::new();
            unsafe {
                item.setString_forType(&s, NSPasteboardTypeFileURL);
                if i == 0 {
                    if let Some(png) = png {
                        item.setData_forType(&NSData::with_bytes(png), NSPasteboardTypePNG);
                    }
                }
            }
            items.push(ProtocolObject::from_retained(item));
        }
        if items.is_empty() {
            return Err("no files".into());
        }
        pb.clearContents();
        if pb.writeObjects(&NSArray::from_retained_slice(&items)) {
            Ok(())
        } else {
            Err("the pasteboard refused the files".into())
        }
    }
}

/// Asks the OS whether the clipboard changed, so a large screenshot is read
/// once per copy instead of on every poll. Returns a counter that changes
/// whenever anything new is copied.
type ChangeCounter = Box<dyn FnMut() -> u64>;

#[cfg(windows)]
fn change_counter() -> Option<ChangeCounter> {
    Some(Box::new(|| unsafe { windows::Win32::System::DataExchange::GetClipboardSequenceNumber() as u64 }))
}

#[cfg(target_os = "macos")]
fn change_counter() -> Option<ChangeCounter> {
    use objc2_app_kit::NSPasteboard;
    Some(Box::new(|| NSPasteboard::generalPasteboard().changeCount() as u64))
}

#[cfg(target_os = "linux")]
fn change_counter() -> Option<ChangeCounter> {
    use x11rb::connection::Connection;
    use x11rb::protocol::xfixes::{ConnectionExt as _, SelectionEventMask};
    use x11rb::protocol::xproto::ConnectionExt as _;
    use x11rb::protocol::Event;
    let (conn, n) = x11rb::connect(None).ok()?;
    let root = conn.setup().roots[n].root;
    conn.xfixes_query_version(5, 0).ok()?.reply().ok()?;
    let clipboard = conn.intern_atom(false, b"CLIPBOARD").ok()?.reply().ok()?.atom;
    let mask = SelectionEventMask::SET_SELECTION_OWNER | SelectionEventMask::SELECTION_WINDOW_DESTROY | SelectionEventMask::SELECTION_CLIENT_CLOSE;
    conn.xfixes_select_selection_input(root, clipboard, mask).ok()?;
    conn.flush().ok()?;
    let mut counter = 1u64;
    Some(Box::new(move || {
        while let Ok(Some(ev)) = conn.poll_for_event() {
            if matches!(ev, Event::XfixesSelectionNotify(_)) {
                counter += 1;
            }
        }
        counter
    }))
}

#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
fn change_counter() -> Option<ChangeCounter> {
    None
}

fn run(set_rx: Receiver<ClipData>, changed_tx: Sender<ClipData>) {
    let mut cb = Clip::new();
    let mut changes = change_counter();
    if changes.is_none() {
        log::info!("clipboard change notifications unavailable; polling");
    }
    let mut seen = changes.as_mut().map(|f| f());
    // Don't broadcast whatever was already on the clipboard at startup.
    let mut last = cb.read().map(|d| hash_of(&d));
    let mut polls: u64 = 0;
    loop {
        match set_rx.recv_timeout(POLL) {
            Ok(data) => {
                let h = hash_of(&data);
                if Some(h) != last {
                    match cb.write(&data) {
                        Ok(()) => {
                            log::debug!("clipboard updated from remote");
                            last = Some(h);
                            // Our own write counts as a change; skip it.
                            std::thread::sleep(Duration::from_millis(50));
                            seen = changes.as_mut().map(|f| f());
                            if let ClipData::Png(_) = data {
                                // Images can be re-encoded on read; remember that form too.
                                last = cb.read().map(|d| hash_of(&d)).or(Some(h));
                            }
                        }
                        Err(e) => log::warn!("clipboard write failed: {e}"),
                    }
                }
            }
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => return,
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                polls += 1;
                let d = match changes.as_mut() {
                    Some(f) => {
                        let now = f();
                        if Some(now) == seen {
                            continue;
                        }
                        seen = Some(now);
                        // Let the copying app finish taking ownership.
                        std::thread::sleep(Duration::from_millis(60));
                        cb.read()
                    }
                    // No notifications: text is cheap to poll, everything else every ~2 s.
                    None => match cb.quick_text() {
                        Some(t) if !t.is_empty() && t.len() <= MAX_TEXT_BYTES && !looks_like_file_uris(&t) && !is_reference(&t) => Some(ClipData::Text(t)),
                        _ if polls.is_multiple_of(5) => cb.read(),
                        _ => None,
                    },
                };
                if let Some(d) = d {
                    let h = hash_of(&d);
                    if Some(h) != last {
                        last = Some(h);
                        match &d {
                            ClipData::Png(p) => {
                                log::info!("copied an image ({} KB)", p.len() / 1024)
                            }
                            ClipData::Files(f) => log::info!("copied {} file(s)", f.len()),
                            ClipData::Text(_) => log::debug!("copied text"),
                        }
                        if changed_tx.send(d).is_err() {
                            return;
                        }
                    }
                }
            }
        }
    }
}

/// What's on the clipboard right now, as OpenHop sees it (for `openhop clipboard`).
pub fn inspect() -> String {
    let mut out = String::new();
    #[cfg(target_os = "linux")]
    if let Some(mut r) = crate::clip_x11::Reader::new() {
        match r.offer() {
            Some(o) => out.push_str(&format!("formats: {}\n", o.names().join(", "))),
            None => out.push_str("formats: (empty)\n"),
        }
    }
    let mut cb = Clip::new();
    match cb.read() {
        Some(ClipData::Files(f)) => out.push_str(&format!("OpenHop would send {} file(s): {}\n", f.len(), f.join(", "))),
        Some(ClipData::Png(p)) => out.push_str(&format!("OpenHop would send an image ({} KB)\n", p.len() / 1024)),
        Some(ClipData::Text(t)) => {
            out.push_str(&format!("OpenHop would send text ({} chars): {:?}\n", t.chars().count(), t.chars().take(80).collect::<String>()))
        }
        None => out.push_str("OpenHop would send nothing\n"),
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn file_uri_text() {
        let dir = std::env::temp_dir().join("openhop uri test");
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("clip 1.mp4");
        std::fs::write(&f, b"x").unwrap();
        let uri = format!("file://{}\n", f.to_string_lossy().replace(' ', "%20"));
        assert!(looks_like_file_uris(&uri));
        assert_eq!(uris_to_paths(&uri), vec![f.to_string_lossy().into_owned()]);
        assert!(!looks_like_file_uris("hello file://x"));
        // libfm ends its lists with a NUL.
        assert_eq!(uris_to_paths(&format!("{}\0", uri.trim())), vec![f.to_string_lossy().into_owned()]);
    }

    #[test]
    fn choosing() {
        let png = || Some(vec![1u8]);
        let dir = std::env::temp_dir().join("openhop choose test");
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("a.png");
        std::fs::write(&f, b"x").unwrap();
        // Files beat everything.
        assert_eq!(choose(vec![f.clone()], true, png, Some("x".into())), Some(ClipData::Files(vec![f.to_string_lossy().into_owned()])));
        // An image beats its own URL or path (browsers, screenshot tools).
        assert_eq!(choose(vec![], true, png, Some("https://example.com/cat.jpg".into())), Some(ClipData::Png(vec![1])));
        assert_eq!(choose(vec![], true, png, Some(r"C:\Users\me\Pictures\x.png".into())), Some(ClipData::Png(vec![1])));
        assert_eq!(choose(vec![], true, png, None), Some(ClipData::Png(vec![1])));
        // Real text with a picture (spreadsheet cells) stays text.
        assert_eq!(choose(vec![], true, png, Some("a\tb\n1\t2".into())), Some(ClipData::Text("a\tb\n1\t2".into())));
        assert_eq!(choose(vec![], false, png, Some("hello".into())), Some(ClipData::Text("hello".into())));
        // file:// text is files.
        let uri = format!("file://{}", f.to_string_lossy().replace(' ', "%20"));
        assert_eq!(choose(vec![], false, png, Some(uri)), Some(ClipData::Files(vec![f.to_string_lossy().into_owned()])));
        assert_eq!(existing_paths(&format!("{}\n", f.display())), vec![f.clone()]);
        assert!(existing_paths("hello\n/nonexistent/x").is_empty());
    }

    #[test]
    fn image_files_become_png() {
        let dir = std::env::temp_dir().join("openhop png test");
        std::fs::create_dir_all(&dir).unwrap();
        let rgba: Vec<u8> = (0..4 * 8 * 8).map(|i| i as u8).collect();
        let img = image::RgbaImage::from_raw(8, 8, rgba).unwrap();
        let jpg = dir.join("p.jpg");
        image::DynamicImage::ImageRgba8(img.clone()).to_rgb8().save(&jpg).unwrap();
        let png = file_png(&jpg).unwrap();
        assert_eq!(decode_png(&png).unwrap().0, 8);
        let txt = dir.join("p.txt");
        std::fs::write(&txt, b"x").unwrap();
        assert!(file_png(&txt).is_none());
    }

    #[test]
    fn png_roundtrip() {
        let rgba: Vec<u8> = (0..4 * 3 * 2).map(|i| i as u8).collect();
        let png = encode_png(3, 2, &rgba).unwrap();
        let (w, h, back) = decode_png(&png).unwrap();
        assert_eq!((w, h), (3, 2));
        assert_eq!(back, rgba);
    }
}
