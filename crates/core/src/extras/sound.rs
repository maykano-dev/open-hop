//! Little sounds for things arriving from another computer. They're made
//! here (no sound files to ship) and played with the OS's own player.

use std::f32::consts::PI;
use std::path::PathBuf;

const RATE: u32 = 44_100;

/// Samples in -1..1 for a named sound.
pub fn synth(kind: &str) -> Option<Vec<f32>> {
    let n = |secs: f32| (secs * RATE as f32) as usize;
    let mut rng: u32 = 0x1234_5678;
    let mut noise = move || {
        rng ^= rng << 13;
        rng ^= rng >> 17;
        rng ^= rng << 5;
        (rng as f32 / u32::MAX as f32) * 2.0 - 1.0
    };
    match kind {
        // Filtered noise whose pitch sweeps up then down, like air rushing past.
        "swoosh" => {
            let len = n(0.55);
            let (mut lp, mut lp2) = (0.0f32, 0.0f32);
            Some(
                (0..len)
                    .map(|i| {
                        let t = i as f32 / len as f32;
                        let sweep = (PI * t).sin();
                        let cutoff = 0.02 + 0.25 * sweep;
                        lp += cutoff * (noise() - lp);
                        lp2 += cutoff * (lp - lp2);
                        let band = lp - lp2 * 0.6;
                        let env = (t * 6.0).min(1.0) * (1.0 - t).powf(1.6);
                        band * env * 2.2
                    })
                    .collect(),
            )
        }
        // A soft bubble pop: a quick downward pitch bend.
        "pop" => {
            let len = n(0.16);
            let mut phase = 0.0f32;
            Some(
                (0..len)
                    .map(|i| {
                        let t = i as f32 / RATE as f32;
                        let f = 180.0 + 700.0 * (-t * 38.0).exp();
                        phase += 2.0 * PI * f / RATE as f32;
                        let env = (t * 900.0).min(1.0) * (-t * 28.0).exp();
                        phase.sin() * env * 0.9
                    })
                    .collect(),
            )
        }
        // Two bell-like notes.
        "chime" => {
            let len = n(0.9);
            Some(
                (0..len)
                    .map(|i| {
                        let t = i as f32 / RATE as f32;
                        let bell = |f: f32, start: f32| {
                            if t < start {
                                return 0.0;
                            }
                            let u = t - start;
                            let env = (u * 400.0).min(1.0) * (-u * 4.5).exp();
                            ((2.0 * PI * f * u).sin() + 0.35 * (2.0 * PI * f * 2.76 * u).sin() * (-u * 9.0).exp()) * env
                        };
                        (bell(1046.5, 0.0) + bell(1568.0, 0.09)) * 0.32
                    })
                    .collect(),
            )
        }
        _ => None,
    }
}

/// 16-bit mono WAV.
pub fn wav(samples: &[f32], volume: f32) -> Vec<u8> {
    let data_len = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&RATE.to_le_bytes());
    out.extend_from_slice(&(RATE * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        let v = (s * volume).clamp(-1.0, 1.0);
        out.extend_from_slice(&((v * 32767.0) as i16).to_le_bytes());
    }
    out
}

fn file_for(kind: &str, volume: u32) -> Option<PathBuf> {
    let dir = dirs::cache_dir().unwrap_or_else(std::env::temp_dir).join("openhop");
    let path = dir.join(format!("{kind}-{volume}.wav"));
    if !path.exists() {
        let samples = synth(kind)?;
        std::fs::create_dir_all(&dir).ok()?;
        std::fs::write(&path, wav(&samples, volume as f32 / 100.0)).ok()?;
    }
    Some(path)
}

/// Play a sound without waiting for it to finish.
pub fn play(kind: &str, volume: u32) {
    if kind == "off" || volume == 0 {
        return;
    }
    let Some(path) = file_for(kind, volume.min(100)) else { return };
    let _ = std::thread::Builder::new().name("sound".into()).spawn(move || {
        #[cfg(target_os = "linux")]
        for player in [&["pw-play"][..], &["paplay"], &["aplay", "-q"]] {
            let ok = std::process::Command::new(player[0])
                .args(&player[1..])
                .arg(&path)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
            if ok {
                break;
            }
        }
        #[cfg(target_os = "macos")]
        let _ = std::process::Command::new("afplay").arg(&path).status();
        #[cfg(windows)]
        unsafe {
            use windows::core::HSTRING;
            use windows::Win32::Media::Audio::{PlaySoundW, SND_FILENAME, SND_NODEFAULT};
            let _ = PlaySoundW(&HSTRING::from(path.as_os_str()), None, SND_FILENAME | SND_NODEFAULT);
        }
    });
}

#[cfg(test)]
mod tests {
    #[test]
    fn sounds() {
        for k in ["swoosh", "pop", "chime"] {
            let s = super::synth(k).unwrap();
            assert!(s.len() > 1000);
            let peak = s.iter().fold(0f32, |m, v| m.max(v.abs()));
            assert!(peak > 0.1 && peak <= 1.5, "{k} peak {peak}");
            let w = super::wav(&s, 0.6);
            assert_eq!(&w[..4], b"RIFF");
            assert_eq!(w.len(), 44 + s.len() * 2);
        }
        assert!(super::synth("nope").is_none());
    }
}
