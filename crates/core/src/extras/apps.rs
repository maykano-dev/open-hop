//! The apps installed on this computer (for the launcher on every computer)
//! and opening one.

use crate::protocol::AppEntry;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Installed apps, sorted by name.
pub fn list() -> Vec<AppEntry> {
    let mut v = imp::list();
    v.sort_by_key(|a| a.name.to_lowercase());
    v.dedup_by(|a, b| a.name.eq_ignore_ascii_case(&b.name));
    v
}

/// Open the app `id` (from [`list`]). An error says why it didn't open.
pub fn launch(id: &str) -> Result<(), String> {
    log::info!("opening {id}");
    imp::launch(id).map_err(|e| {
        log::warn!("couldn't open {id}: {e}");
        friendly(&e)
    })
}

/// A launcher's error, in words for people.
fn friendly(e: &str) -> String {
    let low = e.to_lowercase();
    if low.contains("unable to load application information") || low.contains("no such file") || low.contains("not found") || low.contains("cannot find") {
        return "Its program is missing: it may have been removed or moved. Try reinstalling it.".into();
    }
    if low.contains("permission denied") || low.contains("access is denied") {
        return "This computer didn't allow it to start (permission denied).".into();
    }
    let e = e.trim_start_matches("gio: ").trim();
    let short: String = e.chars().take(110).collect();
    if short.len() < e.len() {
        format!("{short}…")
    } else {
        short
    }
}

/// Run `cmd` and wait briefly: a launcher that fails says so at once.
#[allow(dead_code)]
fn run_quick(cmd: &mut std::process::Command) -> Result<(), String> {
    let mut child =
        cmd.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::piped()).spawn().map_err(|e| e.to_string())?;
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(st)) if st.success() => return Ok(()),
            Ok(Some(st)) => {
                let mut err = String::new();
                if let Some(mut e) = child.stderr.take() {
                    let _ = std::io::Read::read_to_string(&mut e, &mut err);
                }
                let err = err.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim().to_string();
                return Err(if err.is_empty() { format!("it stopped ({st})") } else { err });
            }
            Ok(None) if started.elapsed() > Duration::from_secs(4) => return Ok(()), // still starting: fine
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(e) => return Err(e.to_string()),
        }
    }
}

fn walk(dir: &Path, depth: usize, out: &mut Vec<PathBuf>, ext: &str) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.extension().map(|x| x.eq_ignore_ascii_case(ext)).unwrap_or(false) {
            out.push(p);
        } else if depth > 0 && p.is_dir() {
            walk(&p, depth - 1, out, ext);
        }
    }
}

/// "C:\\Program Files\\Code\\Code.exe" → "code".
pub fn exe_key(path: &str) -> String {
    let base = path.rsplit(['/', '\\']).next().unwrap_or(path);
    let base = base.strip_suffix(".exe").or_else(|| base.strip_suffix(".EXE")).unwrap_or(base);
    base.to_lowercase()
}

#[cfg(target_os = "linux")]
mod imp {
    use super::*;

    fn dirs() -> Vec<PathBuf> {
        let mut v = vec![];
        if let Some(d) = dirs::data_dir() {
            v.push(d.join("applications"));
        }
        let sys = std::env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".into());
        for d in sys.split(':').filter(|d| !d.is_empty()) {
            // Not the AppImage's own folder.
            if std::env::var("APPDIR").map(|a| !a.is_empty() && d.starts_with(&a)).unwrap_or(false) {
                continue;
            }
            v.push(Path::new(d).join("applications"));
        }
        v.push("/var/lib/flatpak/exports/share/applications".into());
        if let Some(h) = dirs::home_dir() {
            v.push(h.join(".local/share/flatpak/exports/share/applications"));
        }
        v.push("/var/lib/snapd/desktop/applications".into());
        v
    }

    /// The program in an Exec line ("env X=1 /usr/bin/foo --bar %U" → "foo";
    /// "flatpak run org.gnome.Foo" → "foo").
    pub(super) fn exec_program(exec: &str) -> String {
        let words: Vec<&str> = exec.split_whitespace().map(|w| w.trim_matches('"')).collect();
        let mut i = 0;
        if words.first() == Some(&"env") {
            i = 1;
            while i < words.len() && words[i].contains('=') {
                i += 1;
            }
        }
        let Some(first) = words.get(i) else { return String::new() };
        if exe_key(first) == "flatpak" {
            if let Some(appid) = words[i + 1..].iter().find(|w| !w.starts_with('-') && **w != "run") {
                return appid.rsplit('.').next().unwrap_or(appid).to_lowercase();
            }
        }
        exe_key(first)
    }

