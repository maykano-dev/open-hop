//! What's playing on this computer (music, a video in a browser or a
//! player), and play/pause/next/previous/seek for it, so the island on any
//! computer can show and control it.
//!
//! Linux: MPRIS over D-Bus (Spotify, browsers, VLC, mpv, Rhythmbox…).
//! Windows: the system media controls (Spotify, browsers, Media Player…).
//! macOS: Spotify and Music through AppleScript (macOS keeps other apps'
//! media to itself).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NowPlaying {
    /// The app playing it ("Spotify", "Firefox").
    pub app: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    /// Cover art: a data: URL or an https: URL.
    pub art: Option<String>,
    pub playing: bool,
    /// Seconds into the track when this was read (`at`, ms since 1970).
    pub position: Option<f64>,
    pub duration: Option<f64>,
    pub at: u64,
    pub shuffle: Option<bool>,
}

impl NowPlaying {
    /// Equal apart from the clock moving on.
    pub fn same_track_state(&self, o: &NowPlaying) -> bool {
        self.app == o.app && self.title == o.title && self.artist == o.artist && self.playing == o.playing && self.art == o.art && self.shuffle == o.shuffle
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum MediaCmd {
    PlayPause,
    Next,
    Previous,
    /// Go to this many seconds into the track.
    Seek(f64),
    Shuffle,
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// Shrink cover art to something small enough to send around.
#[cfg_attr(target_os = "macos", allow(dead_code))]
fn art_from_bytes(bytes: &[u8]) -> Option<String> {
    let img = image::load_from_memory(bytes).ok()?;
    let small = img.thumbnail(160, 160).to_rgb8();
    let mut out = Vec::new();
    jpeg_encoder::Encoder::new(&mut out, 82).encode(small.as_raw(), small.width() as u16, small.height() as u16, jpeg_encoder::ColorType::Rgb).ok()?;
    Some(format!("data:image/jpeg;base64,{}", super::base64(&out)))
}

/// Reads what's playing; keeps whatever it needs between reads.
pub struct Media {
    imp: imp::Imp,
}

impl Media {
    pub fn new() -> Self {
        Media { imp: imp::Imp::new() }
    }

    pub fn now(&mut self) -> Option<NowPlaying> {
        self.imp.now()
    }

    pub fn command(&mut self, cmd: MediaCmd) {
        log::info!("media: {cmd:?}");
        if let Err(e) = self.imp.command(cmd) {
            log::info!("media control failed: {e:#}");
        }
    }
}

impl Default for Media {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use super::*;
    use anyhow::{Context, Result};
    use std::collections::HashMap;
    use zbus::blocking::Connection;
    use zbus::names::InterfaceName;
    use zbus::zvariant::{OwnedValue, Value};

    const PATH: &str = "/org/mpris/MediaPlayer2";
    const PLAYER: &str = "org.mpris.MediaPlayer2.Player";

    pub struct Imp {
        conn: Option<Connection>,
        /// The player shown last (kept while it's paused).
        current: Option<String>,
        art_cache: Option<(String, Option<String>)>,
    }

    impl Imp {
        pub fn new() -> Self {
            Imp { conn: None, current: None, art_cache: None }
        }

        fn conn(&mut self) -> Option<Connection> {
            if self.conn.is_none() {
                self.conn = Connection::session().ok();
            }
            self.conn.clone()
        }

        fn get(conn: &Connection, dest: &str, iface: &'static str, prop: &str) -> Result<OwnedValue> {
            let p = zbus::blocking::fdo::PropertiesProxy::builder(conn).destination(dest.to_string())?.path(PATH)?.build()?;
            Ok(p.get(InterfaceName::from_static_str_unchecked(iface), prop)?)
        }

        fn players(conn: &Connection) -> Vec<String> {
            let Ok(dbus) = zbus::blocking::fdo::DBusProxy::new(conn) else { return vec![] };
            dbus.list_names().map(|v| v.into_iter().map(|n| n.to_string()).filter(|n| n.starts_with("org.mpris.MediaPlayer2.")).collect()).unwrap_or_default()
        }

        fn status(conn: &Connection, p: &str) -> String {
            Self::get(conn, p, PLAYER, "PlaybackStatus").ok().and_then(|v| String::try_from(v).ok()).unwrap_or_default()
        }

        fn pick(&mut self, conn: &Connection) -> Option<String> {
            let players = Self::players(conn);
            if let Some(p) = players.iter().find(|p| Self::status(conn, p) == "Playing") {
                self.current = Some(p.clone());
            } else if !self.current.as_ref().map(|c| players.contains(c)).unwrap_or(false) {
                self.current = players.iter().find(|p| Self::status(conn, p) == "Paused").cloned();
            }
            self.current.clone()
        }

        fn art(&mut self, url: &str) -> Option<String> {
            if let Some((u, a)) = &self.art_cache {
                if u == url {
                    return a.clone();
                }
            }
            let a = if url.starts_with("https://") {
                Some(url.to_string())
            } else if let Some(path) = url.strip_prefix("file://") {
                let path = percent_decode(path);
                std::fs::read(path).ok().and_then(|b| art_from_bytes(&b))
            } else {
                None
            };
            self.art_cache = Some((url.to_string(), a.clone()));
            a
        }

        pub fn now(&mut self) -> Option<NowPlaying> {
            let conn = self.conn()?;
            let p = self.pick(&conn)?;
            let status = Self::status(&conn, &p);
            let meta: HashMap<String, OwnedValue> = Self::get(&conn, &p, PLAYER, "Metadata").ok().and_then(|v| HashMap::try_from(v).ok()).unwrap_or_default();
            let text = |k: &str| meta.get(k).and_then(|v| v.try_clone().ok()).and_then(|v| String::try_from(v).ok()).unwrap_or_default();
            let list =
                |k: &str| meta.get(k).and_then(|v| v.try_clone().ok()).and_then(|v| Vec::<String>::try_from(v).ok()).map(|v| v.join(", ")).unwrap_or_default();
            let micros = |v: &OwnedValue| -> Option<f64> {
                v.try_clone()
                    .ok()
                    .and_then(|x| i64::try_from(x).ok())
                    .map(|x| x as f64)
                    .or_else(|| v.try_clone().ok().and_then(|x| u64::try_from(x).ok()).map(|x| x as f64))
                    .map(|x| x / 1e6)
            };
            let title = text("xesam:title");
            if title.is_empty() {
                return None;
            }
            let duration = meta.get("mpris:length").and_then(micros).filter(|d| *d > 0.0);
            let position = Self::get(&conn, &p, PLAYER, "Position").ok().and_then(|v| micros(&v));
            let app = Self::get(&conn, &p, "org.mpris.MediaPlayer2", "Identity")
                .ok()
                .and_then(|v| String::try_from(v).ok())
                .unwrap_or_else(|| p.trim_start_matches("org.mpris.MediaPlayer2.").to_string());
            let shuffle = Self::get(&conn, &p, PLAYER, "Shuffle").ok().and_then(|v| bool::try_from(v).ok());
            let art_url = text("mpris:artUrl");
            let art = if art_url.is_empty() { None } else { self.art(&art_url) };
            Some(NowPlaying {
                app,
                title,
                artist: list("xesam:artist"),
                album: text("xesam:album"),
                art,
                playing: status == "Playing",
                position,
                duration,
                at: now_ms(),
                shuffle,
            })
        }

        pub fn command(&mut self, cmd: MediaCmd) -> Result<()> {
            let conn = self.conn().context("no session bus")?;
            let p = self.pick(&conn).context("nothing is playing")?;
            let call = |m: &str, body: &dyn erased::Body| -> Result<()> { body.call(&conn, &p, m) };
            match cmd {
                MediaCmd::PlayPause => call("PlayPause", &()),
                MediaCmd::Next => call("Next", &()),
                MediaCmd::Previous => call("Previous", &()),
                MediaCmd::Seek(to) => {
                    let now = Self::get(&conn, &p, PLAYER, "Position").ok().and_then(|v| i64::try_from(v).ok()).unwrap_or(0);
                    let offset = (to * 1e6) as i64 - now;
                    call("Seek", &offset)
                }
                MediaCmd::Shuffle => {
                    let on = Self::get(&conn, &p, PLAYER, "Shuffle").ok().and_then(|v| bool::try_from(v).ok()).unwrap_or(false);
                    let props = zbus::blocking::fdo::PropertiesProxy::builder(&conn).destination(p.clone())?.path(PATH)?.build()?;
                    props.set(InterfaceName::from_static_str_unchecked(PLAYER), "Shuffle", Value::from(!on))?;
                    Ok(())
                }
            }
        }
    }

    /// Method bodies of different types behind one call.
    mod erased {
        use anyhow::Result;
        use zbus::blocking::Connection;
        pub trait Body {
            fn call(&self, conn: &Connection, dest: &str, method: &str) -> Result<()>;
        }
        impl Body for () {
            fn call(&self, conn: &Connection, dest: &str, method: &str) -> Result<()> {
                conn.call_method(Some(dest), super::PATH, Some(super::PLAYER), method, &())?;
                Ok(())
            }
        }
        impl Body for i64 {
            fn call(&self, conn: &Connection, dest: &str, method: &str) -> Result<()> {
                conn.call_method(Some(dest), super::PATH, Some(super::PLAYER), method, &(*self,))?;
                Ok(())
            }
        }
    }

    fn percent_decode(s: &str) -> String {
        let b = s.as_bytes();
        let mut out = Vec::with_capacity(b.len());
        let mut i = 0;
        while i < b.len() {
            if b[i] == b'%' && i + 2 < b.len() {
                if let Some(v) = std::str::from_utf8(&b[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok()) {
                    out.push(v);
                    i += 3;
                    continue;
                }
            }
            out.push(b[i]);
            i += 1;
        }
        String::from_utf8_lossy(&out).into_owned()
    }
}

#[cfg(windows)]
mod imp {
    use super::*;
    use anyhow::{Context, Result};
    use windows::Media::Control::{
        GlobalSystemMediaTransportControlsSession as Session, GlobalSystemMediaTransportControlsSessionManager as Manager,
        GlobalSystemMediaTransportControlsSessionPlaybackStatus as Status,
    };
    use windows::Storage::Streams::DataReader;

    pub struct Imp {
        manager: Option<Manager>,
        art_cache: Option<(String, Option<String>)>,
    }

    impl Imp {
        pub fn new() -> Self {
            // The system media controls are a Windows Runtime API.
            unsafe {
                let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_MULTITHREADED);
            }
            Imp { manager: None, art_cache: None }
        }

        fn session(&mut self) -> Option<Session> {
            if self.manager.is_none() {
                self.manager = Manager::RequestAsync().ok().and_then(|op| op.join().ok());
            }
            self.manager.as_ref()?.GetCurrentSession().ok()
        }

        fn art(&mut self, key: String, s: &windows::Media::Control::GlobalSystemMediaTransportControlsSessionMediaProperties) -> Option<String> {
            if let Some((k, a)) = &self.art_cache {
                if *k == key {
                    return a.clone();
                }
            }
            let read = || -> Option<Vec<u8>> {
                let stream = s.Thumbnail().ok()?.OpenReadAsync().ok()?.join().ok()?;
                let size = stream.Size().ok()? as u32;
                if size == 0 || size > 8 << 20 {
                    return None;
                }
                let reader = DataReader::CreateDataReader(&stream).ok()?;
                reader.LoadAsync(size).ok()?.join().ok()?;
                let mut buf = vec![0u8; size as usize];
                reader.ReadBytes(&mut buf).ok()?;
                Some(buf)
            };
            let a = read().and_then(|b| art_from_bytes(&b));
            self.art_cache = Some((key, a.clone()));
            a
        }

        pub fn now(&mut self) -> Option<NowPlaying> {
            let s = self.session()?;
            let props = s.TryGetMediaPropertiesAsync().ok()?.join().ok()?;
            let title = props.Title().ok()?.to_string();
            if title.is_empty() {
                return None;
            }
            let artist = props.Artist().map(|a| a.to_string()).unwrap_or_default();
            let album = props.AlbumTitle().map(|a| a.to_string()).unwrap_or_default();
            let info = s.GetPlaybackInfo().ok()?;
            let playing = info.PlaybackStatus().ok() == Some(Status::Playing);
            let shuffle = info.IsShuffleActive().ok().and_then(|r| r.Value().ok());
            let (mut position, mut duration) = (None, None);
            if let Ok(tl) = s.GetTimelineProperties() {
                let end = tl.EndTime().map(|d| d.Duration as f64 / 1e7).unwrap_or(0.0);
                if end > 0.0 {
                    duration = Some(end);
                    let mut pos = tl.Position().map(|d| d.Duration as f64 / 1e7).unwrap_or(0.0);
                    // The position is as of the last update: add the time since.
                    if playing {
                        if let Ok(updated) = tl.LastUpdatedTime() {
                            let now_ft = (now_ms() as i64 + 11_644_473_600_000) * 10_000;
                            let since = (now_ft - updated.UniversalTime) as f64 / 1e7;
                            if (0.0..86_400.0).contains(&since) {
                                pos += since;
                            }
                        }
                    }
                    position = Some(pos.min(end));
                }
            }
            let app_id = s.SourceAppUserModelId().map(|a| a.to_string()).unwrap_or_default();
            let app = friendly_app(&app_id);
            let art = self.art(format!("{app_id}|{title}|{artist}|{album}"), &props);
            Some(NowPlaying { app, title, artist, album, art, playing, position, duration, at: now_ms(), shuffle })
        }

        pub fn command(&mut self, cmd: MediaCmd) -> Result<()> {
            let s = self.session().context("nothing is playing")?;
            let ok = match cmd {
                MediaCmd::PlayPause => s.TryTogglePlayPauseAsync()?.join()?,
                MediaCmd::Next => s.TrySkipNextAsync()?.join()?,
                MediaCmd::Previous => s.TrySkipPreviousAsync()?.join()?,
                MediaCmd::Seek(to) => s.TryChangePlaybackPositionAsync((to * 1e7) as i64)?.join()?,
                MediaCmd::Shuffle => {
                    let on = s.GetPlaybackInfo()?.IsShuffleActive().ok().and_then(|r| r.Value().ok()).unwrap_or(false);
                    s.TryChangeShuffleActiveAsync(!on)?.join()?
                }
            };
            if !ok {
                anyhow::bail!("the app didn't accept it");
            }
            Ok(())
        }
    }

    fn friendly_app(id: &str) -> String {
        let low = id.to_lowercase();
        for (k, v) in [
            ("spotify", "Spotify"),
            ("chrome", "Chrome"),
            ("msedge", "Edge"),
            ("firefox", "Firefox"),
            ("vlc", "VLC"),
            ("zunemusic", "Media Player"),
            ("zunevideo", "Movies & TV"),
            ("brave", "Brave"),
            ("opera", "Opera"),
        ] {
            if low.contains(k) {
                return v.into();
            }
        }
        id.rsplit(['\\', '!']).next().unwrap_or(id).trim_end_matches(".exe").to_string()
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use super::*;
    use anyhow::{Context, Result};

    pub struct Imp {
        current: Option<&'static str>,
        art_cache: Option<(String, Option<String>)>,
    }

    fn running(app: &str) -> bool {
        std::process::Command::new("pgrep").args(["-x", app]).output().map(|o| o.status.success()).unwrap_or(false)
    }

    fn osascript(script: &str) -> Option<String> {
        let out = std::process::Command::new("osascript").args(["-e", script]).output().ok()?;
        out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim_end().to_string())
    }

    fn num(s: &str) -> Option<f64> {
        s.trim().replace(',', ".").parse().ok()
    }

    impl Imp {
        pub fn new() -> Self {
            Imp { current: None, art_cache: None }
        }

        fn read(app: &'static str) -> Option<NowPlaying> {
            let script = match app {
                "Spotify" => {
                    r#"tell application "Spotify" to return (player state as string) & linefeed & (name of current track) & linefeed & (artist of current track) & linefeed & (album of current track) & linefeed & (player position as string) & linefeed & ((duration of current track) / 1000 as string) & linefeed & (artwork url of current track) & linefeed & (shuffling as string)"#
                }
                _ => {
                    r#"tell application "Music" to return (player state as string) & linefeed & (name of current track) & linefeed & (artist of current track) & linefeed & (album of current track) & linefeed & (player position as string) & linefeed & (duration of current track as string) & linefeed & "" & linefeed & (shuffle enabled as string)"#
                }
            };
            let out = osascript(script)?;
            let f: Vec<&str> = out.split('\n').collect();
            if f.len() < 8 || f[1].is_empty() {
                return None;
            }
            Some(NowPlaying {
                app: app.into(),
                title: f[1].into(),
                artist: f[2].into(),
                album: f[3].into(),
                art: Some(f[6].to_string()).filter(|u| u.starts_with("https://")),
                playing: f[0] == "playing",
                position: num(f[4]),
                duration: num(f[5]),
                at: now_ms(),
                shuffle: Some(f[7] == "true"),
            })
        }

        pub fn now(&mut self) -> Option<NowPlaying> {
            let mut found = None;
            for app in ["Spotify", "Music"] {
                if !running(app) {
                    continue;
                }
                if let Some(n) = Self::read(app) {
                    let playing = n.playing;
                    if playing || found.is_none() || self.current == Some(app) {
                        found = Some((app, n));
                    }
                    if playing {
                        break;
                    }
                }
            }
            let (app, n) = found?;
            self.current = Some(app);
            let _ = &self.art_cache;
            Some(n)
        }

        pub fn command(&mut self, cmd: MediaCmd) -> Result<()> {
            let app = self.current.context("nothing is playing")?;
            let what = match (cmd, app) {
                (MediaCmd::PlayPause, _) => "playpause".to_string(),
                (MediaCmd::Next, _) => "next track".into(),
                (MediaCmd::Previous, _) => "previous track".into(),
                (MediaCmd::Seek(to), _) => format!("set player position to {to:.1}"),
                (MediaCmd::Shuffle, "Spotify") => "set shuffling to not shuffling".into(),
                (MediaCmd::Shuffle, _) => "set shuffle enabled to not shuffle enabled".into(),
            };
            osascript(&format!("tell application \"{app}\" to {what}")).context("the app didn't accept it")?;
            Ok(())
        }
    }
}

#[cfg(not(any(target_os = "linux", windows, target_os = "macos")))]
mod imp {
    use super::*;
    pub struct Imp;
    impl Imp {
        pub fn new() -> Self {
            Imp
        }
        pub fn now(&mut self) -> Option<NowPlaying> {
            None
        }
        pub fn command(&mut self, _: MediaCmd) -> anyhow::Result<()> {
            Ok(())
        }
    }
}
