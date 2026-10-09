//! App icons for the island's app lists, as small data: URLs.

use crate::protocol::AppEntry;

/// The app's icon (64 px PNG, or an SVG), or None.
pub fn icon(app: &AppEntry) -> Option<String> {
    let r = imp::icon(app);
    if r.is_none() {
        log::debug!("no icon for {}", app.name);
    }
    r
}

#[allow(dead_code)]
fn png_url(rgba: image::RgbaImage) -> Option<String> {
    let img = image::DynamicImage::ImageRgba8(rgba);
    let img = if img.width() > 64 || img.height() > 64 { img.resize(64, 64, image::imageops::FilterType::Lanczos3) } else { img };
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png).ok()?;
    Some(format!("data:image/png;base64,{}", super::base64(&out.into_inner())))
}

#[allow(dead_code)]
fn file_url(path: &std::path::Path) -> Option<String> {
    let ext = path.extension()?.to_str()?.to_lowercase();
    let bytes = std::fs::read(path).ok()?;
    match ext.as_str() {
        "svg" if bytes.len() < 400_000 => Some(format!("data:image/svg+xml;base64,{}", super::base64(&bytes))),
        "png" | "jpg" | "jpeg" | "ico" | "bmp" | "webp" => png_url(image::load_from_memory(&bytes).ok()?.to_rgba8()),
        _ => None,
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use super::*;
    use std::path::{Path, PathBuf};
    use std::sync::OnceLock;

    /// Icon theme folders, most specific first.
    fn theme_dirs() -> &'static Vec<PathBuf> {
        static DIRS: OnceLock<Vec<PathBuf>> = OnceLock::new();
        DIRS.get_or_init(|| {
            let mut bases: Vec<PathBuf> = vec![];
            if let Some(h) = dirs::home_dir() {
                bases.push(h.join(".local/share/icons"));
                bases.push(h.join(".icons"));
                bases.push(h.join(".local/share/flatpak/exports/share/icons"));
            }
            let sys = std::env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".into());
            for d in sys.split(':').filter(|d| !d.is_empty()) {
                bases.push(Path::new(d).join("icons"));
            }
            bases.push("/usr/share/icons".into());
            bases.push("/var/lib/flatpak/exports/share/icons".into());
            bases.dedup();
            // The current theme first, hicolor (every app's fallback) last.
            let current = std::process::Command::new("gsettings")
                .args(["get", "org.gnome.desktop.interface", "icon-theme"])
                .output()
                .ok()
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().trim_matches('\'').to_string())
                .unwrap_or_default();
            let mut out = vec![];
            for b in &bases {
                let Ok(rd) = std::fs::read_dir(b) else { continue };
                let mut themes: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
                themes.sort_by_key(|t| {
                    let n = t.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    (n != current, n == "hicolor", n)
                });
                out.extend(themes);
            }
            out
        })
    }

    fn find(name: &str) -> Option<PathBuf> {
        if name.starts_with('/') {
            return Path::new(name).is_file().then(|| PathBuf::from(name));
        }
        const SIZES: [&str; 8] = ["64x64", "48x48", "96x96", "128x128", "256x256", "scalable", "512x512", "32x32"];
        for theme in theme_dirs() {
            for size in SIZES {
                for ext in ["png", "svg"] {
                    for sub in ["apps", "applications"] {
                        let p = theme.join(size).join(sub).join(format!("{name}.{ext}"));
                        if p.is_file() {
                            return Some(p);
                        }
                    }
                }
            }
        }
        for ext in ["png", "svg"] {
            let p = Path::new("/usr/share/pixmaps").join(format!("{name}.{ext}"));
            if p.is_file() {
                return Some(p);
            }
        }
        None
    }

    pub fn icon(app: &AppEntry) -> Option<String> {
        let name = if app.icon.is_empty() { &app.exe } else { &app.icon };
        file_url(&find(name)?)
    }
}

#[cfg(windows)]
mod imp {
    use super::*;
    use windows::core::PCWSTR;
    use windows::Win32::Graphics::Gdi::{CreateCompatibleDC, DeleteDC, DeleteObject, GetDIBits, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS};
    use windows::Win32::Storage::FileSystem::FILE_FLAGS_AND_ATTRIBUTES;
    use windows::Win32::UI::Shell::{SHGetFileInfoW, SHFILEINFOW, SHGFI_ICON, SHGFI_LARGEICON};
    use windows::Win32::UI::WindowsAndMessaging::{DestroyIcon, GetIconInfo, ICONINFO};