    pub(super) fn entry(path: &Path) -> Option<AppEntry> {
        let text = std::fs::read_to_string(path).ok()?;
        let mut in_main = false;
        let (mut name, mut hidden, mut app, mut exe, mut icon) = (None, false, false, String::new(), String::new());
        for line in text.lines() {
            let line = line.trim();
            if line.starts_with('[') {
                in_main = line == "[Desktop Entry]";
                continue;
            }
            if !in_main {
                continue;
            }
            if let Some(n) = line.strip_prefix("Name=") {
                name.get_or_insert_with(|| n.to_string());
            } else if let Some(i) = line.strip_prefix("Icon=") {
                icon = i.trim().to_string();
            } else if let Some(e) = line.strip_prefix("Exec=") {
                exe = exec_program(e);
            } else if line == "NoDisplay=true" || line == "Hidden=true" {
                hidden = true;
            } else if line == "Type=Application" {
                app = true;
            }
        }
        let name = name?;
        (app && !hidden).then(|| AppEntry { id: path.to_string_lossy().into_owned(), name, exe, icon })
    }

    pub fn list() -> Vec<AppEntry> {
        let mut files = vec![];
        for d in dirs() {
            walk(&d, 1, &mut files, "desktop");
        }
        files.iter().filter_map(|p| entry(p)).collect()
    }

    /// Without what OpenHop's own package set up (an AppImage's libraries,
    /// the X11 switch), which can stop other apps from starting.
    fn clean(cmd: &mut std::process::Command) {
        for v in [
            "LD_LIBRARY_PATH",
            "LD_PRELOAD",
            "APPDIR",
            "APPIMAGE",
            "ARGV0",
            "OWD",
            "GDK_PIXBUF_MODULE_FILE",
            "GDK_PIXBUF_MODULEDIR",
            "GIO_MODULE_DIR",
            "GIO_EXTRA_MODULES",
            "GTK_PATH",
            "GTK_EXE_PREFIX",
            "GTK_DATA_PREFIX",
            "GTK_IM_MODULE_FILE",
            "GSETTINGS_SCHEMA_DIR",
            "PYTHONHOME",
            "PYTHONPATH",
            "PERLLIB",
            "QT_PLUGIN_PATH",
            "WEBKIT_DISABLE_COMPOSITING_MODE",
        ] {
            cmd.env_remove(v);
        }
        if std::env::var_os("OPENHOP_SET_GDK_BACKEND").is_some() {
            cmd.env_remove("GDK_BACKEND");
            cmd.env_remove("OPENHOP_SET_GDK_BACKEND");
        }
        if let (Ok(dirs), Ok(appdir)) = (std::env::var("XDG_DATA_DIRS"), std::env::var("APPDIR")) {
            if !appdir.is_empty() {
                let kept: Vec<&str> = dirs.split(':').filter(|d| !d.starts_with(&appdir)).collect();
                cmd.env("XDG_DATA_DIRS", kept.join(":"));
            }
        }
    }

    pub fn launch(id: &str) -> Result<(), String> {
        // gio knows how to run a .desktop file properly (field codes, terminal apps).
        let mut gio = std::process::Command::new("gio");
        gio.args(["launch", id]);
        clean(&mut gio);
        let first = match run_quick(&mut gio) {
            Ok(()) => return Ok(()),
            Err(e) => e,
        };
        let name = Path::new(id).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let mut gtk = std::process::Command::new("gtk-launch");
        gtk.arg(&name);
        clean(&mut gtk);
        if run_quick(&mut gtk).is_ok() {
            return Ok(());
        }
        // Last resort: run its Exec line ourselves.
        let text = std::fs::read_to_string(id).map_err(|e| e.to_string())?;
        let exec = text.lines().map(str::trim).find_map(|l| l.strip_prefix("Exec=")).ok_or(first.clone())?;
        let line: String = exec.split_whitespace().filter(|w| !(w.len() == 2 && w.starts_with('%'))).collect::<Vec<_>>().join(" ");
        let mut sh = std::process::Command::new("sh");
        sh.args(["-c", &line]);
        clean(&mut sh);
        run_quick(&mut sh).map_err(|_| first)
    }
}

#[cfg(any(windows, test))]
mod lnk {
    /// The program a Windows shortcut (.lnk) points at, if it's a local file.
    pub fn target(data: &[u8]) -> Option<String> {
        let u16at = |o: usize| data.get(o..o + 2).map(|b| u16::from_le_bytes([b[0], b[1]]) as usize);
        let u32at = |o: usize| data.get(o..o + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize);
        if u32at(0)? != 0x4C {
            return None;
        }
        let flags = u32at(0x14)?;
        let mut pos = 0x4C;
        if flags & 1 != 0 {
            pos += 2 + u16at(pos)?;
        }
        if flags & 2 == 0 {
            return None;
        }
        let info = pos;
        let info_flags = u32at(info + 8)?;
        if info_flags & 1 == 0 {
            return None;
        }
        let base = info + u32at(info + 16)?;
        let end = data[base..].iter().position(|&b| b == 0)? + base;
        let s: String = data[base..end].iter().map(|&b| b as char).collect();
        (!s.is_empty()).then_some(s)
    }
}

