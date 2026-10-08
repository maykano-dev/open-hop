//! Test helper: input as if from a separate physical device (an XTEST device
//! attached to the core pointer/keyboard), so XInput 2 code sees "real" input.
//! physin rel DX DY | click BUTTON | key KEYCODE | text STRING
#[cfg(target_os = "linux")]
fn main() {
    use x11rb::connection::Connection;
    use x11rb::protocol::xinput::ConnectionExt as _;
    use x11rb::protocol::xtest::ConnectionExt as _;
    let args: Vec<String> = std::env::args().collect();
    let (conn, n) = x11rb::connect(None).unwrap();
    let root = conn.setup().roots[n].root;
    let devs = conn.xinput_xi_query_device(0u16).unwrap().reply().unwrap().infos;
    let find = |s: &str| devs.iter().find(|d| String::from_utf8_lossy(&d.name) == s).map(|d| d.deviceid as u8).expect(s);
    let (ptr, kbd) = (find("Xvfb mouse"), find("Xvfb keyboard"));
    let fake = |t: u8, detail: u8, x: i16, y: i16, dev: u8| {
        conn.xtest_fake_input(t, detail, 0, root, x, y, dev).unwrap();
        conn.flush().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(12));
    };
    match args[1].as_str() {
        "rel" => fake(6, 1, args[2].parse().unwrap(), args[3].parse().unwrap(), ptr),
        "click" => {
            let b: u8 = args[2].parse().unwrap();
            fake(4, b, 0, 0, ptr);
            fake(5, b, 0, 0, ptr);
        }
        "down" => fake(4, args[2].parse().unwrap(), 0, 0, ptr),
        "up" => fake(5, args[2].parse().unwrap(), 0, 0, ptr),
        "key" => {
            let k: u8 = args[2].parse().unwrap();
            fake(2, k, 0, 0, kbd);
            fake(3, k, 0, 0, kbd);
        }
        "text" => {
            // US layout keycodes for a-z and space.
            let row = "qwertyuiop";
            let home = "asdfghjkl";
            let low = "zxcvbnm";
            for c in args[2].chars() {
                let k: u8 = if c == ' ' {
                    65
                } else if let Some(i) = row.find(c) {
                    24 + i as u8
                } else if let Some(i) = home.find(c) {
                    38 + i as u8
                } else if let Some(i) = low.find(c) {
                    52 + i as u8
                } else {
                    continue;
                };
                fake(2, k, 0, 0, kbd);
                fake(3, k, 0, 0, kbd);
                std::thread::sleep(std::time::Duration::from_millis(60));
            }
        }
        _ => panic!("?"),
    }
}
#[cfg(not(target_os = "linux"))]
fn main() {}
