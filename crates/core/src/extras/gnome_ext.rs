#![cfg_attr(not(target_os = "linux"), allow(dead_code))]
//! Installs OpenHop's small GNOME Shell extension (GNOME on Wayland), which
//! tells OpenHop about windows and the pointer: see `wins::gnome`.

pub const UUID: &str = "openhop@openhop.dev";

const IFACE: &str = r#"<node><interface name="dev.openhop.Shell">
<method name="Windows"><arg type="s" direction="out"/></method>
<method name="Pointer"><arg type="i" direction="out"/><arg type="i" direction="out"/><arg type="u" direction="out"/></method>
<method name="Activate"><arg type="t" direction="in"/></method>
<method name="Lane"><arg type="u" direction="in"/><arg type="i" direction="out"/></method>
<method name="LaneOff"/>
</interface></node>"#;

const SERVICE: &str = r#"
class Service {
    Windows() {
        const out = [];
        for (const a of global.get_window_actors()) {
            const w = a.meta_window;
            const t = w.get_window_type();
            if (t !== Meta.WindowType.NORMAL && t !== Meta.WindowType.DIALOG) continue;
            const r = w.get_frame_rect();
            out.push({ id: w.get_id(), title: w.get_title() || '', app: w.get_wm_class() || '', pid: w.get_pid(),
                x: r.x, y: r.y, w: r.width, h: r.height, minimized: !!w.minimized, skip: w.is_skip_taskbar() });
        }
        return JSON.stringify(out);
    }
    Pointer() {
        const [x, y, mods] = global.get_pointer();
        return [x, y, mods];
    }
    // A strip under the top bar that maximized windows leave free: the
    // island's own lane. Returns where it starts.
    Lane(height) {
        this.LaneOff();
        const m = Main.layoutManager.primaryMonitor;
        const top = m.y + (Main.panel && Main.panel.visible ? Main.panel.height : 0);
        this._lane = new St.Widget({ reactive: false, x: m.x, y: top, width: m.width, height });
        Main.layoutManager.addChrome(this._lane, { affectsStruts: true, trackFullscreen: true });
        return top;
    }
    LaneOff() {
        if (this._lane) {
            Main.layoutManager.removeChrome(this._lane);
            this._lane.destroy();
            this._lane = null;
        }
    }
    Activate(id) {
        for (const a of global.get_window_actors()) {
            const w = a.meta_window;
            if (String(w.get_id()) === String(id)) w.activate(global.get_current_time());
        }
    }
}
"#;

/// GNOME 45 and later (JavaScript modules).
fn modern() -> String {
    format!(
        "import Gio from 'gi://Gio';\nimport Meta from 'gi://Meta';\nimport St from 'gi://St';\nimport * as Main from 'resource:///org/gnome/shell/ui/main.js';\nimport {{Extension}} from 'resource:///org/gnome/shell/extensions/extension.js';\nconst IFACE = `{IFACE}`;\n{SERVICE}\nexport default class OpenHopExtension extends Extension {{\n    enable() {{\n        this._svc = new Service();\n        this._obj = Gio.DBusExportedObject.wrapJSObject(IFACE, this._svc);\n        this._obj.export(Gio.DBus.session, '/dev/openhop/Shell');\n        this._name = Gio.bus_own_name(Gio.BusType.SESSION, 'dev.openhop.Shell', Gio.BusNameOwnerFlags.NONE, null, null, null);\n    }}\n    disable() {{\n        this._svc?.LaneOff();\n        this._obj?.unexport();\n        this._obj = null;\n        if (this._name) Gio.bus_unown_name(this._name);\n        this._name = 0;\n    }}\n}}\n"
    )
}

/// GNOME 40 to 44.
fn legacy() -> String {
    format!(
        "const {{ Gio, Meta, St }} = imports.gi;\nconst Main = imports.ui.main;\nconst IFACE = `{IFACE}`;\n{SERVICE}\nclass OpenHopExtension {{\n    enable() {{\n        this._svc = new Service();\n        this._obj = Gio.DBusExportedObject.wrapJSObject(IFACE, this._svc);\n        this._obj.export(Gio.DBus.session, '/dev/openhop/Shell');\n        this._name = Gio.bus_own_name(Gio.BusType.SESSION, 'dev.openhop.Shell', Gio.BusNameOwnerFlags.NONE, null, null, null);\n    }}\n    disable() {{\n        if (this._svc) this._svc.LaneOff();\n        if (this._obj) this._obj.unexport();\n        this._obj = null;\n        if (this._name) Gio.bus_unown_name(this._name);\n        this._name = 0;\n    }}\n}}\nfunction init() {{\n    return new OpenHopExtension();\n}}\n"
    )
}