#[cfg(windows)]
mod imp {
    use super::*;

    pub fn list() -> Vec<AppEntry> {
        let mut files = vec![];
        for base in [std::env::var_os("ProgramData"), std::env::var_os("APPDATA")].into_iter().flatten() {
            walk(&Path::new(&base).join(r"Microsoft\Windows\Start Menu\Programs"), 3, &mut files, "lnk");
        }
        files
            .into_iter()
            .filter_map(|p| {
                let name = p.file_stem()?.to_string_lossy().into_owned();
                let low = name.to_lowercase();
                // Uninstallers, readmes and help files aren't apps to open.
                if ["uninstall", "readme", "help", "documentation", "website", "release notes"].iter().any(|w| low.contains(w)) {
                    return None;
                }
                let exe = std::fs::read(&p).ok().and_then(|d| lnk::target(&d)).map(|t| exe_key(&t)).unwrap_or(low);
                Some(AppEntry { id: p.to_string_lossy().into_owned(), name, exe, icon: String::new() })
            })
            .collect()
    }

    pub fn launch(id: &str) -> Result<(), String> {
        use std::os::windows::process::CommandExt;
        run_quick(std::process::Command::new("cmd").args(["/C", "start", "", id]).creation_flags(0x08000000))
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use super::*;

    pub fn list() -> Vec<AppEntry> {
        let mut files = vec![];
        let mut roots: Vec<PathBuf> = vec!["/Applications".into(), "/System/Applications".into(), "/System/Applications/Utilities".into()];
        if let Some(h) = dirs::home_dir() {
            roots.push(h.join("Applications"));
        }
        for r in roots {
            walk(&r, 1, &mut files, "app");
        }
        files
            .into_iter()
            .filter_map(|p| {
                let name = p.file_stem()?.to_string_lossy().into_owned();
                Some(AppEntry { exe: name.to_lowercase(), name, id: p.to_string_lossy().into_owned(), icon: String::new() })
            })
            .collect()
    }

    pub fn launch(id: &str) -> Result<(), String> {
        run_quick(std::process::Command::new("open").args(["-a", id]))
    }
}

#[cfg(not(any(target_os = "linux", windows, target_os = "macos")))]
mod imp {
    use super::*;
    pub fn list() -> Vec<AppEntry> {
        vec![]
    }
    pub fn launch(_: &str) -> Result<(), String> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #[cfg(target_os = "linux")]
    #[test]
    fn reads_desktop_files() {
        let dir = std::env::temp_dir().join(format!("openhop-apps-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("a.desktop"),
            "[Desktop Entry]\nType=Application\nName=Notes\nExec=env A=1 /usr/bin/notes %U\n[Desktop Action x]\nName=Other\n",
        )
        .unwrap();
        std::fs::write(dir.join("b.desktop"), "[Desktop Entry]\nType=Application\nName=Hidden\nNoDisplay=true\n").unwrap();
        let mut files = vec![];
        super::walk(&dir, 0, &mut files, "desktop");
        let apps: Vec<_> = files.iter().filter_map(|p| super::imp::entry(p)).collect();
        assert_eq!(apps.len(), 1);
        assert_eq!((apps[0].name.as_str(), apps[0].exe.as_str()), ("Notes", "notes"));
        assert_eq!(super::imp::exec_program("flatpak run --branch=stable org.gnome.Snapshot"), "snapshot");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn failed_launch_says_why() {
        let r = super::run_quick(std::process::Command::new("sh").args(["-c", "echo 'no such app' >&2; exit 3"]));
        assert_eq!(r, Err("no such app".to_string()));
        assert!(super::friendly("gio: Unable to load application information for x").starts_with("Its program is missing"));
        assert!(super::run_quick(&mut std::process::Command::new("true")).is_ok());
    }

    #[test]
    fn shortcut_target() {
        // A minimal shortcut: header, no ID list, a LinkInfo with a local path.
        let mut d = vec![0u8; 0x4C];
        d[0] = 0x4C;
        d[0x14] = 2; // HasLinkInfo
        let path = b"C:\\Program Files\\Code\\Code.exe\0";
        let mut info = vec![0u8; 28];
        let size = (28 + path.len()) as u32;
        info[0..4].copy_from_slice(&size.to_le_bytes());
        info[4..8].copy_from_slice(&28u32.to_le_bytes());
        info[8..12].copy_from_slice(&1u32.to_le_bytes());
        info[16..20].copy_from_slice(&28u32.to_le_bytes());
        d.extend(info);
        d.extend_from_slice(path);
        let t = super::lnk::target(&d).unwrap();
        assert_eq!(super::exe_key(&t), "code");
    }
}