    pub fn icon(app: &AppEntry) -> Option<String> {
        unsafe {
            let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_APARTMENTTHREADED);
            let wide: Vec<u16> = app.id.encode_utf16().chain(std::iter::once(0)).collect();
            let mut info = SHFILEINFOW::default();
            let ok = SHGetFileInfoW(
                PCWSTR(wide.as_ptr()),
                FILE_FLAGS_AND_ATTRIBUTES(0),
                Some(&mut info),
                std::mem::size_of::<SHFILEINFOW>() as u32,
                SHGFI_ICON | SHGFI_LARGEICON,
            );
            if ok == 0 || info.hIcon.is_invalid() {
                return None;
            }
            let hicon = info.hIcon;
            let mut ii = ICONINFO::default();
            let r = GetIconInfo(hicon, &mut ii);
            let mut out = None;
            if r.is_ok() && !ii.hbmColor.is_invalid() {
                let dc = CreateCompatibleDC(None);
                let mut bmi = BITMAPINFO::default();
                bmi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
                // Ask for the size first.
                if GetDIBits(dc, ii.hbmColor, 0, 0, None, &mut bmi, DIB_RGB_COLORS) != 0 {
                    let (w, h) = (bmi.bmiHeader.biWidth.unsigned_abs(), bmi.bmiHeader.biHeight.unsigned_abs());
                    bmi.bmiHeader.biBitCount = 32;
                    bmi.bmiHeader.biCompression = BI_RGB.0;
                    bmi.bmiHeader.biHeight = -(h as i32); // top row first
                    let mut px = vec![0u8; (w * h * 4) as usize];
                    if w > 0 && h > 0 && GetDIBits(dc, ii.hbmColor, 0, h, Some(px.as_mut_ptr() as *mut _), &mut bmi, DIB_RGB_COLORS) != 0 {
                        let has_alpha = px.chunks(4).any(|p| p[3] != 0);
                        for p in px.chunks_mut(4) {
                            p.swap(0, 2);
                            if !has_alpha {
                                p[3] = 255;
                            }
                        }
                        out = image::RgbaImage::from_raw(w, h, px).and_then(png_url);
                    }
                }
                let _ = DeleteDC(dc);
            }
            if !ii.hbmColor.is_invalid() {
                let _ = DeleteObject(ii.hbmColor.into());
            }
            if !ii.hbmMask.is_invalid() {
                let _ = DeleteObject(ii.hbmMask.into());
            }
            let _ = DestroyIcon(hicon);
            out
        }
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use super::*;
    use objc2_app_kit::NSWorkspace;
    use objc2_foundation::NSString;

    pub fn icon(app: &AppEntry) -> Option<String> {
        let img = NSWorkspace::sharedWorkspace().iconForFile(&NSString::from_str(&app.id));
        let tiff = img.TIFFRepresentation()?;
        let bytes = tiff.to_vec();
        png_url(image::load_from_memory_with_format(&bytes, image::ImageFormat::Tiff).ok()?.to_rgba8())
    }
}

#[cfg(not(any(target_os = "linux", windows, target_os = "macos")))]
mod imp {
    use super::*;
    pub fn icon(_: &AppEntry) -> Option<String> {
        None
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    #[test]
    fn svg_and_png_files() {
        let dir = std::env::temp_dir().join(format!("openhop-icons-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let svg = dir.join("a.svg");
        std::fs::write(&svg, "<svg xmlns='http://www.w3.org/2000/svg'/>").unwrap();
        assert!(super::file_url(&svg).unwrap().starts_with("data:image/svg+xml;base64,"));
        let png = dir.join("b.png");
        image::RgbaImage::from_pixel(128, 128, image::Rgba([1, 2, 3, 255])).save(&png).unwrap();
        assert!(super::file_url(&png).unwrap().starts_with("data:image/png;base64,"));
        let app = crate::protocol::AppEntry { id: "x".into(), name: "B".into(), exe: "b".into(), icon: png.to_string_lossy().into_owned() };
        assert!(super::icon(&app).is_some());
        let _ = std::fs::remove_dir_all(dir);
    }
}

#[cfg(all(test, target_os = "linux"))]
mod live {
    #[test]
    fn installed_icons() {
        if std::env::var_os("OPENHOP_ICON_TEST").is_none() {
            return;
        }
        for a in crate::extras::apps::list().into_iter().take(12) {
            let i = super::icon(&a);
            println!("{} [{}] -> {:?}", a.name, a.icon, i.map(|u| u.chars().take(60).collect::<String>()));
        }
    }
}
