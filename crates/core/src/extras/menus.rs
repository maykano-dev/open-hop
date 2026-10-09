//! "Send with OpenHop" in the file manager's own right-click menu (Files,
//! Dolphin, Nemo, Thunar, Caja; Explorer; Finder), so files can go to
//! another computer without opening OpenHop.
//!
//! Each entry runs `openhop-app --send NAME FILES…` (or `--shelf FILES…`);
//! the running OpenHop picks it up.

use std::path::{Path, PathBuf};

/// One entry in the menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    /// What the menu shows.
    pub label: String,
    /// The arguments before the file names.
    pub args: Vec<String>,
    /// A short, file-name-safe key.
    pub key: String,
}

/// The menu for these computers: each of them, every computer, and the shelf.
pub fn targets(names: &[String]) -> Vec<Target> {
    let mut v: Vec<Target> = names.iter().map(|n| Target { label: n.clone(), args: vec!["--send".into(), n.clone()], key: slug(n) }).collect();
    if names.len() > 1 {
        v.push(Target { label: "All Computers".into(), args: vec!["--send".into(), "*".into()], key: "all-computers".into() });
    }
    v.push(Target { label: "Put on the Shelf".into(), args: vec!["--shelf".into()], key: "shelf".into() });
    v
}

pub fn slug(name: &str) -> String {
    let s: String = name.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' }).collect();
    let s = s.trim_matches('-').to_string();
    if s.is_empty() {
        format!("pc{:x}", name.bytes().fold(0u32, |h, b| h.wrapping_mul(31).wrapping_add(b as u32)))
    } else {
        s
    }
}

/// Write (or rewrite) the menus for `names` (and a paired phone), run by `exe`.
pub fn install(exe: &Path, names: &[String], phone: bool) {
    let mut t = targets(names);
    if phone {
        t.insert(t.len() - 1, Target { label: "Your Phone".into(), args: vec!["--phone".into()], key: "phone".into() });
    }
    if let Err(e) = imp::install(exe, &t) {
        log::warn!("couldn't add Send with OpenHop to the file manager: {e:#}");
    } else {
        log::info!("Send with OpenHop menu: {}", t.iter().map(|t| t.label.as_str()).collect::<Vec<_>>().join(", "));
    }
}

/// The program the menus should run (an AppImage moves on every start, so
/// use the AppImage itself).
pub fn exe() -> Option<PathBuf> {
    if let Some(a) = std::env::var_os("APPIMAGE") {
        return Some(a.into());
    }
    std::env::current_exe().ok()
}

