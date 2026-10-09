//! The same-Wi-Fi way in: this computer serves the phone app itself, for
//! when there's no internet (or the phone can't reach the meeting point).
//! Every address carries a long random key, so other people on the network
//! can't use it. (It's plain HTTP: the direct connection is the encrypted
//! one, and the app uses that whenever it can.)

use super::{Conn, Phone, PhoneEvent};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Weak};
use std::time::Duration;

/// Ports tried, in order (then any free one).
const PORTS: [u16; 3] = [24852, 24862, 24872];

pub struct LanServer {
    port: u16,
    stop: Arc<std::sync::atomic::AtomicBool>,
}

impl Drop for LanServer {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
        // Wake the listener so it notices.
        let _ = TcpStream::connect_timeout(&(std::net::Ipv4Addr::LOCALHOST, self.port).into(), Duration::from_millis(200));
    }
}

/// The phone app's files (the same ones as on the web).
const FILES: &[(&str, &str, &str)] = &[
    ("", "text/html; charset=utf-8", include_str!("../../../../../web/phone/index.html")),
    ("app.js", "text/javascript; charset=utf-8", include_str!("../../../../../web/phone/app.js")),
    ("app.css", "text/css; charset=utf-8", include_str!("../../../../../web/phone/app.css")),
];

impl LanServer {
    pub fn start(token: &str, phone: Weak<Phone>) -> std::io::Result<LanServer> {
        let listener = PORTS.iter().find_map(|p| TcpListener::bind(("0.0.0.0", *p)).ok()).map(Ok).unwrap_or_else(|| TcpListener::bind(("0.0.0.0", 0)))?;
        let port = listener.local_addr()?.port();
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (token, st) = (token.to_string(), stop.clone());
        std::thread::Builder::new().name("phone-lan".into()).spawn(move || {
            for conn in listener.incoming().flatten() {
                if st.load(std::sync::atomic::Ordering::Relaxed) {
                    break;
                }
                let Some(phone) = phone.upgrade() else { break };
                let token = token.clone();
                let _ = std::thread::Builder::new().name("phone-conn".into()).spawn(move || {
                    let _ = conn.set_read_timeout(Some(Duration::from_secs(60)));
                    let _ = conn.set_write_timeout(Some(Duration::from_secs(60)));
                    let keep = conn.try_clone().ok();
                    if let Err(e) = serve(&phone, &token, conn) {
                        log::debug!("phone page: {e}");
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
        log::info!("phone page (same Wi-Fi) on port {port}");
        Ok(LanServer { port, stop })
    }

    pub fn port(&self) -> u16 {
        self.port
    }
}

/// A QR code as an SVG data: URL (dark on white, which every camera reads).
pub fn qr_url(text: &str) -> String {
    use qrcode::render::svg;
    let Ok(code) = qrcode::QrCode::with_error_correction_level(text.as_bytes(), qrcode::EcLevel::L) else { return String::new() };
    let svg = code.render::<svg::Color>().min_dimensions(180, 180).quiet_zone(true).dark_color(svg::Color("#000")).light_color(svg::Color("#fff")).build();
    format!("data:image/svg+xml;base64,{}", crate::extras::base64(svg.as_bytes()))
}

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
    fn q(&self, name: &str) -> &str {
        self.query.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str()).unwrap_or("")
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

pub fn pct_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'%' if i + 2 < b.len() => match u8::from_str_radix(std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or("zz"), 16) {
                Ok(v) => {
                    out.push(v);
                    i += 3;
                    continue;
                }
                Err(_) => out.push(b'%'),
            },
            b'+' => out.push(b' '),
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub fn pct_encode(s: &str) -> String {
    s.bytes().map(|c| if c.is_ascii_alphanumeric() || b"-._~".contains(&c) { (c as char).to_string() } else { format!("%{c:02X}") }).collect()
}

fn respond(w: &mut TcpStream, status: &str, ctype: &str, body: &[u8]) -> std::io::Result<()> {
    write!(
        w,
        "HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nReferrer-Policy: no-referrer\r\nX-Content-Type-Options: nosniff\r\nConnection: close\r\n\r\n",
        body.len()
    )?;
    w.write_all(body)?;
    w.flush()
}

fn json_reply(w: &mut TcpStream, v: &Value) -> std::io::Result<()> {
    respond(w, "200 OK", "application/json", v.to_string().as_bytes())
}

/// A short name for the phone from its browser's description.
pub fn device_of(ua: &str) -> String {
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

pub fn mime_of(name: &str) -> &'static str {
    let ext = Path::new(name).extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    match ext.as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "heic" => "image/heic",
        "mp4" | "m4v" => "video/mp4",
        "mov" => "video/quicktime",
        "mp3" => "audio/mpeg",
        "m4a" => "audio/mp4",
        "pdf" => "application/pdf",
        "txt" => "text/plain",
        "zip" => "application/zip",
        _ => "application/octet-stream",
    }
}

/// A safe file name: no folders, no hidden files, not too long.
pub fn clean_name(s: &str) -> String {
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
pub fn free_path(dir: &Path, name: &str) -> PathBuf {
    let p = dir.join(name);
    if !p.exists() {
        return p;
    }
    let path = Path::new(name);
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| name.into());
    let ext = path.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    (2..10_000).map(|i| dir.join(format!("{stem} ({i}){ext}"))).find(|p| !p.exists()).unwrap_or(p)
}

/// The browser's own id for itself (so each phone is one connection).
fn browser_key(req: &Request, cid: &str) -> u64 {
    let s = if cid.is_empty() { req.header("User-Agent").unwrap_or("") } else { cid };
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.bytes() {
        h = (h ^ b as u64).wrapping_mul(0x100000001b3);
    }
    h & ((1 << 39) - 1)
}

fn serve(phone: &Arc<Phone>, token: &str, conn: TcpStream) -> std::io::Result<()> {
    let mut w = conn.try_clone()?;
    let mut r = BufReader::new(conn);
    let Some(req) = read_request(&mut r)? else { return Ok(()) };
    let prefix = format!("/{token}/");
    let Some(rest) = req.path.strip_prefix(&prefix).map(str::to_string).or_else(|| (req.path == prefix.trim_end_matches('/')).then(String::new)) else {
        return respond(&mut w, "404 Not Found", "text/plain", b"Not found");
    };
    let ua = req.header("User-Agent").unwrap_or("").to_string();
    if req.method == "GET" {
        if let Some((_, ctype, body)) = FILES.iter().find(|(p, _, _)| *p == rest) {
            return respond(&mut w, "200 OK", ctype, body.as_bytes());
        }
    }
    match (req.method.as_str(), rest.as_str()) {
        ("POST", "op") => {
            let len = req.header("Content-Length").and_then(|v| v.parse::<u64>().ok()).unwrap_or(0).min(1 << 20);
            let mut body = String::new();
            (&mut r).take(len).read_to_string(&mut body)?;
            let Ok(v) = serde_json::from_str::<Value>(&body) else {
                return respond(&mut w, "400 Bad Request", "text/plain", b"Bad request");
            };
            let cid = v.get("cid").and_then(|c| c.as_str()).unwrap_or("").to_string();
            let c = phone.http_conn(browser_key(&req, &cid), &device_of(&ua));
            let out = phone.op(&c, &v);
            json_reply(&mut w, &out)
        }
        ("PUT", "up") | ("POST", "up") => {
            let len: u64 = req.header("Content-Length").and_then(|v| v.parse().ok()).unwrap_or(0);
            let c = phone.http_conn(browser_key(&req, req.q("cid")), &device_of(&ua));
            let x: u32 = req.q("x").parse().unwrap_or(0);
            match phone.upload_begin(&c, x, req.q("name"), len, req.q("to")) {
                Ok(Some(name)) => return json_reply(&mut w, &json!({ "ok": true, "name": name })),
                Ok(None) => {}
                Err(e) => return json_reply(&mut w, &json!({ "ok": false, "error": e })),
            }
            let mut buf = vec![0u8; 64 * 1024];
            let mut got = 0u64;
            while got < len {
                let want = buf.len().min((len - got) as usize);
                let n = r.read(&mut buf[..want])?;
                if n == 0 {
                    break;
                }
                got += n as u64;
                phone.upload_bytes_http(&c, x, &buf[..n]);
            }
            let name = phone.upload_result(&c, x);
            json_reply(&mut w, &json!({ "ok": name.is_some(), "name": name.unwrap_or_default() }))
        }
        ("GET", d) if d.starts_with("dl/") => {
            let id: u64 = d[3..].parse().unwrap_or(0);
            let Some(o) = phone.offered_file(id) else {
                return respond(&mut w, "404 Not Found", "text/plain", b"That file isn't shared anymore");
            };
            let mut f = std::fs::File::open(&o.path)?;
            let len = f.metadata()?.len();
            write!(
                w,
                "HTTP/1.1 200 OK\r\nContent-Type: {}\r\nContent-Length: {len}\r\nContent-Disposition: attachment; filename*=UTF-8''{}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
                mime_of(&o.name),
                pct_encode(&o.name)
            )?;
            let device = device_of(&ua);
            let xfer = phone.xfer_start(&o.name, &device, false, len);
            let mut buf = vec![0u8; 64 * 1024];
            let mut sent = 0u64;
            let r = (|| -> std::io::Result<()> {
                loop {
                    let n = f.read(&mut buf)?;
                    if n == 0 {
                        return Ok(());
                    }
                    w.write_all(&buf[..n])?;
                    sent += n as u64;
                    phone.xfer_update(xfer, sent, false);
                }
            })();
            let _ = w.flush();
            phone.xfer_update(xfer, sent, true);
            if r.is_ok() && sent == len {
                phone.host.event(PhoneEvent::Sent { name: o.name, device });
            }
            r
        }
        _ => respond(&mut w, "404 Not Found", "text/plain", b"Not found"),
    }
}

impl Phone {
    /// Same-Wi-Fi uploads come in one request: no reply per file here.
    fn upload_bytes_http(&self, c: &Conn, x: u32, data: &[u8]) {
        let mut ups = c.uploads.lock();
        let Some(u) = ups.get_mut(&x) else { return };
        if let Some(f) = u.file.as_mut() {
            if f.write_all(data).is_err() {
                u.file = None;
            }
        }
        u.got += data.len() as u64;
        let (xfer, got) = (u.xfer, u.got);
        drop(ups);
        self.xfer_update(xfer, got, false);
    }

    fn upload_result(&self, c: &Conn, x: u32) -> Option<String> {
        let mut u = c.uploads.lock().remove(&x)?;
        if u.file.is_some() && u.got == u.size {
            self.upload_finish(c, &mut u)
        } else {
            let _ = std::fs::remove_file(&u.tmp);
            self.xfer_update(u.xfer, u.got, true);
            None
        }
    }
}
