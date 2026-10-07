//! File transfer: offers, streaming and receiving.
//!
//! The computer that has the files makes an *offer* (a list of relative
//! paths and sizes). A computer that wants them sends a request; the files
//! are streamed in order as chunks on the bulk lane and written into
//! `Downloads/OpenHop`. The server relays between clients.

use crate::net::{Link, CHUNK};
use crate::protocol::{FileMeta, Msg, OfferKind};
use parking_lot::Mutex;
use serde::Serialize;
use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

/// Upper bound for one offer (copying a 200 GB folder by accident shouldn't start a transfer).
pub const MAX_OFFER_BYTES: u64 = 64 * 1024 * 1024 * 1024;
const MAX_FILES: usize = 20_000;

/// Transfer speed limit in bytes per second, shared by every outgoing
/// transfer (0 = unlimited). Keeps big copies from saturating the Wi-Fi.
static RATE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static NEXT_SLOT: Mutex<Option<Instant>> = Mutex::new(None);

/// Set the limit in megabits per second (0 = unlimited).
pub fn set_speed_limit_mbps(mbps: u32) {
    RATE.store(mbps as u64 * 1_000_000 / 8, std::sync::atomic::Ordering::Relaxed);
}

/// Wait until `len` more bytes may be sent under the speed limit.
pub fn pace(len: usize) {
    let rate = RATE.load(std::sync::atomic::Ordering::Relaxed);
    if rate == 0 {
        return;
    }
    let cost = std::time::Duration::from_secs_f64(len as f64 / rate as f64);
    let wait = {
        let mut next = NEXT_SLOT.lock();
        let now = Instant::now();
        let start = next.map(|n| n.max(now)).unwrap_or(now);
        *next = Some(start + cost);
        start.saturating_duration_since(now)
    };
    if !wait.is_zero() {
        std::thread::sleep(wait);
    }
}

pub fn new_id() -> u64 {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos());
    h.finish() | 1
}

/// A local file that is part of an offer.
#[derive(Debug, Clone)]
pub struct Entry {
    pub abs: PathBuf,
    pub meta: FileMeta,
}

/// Expand files and folders into a flat list with relative paths.
pub fn collect(paths: &[PathBuf]) -> Result<Vec<Entry>, String> {
    let mut out = Vec::new();
    let mut total = 0u64;
    for p in paths {
        let name = p.file_name().ok_or("bad path")?.to_string_lossy().into_owned();
        walk(p, &name, &mut out, &mut total)?;
    }
    if out.is_empty() {
        return Err("nothing to send".into());
    }
    Ok(out)
}

fn walk(abs: &Path, rel: &str, out: &mut Vec<Entry>, total: &mut u64) -> Result<(), String> {
    let md = std::fs::metadata(abs).map_err(|e| format!("{}: {e}", abs.display()))?;
    if md.is_dir() {
        let mut kids: Vec<_> = std::fs::read_dir(abs).map_err(|e| e.to_string())?.flatten().collect();
        kids.sort_by_key(|k| k.file_name());
        if kids.is_empty() {
            // Keep empty folders: a zero-size entry with a trailing slash.
            out.push(Entry { abs: abs.to_path_buf(), meta: FileMeta { path: format!("{rel}/"), size: 0 } });
        }
        for k in kids {
            let name = k.file_name().to_string_lossy().into_owned();
            walk(&k.path(), &format!("{rel}/{name}"), out, total)?;
        }
    } else if md.is_file() {
        *total += md.len();
        if *total > MAX_OFFER_BYTES || out.len() >= MAX_FILES {
            return Err("too much to transfer at once".into());
        }
        out.push(Entry { abs: abs.to_path_buf(), meta: FileMeta { path: rel.to_string(), size: md.len() } });
    }
    Ok(())
}

/// Offers this computer has made, kept so others can fetch them later.
type Offers = Vec<(u64, Instant, Vec<Entry>)>;

#[derive(Default, Clone)]
pub struct Outbox {
    inner: Arc<Mutex<Offers>>,
}

impl Outbox {
    pub fn add(&self, offer: u64, entries: Vec<Entry>) {
        let mut v = self.inner.lock();
        v.push((offer, Instant::now(), entries));
        // Keep the most recent offers only.
        while v.len() > 16 {
            v.remove(0);
        }
    }

