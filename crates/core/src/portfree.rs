//! Find which program is listening on a TCP port, and close it if it's an
//! older copy of OpenHop (so the port is taken over automatically).

/// Program names OpenHop runs as.
const NAMES: [&str; 4] = ["openhop", "openhop-app", "openhop.exe", "openhop-app.exe"];

fn is_openhop(name: &str) -> bool {
    let n = name.trim().to_ascii_lowercase();
    let n = n.rsplit(['/', '\\']).next().unwrap_or(&n).to_string();
    NAMES.contains(&n.as_str())
}

/// Close the older OpenHop listening on `port`, if that's who has it.
/// Returns true if one was closed.
pub fn close_old_openhop(port: u16) -> bool {
    let me = std::process::id();
    let owners: Vec<(u32, String)> = listeners(port).into_iter().filter(|(pid, name)| *pid != me && is_openhop(name)).collect();
    for (pid, _) in &owners {
        kill(*pid);
    }
    if !owners.is_empty() {
        std::thread::sleep(std::time::Duration::from_millis(600));
    }
    !owners.is_empty()
}

/// (pid, program name) of processes listening on the TCP port.
#[cfg(target_os = "linux")]
fn listeners(port: u16) -> Vec<(u32, String)> {
    // Socket inodes listening on the port (state 0A = LISTEN).
    let mut inodes = std::collections::HashSet::new();
    for f in ["/proc/net/tcp", "/proc/net/tcp6"] {
        let Ok(text) = std::fs::read_to_string(f) else {
            continue;
        };
        for line in text.lines().skip(1) {
            let cols: Vec<&str> = line.split_whitespace().collect();
            if cols.len() < 10 || cols[3] != "0A" {
                continue;
            }
            let p = cols[1].rsplit(':').next().and_then(|h| u16::from_str_radix(h, 16).ok());
            if p == Some(port) {
                inodes.insert(format!("socket:[{}]", cols[9]));
            }
        }
    }
    if inodes.is_empty() {
        return vec![];
    }
    let mut out = vec![];
    let Ok(dir) = std::fs::read_dir("/proc") else {
        return out;
    };
    for e in dir.flatten() {
        let Some(pid) = e.file_name().to_str().and_then(|s| s.parse::<u32>().ok()) else {
            continue;
        };
        let Ok(fds) = std::fs::read_dir(e.path().join("fd")) else {
            continue;
        };
        let owns = fds.flatten().any(|fd| std::fs::read_link(fd.path()).map(|l| inodes.contains(&l.to_string_lossy().into_owned())).unwrap_or(false));
        if owns {
            let name = std::fs::read_to_string(e.path().join("comm")).unwrap_or_default();
            out.push((pid, name.trim().to_string()));
        }
    }
    out
}

#[cfg(target_os = "macos")]
fn listeners(port: u16) -> Vec<(u32, String)> {
    let Ok(o) = std::process::Command::new("lsof").args(["-nP", &format!("-iTCP:{port}"), "-sTCP:LISTEN", "-t"]).output() else {
        return vec![];
    };
    String::from_utf8_lossy(&o.stdout)
        .lines()
        .filter_map(|l| l.trim().parse::<u32>().ok())
        .map(|pid| {
            let name = std::process::Command::new("ps")
                .args(["-p", &pid.to_string(), "-o", "comm="])
                .output()
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                .unwrap_or_default();
            (pid, name)
        })
        .collect()
}

#[cfg(windows)]
fn listeners(port: u16) -> Vec<(u32, String)> {
    use std::os::windows::process::CommandExt;
    const NO_WINDOW: u32 = 0x08000000;
    let Ok(o) = std::process::Command::new("netstat").args(["-ano", "-p", "TCP"]).creation_flags(NO_WINDOW).output() else {
        return vec![];
    };
    let mut pids: Vec<u32> = String::from_utf8_lossy(&o.stdout)
        .lines()
        .filter_map(|l| {
            let c: Vec<&str> = l.split_whitespace().collect();
            (c.len() >= 5 && c[3].eq_ignore_ascii_case("LISTENING") && c[1].rsplit(':').next() == Some(&port.to_string())).then(|| c[4].parse().ok()).flatten()
        })
        .collect();
    pids.sort();
    pids.dedup();
    pids.into_iter()
        .map(|pid| {
            let name = std::process::Command::new("tasklist")
                .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
                .creation_flags(NO_WINDOW)
                .output()
                .ok()
                .and_then(|o| String::from_utf8_lossy(&o.stdout).lines().next().and_then(|l| l.split(',').next()).map(|s| s.trim_matches('"').to_string()))
                .unwrap_or_default();
            (pid, name)
        })
        .collect()
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
fn listeners(_: u16) -> Vec<(u32, String)> {
    vec![]
}

fn kill(pid: u32) {
    log::info!("closing an older OpenHop (pid {pid}) that holds the network port");
    #[cfg(unix)]
    let _ = std::process::Command::new("kill").arg(pid.to_string()).status();
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let _ = std::process::Command::new("taskkill").args(["/PID", &pid.to_string(), "/F"]).creation_flags(0x08000000).status();
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn names() {
        assert!(super::is_openhop("openhop-app"));
        assert!(super::is_openhop("C:\\Program Files\\OpenHop\\openhop-app.exe"));
        assert!(!super::is_openhop("nginx"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn finds_listener() {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let found = super::listeners(port);
        assert!(found.iter().any(|(pid, _)| *pid == std::process::id()), "{found:?}");
        // We're not an older OpenHop: nothing is closed.
        assert!(!super::close_old_openhop(port));
    }
}
