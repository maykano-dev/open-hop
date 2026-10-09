//! A folder kept the same on every computer ("OpenHop Shared" in the home
//! folder). Each computer says what it has (path, size, when it last
//! changed); the others fetch what's newer, and remove what was deleted.
//! The newest change wins.

use crate::files::{Entry, Outbox};
use crate::protocol::{FileMeta, FolderEntry};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Deletions are remembered this long (so a computer that was off catches up).
const KEEP_GONE: u64 = 7 * 24 * 3600 * 1000;
/// A file changed this recently may still be being written: wait.
const SETTLE_MS: u64 = 2000;
const MAX_FILES: usize = 20_000;

pub fn default_root() -> PathBuf {
    dirs::home_dir().unwrap_or_else(std::env::temp_dir).join("OpenHop Shared")
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

fn mtime_ms(m: &std::fs::Metadata) -> u64 {
    m.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// A stable number for this version of this file (the same on every
/// computer that has it), small enough for JavaScript.
fn offer_of(path: &str, size: u64, mtime: u64) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in path.bytes().chain(size.to_le_bytes()).chain(mtime.to_le_bytes()) {
        h = (h ^ b as u64).wrapping_mul(0x100000001b3);
    }
    (h & ((1 << 52) - 1)) | (1 << 51)
}

pub struct Shared {
    pub root: PathBuf,
    /// Our files: path → (size, mtime).
    mine: HashMap<String, (u64, u64)>,
    /// Deleted here or elsewhere: path → when.
    gone: HashMap<String, u64>,
    /// Being fetched: offer → (path, mtime).
    fetching: HashMap<u64, (String, u64)>,
    offered: HashSet<u64>,
    scanned: bool,
}

/// What to fetch: from whom, which offer, the file.
pub type Fetch = (String, u64, FileMeta);

impl Shared {
    pub fn new(root: PathBuf) -> Shared {
        let _ = std::fs::create_dir_all(&root);
        Shared { root, mine: HashMap::new(), gone: HashMap::new(), fetching: HashMap::new(), offered: HashSet::new(), scanned: false }
    }

    fn walk(&self) -> HashMap<String, (u64, u64)> {
        let mut out = HashMap::new();
        let mut stack = vec![self.root.clone()];
        while let Some(dir) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&dir) else { continue };
            for e in rd.flatten() {
                let p = e.path();
                let name = e.file_name().to_string_lossy().into_owned();
                // Hidden files and the system's clutter stay local.
                if name.starts_with('.') || name == "desktop.ini" || name == "Thumbs.db" || name.ends_with(".part") {
                    continue;
                }
                let Ok(m) = e.metadata() else { continue };
                if m.is_dir() {
                    stack.push(p);
                } else if m.is_file() {
                    if let Ok(rel) = p.strip_prefix(&self.root) {
                        let rel = rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect::<Vec<_>>().join("/");
                        out.insert(rel, (m.len(), mtime_ms(&m)));
                    }
                }
                if out.len() >= MAX_FILES {
                    return out;
                }
            }
        }
        out
    }

    /// Look at the folder again. Returns true if anything changed; keeps
    /// every file offered (in `outbox`).
    pub fn scan(&mut self, outbox: &Outbox) -> bool {
        let now = self.walk();
        let t = now_ms();
        let mut changed = !self.scanned;
        // Deleted here since the last look.
        if self.scanned {
            for p in self.mine.keys() {
                if !now.contains_key(p) {
                    self.gone.insert(p.clone(), t);
                    changed = true;
                }
            }
        }
        let settled: HashMap<String, (u64, u64)> = now.into_iter().filter(|(_, (_, m))| t.saturating_sub(*m) >= SETTLE_MS).collect();
        changed |= settled != self.mine;
        for p in settled.keys() {
            self.gone.remove(p);
        }
        self.gone.retain(|_, at| t.saturating_sub(*at) < KEEP_GONE);
        self.mine = settled;
        self.scanned = true;
        // Offer every file as it is now.
        let wanted: HashSet<u64> = self.mine.iter().map(|(p, (s, m))| offer_of(p, *s, *m)).collect();
        for o in self.offered.difference(&wanted) {
            outbox.unkeep(*o);
        }
        for (p, (s, m)) in &self.mine {
            let o = offer_of(p, *s, *m);
            if !self.offered.contains(&o) {
                outbox.keep(o, vec![Entry { abs: self.root.join(p), meta: FileMeta { path: leaf(p), size: *s } }]);
            }
        }
        self.offered = wanted;
        changed
    }

    /// What to tell the others.
    pub fn listing(&self) -> (Vec<FolderEntry>, Vec<(String, u64)>) {
        let files = self.mine.iter().map(|(p, (s, m))| FolderEntry { path: p.clone(), size: *s, mtime: *m, offer: offer_of(p, *s, *m) }).collect();
        (files, self.gone.iter().map(|(p, t)| (p.clone(), *t)).collect())
    }

    /// Another computer's listing: remove what it deleted after our change,
    /// and return what to fetch.
    pub fn compare(&mut self, from: &str, files: &[FolderEntry], gone: &[(String, u64)]) -> Vec<Fetch> {
        for (p, at) in gone {
            let Some(rel) = safe(p) else { continue };
            match self.mine.get(p) {
                Some((_, m)) if *m <= *at => {
                    let _ = std::fs::remove_file(self.root.join(&rel));
                    self.mine.remove(p);
                    self.gone.insert(p.clone(), *at);
                    log::info!("shared folder: {p} was deleted on {from}");
                    remove_empty_dirs(&self.root, &rel);
                }
                None => {
                    let e = self.gone.entry(p.clone()).or_insert(*at);
                    *e = (*e).max(*at);
                }
                _ => {}
            }
        }
        let mut out = vec![];
        for f in files {
            if safe(&f.path).is_none() {
                continue;
            }
            let newer = match self.mine.get(&f.path) {
                Some((s, m)) => f.mtime > *m || (f.mtime == *m && f.size != *s && f.size > *s),
                None => true,
            };
            let deleted_after = self.gone.get(&f.path).map(|at| *at >= f.mtime).unwrap_or(false);
            if newer && !deleted_after && !self.fetching.contains_key(&f.offer) {
                self.fetching.insert(f.offer, (f.path.clone(), f.mtime));
                out.push((from.to_string(), f.offer, FileMeta { path: leaf(&f.path), size: f.size }));
            }
        }
        out
    }

    /// A fetched file is here (`tops`: where it arrived): put it in place,
    /// with the time it was changed on its computer.
    pub fn landed(&mut self, offer: u64, tops: &[PathBuf], ok: bool) {
        let Some((path, mtime)) = self.fetching.remove(&offer) else { return };
        let Some(src) = tops.first() else { return };
        if ok {
            if let Some(rel) = safe(&path) {
                let dest = self.root.join(&rel);
                if let Some(parent) = dest.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                let placed = std::fs::rename(src, &dest).is_ok() || (std::fs::copy(src, &dest).is_ok() && std::fs::remove_file(src).is_ok());
                if placed {
                    let t = UNIX_EPOCH + Duration::from_millis(mtime);
                    if let Ok(f) = std::fs::OpenOptions::new().write(true).open(&dest) {
                        let _ = f.set_modified(t);
                    }
                    let size = std::fs::metadata(&dest).map(|m| m.len()).unwrap_or(0);
                    self.mine.insert(path.clone(), (size, mtime));
                    log::info!("shared folder: {path} updated");
                }
            }
        }
        if let Some(dir) = src.parent() {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}

fn leaf(p: &str) -> String {
    p.rsplit('/').next().unwrap_or(p).to_string()
}

/// A relative path that stays inside the folder.
fn safe(p: &str) -> Option<PathBuf> {
    let mut out = PathBuf::new();
    for c in p.split('/') {
        if c.is_empty() || c == "." || c == ".." || c.contains('\\') || c.contains(':') {
            return None;
        }
        out.push(c);
    }
    (!out.as_os_str().is_empty()).then_some(out)
}

fn remove_empty_dirs(root: &Path, rel: &Path) {
    let mut d = rel.parent();
    while let Some(p) = d.filter(|p| !p.as_os_str().is_empty()) {
        if std::fs::remove_dir(root.join(p)).is_err() {
            break;
        }
        d = p.parent();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("openhop-folder-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    fn age(p: &Path, ms_ago: u64) {
        let f = std::fs::OpenOptions::new().write(true).open(p).unwrap();
        f.set_modified(SystemTime::now() - Duration::from_millis(ms_ago)).unwrap();
    }

    #[test]
    fn two_computers_agree() {
        let (ra, rb) = (tmp("a"), tmp("b"));
        let (mut a, mut b) = (Shared::new(ra.clone()), Shared::new(rb.clone()));
        let (oa, ob) = (Outbox::default(), Outbox::default());
        std::fs::create_dir_all(ra.join("notes")).unwrap();
        std::fs::write(ra.join("notes/todo.txt"), "milk").unwrap();
        age(&ra.join("notes/todo.txt"), 5000);
        std::fs::write(ra.join(".hidden"), "x").unwrap();
        // Being written right now: not yet.
        std::fs::write(ra.join("fresh.txt"), "x").unwrap();
        assert!(a.scan(&oa));
        b.scan(&ob);
        let (files, gone) = a.listing();
        assert_eq!(files.len(), 1, "{files:?}");
        assert!(oa.get(files[0].offer).is_some());
        // B wants it; it "arrives" and goes in place with A's time.
        let want = b.compare("a", &files, &gone);
        assert_eq!(want.len(), 1);
        assert_eq!(want[0].2.path, "todo.txt");
        assert!(b.compare("a", &files, &gone).is_empty(), "already on its way");
        let arrived = rb.join(".sync/1/todo.txt");
        std::fs::create_dir_all(arrived.parent().unwrap()).unwrap();
        std::fs::write(&arrived, "milk").unwrap();
        b.landed(want[0].1, &[arrived], true);
        assert_eq!(std::fs::read_to_string(rb.join("notes/todo.txt")).unwrap(), "milk");
        assert!(!rb.join(".sync/1").exists());
        // Same time on both: nothing more to fetch either way.
        b.scan(&ob);
        assert!(a.compare("b", &b.listing().0, &b.listing().1).is_empty());
        assert!(b.compare("a", &files, &gone).is_empty());
        // Deleted on A: gone from B too.
        std::fs::remove_file(ra.join("notes/todo.txt")).unwrap();
        assert!(a.scan(&oa));
        let (files, gone) = a.listing();
        assert!(files.is_empty() && gone.len() == 1);
        b.compare("a", &files, &gone);
        assert!(!rb.join("notes/todo.txt").exists());
        assert!(!rb.join("notes").exists(), "empty folder removed");
        // Unsafe paths are ignored.
        let evil = FolderEntry { path: "../evil".into(), size: 1, mtime: 1, offer: 9 };
        assert!(b.compare("a", &[evil], &[("../../x".into(), u64::MAX)]).is_empty());
        let _ = std::fs::remove_dir_all(ra);
        let _ = std::fs::remove_dir_all(rb);
    }
}
