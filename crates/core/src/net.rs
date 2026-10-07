//! Encrypted, framed transport over TCP.
//!
//! Every connection runs a Noise `NNpsk2` handshake (X25519, ChaCha20-Poly1305,
//! BLAKE2s). The pre-shared key is derived from the passphrase you type on
//! each computer, so only machines that know it can connect, and an
//! eavesdropper can't brute-force the passphrase offline from a recording.
//! There is no plaintext fallback.

use crate::protocol::{decode, encode, Msg};
use anyhow::{anyhow, bail, Context, Result};
use blake2::{Blake2s256, Digest};
use parking_lot::Mutex;
use snow::{Builder, StatelessTransportState};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::Arc;
use std::time::Duration;

const NOISE_PARAMS: &str = "Noise_NNpsk2_25519_ChaChaPoly_BLAKE2s";
const MAX_NOISE: usize = 65535;
const TAG: usize = 16;
const MAX_CHUNK: usize = MAX_NOISE - TAG;
/// Largest application message we accept (clipboard images can be big).
pub const MAX_MSG: usize = 64 * 1024 * 1024;
const WRITE_TIMEOUT: Duration = Duration::from_secs(8);
/// Bulk data is sent in pieces this big so input can slip in between.
pub const CHUNK: usize = 256 * 1024;

/// Sent in the clear before the handshake: tells the server which secret
/// this connection will prove knowledge of. It reveals nothing secret.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Auth {
    /// The shared passphrase.
    Passphrase,
    /// The key saved when this device was paired.
    Key { device: String },
    /// First-time pairing with the 6-digit code shown on the server.
    Pair { device: String, name: String },
}

pub fn write_auth(s: &mut TcpStream, auth: &Auth) -> Result<()> {
    write_frame(s, &bincode::serialize(auth)?)?;
    Ok(())
}

pub fn read_auth(s: &mut TcpStream) -> Result<Auth> {
    s.set_read_timeout(Some(Duration::from_secs(10)))?;
    let mut buf = Vec::new();
    read_frame(s, &mut buf)?;
    if buf.len() > 1024 {
        bail!("bad hello");
    }
    Ok(bincode::deserialize(&buf)?)
}

/// Pre-shared key for a pairing attempt with `code`.
pub fn pairing_psk(code: &str) -> [u8; 32] {
    let digits: String = code.chars().filter(|c| c.is_ascii_digit()).collect();
    derive_psk(&format!("openhop-pairing-code:{digits}"))
}

pub fn derive_psk(passphrase: &str) -> [u8; 32] {
    let mut h = Blake2s256::new();
    h.update(b"openhop-psk-v1\0");
    h.update(passphrase.trim().as_bytes());
    let out = h.finalize();
    let mut psk = [0u8; 32];
    psk.copy_from_slice(out.as_slice());
    psk
}

fn write_frame(s: &mut TcpStream, data: &[u8]) -> std::io::Result<()> {
    let mut buf = Vec::with_capacity(2 + data.len());
    buf.extend_from_slice(&(data.len() as u16).to_be_bytes());
    buf.extend_from_slice(data);
    s.write_all(&buf)
}

fn read_frame(s: &mut TcpStream, buf: &mut Vec<u8>) -> std::io::Result<()> {
    let mut len = [0u8; 2];
    s.read_exact(&mut len)?;
    buf.resize(u16::from_be_bytes(len) as usize, 0);
    s.read_exact(buf)
}

/// Run the handshake. `initiator` is the side that opened the connection.
pub fn handshake(mut stream: TcpStream, psk: &[u8; 32], initiator: bool) -> Result<(SecureSender, SecureReceiver)> {
    stream.set_nodelay(true)?;
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    let builder = Builder::new(NOISE_PARAMS.parse()?).psk(2, psk)?;
    let mut hs = if initiator { builder.build_initiator()? } else { builder.build_responder()? };
    let mut msg = vec![0u8; MAX_NOISE];
    let mut payload = vec![0u8; MAX_NOISE];
    let mut frame = Vec::new();
    if initiator {
        let n = hs.write_message(&[], &mut msg)?;
        write_frame(&mut stream, &msg[..n])?;
        read_frame(&mut stream, &mut frame)?;
        hs.read_message(&frame, &mut payload)
            .map_err(|_| anyhow!("handshake failed: passphrase does not match"))?;
    } else {
        read_frame(&mut stream, &mut frame)?;
        hs.read_message(&frame, &mut payload)?;
        let n = hs.write_message(&[], &mut msg)?;
        write_frame(&mut stream, &msg[..n])?;
    }
    if !hs.is_handshake_finished() {
        bail!("handshake incomplete");
    }
    let transport = Arc::new(hs.into_stateless_transport_mode()?);
    stream.set_read_timeout(None)?;
    // A peer that stops reading (asleep, Wi-Fi gone) must not block us forever.
    stream.set_write_timeout(Some(WRITE_TIMEOUT))?;
    let read_stream = stream.try_clone()?;
    Ok((
        SecureSender { inner: Arc::new(Mutex::new(SenderState { stream, nonce: 0, transport: transport.clone() })) },
        SecureReceiver { stream: read_stream, nonce: 0, transport, plain: Vec::new() },
    ))
}

