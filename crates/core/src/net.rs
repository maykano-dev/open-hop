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
        tx.send(&Msg::Clipboard(ClipData::Png(big.clone()))).unwrap();
        tx.send(&Msg::Key { key: 4, down: true }).unwrap();
        assert_eq!(rx.recv().unwrap(), Msg::Move { x: 5, y: 7 });
        assert_eq!(rx.recv().unwrap(), Msg::Clipboard(ClipData::Png(big)));
        assert_eq!(rx.recv().unwrap(), Msg::Key { key: 4, down: true });
    }

    #[test]
    fn wrong_passphrase_rejected() {
        let (a, _b) = pair("one", "two");
        assert!(a.is_err());
    }
}
