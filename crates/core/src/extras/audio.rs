#![cfg_attr(not(any(target_os = "linux", windows, target_os = "macos")), allow(dead_code))]
//! Audio follows you: the sound of the computer you're using plays on the
//! one with your headphones or speakers ("Play other computers' sound here").
//!
//! Plain 48 kHz stereo 16-bit audio in 20 ms pieces (about 1.5 Mbit/s, only
//! while there's sound), on the same encrypted connection as the rest.
//!
//! Capturing: Linux (PulseAudio or PipeWire's monitor), Windows (WASAPI
//! loopback). macOS can't record what it plays without an extra driver, so
//! a Mac can only be where the sound plays.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

pub const RATE: u32 = 48_000;
pub const CHANNELS: usize = 2;
/// Frames in a piece (20 ms).
pub const PIECE: usize = 960;
pub const PIECE_BYTES: usize = PIECE * CHANNELS * 2;

/// Silence (or close enough) isn't sent.
pub fn silent(pcm: &[u8]) -> bool {
    pcm.chunks_exact(2).all(|s| i16::from_le_bytes([s[0], s[1]]).unsigned_abs() < 12)
}

/// Recording what this computer plays. Stops when dropped.
pub struct Capture {
    stop: Arc<AtomicBool>,
}

impl Drop for Capture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

/// Start recording what this computer plays; `piece` gets 20 ms at a time
/// (s16le stereo 48 kHz).
pub fn capture(piece: impl FnMut(Vec<u8>) + Send + 'static) -> Result<Capture, String> {
    let stop = Arc::new(AtomicBool::new(false));
    imp::capture(stop.clone(), Box::new(piece))?;
    Ok(Capture { stop })
}

/// Where other computers' sound plays. Mixes several at once.
pub struct Player {
    buffers: Arc<parking_lot::Mutex<HashMap<String, VecDeque<i16>>>>,
    stop: Arc<AtomicBool>,
}

impl Drop for Player {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

/// About 60 ms of sound waits before playing (smooths the network out);
/// more than half a second behind and the oldest is dropped.
const START_FRAMES: usize = PIECE * 3;
const MAX_FRAMES: usize = RATE as usize / 2;

impl Player {
    pub fn start() -> Result<Player, String> {
        let buffers: Arc<parking_lot::Mutex<HashMap<String, VecDeque<i16>>>> = Default::default();
        let stop = Arc::new(AtomicBool::new(false));
        imp::play(stop.clone(), buffers.clone())?;
        Ok(Player { buffers, stop })
    }

