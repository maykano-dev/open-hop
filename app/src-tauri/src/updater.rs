//! "Check for Updates": looks at the latest GitHub release, downloads the
//! installer that matches how OpenHop was installed here, and runs it.

use anyhow::{anyhow, bail, Context, Result};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

const LATEST: &str = "https://api.github.com/repos/maykano-dev/open-hop/releases/latest";

#[derive(Debug, Clone, Serialize, Default)]
pub struct UpdateState {
    /// "idle", "checking", "available", "current", "downloading", "installing", "error"
    pub phase: String,
    pub current: String,
    pub latest: Option<String>,
    pub notes_url: Option<String>,
    pub progress: Option<f64>,
    pub message: Option<String>,
}

pub static STATE: Mutex<Option<UpdateState>> = Mutex::new(None);

pub fn state() -> UpdateState {
    STATE.lock().clone().unwrap_or_else(|| UpdateState { phase: "idle".into(), current: current(), ..Default::default() })
}

fn set(f: impl FnOnce(&mut UpdateState)) {
    let mut g = STATE.lock();
    let s = g.get_or_insert_with(|| UpdateState { phase: "idle".into(), current: current(), ..Default::default() });
    f(s);
}

pub fn current() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    html_url: String,
    assets: Vec<Asset>,
}

#[derive(Deserialize, Clone)]
struct Asset {
    name: String,
    browser_download_url: String,
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(30)))
        .user_agent(format!("OpenHop/{}", current()))
        .build()
        .into()
}

/// How this copy of OpenHop was installed, which decides the installer to fetch.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Kind {
    WindowsSetup,
    MacDmg,
    LinuxDeb,
    LinuxRpm,
    AppImage,
}

fn install_kind() -> Kind {
    if cfg!(windows) {
        return Kind::WindowsSetup;
    }
    if cfg!(target_os = "macos") {
        return Kind::MacDmg;
    }
    if std::env::var_os("APPIMAGE").is_some() {
        return Kind::AppImage;
    }
    let exe = std::env::current_exe().unwrap_or_default();
    let owned_by = |tool: &str, args: &[&str]| {
        std::process::Command::new(tool).args(args).arg(&exe).output().map(|o| o.status.success()).unwrap_or(false)
    };
    if owned_by("dpkg", &["-S"]) {
        Kind::LinuxDeb
    } else if owned_by("rpm", &["-qf"]) {
        Kind::LinuxRpm
    } else {
        Kind::AppImage
    }
}

fn pick_asset(assets: &[Asset], kind: Kind) -> Option<Asset> {
    let suffix = match kind {
        Kind::WindowsSetup => "_x64-setup.exe",
        Kind::MacDmg => ".dmg",
        Kind::LinuxDeb => "_amd64.deb",
        Kind::LinuxRpm => ".x86_64.rpm",
        Kind::AppImage => "_amd64.AppImage",
    };
    assets.iter().find(|a| a.name.ends_with(suffix)).cloned()
}

fn newer(latest: &str, current: &str) -> bool {
    let p = |s: &str| semver::Version::parse(s.trim_start_matches('v')).ok();
    match (p(latest), p(current)) {
        (Some(l), Some(c)) => l > c,
        _ => false,
    }
}

fn fetch_release() -> Result<Release> {
    let mut resp = agent()
        .get(LATEST)
        .header("Accept", "application/vnd.github+json")
        .call()
        .context("couldn't reach GitHub")?;
    Ok(resp.body_mut().read_json::<Release>()?)
}

/// Ask GitHub for the latest release. Updates [`STATE`].
pub fn check() -> UpdateState {
    set(|s| {
        s.phase = "checking".into();
        s.message = None;
    });
    match fetch_release() {
        Ok(r) => {
            let latest = r.tag_name.trim_start_matches('v').to_string();
            let avail = newer(&latest, &current());
            set(|s| {
                s.phase = if avail { "available" } else { "current" }.into();
                s.latest = Some(latest);
                s.notes_url = Some(r.html_url);
            });
        }
        Err(e) => set(|s| {
            s.phase = "error".into();
            s.message = Some(format!("Couldn't check for updates: {e:#}"));
        }),
    }
    state()
}

fn download(url: &str, dest: &Path) -> Result<()> {
    let mut resp = agent().get(url).call().context("download failed")?;
    let total = resp.headers().get("content-length").and_then(|v| v.to_str().ok()).and_then(|v| v.parse::<f64>().ok());
    let mut reader = resp.body_mut().with_config().limit(1 << 30).reader();
    let mut out = std::fs::File::create(dest)?;
    let mut buf = vec![0u8; 256 * 1024];
    let mut done = 0f64;
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        out.write_all(&buf[..n])?;
        done += n as f64;
        if let Some(t) = total {
            set(|s| s.progress = Some((done / t).min(1.0)));
        }
    }
    Ok(())
}

/// Download and launch the installer. On success the caller should quit
/// the app; a helper script finishes the install and reopens OpenHop.
pub fn install() -> Result<()> {
    set(|s| {
        s.phase = "downloading".into();
        s.progress = Some(0.0);
        s.message = None;
    });
    let r = (|| -> Result<()> {
        let rel = fetch_release()?;
        let kind = install_kind();
        let asset = pick_asset(&rel.assets, kind).ok_or_else(|| anyhow!("this release has no installer for this system"))?;
        let dir = std::env::temp_dir().join(format!("openhop-update-{}", std::process::id()));
        std::fs::create_dir_all(&dir)?;
        let file = dir.join(&asset.name);
        download(&asset.browser_download_url, &file)?;
        set(|s| s.phase = "installing".into());
        launch_installer(kind, &file)
    })();
    if let Err(e) = &r {
        set(|s| {
            s.phase = "error".into();
            s.message = Some(format!("Update failed: {e:#}"));
        });
    }
    r
}

