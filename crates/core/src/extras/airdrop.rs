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
//!
//! Windows has no way to drive a Wi‑Fi card like that, so there OpenHop runs
//! the same thing in Linux under WSL2, on a USB adapter handed over to it
//! (see [`wsl`]).

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

/// omdrop's command, if it's installed (Linux).
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

/// Can this kind of computer AirDrop at all (with the right set-up)?
pub fn possible() -> bool {
    cfg!(any(target_os = "linux", windows))
}

/// Ready to AirDrop: omdrop on Linux; on Windows, OpenHop's set-up done.
pub fn available() -> bool {
    if cfg!(windows) {
        return wsl::ready().is_some();
    }
    program().is_some()
}

/// What's needed, in a sentence (for when it isn't set up).
pub fn needs() -> &'static str {
    if cfg!(windows) {
        "AirDrop on Windows needs a MediaTek MT7612U USB Wi‑Fi adapter (about $20, any brand) and a one-time set-up from Settings › Sharing › AirDrop."
    } else {
        "AirDrop on Linux needs omdrop, and a Wi‑Fi card that can talk to iPhones (omdrop-owl lists them)."
    }
}

/// The command that runs omdrop with `args`.
fn command(args: &[String]) -> Result<Command, String> {
    if cfg!(windows) {
        return wsl::command(args);
    }
    let exe = program().ok_or_else(|| needs().to_string())?;
    let mut c = Command::new(exe);
    c.args(args);
    Ok(c)
}

/// Run omdrop with `args`, at most `limit`; its standard output.
fn run(args: &[&str], limit: Duration) -> Result<String, String> {
    let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
    let mut child = command(&args)?.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(|e| e.to_string())?;
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
    for f in files {
        // Windows: the same file, as Linux (in WSL) sees it.
        args.push(if cfg!(windows) {
            wsl::linux_path(f).ok_or_else(|| format!("{} can't be sent from here", f.display()))?
        } else {
            f.to_string_lossy().into_owned()
        });
    }
    if cfg!(windows) {
        wsl::stay_up(Duration::from_secs(60 * 30));
    }
    let a: Vec<&str> = args.iter().map(String::as_str).collect();
    run(&a, Duration::from_secs(60 * 30)).map(|_| ())
}

/// Visible to iPhones and Macs nearby for `minutes`, so they can send here.
pub fn receive(minutes: u32) -> Result<(), String> {
    if cfg!(windows) {
        wsl::stay_up(Duration::from_secs(minutes as u64 * 60 + 30));
    }
    run(&["on", &minutes.to_string()], Duration::from_secs(30)).map(|_| ())
}

pub fn stop_receiving() -> Result<(), String> {
    run(&["off"], Duration::from_secs(20)).map(|_| ())
}

/// Windows: AirDrop runs in Linux under WSL2, on a USB Wi‑Fi adapter
/// handed to it (usbipd-win), with a WSL kernel that has the adapter's
/// driver. [`setup`] does all that once, in a window the person watches.
pub mod wsl {
    use super::*;

    /// MediaTek MT7612U (omdrop-owl's USB adapter).
    pub const ADAPTER: &str = "0e8d:7612";
    pub const SETUP_PS1: &str = include_str!("airdrop_setup.ps1");
    pub const SETUP_SH: &str = include_str!("airdrop_setup.sh");

    /// OpenHop's folder for this (the kernel, the done marker).
    pub fn folder() -> Option<PathBuf> {
        dirs::home_dir().map(|h| h.join("OpenHop").join("AirDrop"))
    }

    /// Set up: the adapter's hardware id.
    pub fn ready() -> Option<String> {
        let id = std::fs::read_to_string(folder()?.join("ready")).ok()?;
        let id = id.trim().to_string();
        (!id.is_empty()).then_some(id)
    }

    /// `C:\Users\a\x.png` → `/mnt/c/Users/a/x.png`.
    pub fn linux_path(p: &Path) -> Option<String> {
        let s = p.to_string_lossy().replace('\\', "/");
        let mut c = s.chars();
        let drive = c.next()?.to_ascii_lowercase();
        if !drive.is_ascii_alphabetic() || c.next()? != ':' {
            return None;
        }
        Some(format!("/mnt/{drive}{}", c.as_str()))
    }

    fn hidden(c: &mut Command) -> &mut Command {
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        c
    }