    pub fn get(&self, offer: u64) -> Option<Vec<Entry>> {
        self.inner.lock().iter().find(|(o, _, _)| *o == offer).map(|(_, _, e)| e.clone())
    }

    /// Stream an offer to `dest` over `link` on a background thread.
    pub fn serve(&self, offer: u64, dest: String, link: Link) {
        let Some(entries) = self.get(offer) else {
            link.send(Msg::FileEnd { offer, dest, error: Some("those files are no longer offered".into()) });
            return;
        };
        let _ = std::thread::Builder::new().name("file-send".into()).spawn(move || {
            let mut buf = vec![0u8; CHUNK];
            let started = Instant::now();
            let mut sent = 0u64;
            for (i, e) in entries.iter().enumerate() {
                if e.meta.path.ends_with('/') {
                    continue;
                }
                let mut f = match File::open(&e.abs) {
                    Ok(f) => f,
                    Err(err) => {
                        link.send_bulk_wait(Msg::FileEnd { offer, dest, error: Some(format!("{}: {err}", e.meta.path)) });
                        return;
                    }
                };
                loop {
                    let n = match f.read(&mut buf) {
                        Ok(n) => n,
                        Err(err) => {
                            link.send_bulk_wait(Msg::FileEnd { offer, dest, error: Some(format!("{}: {err}", e.meta.path)) });
                            return;
                        }
                    };
                    if n == 0 {
                        break;
                    }
                    sent += n as u64;
                    pace(n);
                    let msg = Msg::FileData { offer, dest: dest.clone(), index: i as u32, data: buf[..n].to_vec() };
                    if !link.send_bulk_wait(msg) {
                        log::warn!("transfer to {dest} interrupted");
                        return;
                    }
                }
            }
            link.send_bulk_wait(Msg::FileEnd { offer, dest: dest.clone(), error: None });
            let secs = started.elapsed().as_secs_f64().max(0.001);
            log::info!("sent {} to {dest} ({:.1} MB/s)", human(sent), sent as f64 / secs / 1e6);
        });
    }
}

pub fn human(b: u64) -> String {
    match b {
        b if b >= 1 << 30 => format!("{:.1} GB", b as f64 / (1u64 << 30) as f64),
        b if b >= 1 << 20 => format!("{:.1} MB", b as f64 / (1u64 << 20) as f64),
        b if b >= 1 << 10 => format!("{:.0} KB", b as f64 / 1024.0),
        b => format!("{b} B"),
    }
}

/// Reject anything that could escape the download folder.
fn safe_rel(path: &str) -> Option<PathBuf> {
    let p = Path::new(path.trim_end_matches('/'));
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::Normal(s) => {
                let s = s.to_string_lossy();
                if s.contains('\\') || s.contains(':') {
                    return None;
                }
                out.push(&*s);
            }
            _ => return None,
        }
    }
    (!out.as_os_str().is_empty()).then_some(out)
}

fn unique(dir: &Path, name: &str) -> String {
    if !dir.join(name).exists() {
        return name.to_string();
    }
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    };
    for n in 2.. {
        let cand = format!("{stem} ({n}){ext}");
        if !dir.join(&cand).exists() {
            return cand;
        }
    }
    unreachable!()
}

pub fn download_root() -> PathBuf {
    dirs::download_dir().or_else(dirs::home_dir).unwrap_or_else(std::env::temp_dir).join("OpenHop")
}

#[derive(Debug, Clone, Serialize)]
pub struct TransferInfo {
    pub offer: u64,
    pub label: String,
    pub peer: String,
    pub incoming: bool,
    pub done: u64,
    pub total: u64,
    pub finished: bool,
    pub error: Option<String>,
}

struct Download {
    origin: String,
    kind: OfferKind,
    files: Vec<FileMeta>,
    /// Destination of each file.
    targets: Vec<PathBuf>,
    /// Top-level items (what goes on the clipboard / gets revealed).
    tops: Vec<PathBuf>,
    open: Option<(u32, File)>,
    done: u64,
}

/// What a finished download produced.
#[derive(Debug, Clone)]
pub struct Finished {
    pub offer: u64,
    pub origin: String,
    pub kind: OfferKind,
    pub tops: Vec<PathBuf>,
    pub error: Option<String>,
}

