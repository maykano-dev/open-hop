use crate::layout::Layout;
use crate::protocol::{Rect, DEFAULT_PORT};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// This computer has the physical keyboard and mouse and shares them.
    Server,
    /// This computer is controlled by a server.
    Client,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Name shown to other computers.
    pub name: String,
    pub role: Role,
    /// Shared secret. Every computer must use the same one.
    pub passphrase: String,
    /// TCP port the server listens on.
    pub port: u16,
    /// Client only: connect straight to this "host:port" instead of auto-discovery.
    pub server_addr: Option<String>,
    /// Client only: only auto-connect to a server with this name.
    pub server_name: Option<String>,
    /// Server only: where each screen sits.
    pub layout: Layout,
    /// Swap Ctrl and Cmd when moving between macOS and Windows/Linux.
    pub swap_cmd_ctrl: bool,
    pub clipboard_sync: bool,
    /// Override the detected desktop size (useful on some Wayland setups).
    pub screen: Option<Rect>,
    /// Linux client: "auto", "x11" or "uinput".
    pub linux_backend: String,
    /// Server: MAC address of each client, for Wake-on-LAN.
    pub macs: std::collections::BTreeMap<String, String>,
    /// Server: each client's last IP address (helps Wake-on-LAN reach it).
    pub last_ips: std::collections::BTreeMap<String, String>,
    /// Wake sleeping computers when you move the pointer toward them.
    pub wake_on_lan: bool,
    /// Where received files are saved (default: Downloads/OpenHop).
    pub download_dir: Option<String>,
    /// Show small notifications (received files, links copied on another computer).
    pub notifications: bool,
    /// Cap file and image transfers at this many megabits per second (0 = no limit).
    pub transfer_limit_mbps: u32,
    /// Stable random id for this installation (used for pairing).
    pub device_id: String,
    /// Computers paired with a code: device id -> name and shared key.
    pub trusted: std::collections::BTreeMap<String, Trusted>,
    /// Client: the paired server to connect to.
    pub server_device: Option<String>,
    /// Turn dark mode on/off everywhere when it's switched on one computer.
    pub theme_sync: bool,
    /// Silence notifications here while another computer is in Do Not
    /// Disturb or presenting.
    pub dnd_sync: bool,
    /// Live window picture quality (always "high").
    pub stream_quality: String,
    /// Dragging a window by its title bar onto another screen opens it there live.
    pub window_drag: bool,
    /// Accent colour name.
    pub ui_accent: String,
    /// Start OpenHop (in the background) when you log in.
    pub open_at_login: bool,
    /// OpenHop is switched on. Connect once: it stays on (across restarts,
    /// with the window closed) until it's switched off in the app.
    pub enabled: bool,
    /// Other computers' keyboards and mice may come onto this screen.
    #[serde(default = "yes")]
    pub allow_control: bool,
    /// This computer's keyboard and mouse may go onto other screens.
    #[serde(default = "yes")]
    pub share_input: bool,
    /// Focus (Do Not Disturb) is shared with the other computers.
    #[serde(default = "yes")]
    pub allow_focus: bool,
    /// Lock All locks this computer too, and locking it locks the others.
    #[serde(default = "yes")]
    pub allow_lock: bool,
    /// Sleep All puts this computer to sleep too.
    #[serde(default = "yes")]
    pub allow_sleep: bool,
    /// Meeting points for phones (Nostr relays, wss://…). Empty: OpenHop's
    /// default public ones. A school or company can run its own.
    #[serde(default)]
    pub phone_relays: Vec<String>,
    /// Where the phone app lives (empty: OpenHop's own, on GitHub Pages).
    #[serde(default)]
    pub phone_app_url: Option<String>,
}

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Trusted {
    pub name: String,
    /// 32-byte key, hex.
    pub key: String,
}

pub fn random_hex(bytes: usize) -> String {
    let mut b = vec![0u8; bytes];
    getrandom::fill(&mut b).expect("OS random number generator");
    b.iter().map(|x| format!("{x:02x}")).collect()
}