    /// A piece from `from`.
    pub fn feed(&self, from: &str, pcm: &[u8]) {
        let mut b = self.buffers.lock();
        let q = b.entry(from.to_string()).or_default();
        q.extend(pcm.chunks_exact(2).map(|s| i16::from_le_bytes([s[0], s[1]])));
        while q.len() > MAX_FRAMES * CHANNELS {
            q.drain(..PIECE * CHANNELS);
        }
    }
}

/// Mix `n` frames from every source (s16 stereo) into `out` (silence where
/// a source has nothing yet).
pub fn mix(buffers: &mut HashMap<String, VecDeque<i16>>, out: &mut [i16]) {
    out.iter_mut().for_each(|s| *s = 0);
    let mut acc = vec![0i32; out.len()];
    for q in buffers.values_mut() {
        // Wait for a little to gather after a gap, then play steadily.
        if q.len() < START_FRAMES * CHANNELS {
            continue;
        }
        let n = out.len().min(q.len());
        for (a, s) in acc.iter_mut().zip(q.drain(..n)) {
            *a += s as i32;
        }
    }
    for (o, a) in out.iter_mut().zip(acc) {
        *o = a.clamp(i16::MIN as i32, i16::MAX as i32) as i16;
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use super::*;
    use std::io::{Read, Write};
    use std::process::{Command, Stdio};

    /// PulseAudio's tools first (PipeWire systems usually have them too),
    /// then PipeWire's own.
    fn spawn(record: bool) -> Result<std::process::Child, String> {
        let tries: [(&str, Vec<&str>); 2] = if record {
            [
                ("parec", vec!["--format=s16le", "--rate=48000", "--channels=2", "--latency-msec=20", "-d", "@DEFAULT_MONITOR@"]),
                ("pw-record", vec!["-P", "{ stream.capture.sink=true }", "--format", "s16", "--rate", "48000", "--channels", "2", "-"]),
            ]
        } else {
            [
                ("pacat", vec!["--playback", "--format=s16le", "--rate=48000", "--channels=2", "--latency-msec=40", "--client-name=OpenHop"]),
                ("pw-play", vec!["--format", "s16", "--rate", "48000", "--channels", "2", "-"]),
            ]
        };
        let mut last = String::new();
        for (cmd, args) in tries {
            let mut c = Command::new(cmd);
            c.args(&args)
                .stdin(if record { Stdio::null() } else { Stdio::piped() })
                .stdout(if record { Stdio::piped() } else { Stdio::null() })
                .stderr(Stdio::null());
            match c.spawn() {
                Ok(child) => return Ok(child),
                Err(e) => last = format!("{cmd}: {e}"),
            }
        }
        Err(format!("no sound tools ({last}); install pulseaudio-utils"))
    }

    pub fn capture(stop: Arc<AtomicBool>, mut piece: Box<dyn FnMut(Vec<u8>) + Send>) -> Result<(), String> {
        let mut child = spawn(true)?;
        let mut out = child.stdout.take().ok_or("no output")?;
        std::thread::Builder::new()
            .name("audio-capture".into())
            .spawn(move || {
                let mut buf = vec![0u8; PIECE_BYTES];
                while !stop.load(Ordering::SeqCst) {
                    if out.read_exact(&mut buf).is_err() {
                        break;
                    }
                    piece(buf.clone());
                }
                let _ = child.kill();
                let _ = child.wait();
            })
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn play(stop: Arc<AtomicBool>, buffers: Arc<parking_lot::Mutex<HashMap<String, VecDeque<i16>>>>) -> Result<(), String> {
        let mut child = spawn(false)?;
        let mut input = child.stdin.take().ok_or("no input")?;
        std::thread::Builder::new()
            .name("audio-play".into())
            .spawn(move || {
                let mut frame = vec![0i16; PIECE * CHANNELS];
                let mut next = std::time::Instant::now();
                while !stop.load(Ordering::SeqCst) {
                    mix(&mut buffers.lock(), &mut frame);
                    let bytes: Vec<u8> = frame.iter().flat_map(|s| s.to_le_bytes()).collect();
                    if input.write_all(&bytes).is_err() {
                        break;
                    }
                    next += std::time::Duration::from_millis(20);
                    let now = std::time::Instant::now();
                    if next > now {
                        std::thread::sleep(next - now);
                    } else {
                        next = now;
                    }
                }
                let _ = child.kill();
                let _ = child.wait();
            })
            .map_err(|e| e.to_string())?;
        Ok(())
    }
}

#[cfg(any(windows, target_os = "macos"))]
mod imp {
    use super::*;
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

    /// Linear resampling and channel mapping to 48 kHz stereo.
    struct Convert {
        rate: u32,
        channels: usize,
        pos: f64,
        last: [f32; 2],
    }

    impl Convert {
        fn push(&mut self, data: &[f32], out: &mut Vec<i16>) {
            let step = self.rate as f64 / RATE as f64;
            let frames = data.len() / self.channels.max(1);
            let get = |i: usize, c: usize| -> f32 {
                let ch = if self.channels == 1 { 0 } else { c.min(self.channels - 1) };
                data[i * self.channels + ch]
            };
            while self.pos < frames as f64 {
                let i = self.pos.floor() as usize;
                let t = (self.pos - i as f64) as f32;
                for c in 0..2 {
                    let a = if i == 0 { self.last[c] } else { get(i - 1, c) };
                    let b = get(i, c);
                    out.push(((a + (b - a) * t).clamp(-1.0, 1.0) * 32767.0) as i16);
                }
                self.pos += step;
            }
            self.pos -= frames as f64;
            if frames > 0 {
                self.last = [get(frames - 1, 0), get(frames - 1, 1)];
            }
        }
    }

    pub fn capture(stop: Arc<AtomicBool>, mut piece: Box<dyn FnMut(Vec<u8>) + Send>) -> Result<(), String> {
        if cfg!(target_os = "macos") {
            return Err("macOS can't record what it plays without an extra driver".into());
        }
        let (tx, rx) = crossbeam_channel::bounded::<Result<(), String>>(1);
        std::thread::Builder::new()
            .name("audio-capture".into())
            .spawn(move || {
                let host = cpal::default_host();
                // An output device used for input: WASAPI records what it plays.
                let Some(dev) = host.default_output_device() else {
                    let _ = tx.send(Err("no sound output".into()));
                    return;
                };
                let Ok(cfg) = dev.default_output_config() else {
                    let _ = tx.send(Err("no sound format".into()));
                    return;
                };
                let mut conv = Convert { rate: cfg.sample_rate(), channels: cfg.channels() as usize, pos: 0.0, last: [0.0; 2] };
                let mut pending: Vec<i16> = vec![];
                let (ptx, prx) = crossbeam_channel::unbounded::<Vec<u8>>();
                let stream = dev.build_input_stream::<f32, _, _>(
                    cfg.config(),
                    move |data: &[f32], _| {
                        conv.push(data, &mut pending);
                        while pending.len() >= PIECE * CHANNELS {
                            let bytes: Vec<u8> = pending.drain(..PIECE * CHANNELS).flat_map(|s| s.to_le_bytes()).collect();
                            let _ = ptx.send(bytes);
                        }
                    },
                    |e| log::debug!("audio capture: {e}"),
                    None,
                );
                let stream = match stream {
                    Ok(s) => s,
                    Err(e) => {
                        let _ = tx.send(Err(e.to_string()));
                        return;
                    }
                };
                if let Err(e) = stream.play() {
                    let _ = tx.send(Err(e.to_string()));
                    return;
                }
                let _ = tx.send(Ok(()));
                while !stop.load(Ordering::SeqCst) {
                    if let Ok(p) = prx.recv_timeout(std::time::Duration::from_millis(200)) {
                        piece(p);
                    }
                }
                drop(stream);
            })
            .map_err(|e| e.to_string())?;
        rx.recv().map_err(|e| e.to_string())?
    }

    pub fn play(stop: Arc<AtomicBool>, buffers: Arc<parking_lot::Mutex<HashMap<String, VecDeque<i16>>>>) -> Result<(), String> {
        let (tx, rx) = crossbeam_channel::bounded::<Result<(), String>>(1);
        std::thread::Builder::new()
            .name("audio-play".into())
            .spawn(move || {
                let host = cpal::default_host();
                let Some(dev) = host.default_output_device() else {
                    let _ = tx.send(Err("no sound output".into()));
                    return;
                };
                let Ok(cfg) = dev.default_output_config() else {
                    let _ = tx.send(Err("no sound format".into()));
                    return;
                };
                let (rate, channels) = (cfg.sample_rate(), cfg.channels() as usize);
                let mut src: Vec<i16> = vec![];
                let mut pos = 0.0f64;
                let stream = dev.build_output_stream::<f32, _, _>(
                    cfg.config(),
                    move |out: &mut [f32], _| {
                        let frames = out.len() / channels.max(1);
                        // How many 48 kHz frames this many device frames take.
                        let need = ((frames as f64) * RATE as f64 / rate as f64).ceil() as usize + 2;
                        if src.len() / 2 < need {
                            let mut more = vec![0i16; (need - src.len() / 2) * 2];
                            mix(&mut buffers.lock(), &mut more);
                            src.extend(more);
                        }
                        let step = RATE as f64 / rate as f64;
                        for f in 0..frames {
                            let i = (pos.floor() as usize).min(src.len() / 2 - 1);
                            for c in 0..channels {
                                out[f * channels + c] = src[i * 2 + c.min(1)] as f32 / 32768.0;
                            }
                            pos += step;
                        }
                        let used = (pos.floor() as usize).min(src.len() / 2);
                        src.drain(..used * 2);
                        pos -= used as f64;
                    },
                    |e| log::debug!("audio play: {e}"),
                    None,
                );
                let stream = match stream {
                    Ok(s) => s,
                    Err(e) => {
                        let _ = tx.send(Err(e.to_string()));
                        return;
                    }
                };
                if let Err(e) = stream.play() {
                    let _ = tx.send(Err(e.to_string()));
                    return;
                }
                let _ = tx.send(Ok(()));
                while !stop.load(Ordering::SeqCst) {
                    std::thread::sleep(std::time::Duration::from_millis(200));
                }
                drop(stream);
            })
            .map_err(|e| e.to_string())?;
        rx.recv().map_err(|e| e.to_string())?
    }
}

#[cfg(not(any(target_os = "linux", windows, target_os = "macos")))]
mod imp {
    use super::*;
    pub fn capture(_: Arc<AtomicBool>, _: Box<dyn FnMut(Vec<u8>) + Send>) -> Result<(), String> {
        Err("not on this system".into())
    }
    pub fn play(_: Arc<AtomicBool>, _: Arc<parking_lot::Mutex<HashMap<String, VecDeque<i16>>>>) -> Result<(), String> {
        Err("not on this system".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mixing_waits_then_adds() {
        let mut b: HashMap<String, VecDeque<i16>> = HashMap::new();
        b.insert("a".into(), std::iter::repeat_n(1000i16, PIECE * CHANNELS).collect());
        let mut out = vec![0i16; PIECE * CHANNELS];
        // Not enough gathered yet: silence.
        mix(&mut b, &mut out);
        assert!(out.iter().all(|s| *s == 0));
        b.get_mut("a").unwrap().extend(std::iter::repeat_n(1000i16, PIECE * CHANNELS * 3));
        b.insert("b".into(), std::iter::repeat_n(32000i16, PIECE * CHANNELS * 4).collect());
        mix(&mut b, &mut out);
        assert!(out.iter().all(|s| *s == i16::MAX), "two sources add up (and clip)");
        assert!(silent(&[3, 0, 255, 255]));
        assert!(!silent(&[0, 10]));
    }
}
