//! Headless OpenHop: `openhop server` on the computer with the keyboard and
//! mouse, `openhop client` on the others.

use anyhow::Result;
use clap::{Parser, Subcommand};
use openhop_core::discovery::Discovery;
use openhop_core::{Config, Engine, Role};
use std::path::PathBuf;
use std::time::Duration;

#[derive(Parser)]
#[command(name = "openhop", version, about = "Share one keyboard and mouse across Windows, macOS and Linux")]
struct Cli {
    /// Config file (default: your OS config folder / openhop / config.toml).
    #[arg(long, global = true)]
    config: Option<PathBuf>,
    /// Shared passphrase (optional: computers can pair with a code instead).
    #[arg(long, short, global = true)]
    passphrase: Option<String>,
    /// Name this computer shows to the others.
    #[arg(long, short, global = true)]
    name: Option<String>,
    /// Save the given options to the config file.
    #[arg(long, global = true)]
    save: bool,
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Share this computer's keyboard and mouse.
    Server {
        #[arg(long)]
        port: Option<u16>,
    },
    /// Let another computer control this one.
    Client {
        /// Connect to this host[:port] instead of auto-discovering.
        #[arg(long)]
        server: Option<String>,
        /// Pair with the server using the 6-digit code it shows.
        #[arg(long, value_name = "CODE")]
        pair: Option<String>,
        /// With --pair: which server (by name) if there are several.
        #[arg(long)]
        server_name: Option<String>,
    },
    /// Put a screen on the grid. This computer (the server) is at 0,0;
    /// 1,0 is to its right, -1,0 to its left, 0,-1 above, 0,1 below.
    Place { screen: String, x: i32, y: i32 },
    /// List OpenHop computers on the network.
    Devices,
    /// List paired computers, or forget one: `openhop paired --forget NAME`.
    Paired {
        #[arg(long)]
        forget: Option<String>,
    },
    /// Print the config file location and contents.
    Config,
}

fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let cli = Cli::parse();
    let path = cli.config.clone().unwrap_or_else(Config::default_path);
    let mut cfg = Config::load(&path)?;
    if let Some(p) = &cli.passphrase {
        cfg.passphrase = p.clone();
    }
    if let Some(n) = &cli.name {
        cfg.name = n.clone();
    }

    let mut pair_request: Option<(String, Option<String>)> = None;
    match cli.cmd {
        Some(Cmd::Server { port }) => {
            cfg.role = Role::Server;
            if let Some(p) = port {
                cfg.port = p;
            }
        }
        Some(Cmd::Client { server, pair, server_name }) => {
            cfg.role = Role::Client;
            if server.is_some() {
                cfg.server_addr = server;
            }
            pair_request = pair.map(|code| (code, server_name));
        }
        Some(Cmd::Place { screen, x, y }) => {
            cfg.layout.place(&screen, x, y);
            cfg.save(&path)?;
            println!("Saved. Layout:");
            for s in &cfg.layout.screens {
                println!("  {:>3},{:<3} {}", s.x, s.y, s.name);
            }
            return Ok(());
        }
        Some(Cmd::Devices) => {
            let d = Discovery::start(format!("{}-probe", cfg.device_id), cfg.name.clone(), Role::Client, 0)?;
            println!("Listening for 5 seconds…");
            std::thread::sleep(Duration::from_secs(5));
            for p in d.peers() {
                let paired = if cfg.trusted.contains_key(&p.device) { "paired" } else { "" };
                println!("  {:<22} {:<8} {:<7} {:<8} {:<7} {}", p.name, p.os.label(), format!("{:?}", p.role), p.link, paired, p.addr);
            }
            return Ok(());
        }
        Some(Cmd::Paired { forget }) => {
            if let Some(name) = forget {
                let before = cfg.trusted.len();
                cfg.trusted.retain(|_, t| t.name != name);
                if cfg.trusted.len() == before {
                    anyhow::bail!("no paired computer named {name}");
                }
                if cfg.server_device.as_ref().map(|d| !cfg.trusted.contains_key(d)).unwrap_or(false) {
                    cfg.server_device = None;
                }
                cfg.save(&path)?;
                println!("Forgot {name}.");
            }
            for (dev, t) in &cfg.trusted {
                println!("  {:<24} {}", t.name, dev);
            }
            return Ok(());
        }
        Some(Cmd::Config) => {
            println!("# {}", path.display());
            println!("{}", toml_string(&cfg));
            return Ok(());
        }
        None => {}
    }
    if cli.save {
        cfg.save(&path)?;
        log::info!("saved settings to {}", path.display());
    }
    if let Some(problem) = openhop_core::platform::check_permissions(true) {
        anyhow::bail!(problem);
    }

    let engine = Engine::start(cfg.clone(), Some(path))?;
    let (stop_tx, stop_rx) = std::sync::mpsc::channel();
    ctrlc::set_handler(move || {
        let _ = stop_tx.send(());
    })?;
    log::info!(
        "{} running as {}. Press Ctrl+C to quit.",
        cfg.name,
        if cfg.role == Role::Server { "server (sharing this keyboard and mouse)" } else { "client" }
    );
    let mut last = String::new();
    let mut last_code = String::new();
    loop {
        if stop_rx.recv_timeout(Duration::from_millis(500)).is_ok() {
            break;
        }
        let s = engine.status();
        if let Some(code) = &s.pairing_code {
            if *code != last_code {
                log::info!("Pairing code: {} {}  (enter it on the other computer)", &code[..3], &code[3..]);
                last_code = code.clone();
            }
        }
        if let Some((code, name)) = &pair_request {
            let server = s.discovered.iter().find(|d| {
                d.role == Role::Server && name.as_ref().map(|n| n == &d.name).unwrap_or(true)
            });
            if let Some(d) = server {
                log::info!("pairing with {}…", d.name);
                engine.pair(d.device.clone(), code.clone());
                pair_request = None;
            }
        }
        if let Some(e) = &s.pair_error {
            if !last.contains(e.as_str()) {
                log::warn!("{e}");
                last = e.clone();
            }
        }
        let line = match &s.error {
            Some(e) => format!("{} [{}]", s.message, e),
            None => s.message.clone(),
        };
        if line != last {
            log::info!("{line}");
            last = line;
        }
        if !s.running {
            break;
        }
    }
    engine.stop();
    Ok(())
}

fn toml_string(cfg: &Config) -> String {
    let mut c = cfg.clone();
    if !c.passphrase.is_empty() {
        c.passphrase = "********".into();
    }
    openhop_core::config::to_toml(&c)
}
