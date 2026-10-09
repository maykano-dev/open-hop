//! Keeping this computer awake while it's being used from elsewhere: a live
//! window or screen shown on another computer, a file on its way, a phone
//! connected, someone helping remotely. Nothing else: when it's idle and
//! nobody needs it, it sleeps on its own schedule (and the others don't
//! follow it to sleep).

pub struct KeepAwake {
    imp: Option<imp::Hold>,
    wanted: bool,
}

impl Default for KeepAwake {
    fn default() -> Self {
        Self::new()
    }
}

impl KeepAwake {
    pub fn new() -> Self {
        KeepAwake { imp: None, wanted: false }
    }

    pub fn on(&self) -> bool {
        self.imp.is_some()
    }

    /// Hold (or let go of) the computer's sleep and screen-off timers.
    pub fn set(&mut self, on: bool, why: &str) {
        if on == self.wanted {
            return;
        }
        self.wanted = on;
        if on {
            self.imp = imp::Hold::take(why);
            log::info!("keeping this computer awake ({why}){}", if self.imp.is_none() { ": not possible here" } else { "" });
        } else {
            self.imp = None;
            log::info!("this computer can sleep again");
        }
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use zbus::blocking::Connection;
    use zbus::zvariant::OwnedFd;

    /// logind's inhibitor (system sleep) and the desktop's (screen blanking).
    pub struct Hold {
        _fd: Option<OwnedFd>,
        gnome: Option<(Connection, u32)>,
        portal: Option<Connection>,
    }

    impl Hold {
        pub fn take(why: &str) -> Option<Hold> {
            let fd = Connection::system().ok().and_then(|c| {
                c.call_method(Some("org.freedesktop.login1"), "/org/freedesktop/login1", Some("org.freedesktop.login1.Manager"), "Inhibit", &("sleep:idle", "OpenHop", why, "block"))
                    .ok()
                    .and_then(|r| r.body().deserialize::<OwnedFd>().ok())
            });
            // GNOME (and others implementing it) blank the screen on their own clock.
            let gnome = Connection::session().ok().and_then(|c| {
                let r = c
                    .call_method(Some("org.gnome.SessionManager"), "/org/gnome/SessionManager", Some("org.gnome.SessionManager"), "Inhibit", &("openhop", 0u32, why, 4u32 | 8u32))
                    .ok()?;
                let cookie: u32 = r.body().deserialize().ok()?;
                Some((c, cookie))
            });
            // KDE and others: the freedesktop screensaver interface (held while
            // the connection stays open).
            let portal = if gnome.is_none() {
                Connection::session().ok().filter(|c| {
                    c.call_method(Some("org.freedesktop.ScreenSaver"), "/org/freedesktop/ScreenSaver", Some("org.freedesktop.ScreenSaver"), "Inhibit", &("OpenHop", why)).is_ok()
                })
            } else {
                None
            };
            if fd.is_none() && gnome.is_none() && portal.is_none() {
                return None;
            }
            Some(Hold { _fd: fd, gnome, portal })
        }
    }

    impl Drop for Hold {
        fn drop(&mut self) {
            if let Some((c, cookie)) = &self.gnome {
                let _ = c.call_method(Some("org.gnome.SessionManager"), "/org/gnome/SessionManager", Some("org.gnome.SessionManager"), "Uninhibit", &(*cookie,));
            }
            let _ = &self.portal;
        }
    }
}

#[cfg(windows)]
mod imp {
    use windows::Win32::System::Power::{SetThreadExecutionState, ES_CONTINUOUS, ES_DISPLAY_REQUIRED, ES_SYSTEM_REQUIRED};

    /// The setting belongs to a thread, so a thread holds it.
    pub struct Hold {
        stop: std::sync::mpsc::Sender<()>,
    }

    impl Hold {
        pub fn take(_: &str) -> Option<Hold> {
            let (tx, rx) = std::sync::mpsc::channel::<()>();
            std::thread::Builder::new()
                .name("keep-awake".into())
                .spawn(move || {
                    unsafe { SetThreadExecutionState(ES_CONTINUOUS | ES_SYSTEM_REQUIRED | ES_DISPLAY_REQUIRED) };
                    let _ = rx.recv();
                    unsafe { SetThreadExecutionState(ES_CONTINUOUS) };
                })
                .ok()?;
            Some(Hold { stop: tx })
        }
    }

    impl Drop for Hold {
        fn drop(&mut self) {
            let _ = self.stop.send(());
        }
    }
}

#[cfg(target_os = "macos")]
mod imp {
    /// `caffeinate`, tied to OpenHop's life (it stops by itself if OpenHop goes).
    pub struct Hold {
        child: std::process::Child,
    }

    impl Hold {
        pub fn take(_: &str) -> Option<Hold> {
            let pid = std::process::id().to_string();
            let child = std::process::Command::new("/usr/bin/caffeinate").args(["-d", "-i", "-w", &pid]).spawn().ok()?;
            Some(Hold { child })
        }
    }

    impl Drop for Hold {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

#[cfg(not(any(target_os = "linux", windows, target_os = "macos")))]
mod imp {
    pub struct Hold;
    impl Hold {
        pub fn take(_: &str) -> Option<Hold> {
            None
        }
    }
}
