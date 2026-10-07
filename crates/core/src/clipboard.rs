//! Clipboard sync: watches the local clipboard and applies remote updates.

use crate::protocol::ClipData;
use crossbeam_channel::{Receiver, Sender};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
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
    std::thread::Builder::new()
        .name("clipboard".into())
        .spawn(move || run(set_rx, changed_tx))
        .expect("spawn clipboard");
    (set_tx, changed_rx)
}

fn read(cb: &mut arboard::Clipboard) -> Option<ClipData> {
    if let Ok(t) = cb.get_text() {
        if !t.is_empty() && t.len() <= MAX_TEXT_BYTES {
            return Some(ClipData::Text(t));
        }
    }
    if let Ok(img) = cb.get_image() {
        if img.bytes.len() <= MAX_IMAGE_BYTES && img.width > 0 && img.height > 0 {
            return encode_png(img.width, img.height, &img.bytes).map(ClipData::Png);
        }
    }
    None
}

fn write(cb: &mut arboard::Clipboard, data: &ClipData) -> Result<(), String> {
    match data {
        ClipData::Text(t) => cb.set_text(t.clone()).map_err(|e| e.to_string()),
        ClipData::Png(p) => {
            let (width, height, rgba) = decode_png(p).ok_or("bad png")?;
            cb.set_image(arboard::ImageData { width, height, bytes: rgba.into() }).map_err(|e| e.to_string())
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
    let mask = SelectionEventMask::SET_SELECTION_OWNER
        | SelectionEventMask::SELECTION_WINDOW_DESTROY
        | SelectionEventMask::SELECTION_CLIENT_CLOSE;
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
    let mut cb = loop {
        match arboard::Clipboard::new() {
            Ok(c) => break c,
            Err(e) => {
                log::warn!("clipboard unavailable: {e}; retrying");
                std::thread::sleep(Duration::from_secs(5));
            }
        }
    };
    let mut changes = change_counter();
    if changes.is_none() {
        log::info!("clipboard change notifications unavailable; polling");
    }
    let mut seen = changes.as_mut().map(|f| f());
    // Don't broadcast whatever was already on the clipboard at startup.
    let mut last = read(&mut cb).map(|d| hash_of(&d));
    let mut polls: u64 = 0;
    loop {
        match set_rx.recv_timeout(POLL) {
            Ok(data) => {
                let h = hash_of(&data);
                if Some(h) != last {
                    match write(&mut cb, &data) {
                        Ok(()) => {
                            log::debug!("clipboard updated from remote");
                            last = Some(h);
                            // Our own write counts as a change; skip it.
                            std::thread::sleep(Duration::from_millis(50));
                            seen = changes.as_mut().map(|f| f());
                            if let ClipData::Png(_) = data {
                                // Images can be re-encoded on read; remember that form too.
                                last = read(&mut cb).map(|d| hash_of(&d)).or(Some(h));
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
                        read(&mut cb)
                    }
                    // No notifications: text is cheap to poll, images every ~2 s.
                    None => match cb.get_text() {
                        Ok(t) if !t.is_empty() && t.len() <= MAX_TEXT_BYTES => Some(ClipData::Text(t)),
                        _ if polls % 5 == 0 => read(&mut cb),
                        _ => None,
                    },
                };
                if let Some(d) = d {
                    let h = hash_of(&d);
                    if Some(h) != last {
                        last = Some(h);
                        if let ClipData::Png(p) = &d {
                            log::debug!("local clipboard image ({} KB) -> peers", p.len() / 1024);
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn png_roundtrip() {
        let rgba: Vec<u8> = (0..4 * 3 * 2).map(|i| i as u8).collect();
        let png = encode_png(3, 2, &rgba).unwrap();
        let (w, h, back) = decode_png(&png).unwrap();
        assert_eq!((w, h), (3, 2));
        assert_eq!(back, rgba);
    }
}
