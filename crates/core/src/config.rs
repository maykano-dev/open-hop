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
        if !path.exists() {
            return Ok(Config::default());
        }
        let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))
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
    }
}

pub fn to_toml(cfg: &Config) -> String {
    toml::to_string_pretty(cfg).unwrap_or_default()
}