/// Incoming transfers. Shared between the network reader threads (which
/// write the data) and the engine (which starts downloads and shows progress).
#[derive(Clone)]
pub struct Inbox {
    root: PathBuf,
    inner: Arc<Mutex<InboxState>>,
}

#[derive(Default)]
struct InboxState {
    active: HashMap<u64, Download>,
    history: Vec<TransferInfo>,
}

impl Inbox {
    pub fn new(root: PathBuf) -> Inbox {
        Inbox { root, inner: Default::default() }
    }

    pub fn is_active(&self, offer: u64) -> bool {
        self.inner.lock().active.contains_key(&offer)
    }

    /// Prepare to receive `files`. Creates folders and empty files up front.
    pub fn start(&self, offer: u64, origin: &str, kind: OfferKind, files: Vec<FileMeta>) -> Result<(), String> {
        std::fs::create_dir_all(&self.root).map_err(|e| format!("{}: {e}", self.root.display()))?;
        let mut renamed: HashMap<String, String> = HashMap::new();
        let mut targets = Vec::new();
        let mut tops = Vec::new();
        for f in &files {
            let rel = safe_rel(&f.path).ok_or_else(|| format!("refusing unsafe path {:?}", f.path))?;
            let mut comps = rel.components();
            let top = comps.next().unwrap().as_os_str().to_string_lossy().into_owned();
            let top_local = renamed.entry(top.clone()).or_insert_with(|| {
                let t = unique(&self.root, &top);
                tops.push(self.root.join(&t));
                t
            });
            let rest = comps.as_path();
            let mut target = self.root.join(&*top_local);
            if !rest.as_os_str().is_empty() {
                target = target.join(rest);
            }
            if f.path.ends_with('/') {
                std::fs::create_dir_all(&target).map_err(|e| e.to_string())?;
            } else {
                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
                }
                File::create(&target).map_err(|e| format!("{}: {e}", target.display()))?;
            }
            targets.push(target);
        }
        let total = files.iter().map(|f| f.size).sum();
        let label = label_for(&files);
        let mut st = self.inner.lock();
        st.history.retain(|h| h.offer != offer);
        st.history.push(TransferInfo {
            offer,
            label,
            peer: origin.to_string(),
            incoming: true,
            done: 0,
            total,
            finished: false,
            error: None,
        });
        if st.history.len() > 20 {
            st.history.remove(0);
        }
        st.active.insert(offer, Download { origin: origin.to_string(), kind, files, targets, tops, open: None, done: 0 });
        Ok(())
    }

    pub fn data(&self, offer: u64, index: u32, data: &[u8]) -> Result<(), String> {
        let mut st = self.inner.lock();
        let Some(d) = st.active.get_mut(&offer) else { return Ok(()) };
        let i = index as usize;
        if i >= d.targets.len() {
            return Err("bad file index".into());
        }
        if d.open.as_ref().map(|(n, _)| *n) != Some(index) {
            let f = std::fs::OpenOptions::new().append(true).open(&d.targets[i]).map_err(|e| e.to_string())?;
            d.open = Some((index, f));
        }
        let (_, f) = d.open.as_mut().unwrap();
        f.write_all(data).map_err(|e| format!("writing {}: {e}", d.targets[i].display()))?;
        d.done += data.len() as u64;
        let (done, offer_id) = (d.done, offer);
        if let Some(h) = st.history.iter_mut().find(|h| h.offer == offer_id) {
            h.done = done;
        }
        Ok(())
    }

    pub fn finish(&self, offer: u64, error: Option<String>) -> Option<Finished> {
        let mut st = self.inner.lock();
        let d = st.active.remove(&offer)?;
        drop(d.open);
        let mut error = error;
        if error.is_none() {
            for (f, t) in d.files.iter().zip(&d.targets) {
                if !f.path.ends_with('/') && std::fs::metadata(t).map(|m| m.len()).unwrap_or(0) != f.size {
                    error = Some(format!("{} arrived incomplete", f.path));
                    break;
                }
            }
        }
        if error.is_some() {
            for t in &d.tops {
                let _ = if t.is_dir() { std::fs::remove_dir_all(t) } else { std::fs::remove_file(t) };
            }
        }
        if let Some(h) = st.history.iter_mut().find(|h| h.offer == offer) {
            h.finished = true;
            h.error = error.clone();
            if error.is_none() {
                h.done = h.total;
            }
        }
        Some(Finished { offer, origin: d.origin, kind: d.kind, tops: d.tops, error })
    }

    /// Abort everything coming from `origin` (it disconnected).
    pub fn abort_from(&self, origin: &str) -> Vec<Finished> {
        let offers: Vec<u64> = self.inner.lock().active.iter().filter(|(_, d)| d.origin == origin).map(|(o, _)| *o).collect();
        offers.into_iter().filter_map(|o| self.finish(o, Some(format!("{origin} disconnected")))).collect()
    }

    pub fn history(&self) -> Vec<TransferInfo> {
        self.inner.lock().history.clone()
    }
}

