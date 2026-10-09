//! Keys for the phone link.
//!
//! Pairing puts a random 256-bit secret in the QR code (in the part of the
//! address that never leaves the phone). Everything else comes from it:
//!
//! - the meeting-point topic (a hash: says nothing about the secret),
//! - the key that seals the connection set-up messages (AES-256-GCM), so a
//!   relay can't read them and nobody without the secret can forge one
//!   (that's what stops someone sitting in the middle: the set-up carries
//!   the fingerprints of the encrypted connection),
//! - per-connection keys for a second layer of encryption inside the
//!   WebRTC connection's own (DTLS), from fresh random values each side
//!   picks, so every connection has its own key.

use aes_gcm::aead::{Aead, Payload};
use aes_gcm::{Aes256Gcm, KeyInit};
use hkdf::Hkdf;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::path::Path;

#[derive(Clone, Serialize, Deserialize)]
pub struct Pairing {
    /// This computer, as phones know it (16 hex digits).
    pub id: String,
    /// The shared secret (base64url, 32 bytes).
    pub secret: String,
    /// The key in the local (same Wi-Fi) page's address.
    pub lan: String,
}

impl Pairing {
    pub fn secret_bytes(&self) -> [u8; 32] {
        let mut out = [0u8; 32];
        if let Some(v) = b64url_decode(&self.secret) {
            if v.len() == 32 {
                out.copy_from_slice(&v);
            }
        }
        out
    }

    fn new() -> Pairing {
        let mut s = [0u8; 32];
        getrandom::fill(&mut s).expect("random numbers");
        Pairing { id: crate::config::random_hex(8), secret: b64url(&s), lan: crate::config::random_hex(16) }
    }

    fn valid(&self) -> bool {
        self.id.len() == 16 && b64url_decode(&self.secret).map(|v| v.len() == 32).unwrap_or(false) && self.lan.len() == 32
    }

    /// Kept in `phone.json` beside the config file.
    pub fn load_or_create(config_dir: &Path) -> Pairing {
        let f = config_dir.join("phone.json");
        if let Some(p) = std::fs::read_to_string(&f).ok().and_then(|s| serde_json::from_str::<Pairing>(&s).ok()) {
            if p.valid() {
                return p;
            }
        }
        let p = Pairing::new();
        p.save(config_dir);
        p
    }

    /// A new secret: phones paired before can't connect anymore.
    pub fn renew(config_dir: &Path) -> Pairing {
        let old = Pairing::load_or_create(config_dir);
        let mut p = Pairing::new();
        p.id = old.id;
        p.save(config_dir);
        p
    }

    fn save(&self, config_dir: &Path) {
        let _ = std::fs::create_dir_all(config_dir);
        let f = config_dir.join("phone.json");
        let _ = std::fs::write(&f, serde_json::to_string_pretty(self).unwrap_or_default());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o600));
        }
    }
}

pub fn derive(secret: &[u8], salt: &[u8], info: &str) -> [u8; 32] {
    let hk = Hkdf::<Sha256>::new(Some(salt), secret);
    let mut out = [0u8; 32];
    hk.expand(info.as_bytes(), &mut out).expect("32 bytes is a valid length");
    out
}

/// Where this computer and its phones meet (a public name that reveals
/// nothing).
pub fn topic(secret: &[u8]) -> String {
    hex(&derive(secret, b"openhop-phone", "topic")[..16])
}

/// Seals set-up messages.
pub fn signal_key(secret: &[u8]) -> [u8; 32] {
    derive(secret, b"openhop-phone", "signal")
}

/// Seal with a random nonce: nonce (12 bytes) followed by the sealed data.
pub fn seal(key: &[u8; 32], aad: &[u8], msg: &[u8]) -> Vec<u8> {
    let mut nonce = [0u8; 12];
    getrandom::fill(&mut nonce).expect("random numbers");
    let c = Aes256Gcm::new_from_slice(key).expect("32-byte key");
    let mut out = nonce.to_vec();
    out.extend(c.encrypt(&nonce.into(), Payload { msg, aad }).expect("sealing"));
    out
}

pub fn open(key: &[u8; 32], aad: &[u8], data: &[u8]) -> Option<Vec<u8>> {
    if data.len() < 12 + 16 {
        return None;
    }
    let c = Aes256Gcm::new_from_slice(key).ok()?;
    let nonce: [u8; 12] = data[..12].try_into().ok()?;
    c.decrypt(&nonce.into(), Payload { msg: &data[12..], aad }).ok()
}

