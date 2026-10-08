//! OpenHop: share one keyboard and mouse between Windows, macOS and Linux
//! computers on your local network.

#[cfg(target_os = "linux")]
pub mod clip_x11;
pub mod clipboard;
pub mod config;
pub mod discovery;
pub mod engine;
pub mod extras;
pub mod files;
pub mod keys;
pub mod layout;
pub mod net;
pub mod platform;
pub mod portfree;
pub mod protocol;
pub mod wins;
pub mod wol;

pub use config::{Config, Role};
pub use engine::{Engine, Status};