pub fn label_for(files: &[FileMeta]) -> String {
    let tops: std::collections::BTreeSet<&str> = files.iter().map(|f| f.path.split('/').next().unwrap_or("")).collect();
    match tops.len() {
        1 => tops.into_iter().next().unwrap().to_string(),
        n => format!("{n} items"),
    }
}

pub fn total_size(files: &[FileMeta]) -> u64 {
    files.iter().map(|f| f.size).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speed_limit_paces_sends() {
        set_speed_limit_mbps(80); // 10 MB/s
        let t = Instant::now();
        for _ in 0..20 {
            pace(256 * 1024); // 5 MB total
        }
        let secs = t.elapsed().as_secs_f64();
        set_speed_limit_mbps(0);
        assert!(secs > 0.4 && secs < 0.8, "took {secs}s, expected ~0.5s");
    }

    #[test]
    fn rejects_escapes() {
        assert!(safe_rel("../etc/passwd").is_none());
        assert!(safe_rel("/etc/passwd").is_none());
        assert!(safe_rel("a/../../x").is_none());
        assert!(safe_rel("C:\\x").is_none());
        assert_eq!(safe_rel("folder/clip.mp4"), Some(PathBuf::from("folder").join("clip.mp4")));
    }

    #[test]
    fn collect_and_receive_roundtrip() {
        let base = std::env::temp_dir().join(format!("openhop-files-{}", new_id()));
        let src = base.join("src");
        std::fs::create_dir_all(src.join("album/empty")).unwrap();
        std::fs::write(src.join("video.mp4"), vec![7u8; 600_000]).unwrap();
        std::fs::write(src.join("album/a.txt"), b"hello").unwrap();
        let entries = collect(&[src.join("video.mp4"), src.join("album")]).unwrap();
        let metas: Vec<FileMeta> = entries.iter().map(|e| e.meta.clone()).collect();
        assert_eq!(label_for(&metas), "2 items");
        assert_eq!(total_size(&metas), 600_005);

        let inbox = Inbox::new(base.join("dl"));
        std::fs::create_dir_all(base.join("dl")).unwrap();
        std::fs::write(base.join("dl/video.mp4"), b"existing").unwrap(); // forces a rename
        inbox.start(9, "pc", OfferKind::Clipboard, metas.clone()).unwrap();
        for (i, e) in entries.iter().enumerate() {
            if e.meta.path.ends_with('/') {
                continue;
            }
            let data = std::fs::read(&e.abs).unwrap();
            for chunk in data.chunks(CHUNK) {
                inbox.data(9, i as u32, chunk).unwrap();
            }
        }
        let fin = inbox.finish(9, None).unwrap();
        assert!(fin.error.is_none(), "{:?}", fin.error);
        assert_eq!(fin.tops, vec![base.join("dl/video (2).mp4"), base.join("dl/album")]);
        assert_eq!(std::fs::read(base.join("dl/video (2).mp4")).unwrap().len(), 600_000);
        assert_eq!(std::fs::read(base.join("dl/album/a.txt")).unwrap(), b"hello");
        assert!(base.join("dl/album/empty").is_dir());
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn incomplete_transfer_is_cleaned_up() {
        let base = std::env::temp_dir().join(format!("openhop-files-{}", new_id()));
        let inbox = Inbox::new(base.clone());
        inbox.start(1, "pc", OfferKind::Drop, vec![FileMeta { path: "big.bin".into(), size: 10 }]).unwrap();
        inbox.data(1, 0, b"12345").unwrap();
        let fin = inbox.finish(1, None).unwrap();
        assert!(fin.error.is_some());
        assert!(!base.join("big.bin").exists());
        std::fs::remove_dir_all(&base).ok();
    }
}