fn metadata(versions: &[&str]) -> String {
    let v: Vec<String> = versions.iter().map(|s| format!("\"{s}\"")).collect();
    format!(
        "{{\n  \"uuid\": \"{UUID}\",\n  \"name\": \"OpenHop\",\n  \"description\": \"Lets OpenHop see your windows and the pointer on Wayland, for its island and app list.\",\n  \"shell-version\": [{}],\n  \"url\": \"https://github.com/maykano-dev/open-hop\",\n  \"version\": 1\n}}\n",
        v.join(", ")
    )
}

/// The running GNOME Shell's major version.
#[cfg(target_os = "linux")]
fn shell_version() -> Option<u32> {
    let out = std::process::Command::new("gnome-shell").arg("--version").output().ok()?;
    let s = String::from_utf8_lossy(&out.stdout);
    s.split_whitespace().last()?.split('.').next()?.parse().ok()
}

#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Not GNOME on Wayland: nothing needed.
    NotNeeded,
    /// Running already.
    Active,
    /// Installed (or updated): works after the next login.
    AfterLogin,
    Failed,
}

/// Put the extension in place and switch it on, when it's needed.
pub fn ensure() -> Outcome {
    #[cfg(target_os = "linux")]
    {
        let gnome = std::env::var("XDG_CURRENT_DESKTOP").map(|d| d.to_uppercase().contains("GNOME")).unwrap_or(false);
        if !gnome || !crate::platform::linux_is_wayland() {
            return Outcome::NotNeeded;
        }
        let Some(ver) = shell_version() else { return Outcome::Failed };
        let Some(dir) = dirs::data_dir().map(|d| d.join("gnome-shell/extensions").join(UUID)) else { return Outcome::Failed };
        let (js, versions): (String, Vec<String>) =
            if ver >= 45 { (modern(), (45..=60).map(|v| v.to_string()).collect()) } else { (legacy(), (40..=44).map(|v| v.to_string()).collect()) };
        let refs: Vec<&str> = versions.iter().map(String::as_str).collect();
        let meta = metadata(&refs);
        let changed = std::fs::read_to_string(dir.join("extension.js")).ok().as_deref() != Some(js.as_str());
        if changed {
            if std::fs::create_dir_all(&dir).is_err()
                || std::fs::write(dir.join("extension.js"), &js).is_err()
                || std::fs::write(dir.join("metadata.json"), &meta).is_err()
            {
                return Outcome::Failed;
            }
            log::info!("installed OpenHop's GNOME Shell helper (GNOME {ver})");
        }
        // Switched on (takes effect when GNOME Shell loads extensions: next login).
        let list = std::process::Command::new("gsettings")
            .args(["get", "org.gnome.shell", "enabled-extensions"])
            .output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_default();
        if !list.contains(UUID) {
            let inner = list.trim_start_matches("@as").trim().trim_start_matches('[').trim_end_matches(']').trim().to_string();
            let new = if inner.is_empty() { format!("['{UUID}']") } else { format!("[{inner}, '{UUID}']") };
            let _ = std::process::Command::new("gsettings").args(["set", "org.gnome.shell", "enabled-extensions", &new]).status();
            let _ = std::process::Command::new("gnome-extensions").args(["enable", UUID]).status();
        }
        if crate::wins::gnome::available() {
            return if changed { Outcome::AfterLogin } else { Outcome::Active };
        }
        return Outcome::AfterLogin;
    }
    #[allow(unreachable_code)]
    Outcome::NotNeeded
}

#[cfg(test)]
mod tests {
    #[test]
    fn extension_sources() {
        let m = super::modern();
        assert!(m.contains("export default class") && m.contains("dev.openhop.Shell") && m.contains("get_window_actors"));
        let l = super::legacy();
        assert!(l.contains("function init()") && !l.contains("import "));
        assert!(super::metadata(&["46"]).contains("\"shell-version\": [\"46\"]"));
        if let Ok(d) = std::env::var("OPENHOP_EXT_OUT") {
            super::write_for_check(std::path::Path::new(&d));
        }
    }
}

#[cfg(test)]
pub fn write_for_check(dir: &std::path::Path) {
    std::fs::write(dir.join("modern.mjs"), modern()).unwrap();
    std::fs::write(dir.join("legacy.js"), legacy()).unwrap();
}
