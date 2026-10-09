use super::*;
use std::net::{TcpListener, TcpStream, UdpSocket};

struct FakeHost {
    events: Mutex<Vec<PhoneEvent>>,
    sent: Mutex<Vec<(String, Vec<String>)>>,
    shelved: Mutex<Vec<String>>,
}

impl PhoneHost for FakeHost {
    fn hub(&self) -> Option<Arc<Hub>> {
        None
    }
    fn send_files(&self, to: &str, paths: Vec<String>) {
        self.sent.lock().push((to.to_string(), paths));
    }
    fn shelf_add(&self, paths: Vec<String>) {
        self.shelved.lock().extend(paths);
    }
    fn shelf_take(&self, _: &str, _: u64) {}
    fn local_files(&self, _: u64) -> Option<Vec<(PathBuf, String)>> {
        None
    }
    fn transfers(&self) -> Vec<TransferInfo> {
        vec![]
    }
    fn event(&self, ev: PhoneEvent) {
        self.events.lock().push(ev);
    }
    fn name(&self) -> String {
        "Desk <1>".into()
    }
}

fn setup(tag: &str) -> (Arc<Phone>, Arc<FakeHost>, PathBuf) {
    std::env::set_var("OPENHOP_PHONE_LOOPBACK", "1");
    let base = std::env::temp_dir().join(format!("openhop-phone-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let host = Arc::new(FakeHost { events: Mutex::new(vec![]), sent: Mutex::new(vec![]), shelved: Mutex::new(vec![]) });
    let phone = Phone::start(host.clone(), &base.join("cfg"), base.join("in"), vec![], None);
    (phone, host, base)
}

fn http(port: u16, req: &str, body: &[u8]) -> (String, Vec<u8>) {
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.write_all(req.as_bytes()).unwrap();
    s.write_all(body).unwrap();
    let mut out = vec![];
    s.read_to_end(&mut out).unwrap();
    let i = out.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
    (String::from_utf8_lossy(&out[..i]).into_owned(), out[i + 4..].to_vec())
}

#[test]
fn pair_url_carries_everything() {
    let (phone, _, base) = setup("url");
    let url = phone.pair_url();
    assert!(url.starts_with(APP_URL));
    let frag = url.split("#p=").nth(1).unwrap();
    let parts: Vec<&str> = frag.split('.').collect();
    assert_eq!(parts.len(), 4);
    assert_eq!(parts[0].len(), 16);
    assert_eq!(crypto::b64url_decode(parts[1]).unwrap().len(), 32);
    assert_eq!(String::from_utf8(crypto::b64url_decode(parts[2]).unwrap()).unwrap(), "Desk <1>");
    assert!(phone.state().qr.starts_with("data:image/svg+xml"));
    // The pairing is kept; renewing changes the secret, not the id.
    let again = crypto::Pairing::load_or_create(&base.join("cfg"));
    assert_eq!(again.secret, parts[1]);
    phone.renew();
    let renewed = phone.pair_url();
    assert!(renewed.contains(parts[0]) && !renewed.contains(parts[1]));
    let _ = std::fs::remove_dir_all(base);
}

#[test]
fn same_wifi_page() {
    let (phone, host, base) = setup("lan");
    let port = phone.lan.lock().as_ref().unwrap().port();
    let t = phone.lan_token();
    // Without the key: nothing.
    let (h, _) = http(port, "GET / HTTP/1.1\r\n\r\n", b"");
    assert!(h.starts_with("HTTP/1.1 404"));
    let (h, _) = http(port, &format!("GET /{t}/ HTTP/1.1\r\n\r\n"), b"");
    assert!(h.starts_with("HTTP/1.1 200"), "{h}");
    // Requests.
    let body = br#"{"op":"hello","cid":"abc","device":"Test phone"}"#;
    let (_, b) = http(port, &format!("POST /{t}/op HTTP/1.1\r\nContent-Length: {}\r\nUser-Agent: Mozilla (iPhone)\r\n\r\n", body.len()), body);
    let v: Value = serde_json::from_slice(&b).unwrap();
    assert_eq!(v["me"], "Desk <1>");
    // Upload twice (the second gets a new name), and one for the shelf, one for another computer.
    for to in ["", "", "shelf", "laptop"] {
        let (h, b) = http(port, &format!("PUT /{t}/up?x=1&name=..%2Fa%20b.txt&to={to}&cid=abc HTTP/1.1\r\nContent-Length: 5\r\n\r\n"), b"hello");
        assert!(h.starts_with("HTTP/1.1 200"), "{h}");
        assert_eq!(serde_json::from_slice::<Value>(&b).unwrap()["ok"], true);
    }
    let dir = base.join("in");
    assert_eq!(std::fs::read_to_string(dir.join("a b.txt")).unwrap(), "hello");
    assert!(dir.join("a b (2).txt").is_file() && dir.join("a b (4).txt").is_file());
    assert_eq!(host.shelved.lock().len(), 1);
    assert_eq!(host.sent.lock()[0].0, "laptop");
    // Offer a file; it's listed, and downloads.
    let f = base.join("out.bin");
    std::fs::write(&f, b"0123456789").unwrap();
    assert_eq!(phone.offer(&[f.clone(), base.clone()]), 1);
    let body = br#"{"op":"state","cid":"abc"}"#;
    let (_, b) = http(port, &format!("POST /{t}/op HTTP/1.1\r\nContent-Length: {}\r\n\r\n", body.len()), body);
    let v: Value = serde_json::from_slice(&b).unwrap();
    let id = v["offered"][0]["id"].as_u64().unwrap();
    assert_eq!(v["offered"][0]["name"], "out.bin");
    let (h, b) = http(port, &format!("GET /{t}/dl/{id} HTTP/1.1\r\n\r\n"), b"");
    assert!(h.contains("filename*=UTF-8''out.bin"));
    assert_eq!(b, b"0123456789");
    let ev = host.events.lock();
    assert!(matches!(&ev[0], PhoneEvent::Connected { device, how: "local" } if device == "iPhone"));
    assert_eq!(ev.iter().filter(|e| matches!(e, PhoneEvent::Received { .. })).count(), 4);
    assert!(ev.iter().any(|e| matches!(e, PhoneEvent::Sent { name, .. } if name == "out.bin")));
    assert!(phone.state().connected);
    drop(ev);
    let _ = std::fs::remove_dir_all(base);
}

#[test]
fn names_and_decoding() {
    use http::{clean_name, pct_decode, pct_encode};
    assert_eq!(clean_name("../../etc/passwd"), "passwd");
    assert_eq!(clean_name("..hidden"), "hidden");
    assert_eq!(clean_name("C:\\x\\Photo 1.HEIC"), "Photo 1.HEIC");
    assert_eq!(clean_name(""), "file");
    assert_eq!(pct_decode("a%20b+c%2"), "a b c%2");
    assert_eq!(pct_decode("%E2%9C%93"), "✓");
    assert_eq!(pct_encode("a b✓"), "a%20b%E2%9C%93");
}

// ------------------------------------------------------------ direct connection

/// The phone's side of the second layer (the PC side is `crypto::Session`).
struct PhoneSide {
    key: [u8; 32],
    sent: u64,
    got: u64,
}

impl PhoneSide {
    fn seal(&mut self, m: &[u8]) -> Vec<u8> {
        use aes_gcm::aead::Aead;
        use aes_gcm::{Aes256Gcm, KeyInit};
        self.sent += 1;
        let mut n = [0u8; 12];
        n[0] = 1;
        n[4..].copy_from_slice(&self.sent.to_be_bytes());
        Aes256Gcm::new_from_slice(&self.key).unwrap().encrypt(&n.into(), m).unwrap()
    }
    fn open(&mut self, m: &[u8]) -> Vec<u8> {
        use aes_gcm::aead::Aead;
        use aes_gcm::{Aes256Gcm, KeyInit};
        self.got += 1;
        let mut n = [0u8; 12];
        n[0] = 2;
        n[4..].copy_from_slice(&self.got.to_be_bytes());
        Aes256Gcm::new_from_slice(&self.key).unwrap().decrypt(&n.into(), m).unwrap()
    }
}

#[test]
fn direct_connection_end_to_end() {
    use str0m::change::SdpAnswer;
    use str0m::net::Receive;
    use str0m::{Candidate, Event, Input, Output, Rtc};
    let (phone, host, base) = setup("rtc");
    let secret = phone.pairing.lock().secret_bytes();

    // The "phone": str0m on loopback, opening a data channel.
    let sock = UdpSocket::bind("127.0.0.1:0").unwrap();
    let local = sock.local_addr().unwrap();
    let mut rtc = Rtc::builder().build(Instant::now());
    rtc.add_local_candidate(Candidate::host(local, "udp").unwrap()).unwrap();
    let mut api = rtc.sdp_api();
    let cid = api.add_channel("openhop".into());
    let (offer, pending) = api.apply().unwrap();
    let phone_nonce = [9u8; 16];
    let (answer, pc_nonce) = phone.answer_offer(&offer.to_sdp_string(), &phone_nonce, "Test phone").unwrap();
    rtc.sdp_api().accept_answer(pending, SdpAnswer::from_sdp_string(&answer).unwrap()).unwrap();
    let mut side = PhoneSide { key: crypto::derive(&secret, &[&phone_nonce[..], &pc_nonce[..]].concat(), "data"), sent: 0, got: 0 };

    // Drive the phone side until `done` says so.
    sock.set_read_timeout(Some(Duration::from_millis(5))).unwrap();
    let mut buf = vec![0u8; 2048];
    let mut open = false;
    let mut inbox: Vec<Vec<u8>> = vec![];
    let mut drive = |rtc: &mut Rtc, open: &mut bool, inbox: &mut Vec<Vec<u8>>, ms: u64| {
        let until = Instant::now() + Duration::from_millis(ms);
        while Instant::now() < until {
            loop {
                match rtc.poll_output().unwrap() {
                    Output::Timeout(_) => break,
                    Output::Transmit(t) => {
                        // (Only the loopback pair works from this socket.)
                        let _ = sock.send_to(&t.contents, t.destination);
                    }
                    Output::Event(Event::ChannelOpen(..)) => *open = true,
                    Output::Event(Event::ChannelData(d)) => inbox.push(d.data),
                    Output::Event(_) => {}
                }
            }
            if let Ok((n, from)) = sock.recv_from(&mut buf) {
                let input = Input::Receive(Instant::now(), Receive { proto: str0m::net::Protocol::Udp, source: from, destination: local, contents: buf[..n].try_into().unwrap() });
                rtc.handle_input(input).unwrap();
            }
            rtc.handle_input(Input::Timeout(Instant::now())).unwrap();
        }
    };
    for _ in 0..100 {
        drive(&mut rtc, &mut open, &mut inbox, 50);
        if open && phone.state().connected {
            break;
        }
    }
    assert!(open && phone.state().connected, "connected");
    assert!(matches!(host.events.lock().last(), Some(PhoneEvent::Connected { how: "direct", .. })));

    // A request.
    let mut m = vec![1u8];
    m.extend_from_slice(br#"{"op":"hello","rid":7,"device":"Pixel"}"#);
    let sealed = side.seal(&m);
    rtc.channel(cid).unwrap().write(true, &sealed).unwrap();
    drive(&mut rtc, &mut open, &mut inbox, 300);
    let reply = side.open(&inbox.remove(0));
    let v: Value = serde_json::from_slice(&reply[1..]).unwrap();
    assert_eq!(v["re"], 7);
    assert_eq!(v["me"], "Desk <1>");

    // A file to the computer, in two pieces.
    let mut m = vec![1u8];
    m.extend_from_slice(br#"{"op":"put","rid":8,"x":3,"name":"pic.jpg","size":6,"to":""}"#);
    let sealed = side.seal(&m);
    rtc.channel(cid).unwrap().write(true, &sealed).unwrap();
    for part in [&b"abc"[..], &b"def"[..]] {
        let mut m = vec![2u8, 0, 0, 0, 3];
        m.extend_from_slice(part);
        let sealed = side.seal(&m);
        rtc.channel(cid).unwrap().write(true, &sealed).unwrap();
    }
    drive(&mut rtc, &mut open, &mut inbox, 400);
    let msgs: Vec<Value> = inbox.drain(..).map(|m| serde_json::from_slice(&side.open(&m)[1..]).unwrap()).collect();
    assert!(msgs.iter().any(|v| v["push"] == "put_done" && v["ok"] == true), "{msgs:?}");
    assert_eq!(std::fs::read(base.join("in/pic.jpg")).unwrap(), b"abcdef");

    // A file from the computer arrives by itself: header, pieces, end.
    let n: u32 = std::env::var("OPENHOP_PHONE_BIG").ok().and_then(|v| v.parse().ok()).unwrap_or(200_000);
    let big: Vec<u8> = (0..n).map(|i| (i % 251) as u8).collect();
    let started = Instant::now();
    let f = base.join("movie.mp4");
    std::fs::write(&f, &big).unwrap();
    phone.offer(std::slice::from_ref(&f));
    let mut got = vec![];
    let mut ended = false;
    for _ in 0..2000 {
        drive(&mut rtc, &mut open, &mut inbox, 50);
        for m in inbox.drain(..) {
            let p = side.open(&m);
            match p[0] {
                1 => {
                    let v: Value = serde_json::from_slice(&p[1..]).unwrap();
                    if v["push"] == "file" {
                        assert_eq!(v["mime"], "video/mp4");
                    }
                    if v["push"] == "file_end" {
                        ended = true;
                    }
                }
                2 => got.extend_from_slice(&p[5..]),
                _ => {}
            }
        }
        if ended {
            break;
        }
    }
    assert!(ended);
    assert_eq!(got, big);
    eprintln!("{} bytes to the phone in {:?}", n, started.elapsed());
    assert!(host.events.lock().iter().any(|e| matches!(e, PhoneEvent::Sent { name, .. } if name == "movie.mp4")));

    // A message that doesn't check out ends the connection.
    rtc.channel(cid).unwrap().write(true, b"garbage that isn't sealed").unwrap();
    for _ in 0..40 {
        drive(&mut rtc, &mut open, &mut inbox, 50);
        if !phone.state().connected {
            break;
        }
    }
    let _ = std::fs::remove_dir_all(base);
}

// ------------------------------------------------------------ meeting point

/// A tiny Nostr relay: passes events on to subscriptions with the same topic.
fn relay() -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    type Subs = Arc<Mutex<Vec<(String, String, crossbeam_channel::Sender<String>)>>>;
    let subs: Subs = Arc::new(Mutex::new(vec![]));
    std::thread::spawn(move || {
        for s in l.incoming().flatten() {
            let subs = subs.clone();
            std::thread::spawn(move || {
                let mut ws = tungstenite::accept(s).unwrap();
                ws.get_ref().set_read_timeout(Some(Duration::from_millis(20))).unwrap();
                let (tx, rx) = crossbeam_channel::unbounded::<String>();
                loop {
                    while let Ok(m) = rx.try_recv() {
                        if ws.send(tungstenite::Message::text(m)).is_err() {
                            return;
                        }
                    }
                    match ws.read() {
                        Ok(tungstenite::Message::Text(t)) => {
                            let v: Value = serde_json::from_str(t.as_str()).unwrap();
                            match v[0].as_str() {
                                Some("REQ") => {
                                    let topic = v[2]["#t"][0].as_str().unwrap().to_string();
                                    subs.lock().push((v[1].as_str().unwrap().to_string(), topic, tx.clone()));
                                }
                                Some("EVENT") => {
                                    let ev = &v[1];
                                    let topic = ev["tags"][0][1].as_str().unwrap_or("");
                                    for (sid, t, out) in subs.lock().iter() {
                                        if t == topic {
                                            let _ = out.send(json!(["EVENT", sid, ev]).to_string());
                                        }
                                    }
                                    let _ = tx.send(json!(["OK", ev["id"], true, ""]).to_string());
                                }
                                _ => {}
                            }
                        }
                        Ok(_) => {}
                        Err(tungstenite::Error::Io(e)) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
                        Err(_) => return,
                    }
                }
            });
        }
    });
    port
}

#[test]
fn meeting_point_answers_a_phone() {
    let port = relay();
    let url = format!("ws://127.0.0.1:{port}");
    let base = std::env::temp_dir().join(format!("openhop-phone-sig-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let host = Arc::new(FakeHost { events: Mutex::new(vec![]), sent: Mutex::new(vec![]), shelved: Mutex::new(vec![]) });
    let phone = Phone::start(host, &base.join("cfg"), base.join("in"), vec![url.clone()], None);
    let secret = phone.pairing.lock().secret_bytes();
    let got = Arc::new(Mutex::new(vec![]));
    let g = got.clone();
    let me = signal::Signal::start(&[url], &secret, move |m| g.lock().push(m));
    // Both connected.
    for _ in 0..100 {
        if me.relays_up() == 1 && phone.state().relays == 1 {
            break;
        }
        std::thread::sleep(Duration::from_millis(30));
    }
    std::thread::sleep(Duration::from_millis(100));
    me.send(json!({ "t": "hi", "from": "phone1" }));
    for _ in 0..100 {
        if !got.lock().is_empty() {
            break;
        }
        std::thread::sleep(Duration::from_millis(30));
    }
    let m = got.lock().first().cloned().expect("the computer answered");
    assert_eq!(m["t"], "here");
    assert_eq!(m["to"], "phone1");
    assert_eq!(m["name"], "Desk <1>");
    let _ = std::fs::remove_dir_all(base);
}
