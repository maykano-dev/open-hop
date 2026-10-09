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

// ------------------------------------------------------------- battery, storage

/// The main screen's camera notch on a MacBook: (width, height) in points.
/// Must be called on the main thread.
#[cfg(target_os = "macos")]
pub fn notch() -> Option<(f64, f64)> {
    use objc2::MainThreadMarker;
    use objc2_app_kit::NSScreen;
    let mtm = MainThreadMarker::new()?;
    // The built-in screen is the one with the menu bar when it's the only
    // one; with others, the notch is on whichever has a top safe area.
    for screen in NSScreen::screens(mtm).iter() {
        #[allow(unused_unsafe)]
        let (insets, left, right, frame) = unsafe { (screen.safeAreaInsets(), screen.auxiliaryTopLeftArea(), screen.auxiliaryTopRightArea(), screen.frame()) };
        if insets.top > 0.0 && left.size.width > 0.0 && right.size.width > 0.0 {
            let w = frame.size.width - left.size.width - right.size.width;
            if w > 0.0 {
                return Some((w, insets.top));
            }
        }
    }
    None
}

/// The height of the menu bar on the main screen (points).
#[cfg(target_os = "macos")]
pub fn menu_bar_height() -> Option<f64> {
    use objc2::MainThreadMarker;
    use objc2_app_kit::NSScreen;
    let mtm = MainThreadMarker::new()?;
    let s = NSScreen::mainScreen(mtm)?;
    let (f, v) = (s.frame(), s.visibleFrame());
    let h = (f.origin.y + f.size.height) - (v.origin.y + v.size.height);
    (h > 0.0).then_some(h)
}

/// (percent, charging or plugged in) if this computer has a battery.
pub fn battery() -> Option<(u8, bool)> {
    #[cfg(target_os = "linux")]
    {
        for e in std::fs::read_dir("/sys/class/power_supply").ok()?.flatten() {
            let p = e.path();
            if std::fs::read_to_string(p.join("type")).map(|t| t.trim() == "Battery").unwrap_or(false) {
                // Peripherals (mice, keyboards) report batteries too; skip them.
                if std::fs::read_to_string(p.join("scope")).map(|s| s.trim() == "Device").unwrap_or(false) {
                    continue;
                }
                let pct = std::fs::read_to_string(p.join("capacity")).ok()?.trim().parse::<u8>().ok()?;
                let status = std::fs::read_to_string(p.join("status")).unwrap_or_default();
                return Some((pct.min(100), status.trim() != "Discharging"));
            }
        }
        return None;
    }
    #[cfg(target_os = "macos")]
    {
        let s = run("pmset", &["-g", "batt"])?;
        let line = s.lines().find(|l| l.contains('%'))?;
        let pct: u8 = line.split('%').next()?.rsplit(|c: char| !c.is_ascii_digit()).next()?.parse().ok()?;
        return Some((pct.min(100), !line.contains("discharging")));
    }
    #[cfg(windows)]
    {
        use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
        let mut st = SYSTEM_POWER_STATUS::default();
        unsafe { GetSystemPowerStatus(&mut st).ok()? };
        // 128 = no battery, 255 = unknown
        if st.BatteryFlag & 128 != 0 || st.BatteryLifePercent > 100 {
            return None;
        }
        return Some((st.BatteryLifePercent, st.ACLineStatus == 1));
    }
    #[allow(unreachable_code)]
    None
}

/// (free, total) bytes on the disk with the user's files.
pub fn disk() -> Option<(u64, u64)> {
    #[cfg(unix)]
    {
        let home = dirs::home_dir()?;
        let path = std::ffi::CString::new(home.to_string_lossy().as_bytes()).ok()?;
        let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
        if unsafe { libc::statvfs(path.as_ptr(), &mut st) } != 0 {
            return None;
        }
        let unit = st.f_frsize as u64;
        return Some((st.f_bavail as u64 * unit, st.f_blocks as u64 * unit));
    }
    #[cfg(windows)]
    {
        use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
        let (mut free, mut total) = (0u64, 0u64);
        let dir = windows::core::HSTRING::from(dirs::home_dir().map(|h| h.to_string_lossy().into_owned()).unwrap_or_else(|| "C:\\".into()));
        unsafe { GetDiskFreeSpaceExW(&dir, Some(&mut free), Some(&mut total), None).ok()? };
        return Some((free, total));
    }
    #[allow(unreachable_code)]
    None
}