/// Quote for a POSIX shell.
#[allow(dead_code)] // not every system uses every kind of quoting
fn sh(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// Quote for a .desktop Exec line (also used by Nemo and Thunar's %F lines).
#[allow(dead_code)] // not every system uses every kind of quoting
fn desk(s: &str) -> String {
    let mut o = String::from("\"");
    for c in s.chars() {
        if matches!(c, '"' | '`' | '$' | '\\') {
            o.push('\\');
        }
        if c == '%' {
            o.push('%');
        }
        o.push(c);
    }
    o.push('"');
    o
}

#[allow(dead_code)] // not every system uses every kind of quoting
fn xml(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn command(exe: &Path, t: &Target, quote: fn(&str) -> String) -> String {
    let mut parts = vec![quote(&exe.to_string_lossy())];
    parts.extend(t.args.iter().map(|a| quote(a)));
    parts.join(" ")
}

#[cfg(any(target_os = "linux", test))]
mod linux_files {
    use super::*;

    pub fn script(exe: &Path, t: &Target) -> String {
        // Files and Caja pass the selection as arguments, relative to the
        // folder the script runs in.
        format!("#!/bin/sh\n# Added by OpenHop.\nexec {} \"$@\"\n", command(exe, t, sh))
    }

    pub fn nemo(exe: &Path, t: &Target) -> String {
        let name = if t.args[0] == "--shelf" { t.label.clone() } else { format!("Send to {}", t.label) };
        format!(
            "[Nemo Action]\nName={}\nComment=Send with OpenHop\nExec={} %F\nIcon-Name=document-send\nSelection=notnone\nExtensions=any;\nQuote=double\n",
            name.replace('_', "__"),
            command(exe, t, desk)
        )
    }

    pub fn dolphin(exe: &Path, ts: &[Target]) -> String {
        let ids: Vec<String> = (0..ts.len()).map(|i| format!("openhop{i}")).collect();
        let mut s =
            format!("[Desktop Entry]\nType=Service\nMimeType=all/all;\nActions={};\nX-KDE-Submenu=Send with OpenHop\nX-KDE-Priority=TopLevel\n", ids.join(";"));
        for (id, t) in ids.iter().zip(ts) {
            s += &format!("\n[Desktop Action {id}]\nName={}\nIcon=document-send\nExec={} %F\n", t.label, command(exe, t, desk));
        }
        s
    }

    /// Thunar keeps every custom action in one file: replace only ours.
    pub fn thunar(existing: &str, exe: &Path, ts: &[Target]) -> String {
        let mut keep = String::new();
        let mut rest = existing;
        while let Some(start) = rest.find("<action>") {
            let Some(len) = rest[start..].find("</action>") else { break };
            let end = start + len + "</action>".len();
            keep += &rest[..start];
            let block = &rest[start..end];
            if !block.contains("<unique-id>openhop-") {
                keep += block;
            }
            rest = &rest[end..];
        }
        keep += rest;
        let mut ours = String::new();
        for t in ts {
            let name = if t.args[0] == "--shelf" { t.label.clone() } else { format!("Send to {}", t.label) };
            ours += &format!(
                "<action>\n\t<icon>document-send</icon>\n\t<name>{}</name>\n\t<submenu>Send with OpenHop</submenu>\n\t<unique-id>openhop-{}</unique-id>\n\t<command>{} %F</command>\n\t<description>Send with OpenHop</description>\n\t<range>*</range>\n\t<patterns>*</patterns>\n\t<directories/>\n\t<audio-files/>\n\t<image-files/>\n\t<other-files/>\n\t<text-files/>\n\t<video-files/>\n</action>\n",
                xml(&name),
                t.key,
                xml(&command(exe, t, desk))
            );
        }
        let base = if keep.contains("</actions>") { keep } else { "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<actions>\n</actions>\n".to_string() };
        let at = base.rfind("</actions>").unwrap();
        // Tidy the blank lines our old blocks left behind.
        let head = base[..at].trim_end().to_string();
        format!("{head}\n{ours}{}", &base[at..])
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use super::linux_files::*;
    use super::*;
    use anyhow::Result;
    use std::os::unix::fs::PermissionsExt;

    fn installed(prog: &str) -> bool {
        std::env::var_os("PATH").map(|p| std::env::split_paths(&p).any(|d| d.join(prog).is_file())).unwrap_or(false)
    }

    fn write(path: &Path, text: &str, exec: bool) -> Result<()> {
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d)?;
        }
        if std::fs::read_to_string(path).ok().as_deref() != Some(text) {
            std::fs::write(path, text)?;
        }
        if exec {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))?;
        }
        Ok(())
    }

    /// Remove our old files in `dir` that aren't in `wanted`.
    fn prune(dir: &Path, prefix: &str, wanted: &[PathBuf]) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            let p = e.path();
            let ours = p.file_name().and_then(|n| n.to_str()).map(|n| n.starts_with(prefix)).unwrap_or(false);
            if ours && !wanted.contains(&p) {
                let _ = std::fs::remove_file(p);
            }
        }
    }

    fn scripts(dir: PathBuf, exe: &Path, ts: &[Target]) -> Result<()> {
        let dir = dir.join("Send with OpenHop");
        let mut wanted = vec![];
        for t in ts {
            let p = dir.join(&t.label.replace('/', "∕"));
            write(&p, &script(exe, t), true)?;
            wanted.push(p);
        }
        // Everything in this folder is ours.
        prune(&dir, "", &wanted);
        Ok(())
    }

    pub fn install(exe: &Path, ts: &[Target]) -> Result<()> {
        let data = dirs::data_dir().unwrap_or_else(|| PathBuf::from("."));
        let config = dirs::config_dir().unwrap_or_else(|| PathBuf::from("."));
        if installed("nautilus") {
            scripts(data.join("nautilus/scripts"), exe, ts)?;
        }
        if installed("caja") {
            scripts(config.join("caja/scripts"), exe, ts)?;
        }
        if installed("nemo") {
            let dir = data.join("nemo/actions");
            let mut wanted = vec![];
            for t in ts {
                let p = dir.join(format!("openhop-{}.nemo_action", t.key));
                write(&p, &nemo(exe, t), false)?;
                wanted.push(p);
            }
            prune(&dir, "openhop-", &wanted);
        }
        if installed("dolphin") {
            // Plasma 6, and Plasma 5.
            write(&data.join("kio/servicemenus/openhop-send.desktop"), &dolphin(exe, ts), true)?;
            if data.join("kservices5").is_dir() {
                write(&data.join("kservices5/ServiceMenus/openhop-send.desktop"), &dolphin(exe, ts), false)?;
            }
        }
        if installed("thunar") {
            let path = config.join("Thunar/uca.xml");
            let old = std::fs::read_to_string(&path).unwrap_or_default();
            write(&path, &thunar(&old, exe, ts), false)?;
        }
        Ok(())
    }
}

