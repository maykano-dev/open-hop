#![cfg_attr(not(target_os = "linux"), allow(dead_code))]
//! Notifications in one place: what pops up on this computer is shown on
//! the one you're using (when that's another one).
//!
//! Linux: watches the desktop's notification messages. Windows only lets
//! packaged (Store) apps read other apps' notifications, and macOS doesn't
//! let any app, so there it's Linux computers that pass theirs on.

/// A notification seen here.
#[derive(Debug, Clone, PartialEq)]
pub struct Seen {
    pub app: String,
    pub title: String,
    pub body: String,
}

/// Watch notifications; `on` gets each one (on a thread of its own).
pub fn watch(on: impl Fn(Seen) + Send + 'static) {
    #[cfg(target_os = "linux")]
    {
        let _ = std::thread::Builder::new().name("notifications".into()).spawn(move || {
            if let Err(e) = linux::run(&on) {
                log::info!("can't see this computer's notifications: {e}");
            }
        });
    }
    #[cfg(not(target_os = "linux"))]
    let _ = on;
}

/// Shorten and clean a notification's text (some carry markup).
pub fn clean(s: &str, max: usize) -> String {
    let mut out = String::new();
    let mut tag = false;
    for c in s.chars() {
        match c {
            '<' => tag = true,
            '>' if tag => tag = false,
            c if !tag => out.push(if c.is_control() { ' ' } else { c }),
            _ => {}
        }
    }
    let out = out.replace("&amp;", "&").replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"");
    let t: String = out.split_whitespace().collect::<Vec<_>>().join(" ");
    if t.chars().count() > max {
        t.chars().take(max - 1).collect::<String>() + "…"
    } else {
        t
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use super::Seen;
    use zbus::blocking::{Connection, MessageIterator};
    use zbus::message::Type;

    pub fn run(on: &dyn Fn(Seen)) -> zbus::Result<()> {
        let conn = Connection::session()?;
        // Become a monitor: see Notify calls going to the notification server.
        let rules = vec!["type='method_call',interface='org.freedesktop.Notifications',member='Notify'"];
        conn.call_method(Some("org.freedesktop.DBus"), "/org/freedesktop/DBus", Some("org.freedesktop.DBus.Monitoring"), "BecomeMonitor", &(rules, 0u32))?;
        log::info!("passing this computer's notifications on to the one in use");
        for msg in MessageIterator::from(conn) {
            let Ok(msg) = msg else { continue };
            if msg.message_type() != Type::MethodCall {
                continue;
            }
            let h = msg.header();
            if h.member().map(|m| m.as_str() != "Notify").unwrap_or(true) {
                continue;
            }
            type Args<'a> = (String, u32, String, String, String, Vec<String>, std::collections::HashMap<String, zbus::zvariant::Value<'a>>, i32);
            let body = msg.body();
            let Ok((app, _, _, summary, text, _, _, _)) = body.deserialize::<Args>() else { continue };
            if app.eq_ignore_ascii_case("openhop") || summary.trim().is_empty() {
                continue;
            }
            on(Seen { app: super::clean(&app, 40), title: super::clean(&summary, 80), body: super::clean(&text, 160) });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn cleans_markup() {
        assert_eq!(super::clean("<b>Hi</b> there &amp; \n you", 100), "Hi there & you");
        assert_eq!(super::clean("abcdefghij", 5), "abcd…");
    }
}
