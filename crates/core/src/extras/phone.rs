//! Phones (iPhone and Android) without an app: a small web page on the
//! local network. The island shows a QR code; the phone's camera opens the
//! page, which sends photos and files to this computer and picks up the
//! files you drop on "Phone" in the island.
//!
//! Every address carries a long random key (part of the QR code), so other
//! people on the network can't use it.

use parking_lot::Mutex;
use serde::Serialize;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Ports tried, in order (then any free one).
const PORTS: [u16; 3] = [24852, 24862, 24872];
/// Files offered to the phone are listed this long.
const KEEP: Duration = Duration::from_secs(2 * 3600);
/// A phone that asked within this long counts as connected.
const SEEN: Duration = Duration::from_secs(8);

#[derive(Debug, Clone)]
pub enum PhoneEvent {
    /// A file arrived from the phone (saved at `path`).
    Received { name: String, path: PathBuf, size: u64 },
    /// Text sent from the phone (for the clipboard).
    Text(String),
    /// The phone picked up a file.
    Sent { name: String },
    /// A phone opened the page.
    Connected { device: String },
}

#[derive(Debug, Clone, Serialize)]
pub struct Offered {
    pub id: u64,
    pub name: String,
    pub size: u64,
    #[serde(skip)]
    path: PathBuf,
    #[serde(skip)]
    added: Instant,
}

#[derive(Debug, Clone, Serialize)]
pub struct PhoneState {
    pub url: String,
    /// The QR code for `url` (SVG, as a data: URL).
    pub qr: String,
    pub connected: bool,
    pub device: String,
    pub offered: Vec<String>,
}

struct Inner {
    token: String,
    name: String,
    dir: PathBuf,
    port: u16,
    offered: Mutex<Vec<Offered>>,
    next: Mutex<u64>,
    seen: Mutex<Option<(Instant, String)>>,
    notify: Box<dyn Fn(PhoneEvent) + Send + Sync>,
}

#[derive(Clone)]
pub struct PhoneServer {
    inner: Arc<Inner>,
}

/// The key in the page's address, kept so a bookmarked page keeps working.
fn token(config_dir: &Path) -> String {
    let f = config_dir.join("phone-key");
    if let Ok(t) = std::fs::read_to_string(&f) {
        let t = t.trim();
        if t.len() >= 24 && t.chars().all(|c| c.is_ascii_hexdigit()) {
            return t.to_string();
        }
    }
    let t = crate::config::random_hex(16);
    let _ = std::fs::create_dir_all(config_dir);
    let _ = std::fs::write(&f, &t);
    t
}

impl PhoneServer {
    /// Start listening. `dir`: where files from the phone go. `name`: this
    /// computer's name (shown on the page).
    pub fn start(config_dir: &Path, dir: PathBuf, name: &str, notify: impl Fn(PhoneEvent) + Send + Sync + 'static) -> std::io::Result<PhoneServer> {
        let listener = PORTS.iter().find_map(|p| TcpListener::bind(("0.0.0.0", *p)).ok()).map(Ok).unwrap_or_else(|| TcpListener::bind(("0.0.0.0", 0)))?;
        let port = listener.local_addr()?.port();
        let inner = Arc::new(Inner {
            token: token(config_dir),
            name: name.to_string(),
            dir,
            port,
            offered: Mutex::new(vec![]),
            next: Mutex::new(1),
            seen: Mutex::new(None),
            notify: Box::new(notify),
        });
        let me = inner.clone();
        std::thread::Builder::new().name("phone".into()).spawn(move || {
            for conn in listener.incoming().flatten() {
                let me = me.clone();
                let _ = std::thread::Builder::new().name("phone-conn".into()).spawn(move || {
                    let _ = conn.set_read_timeout(Some(Duration::from_secs(60)));
                    let _ = conn.set_write_timeout(Some(Duration::from_secs(60)));
                    let keep = conn.try_clone().ok();
                    if let Err(e) = serve(&me, conn) {
                        log::debug!("phone: {e}");
                    }
                    // Close gently: anything the phone still sends is read
                    // first, or some systems reset the connection and the
                    // phone loses the answer.
                    if let Some(mut c) = keep {
                        let _ = c.shutdown(std::net::Shutdown::Write);
                        let _ = c.set_read_timeout(Some(Duration::from_millis(500)));
                        let mut sink = [0u8; 4096];
                        while matches!(c.read(&mut sink), Ok(n) if n > 0) {}
                    }
                });
            }
        })?;
        log::info!("phone page on port {port}");
        Ok(PhoneServer { inner })
    }

    pub fn port(&self) -> u16 {
        self.inner.port
    }