#[cfg(windows)]
mod imp {
    use super::*;
    use anyhow::Result;
    use std::os::windows::process::CommandExt;

    const NO_WINDOW: u32 = 0x08000000;

    fn reg(args: &[&str]) -> Result<()> {
        let st = std::process::Command::new("reg").args(args).creation_flags(NO_WINDOW).output()?;
        if !st.status.success() && args[0] != "delete" {
            anyhow::bail!("reg {}: {}", args.join(" "), String::from_utf8_lossy(&st.stderr).trim());
        }
        Ok(())
    }

    fn quote(s: &str) -> String {
        format!("\"{s}\"")
    }

    pub fn install(exe: &Path, ts: &[Target]) -> Result<()> {
        let exe_s = exe.to_string_lossy().into_owned();
        // A "Send with OpenHop" submenu for files and folders.
        for base in [r"HKCU\Software\Classes\*\shell\OpenHop", r"HKCU\Software\Classes\Directory\shell\OpenHop"] {
            reg(&["delete", base, "/f"])?;
            reg(&["add", base, "/v", "MUIVerb", "/d", "Send with OpenHop", "/f"])?;
            reg(&["add", base, "/v", "SubCommands", "/d", "", "/f"])?;
            reg(&["add", base, "/v", "Icon", "/d", &format!("{exe_s},0"), "/f"])?;
            for (i, t) in ts.iter().enumerate() {
                let key = format!(r"{base}\shell\{i:02}");
                reg(&["add", &key, "/ve", "/d", &t.label, "/f"])?;
                // Every selected file opens OpenHop once; it gathers them up.
                reg(&["add", &key, "/v", "MultiSelectModel", "/d", "Player", "/f"])?;
                reg(&["add", &format!(r"{key}\command"), "/ve", "/d", &format!("{} \"%1\"", command(exe, t, quote)), "/f"])?;
            }
        }
        // And the classic Send to menu (all the files at once).
        if let Some(appdata) = std::env::var_os("APPDATA") {
            let dir = Path::new(&appdata).join(r"Microsoft\Windows\SendTo");
            let mut wanted = vec![];
            let mut ps = String::new();
            for t in ts {
                let name = if t.args[0] == "--shelf" {
                    "OpenHop Shelf.lnk".to_string()
                } else {
                    format!("{} (OpenHop).lnk", t.label.replace(['\\', '/', ':', '*', '?', '"', '<', '>', '|'], "_"))
                };
                let path = dir.join(&name);
                wanted.push(name);
                let args = t.args.iter().map(|a| quote(a)).collect::<Vec<_>>().join(" ");
                let q = |s: &str| s.replace('\'', "''");
                ps += &format!(
                    "$s=$w.CreateShortcut('{}');$s.TargetPath='{}';$s.Arguments='{}';$s.IconLocation='{},0';$s.Save();",
                    q(&path.to_string_lossy()),
                    q(&exe_s),
                    q(&args),
                    q(&exe_s)
                );
            }
            if let Ok(rd) = std::fs::read_dir(&dir) {
                for e in rd.flatten() {
                    let n = e.file_name().to_string_lossy().into_owned();
                    if (n.ends_with("(OpenHop).lnk") || n == "OpenHop Shelf.lnk") && !wanted.contains(&n) {
                        let _ = std::fs::remove_file(e.path());
                    }
                }
            }
            let script = format!("$w=New-Object -ComObject WScript.Shell;{ps}");
            std::process::Command::new("powershell").args(["-NoProfile", "-NonInteractive", "-Command", &script]).creation_flags(NO_WINDOW).output()?;
        }
        Ok(())
    }
}

#[cfg(any(target_os = "macos", test))]
mod mac_files {
    use super::*;

