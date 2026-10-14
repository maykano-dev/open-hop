//! AirDrop with iPhones, iPads and Macs from Linux, through omdrop
//! (github.com/brentkearney/omdrop-plugin, MIT) and its radio backends:
//! omdrop-awdl (Broadcom Wi‑Fi, Apple Silicon Macs running Linux) and
//! omdrop-owl (github.com/t4t5/omdrop-owl: MediaTek, Realtek, Intel and
//! Qualcomm cards, through OWL's AWDL in userspace).
//!
//! AirDrop needs Apple's own Wi‑Fi link (AWDL), which only some Wi‑Fi cards
//! can do from Linux, and a driver set up as administrator: that is omdrop's
//! job. OpenHop finds it, and uses its command line (which prints JSON) to
//! send, list nearby devices and be visible for a while. Received files land
//! where omdrop puts them (Downloads unless changed).

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Where to learn more and install it.
pub const INFO_URL: &str = "https://github.com/t4t5/omdrop-owl";

/// A nearby Apple device.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Peer {
    pub mac: String,
    pub rssi: Option<i32>,
    pub name: Option<String>,
}

/// What omdrop says about itself.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Status {
    #[serde(default)]
    pub receiving: bool,
    /// None: the radio half isn't installed.
    #[serde(default)]
    pub visible: Option<bool>,
    /// Seconds left to be visible.
    #[serde(default)]
    pub remaining: Option<u64>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub dir: Option<String>,
    #[serde(default)]
    pub reason: Option<String>,
}

/// omdrop's command, if it's installed (Linux only).
pub fn program() -> Option<PathBuf> {
    if !cfg!(target_os = "linux") {
        return None;
    }
    if let Some(p) = std::env::var_os("OPENHOP_OMDROP") {
        return Some(p.into());
    }
    let path = std::env::var_os("PATH")?;
    let mut dirs: Vec<PathBuf> = std::env::split_paths(&path).collect();
    // Installed for this user only (not always on an app's PATH).
    if let Some(h) = dirs::home_dir() {
        dirs.push(h.join(".local/bin"));
        dirs.push(h.join(".local/share/omarchy/bin"));
    }
    dirs.push("/usr/lib/omdrop".into());
    dirs.into_iter().map(|d| d.join("omdrop")).find(|p| is_exe(p))
}

fn is_exe(p: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        p.metadata().map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0).unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        p.is_file()
    }
}

pub fn available() -> bool {
    program().is_some()
}

/// Run omdrop with `args`, at most `limit`; its standard output.
fn run(args: &[&str], limit: Duration) -> Result<String, String> {
    let exe = program().ok_or("AirDrop on Linux needs omdrop (and omdrop-owl or omdrop-awdl for the Wi‑Fi card)")?;
    let mut child = Command::new(exe).args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(|e| e.to_string())?;
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if start.elapsed() > limit => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("AirDrop took too long".into());
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(100)),
            Err(e) => return Err(e.to_string()),
        }
    }
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        let err = String::from_utf8_lossy(&out.stderr);
        let line = err.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("AirDrop didn't work").trim();
        Err(line.trim_start_matches("omdrop:").trim().chars().take(160).collect())
    }
}

pub fn status() -> Option<Status> {
    run(&["status", "--json"], Duration::from_secs(8)).ok().and_then(|s| parse_status(&s))
}

pub fn parse_status(s: &str) -> Option<Status> {
    serde_json::from_str(s.trim()).ok()
}

/// What is still missing before this computer can AirDrop (empty: ready).
pub fn missing() -> Vec<String> {
    #[derive(Deserialize)]
    struct Row {
        say: String,
    }
    #[derive(Deserialize)]
    struct Doc {
        missing: Vec<Row>,
    }
    match run(&["doctor", "--json"], Duration::from_secs(15)) {
        Ok(s) => serde_json::from_str::<Doc>(s.trim()).map(|d| d.missing.into_iter().map(|r| r.say).collect()).unwrap_or_default(),
        Err(e) => vec![e],
    }
}

/// Apple devices nearby, with their names where they answer.
pub fn peers() -> Result<Vec<Peer>, String> {
    run(&["peers", "-n", "--json"], Duration::from_secs(40)).map(|s| parse_peers(&s))
}

pub fn parse_peers(s: &str) -> Vec<Peer> {
    serde_json::from_str(s.trim()).unwrap_or_default()
}

/// Send files to the device called `to` (one transfer). Blocks until the
/// other side accepted (or didn't).
pub fn send(files: &[PathBuf], to: &str) -> Result<(), String> {
    let mut args: Vec<String> = vec!["send".into(), "--wait".into(), "60".into(), to.into()];
    args.extend(files.iter().map(|f| f.to_string_lossy().into_owned()));
    let a: Vec<&str> = args.iter().map(String::as_str).collect();
    run(&a, Duration::from_secs(60 * 30)).map(|_| ())
}

/// Visible to iPhones and Macs nearby for `minutes`, so they can send here.
pub fn receive(minutes: u32) -> Result<(), String> {
    run(&["on", &minutes.to_string()], Duration::from_secs(30)).map(|_| ())
}

pub fn stop_receiving() -> Result<(), String> {
    run(&["off"], Duration::from_secs(20)).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_omdrop_output() {
        let p = parse_peers(r#"[{"mac": "aa:bb:cc:dd:ee:ff", "rssi": -52, "name": "Ada's iPhone"}, {"mac": "11:22:33:44:55:66", "rssi": null, "name": null}]"#);
        assert_eq!(p.len(), 2);
        assert_eq!(p[0].name.as_deref(), Some("Ada's iPhone"));
        assert_eq!(p[1].rssi, None);
        let s = parse_status(r#"{"receiving": true, "visible": true, "remaining": 540, "name": "desk", "dir": "/home/a/Downloads", "mode": "timed", "reason": null, "rx_proven": true, "sound": false, "visibility": "everyone", "senders": 0}"#).unwrap();
        assert!(s.receiving);
        assert_eq!(s.remaining, Some(540));
        assert!(parse_peers("not json").is_empty());
    }
}