pub fn hex_key(s: &str) -> Option<[u8; 32]> {
    if s.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, o) in out.iter_mut().enumerate() {
        *o = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(out)
}

impl Default for Config {
    fn default() -> Self {
        Config {
            name: default_name(),
            role: Role::Server,
            passphrase: String::new(),
            port: DEFAULT_PORT,
            server_addr: None,
            server_name: None,
            layout: Layout::default(),
            swap_cmd_ctrl: true,
            clipboard_sync: true,
            screen: None,
            linux_backend: "auto".into(),
            macs: Default::default(),
            last_ips: Default::default(),
            wake_on_lan: true,
            download_dir: None,
            notifications: true,
            transfer_limit_mbps: 0,
            device_id: String::new(),
            trusted: Default::default(),
            server_device: None,
            theme_sync: true,
            dnd_sync: true,
            stream_quality: "high".into(),
            window_drag: true,
            ui_accent: "blue".into(),
            open_at_login: true,
            enabled: true,
            allow_control: true,
            share_input: true,
            allow_focus: true,
            allow_lock: true,
            allow_sleep: true,
            phone_relays: vec![],
            phone_app_url: None,
        }
    }
}

pub fn default_name() -> String {
    for var in ["COMPUTERNAME", "HOSTNAME"] {
        if let Ok(v) = std::env::var(var) {
            if !v.trim().is_empty() {
                return v.trim().to_string();
            }
        }
    }
    if let Ok(h) = std::fs::read_to_string("/etc/hostname") {
        if !h.trim().is_empty() {
            return h.trim().to_string();
        }
    }
    if let Ok(out) = std::process::Command::new("hostname").output() {
        let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !s.is_empty() {
            return s.trim_end_matches(".local").to_string();
        }
    }
    "computer".into()
}

impl Config {
    pub fn default_path() -> PathBuf {
        dirs::config_dir().unwrap_or_else(|| PathBuf::from(".")).join("openhop").join("config.toml")
    }

    pub fn load(path: &PathBuf) -> Result<Config> {
        let mut cfg: Config = if path.exists() {
            let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
            toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?
        } else {
            Config::default()
        };
        let before = toml::to_string(&cfg).unwrap_or_default();
        cfg.normalize();
        if cfg.device_id.is_empty() {
            cfg.device_id = random_hex(8);
        }
        if path.exists() && toml::to_string(&cfg).unwrap_or_default() != before || !path.exists() {
            let _ = cfg.save(path);
        }
        Ok(cfg)
    }

    /// Features that are simply always on (they used to be options).
    /// Only the transfer speed limit and the accent colour are choices.
    pub fn normalize(&mut self) {
        self.swap_cmd_ctrl = true;
        self.clipboard_sync = true;
        self.wake_on_lan = true;
        self.notifications = true;
        self.theme_sync = true;
        self.dnd_sync = true;
        self.stream_quality = "high".into();
        self.window_drag = true;
        self.open_at_login = true;
    }

    pub fn save(&self, path: &PathBuf) -> Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, toml::to_string_pretty(self)?)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn toml_roundtrip() {
        let mut c = Config::default();
        c.passphrase = "x".into();
        c.layout.place("mac", 1, 0);
        let s = toml::to_string_pretty(&c).unwrap();
        let back: Config = toml::from_str(&s).unwrap();
        assert_eq!(back.layout.position("mac"), Some((1, 0)));
        // Missing fields fall back to defaults.
        let partial: Config = toml::from_str("role = \"client\"\npassphrase = \"abc\"").unwrap();
        assert_eq!(partial.role, Role::Client);
        assert!(partial.swap_cmd_ctrl);
        assert!(partial.enabled);
        // Old files with options that no longer exist still load.
        let old: Config = toml::from_str("sound = \"pop\"\nbattery_saver = true\nui_appearance = \"dark\"").unwrap();
        assert!(old.enabled);
        let k = random_hex(32);
        assert_eq!(hex_key(&k).map(|b| b.len()), Some(32));
        assert!(hex_key("zz").is_none());
    }
}

pub fn to_toml(cfg: &Config) -> String {
    toml::to_string_pretty(cfg).unwrap_or_default()
}