    pub fn info(label: &str) -> String {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>NSServices</key>
	<array>
		<dict>
			<key>NSBackgroundColorName</key>
			<string>background</string>
			<key>NSIconName</key>
			<string>NSActionTemplate</string>
			<key>NSMenuItem</key>
			<dict>
				<key>default</key>
				<string>{}</string>
			</dict>
			<key>NSMessage</key>
			<string>runWorkflowAsService</string>
			<key>NSRequiredContext</key>
			<dict>
				<key>NSApplicationIdentifier</key>
				<string>com.apple.finder</string>
			</dict>
			<key>NSSendFileTypes</key>
			<array>
				<string>public.item</string>
			</array>
		</dict>
	</array>
</dict>
</plist>
"#,
            xml(label)
        )
    }

    pub fn workflow(exe: &Path, t: &Target) -> String {
        // Run in the background: when OpenHop wasn't running, this starts it.
        let cmd = format!("{} \"$@\" >/dev/null 2>&1 &", command(exe, t, sh));
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>AMApplicationBuild</key>
	<string>523</string>
	<key>AMApplicationVersion</key>
	<string>2.10</string>
	<key>AMDocumentVersion</key>
	<string>2</string>
	<key>actions</key>
	<array>
		<dict>
			<key>action</key>
			<dict>
				<key>AMAccepts</key>
				<dict>
					<key>Container</key>
					<string>List</string>
					<key>Optional</key>
					<true/>
					<key>Types</key>
					<array>
						<string>com.apple.cocoa.string</string>
					</array>
				</dict>
				<key>AMActionVersion</key>
				<string>2.0.3</string>
				<key>AMApplication</key>
				<array>
					<string>Automator</string>
				</array>
				<key>AMParameterProperties</key>
				<dict>
					<key>COMMAND_STRING</key>
					<dict/>
					<key>CheckedForUserDefaultShell</key>
					<dict/>
					<key>inputMethod</key>
					<dict/>
					<key>shell</key>
					<dict/>
					<key>source</key>
					<dict/>
				</dict>
				<key>AMProvides</key>
				<dict>
					<key>Container</key>
					<string>List</string>
					<key>Types</key>
					<array>
						<string>com.apple.cocoa.string</string>
					</array>
				</dict>
				<key>ActionBundlePath</key>
				<string>/System/Library/Automator/Run Shell Script.action</string>
				<key>ActionName</key>
				<string>Run Shell Script</string>
				<key>ActionParameters</key>
				<dict>
					<key>COMMAND_STRING</key>
					<string>{}</string>
					<key>CheckedForUserDefaultShell</key>
					<true/>
					<key>inputMethod</key>
					<integer>1</integer>
					<key>shell</key>
					<string>/bin/sh</string>
					<key>source</key>
					<string></string>
				</dict>
				<key>BundleIdentifier</key>
				<string>com.apple.RunShellScript</string>
				<key>CFBundleVersion</key>
				<string>2.0.3</string>
				<key>CanShowSelectedItemsWhenRun</key>
				<false/>
				<key>CanShowWhenRun</key>
				<true/>
				<key>Category</key>
				<array>
					<string>AMCategoryUtilities</string>
				</array>
				<key>Class Name</key>
				<string>RunShellScriptAction</string>
				<key>InputUUID</key>
				<string>0F9E6C2A-4B1D-4E57-9A43-0B5E1C7D2A11</string>
				<key>Keywords</key>
				<array>
					<string>Shell</string>
					<string>Script</string>
				</array>
				<key>OutputUUID</key>
				<string>6C1B9A47-2D3E-4F58-8B69-1A2B3C4D5E6F</string>
				<key>UUID</key>
				<string>A3D2C1B0-9E8F-4A7B-8C6D-5E4F3A2B1C0D</string>
				<key>UnlocalizedApplications</key>
				<array>
					<string>Automator</string>
				</array>
				<key>arguments</key>
				<dict/>
				<key>isViewVisible</key>
				<integer>1</integer>
				<key>location</key>
				<string>300:300</string>
				<key>nibPath</key>
				<string>/System/Library/Automator/Run Shell Script.action/Contents/Resources/Base.lproj/main.nib</string>
			</dict>
			<key>isViewVisible</key>
			<integer>1</integer>
		</dict>
	</array>
	<key>connectors</key>
	<dict/>
	<key>workflowMetaData</key>
	<dict>
		<key>applicationBundleIDsByPath</key>
		<dict/>
		<key>applicationPaths</key>
		<array/>
		<key>inputTypeIdentifier</key>
		<string>com.apple.Automator.fileSystemObject</string>
		<key>outputTypeIdentifier</key>
		<string>com.apple.Automator.nothing</string>
		<key>presentationMode</key>
		<integer>15</integer>
		<key>processesInput</key>
		<false/>
		<key>serviceInputTypeIdentifier</key>
		<string>com.apple.Automator.fileSystemObject</string>
		<key>serviceOutputTypeIdentifier</key>
		<string>com.apple.Automator.nothing</string>
		<key>serviceProcessesInput</key>
		<false/>
		<key>systemImageName</key>
		<string>NSActionTemplate</string>
		<key>useAutomaticInputType</key>
		<false/>
		<key>workflowTypeIdentifier</key>
		<string>com.apple.Automator.servicesMenu</string>
	</dict>
</dict>
</plist>
"#,
            xml(&cmd)
        )
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use super::mac_files::*;
    use super::*;
    use anyhow::Result;

    pub fn install(exe: &Path, ts: &[Target]) -> Result<()> {
        let Some(home) = dirs::home_dir() else { return Ok(()) };
        let dir = home.join("Library/Services");
        std::fs::create_dir_all(&dir)?;
        let mut wanted = vec![];
        for t in ts {
            let label = if t.args[0] == "--shelf" { "Put on the OpenHop Shelf".to_string() } else { format!("Send to {} with OpenHop", t.label) };
            let name = format!("{}.workflow", label.replace(['/', ':'], "-"));
            let contents = dir.join(&name).join("Contents");
            std::fs::create_dir_all(&contents)?;
            std::fs::write(contents.join("Info.plist"), info(&label))?;
            std::fs::write(contents.join("document.wflow"), workflow(exe, t))?;
            wanted.push(name);
        }
        if let Ok(rd) = std::fs::read_dir(&dir) {
            for e in rd.flatten() {
                let n = e.file_name().to_string_lossy().into_owned();
                let ours = (n.starts_with("Send to ") && n.ends_with(" with OpenHop.workflow")) || n == "Put on the OpenHop Shelf.workflow";
                if ours && !wanted.contains(&n) {
                    let _ = std::fs::remove_dir_all(e.path());
                }
            }
        }
        // Let Finder see the new Quick Actions now.
        let _ = std::process::Command::new("/System/Library/CoreServices/pbs").arg("-update").output();
        Ok(())
    }
}

