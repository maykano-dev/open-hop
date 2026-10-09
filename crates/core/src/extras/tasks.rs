//! The apps running on this computer, like a task manager: those with
//! windows (in the dock or taskbar) and those running in the background,
//! with their memory, and quitting one.

use super::apps::exe_key;
use crate::protocol::{AppEntry, RunningApp, WinInfo};
use std::collections::{BTreeMap, HashMap, HashSet};
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

pub struct Tasks {
    sys: System,
}

/// Programs that are part of the system, not apps you'd want to see.
const SKIP: &[&str] = &[
    "openhop-app",
    "openhop",
    "gnome-shell",
    "plasmashell",
    "explorer",
    "dwm",
    "finder",
    "dock",
    "systemuiserver",
    "loginwindow",
    "xdg-desktop-portal",
    "nautilus-desktop",
];

impl Default for Tasks {
    fn default() -> Self {
        Self::new()
    }
}

impl Tasks {
    pub fn new() -> Self {
        Tasks { sys: System::new() }
    }

    fn refresh(&mut self) {
        let kind = ProcessRefreshKind::nothing().with_memory().with_exe(UpdateKind::OnlyIfNotSet).with_user(UpdateKind::OnlyIfNotSet).without_tasks();
        self.sys.refresh_processes_specifics(ProcessesToUpdate::All, true, kind);
    }

    /// What's running now, given this computer's windows and installed apps.
    pub fn list(&mut self, windows: &[WinInfo], installed: &[AppEntry]) -> Vec<RunningApp> {
        self.refresh();
        let me = sysinfo::get_current_pid().ok().and_then(|p| self.sys.process(p)).and_then(|p| p.user_id().cloned());
        let procs = self.sys.processes();
        // Children of each process (a browser's tabs, an editor's helpers).
        let mut kids: HashMap<Pid, Vec<Pid>> = HashMap::new();
        for (pid, p) in procs {
            if let Some(parent) = p.parent() {
                kids.entry(parent).or_default().push(*pid);
            }
        }
        let tree_memory = |root: Pid, seen: &mut HashSet<Pid>| -> u64 {
            let mut total = 0;
            let mut stack = vec![root];
            while let Some(p) = stack.pop() {
                if !seen.insert(p) {
                    continue;
                }
                total += procs.get(&p).map(|x| x.memory()).unwrap_or(0);
                if let Some(k) = kids.get(&p) {
                    stack.extend(k.iter().copied());
                }
            }
            total
        };
        let key_of =
            |p: &sysinfo::Process| -> String { p.exe().map(|e| exe_key(&e.to_string_lossy())).unwrap_or_else(|| exe_key(&p.name().to_string_lossy())) };
        let by_exe: HashMap<&str, &AppEntry> = installed.iter().filter(|a| !a.exe.is_empty()).map(|a| (a.exe.as_str(), a)).collect();
        let by_name: HashMap<String, &AppEntry> = installed.iter().map(|a| (a.name.to_lowercase(), a)).collect();
        // A macOS app's processes live inside its bundle.
        let in_bundle = |p: &sysinfo::Process| -> Option<&AppEntry> {
            let exe = p.exe()?.to_string_lossy().into_owned();
            installed.iter().find(|a| a.id.ends_with(".app") && exe.starts_with(&format!("{}/", a.id)))
        };
        let app_for = |p: &sysinfo::Process| -> Option<&AppEntry> {
            let k = key_of(p);
            by_exe.get(k.as_str()).copied().or_else(|| by_name.get(&k).copied()).or_else(|| in_bundle(p))
        };

        let mut seen: HashSet<Pid> = HashSet::new();
        let mut out: BTreeMap<String, RunningApp> = BTreeMap::new();

        // With windows: grouped by the process that owns them.
        let mut wins_of: BTreeMap<u32, Vec<&WinInfo>> = BTreeMap::new();
        for w in windows {
            wins_of.entry(w.pid).or_default().push(w);
        }
        for (pid, ws) in wins_of {
            let p = procs.get(&Pid::from_u32(pid));
            let app = p.and_then(app_for);
            let name = app.map(|a| a.name.clone()).unwrap_or_else(|| ws[0].app.clone());
            if name.is_empty() || SKIP.contains(&name.to_lowercase().as_str()) {
                continue;
            }
            let memory = if pid != 0 { tree_memory(Pid::from_u32(pid), &mut seen) } else { 0 };
            let e = out.entry(name.clone()).or_insert_with(|| RunningApp { name, pids: vec![], windows: vec![], memory: 0, app_id: app.map(|a| a.id.clone()) });
            if pid != 0 && !e.pids.contains(&pid) {
                e.pids.push(pid);
            }
            e.windows.extend(ws.iter().map(|w| w.id));
            e.memory += memory;
        }

        // In the background: your own processes that belong to an installed
        // app and have no window (the top one of each group).
        let mut bg: Vec<(Pid, &AppEntry)> = vec![];
        for (pid, p) in procs {
            if seen.contains(pid) || (me.is_some() && p.user_id() != me.as_ref()) {
                continue;
            }
            let Some(app) = app_for(p) else { continue };
            if SKIP.contains(&app.exe.as_str()) || SKIP.contains(&app.name.to_lowercase().as_str()) {
                continue;
            }
            // Only the outermost process of the app.
            let parent_same = p.parent().and_then(|pp| procs.get(&pp)).and_then(app_for).map(|a| a.id == app.id).unwrap_or(false);
            if !parent_same {
                bg.push((*pid, app));
            }
        }
        for (pid, app) in bg {
            if seen.contains(&pid) {
                continue;
            }
            let memory = tree_memory(pid, &mut seen);
            let e = out.entry(app.name.clone()).or_insert_with(|| RunningApp {
                name: app.name.clone(),
                pids: vec![],
                windows: vec![],
                memory: 0,
                app_id: Some(app.id.clone()),
            });
            e.pids.push(pid.as_u32());
            e.memory += memory;
        }
        let mut v: Vec<RunningApp> = out.into_values().collect();
        // Open apps first, then by memory.
        v.sort_by(|a, b| b.windows.is_empty().cmp(&a.windows.is_empty()).reverse().then(b.memory.cmp(&a.memory)));
        v
    }

