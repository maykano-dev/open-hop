//! The apps installed on this computer (for the launcher on every computer)
//! and opening one.

use crate::protocol::AppEntry;
use std::path::{Path, PathBuf};

/// Installed apps, sorted by name.
pub fn list() -> Vec<AppEntry> {
    let mut v = imp::list();
    v.sort_by_key(|a| a.name.to_lowercase());
    v.dedup_by(|a, b| a.name.eq_ignore_ascii_case(&b.name));
    v
}

/// Open the app `id` (from [`list`]).
pub fn launch(id: &str) {
    log::info!("opening {id}");
    if let Err(e) = imp::launch(id) {
        log::warn!("couldn't open {id}: {e}");
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
            v.push(Path::new(d).join("applications"));
        }
        v.push("/var/lib/flatpak/exports/share/applications".into());
        v.push("/var/lib/snapd/desktop/applications".into());
        v
    }

    pub(super) fn entry(path: &Path) -> Option<AppEntry> {
        let text = std::fs::read_to_string(path).ok()?;
        let mut in_main = false;
        let (mut name, mut hidden, mut app) = (None, false, false);
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
            } else if line == "NoDisplay=true" || line == "Hidden=true" {
                hidden = true;
            } else if line == "Type=Application" {
                app = true;
            }
        }
        let name = name?;
        (app && !hidden).then(|| AppEntry { id: path.to_string_lossy().into_owned(), name })
    }

    pub fn list() -> Vec<AppEntry> {
        let mut files = vec![];
        for d in dirs() {
            walk(&d, 1, &mut files, "desktop");
        }
        files.iter().filter_map(|p| entry(p)).collect()
    }

    pub fn launch(id: &str) -> std::io::Result<()> {
        // gio knows how to run a .desktop file properly (field codes, terminal apps).
        if std::process::Command::new("gio").args(["launch", id]).spawn().is_ok() {
            return Ok(());
        }
        let name = Path::new(id).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        std::process::Command::new("gtk-launch").arg(name).spawn().map(|_| ())
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
                Some(AppEntry { id: p.to_string_lossy().into_owned(), name })
            })
            .collect()
    }

    pub fn launch(id: &str) -> std::io::Result<()> {
        use std::os::windows::process::CommandExt;
        std::process::Command::new("cmd").args(["/C", "start", "", id]).creation_flags(0x08000000).spawn().map(|_| ())
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
        files.into_iter().filter_map(|p| Some(AppEntry { name: p.file_stem()?.to_string_lossy().into_owned(), id: p.to_string_lossy().into_owned() })).collect()
    }

    pub fn launch(id: &str) -> std::io::Result<()> {
        std::process::Command::new("open").args(["-a", id]).spawn().map(|_| ())
    }
}

#[cfg(not(any(target_os = "linux", windows, target_os = "macos")))]
mod imp {
    use super::*;
    pub fn list() -> Vec<AppEntry> {
        vec![]
    }
    pub fn launch(_: &str) -> std::io::Result<()> {
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
        std::fs::write(dir.join("a.desktop"), "[Desktop Entry]\nType=Application\nName=Notes\nExec=notes\n[Desktop Action x]\nName=Other\n").unwrap();
        std::fs::write(dir.join("b.desktop"), "[Desktop Entry]\nType=Application\nName=Hidden\nNoDisplay=true\n").unwrap();
        let mut files = vec![];
        super::walk(&dir, 0, &mut files, "desktop");
        let names: Vec<String> = files.iter().filter_map(|p| super::imp::entry(p)).map(|a| a.name).collect();
        assert_eq!(names, vec!["Notes".to_string()]);
        let _ = std::fs::remove_dir_all(dir);
    }
}