#[cfg(not(any(target_os = "linux", windows, target_os = "macos")))]
mod imp {
    use super::*;
    pub fn install(_: &Path, _: &[Target]) -> anyhow::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_entries() {
        let t = targets(&["Laptop".into(), "Mac's Desk".into()]);
        assert_eq!(t.iter().map(|t| t.label.as_str()).collect::<Vec<_>>(), ["Laptop", "Mac's Desk", "All Computers", "Put on the Shelf"]);
        assert_eq!(t[1].key, "mac-s-desk");
        assert_eq!(targets(&["A".into()]).len(), 2);
    }

    #[test]
    fn quoting() {
        let exe = Path::new("/opt/Open Hop/openhop-app");
        let t = &targets(&["Bob's \"PC\" $1".into()])[0];
        assert_eq!(command(exe, t, sh), r#"'/opt/Open Hop/openhop-app' '--send' 'Bob'\''s "PC" $1'"#);
        assert_eq!(command(exe, t, desk), r#""/opt/Open Hop/openhop-app" "--send" "Bob's \"PC\" \$1""#);
    }

    #[test]
    fn thunar_keeps_other_actions() {
        let exe = Path::new("/usr/bin/openhop-app");
        let mine = "<?xml version=\"1.0\"?>\n<actions>\n<action>\n\t<name>Open Terminal Here</name>\n\t<unique-id>1-1</unique-id>\n</action>\n</actions>\n";
        let once = linux_files::thunar(mine, exe, &targets(&["Laptop".into()]));
        assert!(once.contains("Open Terminal Here") && once.contains("Send to Laptop") && once.contains("Put on the Shelf"));
        let twice = linux_files::thunar(&once, exe, &targets(&["Desk".into()]));
        assert!(twice.contains("Open Terminal Here") && twice.contains("Send to Desk") && !twice.contains("Laptop"));
        assert_eq!(twice.matches("<unique-id>openhop-").count(), 2);
        assert!(linux_files::thunar("", exe, &targets(&[])).contains("<actions>"));
    }

    #[test]
    fn mac_workflow_is_plist() {
        let w = mac_files::workflow(Path::new("/Applications/OpenHop.app/Contents/MacOS/openhop-app"), &targets(&["A&B".into()])[0]);
        assert!(w.contains("&apos;A&amp;B&apos;") || w.contains("'A&amp;B'"));
        assert!(mac_files::info("Send to A&B").contains("A&amp;B"));
    }
}
