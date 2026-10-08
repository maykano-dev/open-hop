//! Small OS integrations: dark mode, Do Not Disturb / presenting.
//! All best-effort: anything the OS doesn't expose returns None and is skipped.

#[allow(dead_code)]
fn run(cmd: &str, args: &[&str]) -> Option<String> {
    let out = std::process::Command::new(cmd).args(args).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

// ------------------------------------------------------------- dark mode

/// Is the system in dark mode?
pub fn dark_mode() -> Option<bool> {
    #[cfg(target_os = "linux")]
    {
        let s = run("gsettings", &["get", "org.gnome.desktop.interface", "color-scheme"])?;
        return Some(s.contains("dark"));
    }
    #[cfg(target_os = "macos")]
    {
        // The key only exists in dark mode.
        let out = std::process::Command::new("defaults").args(["read", "-g", "AppleInterfaceStyle"]).output().ok()?;
        return Some(String::from_utf8_lossy(&out.stdout).contains("Dark"));
    }
    #[cfg(windows)]
    {
        return win::read_dword(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize", "AppsUseLightTheme").map(|v| v == 0);
    }
    #[allow(unreachable_code)]
    None
}

pub fn set_dark_mode(dark: bool) {
    log::info!("turning dark mode {}", if dark { "on" } else { "off" });
    #[cfg(target_os = "linux")]
    {
        let scheme = if dark { "prefer-dark" } else { "default" };
        let _ = run("gsettings", &["set", "org.gnome.desktop.interface", "color-scheme", scheme]);
        // Ubuntu's Yaru themes come in light and "-dark" variants; older apps follow these.
        if let Some(theme) = run("gsettings", &["get", "org.gnome.desktop.interface", "gtk-theme"]) {
            let theme = theme.trim_matches('\'');
            if theme.starts_with("Yaru") || theme.starts_with("Adwaita") {
                let base = theme.trim_end_matches("-dark");
                let want = if dark { format!("{base}-dark") } else { base.to_string() };
                if want != theme {
                    let _ = run("gsettings", &["set", "org.gnome.desktop.interface", "gtk-theme", &want]);
                }
            }
        }
    }
    #[cfg(target_os = "macos")]
    {
        let script = format!("tell application \"System Events\" to tell appearance preferences to set dark mode to {dark}");
        let _ = run("osascript", &["-e", &script]);
    }
    #[cfg(windows)]
    {
        let v = if dark { 0 } else { 1 };
        let key = r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize";
        win::write_dword(key, "AppsUseLightTheme", v);
        win::write_dword(key, "SystemUsesLightTheme", v);
        win::broadcast_setting("ImmersiveColorSet");
    }
}

// ------------------------------------------------------------- Do Not Disturb

/// Is Do Not Disturb (or Focus) on?
pub fn dnd() -> Option<bool> {
    #[cfg(target_os = "linux")]
    {
        let s = run("gsettings", &["get", "org.gnome.desktop.notifications", "show-banners"])?;
        return Some(s == "false");
    }
    #[cfg(target_os = "macos")]
    {
        // Readable only with Full Disk Access; otherwise unknown.
        let path = dirs::home_dir()?.join("Library/DoNotDisturb/DB/Assertions.json");
        let text = std::fs::read_to_string(path).ok()?;
        return Some(text.contains("\"storeAssertionRecords\"") && text.contains("assertionDetails"));
    }
    #[cfg(windows)]
    {
        return win::read_dword(r"Software\Microsoft\Windows\CurrentVersion\Notifications\Settings", "NOC_GLOBAL_SETTING_TOASTS_ENABLED").map(|v| v == 0);
    }
    #[allow(unreachable_code)]
    None
}

pub fn set_dnd(on: bool) {
    log::info!("Do Not Disturb {}", if on { "on" } else { "off" });
    #[cfg(target_os = "linux")]
    {
        let _ = run("gsettings", &["set", "org.gnome.desktop.notifications", "show-banners", if on { "false" } else { "true" }]);
    }
    #[cfg(target_os = "macos")]
    {
        // macOS has no API for Focus. If the user made these two Shortcuts
        // (each a single "Set Focus" action), run them.
        let name = if on { "OpenHop Focus On" } else { "OpenHop Focus Off" };
        let _ = run("shortcuts", &["run", name]);
    }
    #[cfg(windows)]
    {
        win::write_dword(r"Software\Microsoft\Windows\CurrentVersion\Notifications\Settings", "NOC_GLOBAL_SETTING_TOASTS_ENABLED", if on { 0 } else { 1 });
        win::broadcast_setting("Notifications");
    }
}

/// Is something full-screen in front (a presentation, a video call, a game)?
pub fn presenting() -> bool {
    #[cfg(windows)]
    {
        use windows::Win32::UI::Shell::SHQueryUserNotificationState;
        if let Ok(state) = unsafe { SHQueryUserNotificationState() } {
            // BUSY (full-screen app), RUNNING_D3D_FULL_SCREEN, PRESENTATION_MODE
            return matches!(state.0, 2 | 3 | 4);
        }
        return false;
    }
    #[cfg(target_os = "linux")]
    {
        return linux_fullscreen().unwrap_or(false);
    }
    #[allow(unreachable_code)]
    false
}

#[cfg(target_os = "linux")]
fn linux_fullscreen() -> Option<bool> {
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{AtomEnum, ConnectionExt as _};
    let (conn, n) = x11rb::connect(None).ok()?;
    let root = conn.setup().roots[n].root;
    let atom = |s: &str| conn.intern_atom(false, s.as_bytes()).ok()?.reply().ok().map(|r| r.atom);
    let (active, state, full) = (atom("_NET_ACTIVE_WINDOW")?, atom("_NET_WM_STATE")?, atom("_NET_WM_STATE_FULLSCREEN")?);
    let w = conn.get_property(false, root, active, AtomEnum::WINDOW, 0, 1).ok()?.reply().ok()?.value32()?.next()?;
    if w == 0 {
        return Some(false);
    }
    let st: Vec<u32> = conn.get_property(false, w, state, AtomEnum::ATOM, 0, 64).ok()?.reply().ok()?.value32()?.collect();
    Some(st.contains(&full))
}

#[cfg(windows)]
mod win {
    use windows::core::{HSTRING, PCWSTR};
    use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
    use windows::Win32::System::Registry::{RegGetValueW, RegSetKeyValueW, HKEY_CURRENT_USER, REG_DWORD, RRF_RT_REG_DWORD};
    use windows::Win32::UI::WindowsAndMessaging::{SendMessageTimeoutW, HWND_BROADCAST, SMTO_ABORTIFHUNG, WM_SETTINGCHANGE};

    pub fn read_dword(key: &str, name: &str) -> Option<u32> {
        let mut v = 0u32;
        let mut len = 4u32;
        let r = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                &HSTRING::from(key),
                &HSTRING::from(name),
                RRF_RT_REG_DWORD,
                None,
                Some(&mut v as *mut u32 as *mut _),
                Some(&mut len),
            )
        };
        r.is_ok().then_some(v)
    }

    pub fn write_dword(key: &str, name: &str, v: u32) {
        unsafe {
            let _ = RegSetKeyValueW(HKEY_CURRENT_USER, &HSTRING::from(key), &HSTRING::from(name), REG_DWORD.0, Some(&v as *const u32 as *const _), 4);
        }
    }

    pub fn broadcast_setting(area: &str) {
        let s = HSTRING::from(area);
        unsafe {
            let _ = SendMessageTimeoutW(HWND_BROADCAST, WM_SETTINGCHANGE, WPARAM(0), LPARAM(PCWSTR(s.as_ptr()).0 as isize), SMTO_ABORTIFHUNG, 200, None);
        }
        let _ = HWND::default();
    }
}