/// The second layer inside a connection. Each direction numbers its
/// messages; a message that's replayed, dropped or reordered fails.
pub struct Session {
    key: [u8; 32],
    sent: u64,
    received: u64,
}

/// Directions (first byte of every nonce).
const TO_PC: u8 = 1;
const TO_PHONE: u8 = 2;

impl Session {
    /// `phone_nonce`, `pc_nonce`: the random values each side sent while
    /// setting the connection up.
    pub fn new(secret: &[u8], phone_nonce: &[u8], pc_nonce: &[u8]) -> Session {
        let mut salt = phone_nonce.to_vec();
        salt.extend_from_slice(pc_nonce);
        Session { key: derive(secret, &salt, "data"), sent: 0, received: 0 }
    }

    fn nonce(dir: u8, n: u64) -> [u8; 12] {
        let mut v = [0u8; 12];
        v[0] = dir;
        v[4..].copy_from_slice(&n.to_be_bytes());
        v
    }

    /// PC → phone.
    pub fn seal(&mut self, msg: &[u8]) -> Vec<u8> {
        self.sent += 1;
        let c = Aes256Gcm::new_from_slice(&self.key).expect("32-byte key");
        c.encrypt(&Self::nonce(TO_PHONE, self.sent).into(), msg).expect("sealing")
    }

    /// Phone → PC.
    pub fn open(&mut self, data: &[u8]) -> Option<Vec<u8>> {
        let c = Aes256Gcm::new_from_slice(&self.key).ok()?;
        let n = self.received + 1;
        let out = c.decrypt(&Self::nonce(TO_PC, n).into(), data).ok()?;
        self.received = n;
        Some(out)
    }
}

pub fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

pub fn unhex(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 {
        return None;
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok()).collect()
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

pub fn b64url(data: &[u8]) -> String {
    let mut s = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..=c.len() {
            s.push(B64[((n >> (18 - 6 * i)) & 63) as usize] as char);
        }
    }
    s
}

pub fn b64url_decode(s: &str) -> Option<Vec<u8>> {
    let s = s.trim_end_matches('=');
    let val = |c: u8| -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'-' | b'+' => 62,
            b'_' | b'/' => 63,
            _ => return None,
        } as u32)
    };
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    for chunk in s.as_bytes().chunks(4) {
        let mut n = 0u32;
        for (i, &c) in chunk.iter().enumerate() {
            n |= val(c)? << (18 - 6 * i);
        }
        match chunk.len() {
            4 => out.extend_from_slice(&[(n >> 16) as u8, (n >> 8) as u8, n as u8]),
            3 => out.extend_from_slice(&[(n >> 16) as u8, (n >> 8) as u8]),
            2 => out.push((n >> 16) as u8),
            _ => return None,
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64url_round_trip() {
        for n in 0..40 {
            let v: Vec<u8> = (0..n).map(|i| (i * 37 + 11) as u8).collect();
            assert_eq!(b64url_decode(&b64url(&v)).unwrap(), v);
        }
        assert_eq!(b64url(b"\xfb\xff"), "-_8");
    }

    #[test]
    fn sealing() {
        let k = signal_key(b"0123456789abcdef0123456789abcdef");
        let s = seal(&k, b"t", b"hello");
        assert_eq!(open(&k, b"t", &s).unwrap(), b"hello");
        assert!(open(&k, b"x", &s).is_none());
        let mut bad = s.clone();
        bad[20] ^= 1;
        assert!(open(&k, b"t", &bad).is_none());
        assert_eq!(topic(b"a").len(), 32);
    }

    #[test]
    fn session_numbers_messages() {
        let mut pc = Session::new(b"secret", b"p", b"c");
        // The phone's side, by hand.
        let c = Aes256Gcm::new_from_slice(&pc.key).unwrap();
        let m1 = c.encrypt(&Session::nonce(TO_PC, 1).into(), &b"one"[..]).unwrap();
        let m2 = c.encrypt(&Session::nonce(TO_PC, 2).into(), &b"two"[..]).unwrap();
        assert!(pc.open(&m2).is_none(), "out of order");
        assert_eq!(pc.open(&m1).unwrap(), b"one");
        assert!(pc.open(&m1).is_none(), "replayed");
        assert_eq!(pc.open(&m2).unwrap(), b"two");
        let out = pc.seal(b"back");
        assert_eq!(c.decrypt(&Session::nonce(TO_PHONE, 1).into(), &out[..]).unwrap(), b"back");
    }
}