// ------------------------------------------------------------- lock, sleep, wake

/// Is the screen locked?
pub fn locked() -> Option<bool> {
    #[cfg(target_os = "linux")]
    {
        if let Some(s) = run(
            "gdbus",
            &["call", "--session", "--dest", "org.gnome.ScreenSaver", "--object-path", "/org/gnome/ScreenSaver", "--method", "org.gnome.ScreenSaver.GetActive"],
        ) {
            return Some(s.contains("true"));
        }
        let id = std::env::var("XDG_SESSION_ID").ok()?;
        let s = run("loginctl", &["show-session", &id, "-p", "LockedHint", "--value"])?;
        return Some(s.trim() == "yes");
    }
    #[cfg(windows)]
    {
        use windows::Win32::System::StationsAndDesktops::{CloseDesktop, OpenInputDesktop, DESKTOP_CONTROL_FLAGS, DESKTOP_SWITCHDESKTOP};
        // The lock screen runs on a desktop we can't open.
        return Some(match unsafe { OpenInputDesktop(DESKTOP_CONTROL_FLAGS(0), false, DESKTOP_SWITCHDESKTOP) } {
            Ok(d) => {
                unsafe {
                    let _ = CloseDesktop(d);
                }
                false
            }
            Err(_) => true,
        });
    }
    #[cfg(target_os = "macos")]
    {
        return Some(mac::screen_locked());
    }
    #[allow(unreachable_code)]
    None
}

/// Lock the screen now.
pub fn lock_now() {
    log::info!("locking this computer");
    #[cfg(target_os = "linux")]
    {
        if run("loginctl", &["lock-session"]).is_none() {
            let _ = run("xdg-screensaver", &["lock"]);
        }
    }
    #[cfg(windows)]
    unsafe {
        let _ = windows::Win32::System::Shutdown::LockWorkStation();
    }
    #[cfg(target_os = "macos")]
    {
        // Sleeping the display locks it (with "require password" on, the default).
        let _ = run("pmset", &["displaysleepnow"]);
    }
}

/// Put the computer to sleep now.
pub fn sleep_now() {
    log::info!("putting this computer to sleep");
    #[cfg(target_os = "linux")]
    {
        let _ = run("systemctl", &["suspend"]);
    }
    #[cfg(windows)]
    unsafe {
        let _ = windows::Win32::System::Power::SetSuspendState(false, false, false);
    }
    #[cfg(target_os = "macos")]
    {
        let _ = run("pmset", &["sleepnow"]);
    }
}

/// Wake the display (when another computer is being used again).
pub fn wake_display() {
    #[cfg(target_os = "linux")]
    {
        let _ = run("xset", &["dpms", "force", "on"]);
        let _ = run(
            "gdbus",
            &[
                "call",
                "--session",
                "--dest",
                "org.gnome.ScreenSaver",
                "--object-path",
                "/org/gnome/ScreenSaver",
                "--method",
                "org.gnome.ScreenSaver.SimulateUserActivity",
            ],
        );
    }
    #[cfg(windows)]
    unsafe {
        use windows::Win32::System::Power::{SetThreadExecutionState, ES_DISPLAY_REQUIRED, ES_SYSTEM_REQUIRED};
        let _ = SetThreadExecutionState(ES_DISPLAY_REQUIRED | ES_SYSTEM_REQUIRED);
    }
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("caffeinate").args(["-u", "-t", "2"]).spawn();
    }
}

#[cfg(target_os = "macos")]
mod mac {
    use core_foundation::base::{CFType, TCFType};
    use core_foundation::boolean::CFBoolean;
    use core_foundation::dictionary::{CFDictionary, CFDictionaryRef};
    use core_foundation::string::CFString;

    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGSessionCopyCurrentDictionary() -> CFDictionaryRef;
    }

    pub fn screen_locked() -> bool {
        unsafe {
            let d = CGSessionCopyCurrentDictionary();
            if d.is_null() {
                return false;
            }
            let d: CFDictionary<CFString, CFType> = CFDictionary::wrap_under_create_rule(d);
            d.find(CFString::new("CGSSessionScreenIsLocked")).and_then(|v| v.downcast::<CFBoolean>()).map(bool::from).unwrap_or(false)
        }
    }
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