    /// The page's address on this network (None when offline).
    pub fn url(&self) -> Option<String> {
        let ip = crate::engine::local_addresses().into_iter().next()?;
        Some(format!("http://{ip}:{}/{}/", self.inner.port, self.inner.token))
    }

    pub fn state(&self) -> PhoneState {
        let url = self.url().unwrap_or_default();
        let qr = if url.is_empty() { String::new() } else { qr_url(&url) };
        let seen = self.inner.seen.lock().clone();
        let (connected, device) = match seen {
            Some((t, d)) => (t.elapsed() < SEEN, d),
            None => (false, String::new()),
        };
        let offered = self.offered().into_iter().map(|o| o.name).collect();
        PhoneState { url, qr, connected, device, offered }
    }

    /// Offer files to the phone (folders are skipped). Returns how many.
    pub fn offer(&self, paths: &[PathBuf]) -> usize {
        let mut list = self.inner.offered.lock();
        let mut n = 0;
        for p in paths {
            let Ok(meta) = std::fs::metadata(p) else { continue };
            if !meta.is_file() {
                continue;
            }
            let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "file".into());
            list.retain(|o| o.path != *p);
            let mut next = self.inner.next.lock();
            list.insert(0, Offered { id: *next, name, size: meta.len(), path: p.clone(), added: Instant::now() });
            *next += 1;
            n += 1;
        }
        list.truncate(40);
        n
    }

    pub fn forget(&self, id: u64) {
        self.inner.offered.lock().retain(|o| o.id != id);
    }

    fn offered(&self) -> Vec<Offered> {
        offered(&self.inner)
    }
}

fn offered(inner: &Inner) -> Vec<Offered> {
    let mut list = inner.offered.lock();
    list.retain(|o| o.added.elapsed() < KEEP && o.path.is_file());
    list.clone()
}

/// A QR code as an SVG data: URL (dark on white, which every camera reads).
pub fn qr_url(text: &str) -> String {
    use qrcode::render::svg;
    let Ok(code) = qrcode::QrCode::with_error_correction_level(text.as_bytes(), qrcode::EcLevel::M) else { return String::new() };
    let svg = code.render::<svg::Color>().min_dimensions(180, 180).quiet_zone(true).dark_color(svg::Color("#000")).light_color(svg::Color("#fff")).build();
    format!("data:image/svg+xml;base64,{}", super::base64(svg.as_bytes()))
}

// ---------------------------------------------------------------- HTTP

struct Request {
    method: String,
    path: String,
    query: Vec<(String, String)>,
    headers: Vec<(String, String)>,
}

impl Request {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
    fn q(&self, name: &str) -> Option<&str> {
        self.query.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }
}

fn read_request(r: &mut BufReader<TcpStream>) -> std::io::Result<Option<Request>> {
    let mut line = String::new();
    if r.read_line(&mut line)? == 0 {
        return Ok(None);
    }
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let target = parts.next().unwrap_or("/").to_string();
    let mut headers = vec![];
    let mut total = line.len();
    loop {
        let mut h = String::new();
        let n = r.read_line(&mut h)?;
        total += n;
        if n == 0 || h == "\r\n" || h == "\n" || total > 32 * 1024 {
            break;
        }
        if let Some((k, v)) = h.split_once(':') {
            headers.push((k.trim().to_string(), v.trim().to_string()));
        }
    }
    let (path, qs) = target.split_once('?').unwrap_or((&target, ""));
    let query = qs
        .split('&')
        .filter(|s| !s.is_empty())
        .map(|kv| {
            let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
            (pct_decode(k), pct_decode(v))
        })
        .collect();
    Ok(Some(Request { method, path: pct_decode(path), query, headers }))
}

fn pct_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'%' if i + 2 < b.len() => {
                match u8::from_str_radix(std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or("zz"), 16) {
                    Ok(v) => {
                        out.push(v);
                        i += 3;
                        continue;
                    }
                    Err(_) => out.push(b'%'),
                }
            }
            b'+' => out.push(b' '),
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn pct_encode(s: &str) -> String {
    s.bytes()
        .map(|c| if c.is_ascii_alphanumeric() || b"-._~".contains(&c) { (c as char).to_string() } else { format!("%{c:02X}") })
        .collect()
}

fn respond(w: &mut TcpStream, status: &str, ctype: &str, extra: &str, body: &[u8]) -> std::io::Result<()> {
    write!(
        w,
        "HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nReferrer-Policy: no-referrer\r\nX-Content-Type-Options: nosniff\r\nConnection: close\r\n{extra}\r\n",
        body.len()
    )?;
    w.write_all(body)?;
    w.flush()
}

/// A short name for the phone from its browser's description.
fn device_of(ua: &str) -> String {
    if ua.contains("iPhone") {
        "iPhone".into()
    } else if ua.contains("iPad") {
        "iPad".into()
    } else if ua.contains("Android") {
        "Android phone".into()
    } else {
        "Phone".into()
    }
}