fn shell_quote(p: &Path) -> String {
    format!("'{}'", p.to_string_lossy().replace('\'', r"'\''"))
}

#[allow(unused_variables)]
fn launch_installer(kind: Kind, file: &Path) -> Result<()> {
    let exe = std::env::current_exe()?;
    match kind {
        #[cfg(windows)]
        Kind::WindowsSetup => {
            use std::os::windows::process::CommandExt;
            // Wait for OpenHop to quit, install silently, then reopen it.
            let script = file.with_extension("cmd");
            std::fs::write(
                &script,
                format!(
                    "@echo off\r\ntimeout /t 2 /nobreak >nul\r\n\"{}\" /S\r\nstart \"\" \"{}\"\r\n",
                    file.display(),
                    exe.display()
                ),
            )?;
            std::process::Command::new("cmd").arg("/C").arg(&script).creation_flags(0x08000000).spawn()?;
            Ok(())
        }
        Kind::MacDmg => {
            // .../OpenHop.app/Contents/MacOS/openhop-app -> .../OpenHop.app
            let app = exe.ancestors().nth(3).map(PathBuf::from).ok_or_else(|| anyhow!("can't find the app bundle"))?;
            let script = format!(
                "sleep 2; M=$(mktemp -d); hdiutil attach -nobrowse -quiet -mountpoint \"$M\" {dmg} && \
                 rm -rf {app} && cp -R \"$M/OpenHop.app\" {app} ; hdiutil detach -quiet \"$M\"; \
                 xattr -cr {app} 2>/dev/null; open {app}",
                dmg = shell_quote(file),
                app = shell_quote(&app)
            );
            std::process::Command::new("sh").arg("-c").arg(script).spawn()?;
            Ok(())
        }
        Kind::LinuxDeb | Kind::LinuxRpm => {
            let pm = if kind == Kind::LinuxDeb {
                "apt-get install -y"
            } else if Path::new("/usr/bin/zypper").exists() {
                "zypper --non-interactive install --allow-unsigned-rpm"
            } else {
                "dnf install -y"
            };
            // pkexec shows the system's password prompt.
            let script = format!(
                "sleep 1; pkexec sh -c '{pm} \"$0\"' {pkg} && (setsid {exe} >/dev/null 2>&1 &)",
                pkg = shell_quote(file),
                exe = shell_quote(&exe)
            );
            std::process::Command::new("sh").arg("-c").arg(script).spawn()?;
            Ok(())
        }
        Kind::AppImage => {
            let target = std::env::var_os("APPIMAGE").map(PathBuf::from).unwrap_or_else(|| exe.clone());
            let script = format!(
                "sleep 1; cp -f {new} {target}.new && chmod +x {target}.new && mv -f {target}.new {target} && (setsid {target} >/dev/null 2>&1 &)",
                new = shell_quote(file),
                target = shell_quote(&target)
            );
            std::process::Command::new("sh").arg("-c").arg(script).spawn()?;
            Ok(())
        }
        #[allow(unreachable_patterns)]
        _ => bail!("automatic install isn't supported here"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Talks to GitHub: `cargo test -p openhop-app -- --ignored live`.
    #[test]
    #[ignore]
    fn live_check() {
        let st = check();
        println!("{st:?}");
        assert!(st.latest.is_some(), "{:?}", st.message);
    }

    #[test]
    fn versions() {
        assert!(newer("v0.3.0", "0.2.0"));
        assert!(newer("0.10.0", "0.9.9"));
        assert!(!newer("v0.2.0", "0.2.0"));
        assert!(!newer("garbage", "0.2.0"));
    }

    #[test]
    fn assets() {
        let a = |n: &str| Asset { name: n.into(), browser_download_url: format!("https://x/{n}") };
        let list = vec![
            a("OpenHop-0.3.0-1.x86_64.rpm"),
            a("OpenHop_0.3.0_amd64.AppImage"),
            a("OpenHop_0.3.0_amd64.deb"),
            a("OpenHop_0.3.0_universal.dmg"),
            a("OpenHop_0.3.0_x64-setup.exe"),
            a("OpenHop_0.3.0_x64_en-US.msi"),
        ];
        assert_eq!(pick_asset(&list, Kind::WindowsSetup).unwrap().name, "OpenHop_0.3.0_x64-setup.exe");
        assert_eq!(pick_asset(&list, Kind::MacDmg).unwrap().name, "OpenHop_0.3.0_universal.dmg");
        assert_eq!(pick_asset(&list, Kind::LinuxDeb).unwrap().name, "OpenHop_0.3.0_amd64.deb");
        assert_eq!(pick_asset(&list, Kind::LinuxRpm).unwrap().name, "OpenHop-0.3.0-1.x86_64.rpm");
        assert_eq!(pick_asset(&list, Kind::AppImage).unwrap().name, "OpenHop_0.3.0_amd64.AppImage");
    }
}
