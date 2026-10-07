//! OpenHop: share one keyboard and mouse between Windows, macOS and Linux
//! computers on your local network.

pub mod clipboard;
pub mod config;
pub mod discovery;
pub mod engine;
pub mod keys;
pub mod layout;
pub mod net;
pub mod platform;
pub mod protocol;

pub use config::{Config, Role};
pub use engine::{Engine, Status};
