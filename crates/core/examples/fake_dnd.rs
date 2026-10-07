//! Test helper: behaves like a file manager in the middle of a drag. Owns
//! the X11 `XdndSelection` and serves the given files as `text/uri-list`,
//! until it receives Escape (a cancelled drag) or 60 s pass.
#[cfg(target_os = "linux")]
mod imp {
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::*;
    use x11rb::protocol::Event;
    use x11rb::wrapper::ConnectionExt as _;
    use x11rb::CURRENT_TIME;

    pub fn main() {
        let files: Vec<String> = std::env::args().skip(1).collect();
        let uris: String = files.iter().map(|f| format!("file://{}\r\n", f.replace(' ', "%20"))).collect();
        let (conn, n) = x11rb::connect(None).unwrap();
        let root = conn.setup().roots[n].root;
        let atom = |s: &[u8]| conn.intern_atom(false, s).unwrap().reply().unwrap().atom;
        let (xdnd, uri_list, targets) = (atom(b"XdndSelection"), atom(b"text/uri-list"), atom(b"TARGETS"));
        let win = conn.generate_id().unwrap();
        conn.create_window(0, win, root, 0, 0, 1, 1, 0, WindowClass::INPUT_OUTPUT, 0, &CreateWindowAux::new()).unwrap();
        conn.set_selection_owner(win, xdnd, CURRENT_TIME).unwrap();
        // A real drag source grabs the keyboard so Escape cancels the drag.
        let _ = conn.grab_keyboard(false, root, CURRENT_TIME, GrabMode::ASYNC, GrabMode::ASYNC).unwrap().reply();
        conn.flush().unwrap();
        eprintln!("fake drag of {} file(s) started", files.len());
        let start = std::time::Instant::now();
        while start.elapsed().as_secs() < 60 {
            let Some(ev) = conn.poll_for_event().unwrap() else {
                std::thread::sleep(std::time::Duration::from_millis(5));
                continue;
            };
            match ev {
                Event::SelectionRequest(r) => {
                    let mut prop = r.property;
                    if r.target == uri_list {
                        conn.change_property8(PropMode::REPLACE, r.requestor, r.property, uri_list, uris.as_bytes()).unwrap();
                    } else if r.target == targets {
                        conn.change_property32(PropMode::REPLACE, r.requestor, r.property, AtomEnum::ATOM, &[uri_list]).unwrap();
                    } else {
                        prop = x11rb::NONE;
                    }
                    let notify = SelectionNotifyEvent {
                        response_type: SELECTION_NOTIFY_EVENT,
                        sequence: 0,
                        time: r.time,
                        requestor: r.requestor,
                        selection: r.selection,
                        target: r.target,
                        property: prop,
                    };
                    conn.send_event(false, r.requestor, EventMask::NO_EVENT, notify).unwrap();
                    conn.flush().unwrap();
                    eprintln!("served the file list");
                }
                Event::KeyPress(k) if k.detail == 9 => {
                    eprintln!("drag cancelled with Escape");
                    let _ = conn.ungrab_keyboard(CURRENT_TIME);
                    conn.flush().unwrap();
                    break;
                }
                _ => {}
            }
        }
    }
}

#[cfg(target_os = "linux")]
fn main() {
    imp::main()
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("fake_dnd is a Linux (X11) test helper");
}