/// A safe file name: no folders, no hidden files, not too long.
fn clean_name(s: &str) -> String {
    let base = s.rsplit(['/', '\\']).next().unwrap_or("");
    let mut n: String = base.chars().filter(|c| !c.is_control() && !"<>:\"|?*".contains(*c)).collect();
    n = n.trim().trim_start_matches('.').trim().to_string();
    if n.chars().count() > 180 {
        let ext = Path::new(&n).extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_default();
        let stem: String = n.chars().take(160).collect();
        n = if ext.is_empty() || ext.len() > 12 { stem } else { format!("{stem}.{ext}") };
    }
    if n.is_empty() {
        "file".into()
    } else {
        n
    }
}

/// `dir/name`, or `dir/name (2)` and so on when taken.
fn free_path(dir: &Path, name: &str) -> PathBuf {
    let p = dir.join(name);
    if !p.exists() {
        return p;
    }
    let path = Path::new(name);
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| name.into());
    let ext = path.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    (2..10_000).map(|i| dir.join(format!("{stem} ({i}){ext}"))).find(|p| !p.exists()).unwrap_or(p)
}

fn serve(me: &Inner, conn: TcpStream) -> std::io::Result<()> {
    let mut w = conn.try_clone()?;
    let mut r = BufReader::new(conn);
    let Some(req) = read_request(&mut r)? else { return Ok(()) };
    let prefix = format!("/{}/", me.token);
    let Some(rest) = req.path.strip_prefix(&prefix).map(str::to_string).or_else(|| (req.path == prefix.trim_end_matches('/')).then(String::new)) else {
        return respond(&mut w, "404 Not Found", "text/plain", "", b"Not found");
    };
    let ua = req.header("User-Agent").unwrap_or("").to_string();
    let device = device_of(&ua);
    {
        let mut seen = me.seen.lock();
        let fresh = seen.as_ref().map(|(t, _)| t.elapsed() > Duration::from_secs(60)).unwrap_or(true);
        *seen = Some((Instant::now(), device.clone()));
        if fresh {
            drop(seen);
            (me.notify)(PhoneEvent::Connected { device: device.clone() });
        }
    }
    match (req.method.as_str(), rest.as_str()) {
        ("GET", "") => {
            let page = PAGE.replace("{{NAME}}", &html_escape(&me.name));
            respond(&mut w, "200 OK", "text/html; charset=utf-8", "", page.as_bytes())
        }
        ("GET", "list") => {
            let list = offered(me);
            respond(&mut w, "200 OK", "application/json", "", serde_json::to_string(&list).unwrap_or_default().as_bytes())
        }
        ("GET", d) if d.starts_with("dl/") => {
            let id: u64 = d[3..].parse().unwrap_or(0);
            let Some(o) = offered(me).into_iter().find(|o| o.id == id) else {
                return respond(&mut w, "404 Not Found", "text/plain", "", b"That file isn't shared anymore");
            };
            let mut f = std::fs::File::open(&o.path)?;
            let len = f.metadata()?.len();
            write!(
                w,
                "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: {len}\r\nContent-Disposition: attachment; filename*=UTF-8''{}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
                pct_encode(&o.name)
            )?;
            let sent = std::io::copy(&mut f, &mut w)?;
            w.flush()?;
            if sent == len {
                (me.notify)(PhoneEvent::Sent { name: o.name });
            }
            Ok(())
        }
        ("PUT", "up") | ("POST", "up") => {
            let len: u64 = req.header("Content-Length").and_then(|v| v.parse().ok()).unwrap_or(0);
            let name = clean_name(req.q("name").unwrap_or("file"));
            std::fs::create_dir_all(&me.dir)?;
            let part = me.dir.join(format!(".{}.{}.part", name, crate::config::random_hex(4)));
            let got = {
                let mut f = std::fs::File::create(&part)?;
                std::io::copy(&mut (&mut r).take(len), &mut f)?
            };
            if got != len {
                let _ = std::fs::remove_file(&part);
                return respond(&mut w, "400 Bad Request", "text/plain", "", b"The upload was cut off");
            }
            let dest = free_path(&me.dir, &name);
            std::fs::rename(&part, &dest)?;
            log::info!("phone sent {} ({len} bytes)", dest.display());
            (me.notify)(PhoneEvent::Received { name: dest.file_name().unwrap_or_default().to_string_lossy().into_owned(), path: dest, size: len });
            respond(&mut w, "200 OK", "application/json", "", b"{\"ok\":true}")
        }
        ("POST", "text") => {
            let len = req.header("Content-Length").and_then(|v| v.parse::<u64>().ok()).unwrap_or(0).min(1 << 20);
            let mut s = String::new();
            (&mut r).take(len).read_to_string(&mut s)?;
            if !s.is_empty() {
                (me.notify)(PhoneEvent::Text(s));
            }
            respond(&mut w, "200 OK", "application/json", "", b"{\"ok\":true}")
        }
        _ => respond(&mut w, "404 Not Found", "text/plain", "", b"Not found"),
    }
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

const PAGE: &str = include_str!("phone.html");

#[cfg(test)]
mod tests {
    use super::*;

    fn get(port: u16, req: &str, body: &[u8]) -> (String, Vec<u8>) {
        let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        s.write_all(req.as_bytes()).unwrap();
        s.write_all(body).unwrap();
        let mut out = vec![];
        s.read_to_end(&mut out).unwrap();
        let i = out.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
        (String::from_utf8_lossy(&out[..i]).into_owned(), out[i + 4..].to_vec())
    }

    #[test]
    fn names_and_decoding() {
        assert_eq!(clean_name("../../etc/passwd"), "passwd");
        assert_eq!(clean_name("..hidden"), "hidden");
        assert_eq!(clean_name("C:\\x\\Photo 1.HEIC"), "Photo 1.HEIC");
        assert_eq!(clean_name(""), "file");
        assert_eq!(pct_decode("a%20b+c%2"), "a b c%2");
        assert_eq!(pct_decode("%E2%9C%93"), "✓");
        assert_eq!(pct_encode("a b✓"), "a%20b%E2%9C%93");
    }

    #[test]
    fn upload_list_download() {
        let base = std::env::temp_dir().join(format!("openhop-phone-{}", std::process::id()));
        let cfg = base.join("cfg");
        let dir = base.join("in");
        let events = Arc::new(Mutex::new(vec![]));
        let ev = events.clone();
        let srv = PhoneServer::start(&cfg, dir.clone(), "Desk <1>", move |e| ev.lock().push(e)).unwrap();
        let port = srv.port();
        let t = srv.inner.token.clone();
        // Without the key: nothing.
        let (h, _) = get(port, "GET / HTTP/1.1\r\n\r\n", b"");
        assert!(h.starts_with("HTTP/1.1 404"));
        // The page, with the name escaped.
        let (h, b) = get(port, &format!("GET /{t}/ HTTP/1.1\r\nUser-Agent: Mozilla (iPhone)\r\n\r\n"), b"");
        assert!(h.starts_with("HTTP/1.1 200"));
        assert!(String::from_utf8_lossy(&b).contains("Desk &lt;1&gt;"));
        // Upload twice: the second gets a new name.
        for _ in 0..2 {
            let (h, _) = get(port, &format!("PUT /{t}/up?name=..%2Fa%20b.txt HTTP/1.1\r\nContent-Length: 5\r\n\r\n"), b"hello");
            assert!(h.starts_with("HTTP/1.1 200"), "{h}");
        }
        assert_eq!(std::fs::read_to_string(dir.join("a b.txt")).unwrap(), "hello");
        assert!(dir.join("a b (2).txt").is_file());
        // Text.
        get(port, &format!("POST /{t}/text HTTP/1.1\r\nContent-Length: 2\r\n\r\n"), b"hi");
        // Offer a file and download it.
        let f = base.join("out.bin");
        std::fs::write(&f, b"0123456789").unwrap();
        assert_eq!(srv.offer(&[f.clone(), base.clone()]), 1);
        let (_, b) = get(port, &format!("GET /{t}/list HTTP/1.1\r\n\r\n"), b"");
        let list: serde_json::Value = serde_json::from_slice(&b).unwrap();
        let id = list[0]["id"].as_u64().unwrap();
        assert_eq!(list[0]["name"], "out.bin");
        let (h, b) = get(port, &format!("GET /{t}/dl/{id} HTTP/1.1\r\n\r\n"), b"");
        assert!(h.contains("filename*=UTF-8''out.bin"));
        assert_eq!(b, b"0123456789");
        let ev = events.lock();
        assert!(matches!(&ev[0], PhoneEvent::Connected { device } if device == "iPhone"));
        assert_eq!(ev.iter().filter(|e| matches!(e, PhoneEvent::Received { .. })).count(), 2);
        assert!(ev.iter().any(|e| matches!(e, PhoneEvent::Text(s) if s == "hi")));
        assert!(ev.iter().any(|e| matches!(e, PhoneEvent::Sent { name } if name == "out.bin")));
        assert!(srv.state().connected);
        assert!(qr_url("http://10.0.0.2:24852/abc/").starts_with("data:image/svg+xml;base64,"));
        // The key is kept.
        assert_eq!(token(&cfg), t);
        let _ = std::fs::remove_dir_all(base);
    }
}