struct SenderState {
    stream: TcpStream,
    nonce: u64,
    transport: Arc<StatelessTransportState>,
}

/// Cloneable sending half.
#[derive(Clone)]
pub struct SecureSender {
    inner: Arc<Mutex<SenderState>>,
}

impl SecureSender {
    pub fn send(&self, msg: &Msg) -> Result<()> {
        let body = encode(msg);
        let mut plain = Vec::with_capacity(4 + body.len());
        plain.extend_from_slice(&(body.len() as u32).to_be_bytes());
        plain.extend_from_slice(&body);
        let mut st = self.inner.lock();
        let mut out = Vec::with_capacity(plain.len() + (plain.len() / MAX_CHUNK + 1) * (TAG + 2));
        let mut cipher = vec![0u8; MAX_NOISE];
        for chunk in plain.chunks(MAX_CHUNK) {
            let n = st.transport.write_message(st.nonce, chunk, &mut cipher)?;
            st.nonce += 1;
            out.extend_from_slice(&(n as u16).to_be_bytes());
            out.extend_from_slice(&cipher[..n]);
        }
        st.stream.write_all(&out).context("send")?;
        Ok(())
    }

    pub fn shutdown(&self) {
        let _ = self.inner.lock().stream.shutdown(std::net::Shutdown::Both);
    }
}

pub struct SecureReceiver {
    stream: TcpStream,
    nonce: u64,
    transport: Arc<StatelessTransportState>,
    plain: Vec<u8>,
}

impl SecureReceiver {
    /// A receive that waits longer than this fails (detects dead peers).
    pub fn set_timeout(&self, t: Option<Duration>) -> Result<()> {
        self.stream.set_read_timeout(t)?;
        Ok(())
    }

    /// Blocks until a full message arrives.
    pub fn recv(&mut self) -> Result<Msg> {
        let mut frame = Vec::new();
        let mut buf = vec![0u8; MAX_NOISE];
        loop {
            if self.plain.len() >= 4 {
                let len = u32::from_be_bytes(self.plain[..4].try_into().unwrap()) as usize;
                if len > MAX_MSG {
                    bail!("message too large ({len} bytes)");
                }
                if self.plain.len() >= 4 + len {
                    let msg = decode(&self.plain[4..4 + len]);
                    self.plain.drain(..4 + len);
                    return msg;
                }
            }
            read_frame(&mut self.stream, &mut frame).context("connection closed")?;
            let n = self
                .transport
                .read_message(self.nonce, &frame, &mut buf)
                .map_err(|_| anyhow!("decryption failed"))?;
            self.nonce += 1;
            self.plain.extend_from_slice(&buf[..n]);
        }
    }
}

/// A connection's sending side, owned by a writer thread with two queues:
/// input events (always first) and bulk data (clipboard, files). Callers
/// never block on the network; a stalled peer is detected and dropped.
#[derive(Clone)]
pub struct Link {
    ctl: crossbeam_channel::Sender<Msg>,
    bulk: crossbeam_channel::Sender<Msg>,
    dead: Arc<std::sync::atomic::AtomicBool>,
    sender: SecureSender,
}

impl Link {
    /// `on_dead` runs once, on the writer thread, when the connection fails.
    pub fn new(sender: SecureSender, on_dead: impl FnOnce() + Send + 'static) -> Link {
        use std::sync::atomic::Ordering;
        let (ctl, ctl_rx) = crossbeam_channel::unbounded::<Msg>();
        let (bulk, bulk_rx) = crossbeam_channel::bounded::<Msg>(16);
        let dead = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let link = Link { ctl, bulk, dead: dead.clone(), sender: sender.clone() };
        std::thread::Builder::new()
            .name("link-writer".into())
            .spawn(move || {
                loop {
                    if dead.load(Ordering::Relaxed) {
                        break;
                    }
                    // Input first, always.
                    let msg = match ctl_rx.try_recv() {
                        Ok(m) => m,
                        Err(crossbeam_channel::TryRecvError::Disconnected) => break,
                        Err(crossbeam_channel::TryRecvError::Empty) => {
                            crossbeam_channel::select! {
                                recv(ctl_rx) -> m => match m { Ok(m) => m, Err(_) => break },
                                recv(bulk_rx) -> m => match m { Ok(m) => m, Err(_) => continue },
                                default(Duration::from_millis(500)) => continue,
                            }
                        }
                    };
                    if let Err(e) = sender.send(&msg) {
                        log::debug!("link write failed: {e:#}");
                        break;
                    }
                }
                dead.store(true, Ordering::SeqCst);
                sender.shutdown();
                on_dead();
            })
            .expect("spawn link writer");
        link
    }