    /// Quit these processes. Politely unless `force`.
    pub fn quit(&mut self, pids: &[u32], force: bool) {
        for &pid in pids {
            log::info!("quitting {pid}{}", if force { " (forced)" } else { "" });
            imp::quit(pid, force, &self.sys);
        }
    }
}

#[cfg(unix)]
mod imp {
    use super::*;
    pub fn quit(pid: u32, force: bool, sys: &System) {
        #[cfg(target_os = "macos")]
        if !force {
            // Ask the app to quit the way the Dock does (it can save first).
            if let Some(p) = sys.process(Pid::from_u32(pid)) {
                if let Some(exe) = p.exe() {
                    let s = exe.to_string_lossy();
                    if let Some(i) = s.find(".app/") {
                        let bundle = &s[..i + 4];
                        let script = format!("tell application \"{}\" to quit", bundle.replace('"', ""));
                        if std::process::Command::new("osascript").args(["-e", &script]).status().map(|s| s.success()).unwrap_or(false) {
                            return;
                        }
                    }
                }
            }
        }
        let _ = sys;
        unsafe {
            libc::kill(pid as i32, if force { libc::SIGKILL } else { libc::SIGTERM });
        }
    }
}

#[cfg(windows)]
mod imp {
    use super::*;
    pub fn quit(pid: u32, force: bool, _: &System) {
        use std::os::windows::process::CommandExt;
        let pid = pid.to_string();
        let mut args = vec!["/PID", pid.as_str(), "/T"];
        if force {
            args.push("/F");
        }
        // Without /F, apps get asked to close their windows (and can save).
        let _ = std::process::Command::new("taskkill").args(&args).creation_flags(0x08000000).status();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_a_background_app() {
        // `sleep` running as an "installed app" with no window.
        let mut child = std::process::Command::new(if cfg!(windows) { "ping" } else { "sleep" }).arg(if cfg!(windows) { "-n" } else { "30" }).spawn().unwrap();
        let installed = vec![AppEntry { id: "x".into(), name: "Sleeper".into(), exe: if cfg!(windows) { "ping".into() } else { "sleep".into() } }];
        let mut t = Tasks::new();
        let list = t.list(&[], &installed);
        let me = list.iter().find(|a| a.name == "Sleeper").expect("listed");
        assert!(me.windows.is_empty() && me.pids.contains(&child.id()));
        // With a window it's an open app.
        let w = WinInfo { id: 7, title: "Zzz".into(), app: "sleep".into(), w: 1, h: 1, pid: child.id() };
        let list = t.list(&[w], &installed);
        let me = list.iter().find(|a| a.name == "Sleeper").expect("listed");
        assert_eq!(me.windows, vec![7]);
        let _ = child.kill();
    }
}