    static KEEP: parking_lot::Mutex<Option<(std::process::Child, Instant)>> = parking_lot::Mutex::new(None);
    static ATTACHED: parking_lot::Mutex<Option<Instant>> = parking_lot::Mutex::new(None);

    /// Keep Linux running for `d` (WSL stops it soon after the last
    /// command ends, and the adapter with it), and the adapter in it.
    pub fn stay_up(d: Duration) {
        let mut k = KEEP.lock();
        let until = Instant::now() + d;
        let alive = k.as_mut().map(|(c, u)| c.try_wait().ok().flatten().is_none() && *u >= until).unwrap_or(false);
        if !alive {
            if let Some((mut old, _)) = k.take() {
                let _ = old.kill();
            }
            let secs = d.as_secs().to_string();
            if let Ok(c) = hidden(Command::new("wsl.exe").args(["-e", "sleep", &secs])).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn()
            {
                *k = Some((c, until));
            }
        }
        drop(k);
        attach();
    }

    /// Hand the adapter to Linux (again: it comes back to Windows whenever
    /// Linux stops).
    fn attach() {
        let Some(id) = ready() else { return };
        let mut a = ATTACHED.lock();
        if a.map(|t| t.elapsed() < Duration::from_secs(20)).unwrap_or(false) {
            return;
        }
        let usbipd = std::env::var_os("ProgramFiles")
            .map(|p| PathBuf::from(p).join("usbipd-win").join("usbipd.exe"))
            .filter(|p| p.is_file())
            .unwrap_or_else(|| "usbipd".into());
        let out = hidden(Command::new(usbipd).args(["attach", "--wsl", "--hardware-id", &id])).stdin(Stdio::null()).output();
        if let Ok(o) = out {
            let said = String::from_utf8_lossy(&o.stderr).to_lowercase();
            if o.status.success() {
                // The driver needs a moment to bring the adapter up.
                std::thread::sleep(Duration::from_secs(3));
            } else if !said.contains("already") {
                log::info!("airdrop: couldn't hand the adapter to Linux: {}", said.trim());
            }
        }
        *a = Some(Instant::now());
    }

    pub fn command(args: &[String]) -> Result<Command, String> {
        if ready().is_none() {
            return Err(needs().into());
        }
        stay_up(Duration::from_secs(120));
        let mut c = Command::new("wsl.exe");
        c.args(["-e", "sh", "-lc", "exec \"$HOME/.local/bin/omdrop\" \"$@\"", "omdrop"]).args(args);
        hidden(&mut c);
        Ok(c)
    }

    /// Start the set-up: a PowerShell window, as administrator, that the
    /// person watches (it asks a few things and takes a while: it builds a
    /// Linux kernel with the adapter's driver).
    pub fn setup() -> Result<(), String> {
        let dir = folder().ok_or("no home folder")?;
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let ps1 = dir.join("setup.ps1");
        let sh = dir.join("setup.sh");
        // Windows PowerShell reads a BOM-less script as the old code page.
        std::fs::write(&ps1, [&[0xEF, 0xBB, 0xBF][..], SETUP_PS1.as_bytes()].concat()).map_err(|e| e.to_string())?;
        std::fs::write(&sh, SETUP_SH.replace("\r\n", "\n")).map_err(|e| e.to_string())?;
        let arg = format!(
            "Start-Process powershell.exe -Verb RunAs -ArgumentList '-NoProfile','-ExecutionPolicy','Bypass','-File','\"{}\"','-Folder','\"{}\"','-Adapter','{ADAPTER}'",
            ps1.display(),
            dir.display()
        );
        hidden(Command::new("powershell.exe").args(["-NoProfile", "-Command", &arg])).spawn().map(|_| ()).map_err(|e| e.to_string())
    }
}

/// Set AirDrop up on this computer (Windows: the guided set-up).
pub fn setup() -> Result<(), String> {
    if cfg!(windows) {
        wsl::setup()
    } else {
        Err(needs().into())
    }
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
        assert_eq!(wsl::linux_path(Path::new(r"C:\Users\Ada\Pictures\a b.png")).as_deref(), Some("/mnt/c/Users/Ada/Pictures/a b.png"));
        assert_eq!(wsl::linux_path(Path::new(r"\\server\share\x")), None);
        assert!(wsl::SETUP_PS1.contains("usbipd") && wsl::SETUP_SH.contains("MT76x2U"));
    }
}