    /// Queue a message. Bulk messages are dropped (returns false) if the
    /// bulk queue is full; use [`Link::send_bulk_wait`] from worker threads.
    pub fn send(&self, msg: Msg) -> bool {
        if self.is_dead() {
            return false;
        }
        if msg.is_bulk() {
            self.bulk.try_send(msg).is_ok()
        } else {
            if self.ctl.len() > 10_000 {
                // Nothing has been written for a long time: give up on this peer.
                self.close();
                return false;
            }
            self.ctl.send(msg).is_ok()
        }
    }

    /// Queue bulk data, waiting (with back-pressure) while the queue is full.
    pub fn send_bulk_wait(&self, msg: Msg) -> bool {
        let mut msg = msg;
        loop {
            if self.is_dead() {
                return false;
            }
            match self.bulk.send_timeout(msg, Duration::from_millis(500)) {
                Ok(()) => return true,
                Err(crossbeam_channel::SendTimeoutError::Timeout(m)) => msg = m,
                Err(crossbeam_channel::SendTimeoutError::Disconnected(_)) => return false,
            }
        }
    }

    pub fn is_dead(&self) -> bool {
        self.dead.load(std::sync::atomic::Ordering::Relaxed)
    }

    pub fn close(&self) {
        self.dead.store(true, std::sync::atomic::Ordering::SeqCst);
        self.sender.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::ClipData;
    use std::net::TcpListener;

    fn pair(psk_a: &str, psk_b: &str) -> (Result<(SecureSender, SecureReceiver)>, Result<(SecureSender, SecureReceiver)>) {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        let pb = derive_psk(psk_b);
        let t = std::thread::spawn(move || {
            let (s, _) = l.accept().unwrap();
            handshake(s, &pb, false)
        });
        let a = handshake(TcpStream::connect(addr).unwrap(), &derive_psk(psk_a), true);
        (a, t.join().unwrap())
    }

    #[test]
    fn roundtrip_small_and_large() {
        let (a, b) = pair("correct horse", "correct horse");
        let (tx, _) = a.unwrap();
        let (_, mut rx) = b.unwrap();
        tx.send(&Msg::Move { x: 5, y: 7 }).unwrap();
        let big = vec![7u8; 300_000];
        tx.send(&Msg::Clip { origin: "a".into(), data: ClipData::Png(big.clone()) }).unwrap();
        tx.send(&Msg::Key { key: 4, down: true }).unwrap();
        assert_eq!(rx.recv().unwrap(), Msg::Move { x: 5, y: 7 });
        assert_eq!(rx.recv().unwrap(), Msg::Clip { origin: "a".into(), data: ClipData::Png(big) });
        assert_eq!(rx.recv().unwrap(), Msg::Key { key: 4, down: true });
    }

    #[test]
    fn input_overtakes_bulk_and_stalled_peer_is_detected() {
        let (a, b) = pair("pw", "pw");
        let (tx, _) = a.unwrap();
        let (_, mut rx) = b.unwrap();
        let died = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let d2 = died.clone();
        let link = Link::new(tx, move || d2.store(true, std::sync::atomic::Ordering::SeqCst));
        // Queue far more bulk data than any OS socket buffer holds (16 MB),
        // from a worker thread, as file transfers do.
        let bulk_link = link.clone();
        let filler = std::thread::spawn(move || {
            for i in 0..80u64 {
                if !bulk_link.send_bulk_wait(Msg::ClipPart { origin: String::new(), id: i, total: 1, data: vec![0; 200_000] }) {
                    break;
                }
            }
        });
        std::thread::sleep(Duration::from_millis(300));
        assert!(link.send(Msg::Key { key: 4, down: true }));
        // Input must arrive long before the queued bulk data is through.
        let mut saw_key_at = None;
        for n in 0..81 {
            if rx.recv().unwrap() == (Msg::Key { key: 4, down: true }) {
                saw_key_at = Some(n);
                break;
            }
        }
        let at = saw_key_at.expect("key never arrived");
        assert!(at < 60, "key arrived after {at} bulk messages");
        // The peer stops reading entirely: the link must notice instead of hanging.
        drop(rx);
        let start = std::time::Instant::now();
        while !link.is_dead() && start.elapsed() < Duration::from_secs(20) {
            link.send(Msg::Ping);
            std::thread::sleep(Duration::from_millis(20));
        }
        std::thread::sleep(Duration::from_millis(200));
        assert!(link.is_dead() && died.load(std::sync::atomic::Ordering::SeqCst));
        filler.join().unwrap();
    }

    #[test]
    fn wrong_passphrase_rejected() {
        let (a, _b) = pair("one", "two");
        assert!(a.is_err());
    }
}
