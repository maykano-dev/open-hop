//! Cross-platform key codes.
//!
//! Keys travel over the wire as USB HID usage IDs (keyboard page 0x07). Each
//! platform backend converts its native code to/from HID with the table below,
//! so a key pressed on any OS lands as the same physical key on any other OS.

/// Marker for "no equivalent on this platform".
pub const NONE: u16 = 0xFFFF;

/// (hid, linux evdev, windows set-1 scancode (0xE0xx = extended), macOS virtual keycode)
#[rustfmt::skip]
pub const TABLE: &[(u16, u16, u16, u16)] = &[
    // letters
    (0x04, 30, 0x1E, 0x00), // A
    (0x05, 48, 0x30, 0x0B), // B
    (0x06, 46, 0x2E, 0x08), // C
    (0x07, 32, 0x20, 0x02), // D
    (0x08, 18, 0x12, 0x0E), // E
    (0x09, 33, 0x21, 0x03), // F
    (0x0A, 34, 0x22, 0x05), // G
    (0x0B, 35, 0x23, 0x04), // H
    (0x0C, 23, 0x17, 0x22), // I
    (0x0D, 36, 0x24, 0x26), // J
    (0x0E, 37, 0x25, 0x28), // K
    (0x0F, 38, 0x26, 0x25), // L
    (0x10, 50, 0x32, 0x2E), // M
    (0x11, 49, 0x31, 0x2D), // N
    (0x12, 24, 0x18, 0x1F), // O
    (0x13, 25, 0x19, 0x23), // P
    (0x14, 16, 0x10, 0x0C), // Q
    (0x15, 19, 0x13, 0x0F), // R
    (0x16, 31, 0x1F, 0x01), // S
    (0x17, 20, 0x14, 0x11), // T
    (0x18, 22, 0x16, 0x20), // U
    (0x19, 47, 0x2F, 0x09), // V
    (0x1A, 17, 0x11, 0x0D), // W
    (0x1B, 45, 0x2D, 0x07), // X
    (0x1C, 21, 0x15, 0x10), // Y
    (0x1D, 44, 0x2C, 0x06), // Z
    // digits row
    (0x1E, 2, 0x02, 0x12),  // 1
    (0x1F, 3, 0x03, 0x13),  // 2
    (0x20, 4, 0x04, 0x14),  // 3
    (0x21, 5, 0x05, 0x15),  // 4
    (0x22, 6, 0x06, 0x17),  // 5
    (0x23, 7, 0x07, 0x16),  // 6
    (0x24, 8, 0x08, 0x1A),  // 7
    (0x25, 9, 0x09, 0x1C),  // 8
    (0x26, 10, 0x0A, 0x19), // 9
    (0x27, 11, 0x0B, 0x1D), // 0
    (0x28, 28, 0x1C, 0x24), // Enter
    (0x29, 1, 0x01, 0x35),  // Escape
    (0x2A, 14, 0x0E, 0x33), // Backspace
    (0x2B, 15, 0x0F, 0x30), // Tab
    (0x2C, 57, 0x39, 0x31), // Space
    (0x2D, 12, 0x0C, 0x1B), // Minus
    (0x2E, 13, 0x0D, 0x18), // Equal
    (0x2F, 26, 0x1A, 0x21), // [
    (0x30, 27, 0x1B, 0x1E), // ]
    (0x31, 43, 0x2B, 0x2A), // Backslash
    (0x33, 39, 0x27, 0x29), // ;
    (0x34, 40, 0x28, 0x27), // '
    (0x35, 41, 0x29, 0x32), // `
    (0x36, 51, 0x33, 0x2B), // ,
    (0x37, 52, 0x34, 0x2F), // .
    (0x38, 53, 0x35, 0x2C), // /
    (0x39, 58, 0x3A, 0x39), // Caps Lock
    // function keys
    (0x3A, 59, 0x3B, 0x7A), // F1
    (0x3B, 60, 0x3C, 0x78), // F2
    (0x3C, 61, 0x3D, 0x63), // F3
    (0x3D, 62, 0x3E, 0x76), // F4
    (0x3E, 63, 0x3F, 0x60), // F5
    (0x3F, 64, 0x40, 0x61), // F6
    (0x40, 65, 0x41, 0x62), // F7
    (0x41, 66, 0x42, 0x64), // F8
    (0x42, 67, 0x43, 0x65), // F9
    (0x43, 68, 0x44, 0x6D), // F10
    (0x44, 87, 0x57, 0x67), // F11
    (0x45, 88, 0x58, 0x6F), // F12
    (0x68, 183, 0x64, 0x69), // F13
    (0x69, 184, 0x65, 0x6B), // F14
    (0x6A, 185, 0x66, 0x71), // F15
    (0x6B, 186, 0x67, 0x6A), // F16
    (0x6C, 187, 0x68, 0x40), // F17
    (0x6D, 188, 0x69, 0x4F), // F18
    (0x6E, 189, 0x6A, 0x50), // F19
    (0x6F, 190, 0x6B, 0x5A), // F20
    (0x70, 191, 0x6C, NONE), // F21
    (0x71, 192, 0x6D, NONE), // F22
    (0x72, 193, 0x6E, NONE), // F23
    (0x73, 194, 0x76, NONE), // F24
    // navigation / editing
    (0x46, 99, 0xE037, NONE), // Print Screen
    (0x47, 70, 0x46, NONE),   // Scroll Lock
    (0x48, 119, 0x45, NONE),  // Pause
    (0x49, 110, 0xE052, 0x72), // Insert (Help on Mac)
    (0x4A, 102, 0xE047, 0x73), // Home
    (0x4B, 104, 0xE049, 0x74), // Page Up
    (0x4C, 111, 0xE053, 0x75), // Delete (forward)
    (0x4D, 107, 0xE04F, 0x77), // End
    (0x4E, 109, 0xE051, 0x79), // Page Down
    (0x4F, 106, 0xE04D, 0x7C), // Right
    (0x50, 105, 0xE04B, 0x7B), // Left
    (0x51, 108, 0xE050, 0x7D), // Down
    (0x52, 103, 0xE048, 0x7E), // Up
    // keypad
    (0x53, 69, 0xE045, 0x47), // Num Lock (Clear on Mac)
    (0x54, 98, 0xE035, 0x4B), // KP /
    (0x55, 55, 0x37, 0x43),   // KP *
    (0x56, 74, 0x4A, 0x4E),   // KP -
    (0x57, 78, 0x4E, 0x45),   // KP +
    (0x58, 96, 0xE01C, 0x4C), // KP Enter
    (0x59, 79, 0x4F, 0x53),   // KP 1
    (0x5A, 80, 0x50, 0x54),   // KP 2
    (0x5B, 81, 0x51, 0x55),   // KP 3
    (0x5C, 75, 0x4B, 0x56),   // KP 4
    (0x5D, 76, 0x4C, 0x57),   // KP 5
    (0x5E, 77, 0x4D, 0x58),   // KP 6
    (0x5F, 71, 0x47, 0x59),   // KP 7
    (0x60, 72, 0x48, 0x5B),   // KP 8
    (0x61, 73, 0x49, 0x5C),   // KP 9
    (0x62, 82, 0x52, 0x52),   // KP 0
    (0x63, 83, 0x53, 0x41),   // KP .
    (0x67, 117, 0x59, 0x51),  // KP =
    (0x64, 86, 0x56, 0x0A),   // Non-US backslash (ISO key)
    (0x65, 127, 0xE05D, 0x6E), // Menu / Application
    // media
    (0x7F, 113, 0xE020, 0x4A), // Mute
    (0x80, 115, 0xE030, 0x48), // Volume Up
    (0x81, 114, 0xE02E, 0x49), // Volume Down
    // modifiers
    (0xE0, 29, 0x1D, 0x3B),   // Left Ctrl
    (0xE1, 42, 0x2A, 0x38),   // Left Shift
    (0xE2, 56, 0x38, 0x3A),   // Left Alt / Option
    (0xE3, 125, 0xE05B, 0x37), // Left Meta / Win / Cmd
    (0xE4, 97, 0xE01D, 0x3E), // Right Ctrl
    (0xE5, 54, 0x36, 0x3C),   // Right Shift
    (0xE6, 100, 0xE038, 0x3D), // Right Alt / Option
    (0xE7, 126, 0xE05C, 0x36), // Right Meta / Win / Cmd
];

pub const HID_LCTRL: u16 = 0xE0;
pub const HID_LMETA: u16 = 0xE3;
pub const HID_RCTRL: u16 = 0xE4;
pub const HID_RMETA: u16 = 0xE7;
pub const HID_CAPS: u16 = 0x39;

fn lookup(f: impl Fn(&(u16, u16, u16, u16)) -> bool) -> Option<&'static (u16, u16, u16, u16)> {
    TABLE.iter().find(|r| f(r))
}

pub fn hid_from_evdev(code: u16) -> Option<u16> {
    lookup(|r| r.1 == code).map(|r| r.0)
}
pub fn evdev_from_hid(hid: u16) -> Option<u16> {
    lookup(|r| r.0 == hid).map(|r| r.1).filter(|&c| c != NONE)
}
pub fn hid_from_win(scan: u16) -> Option<u16> {
    lookup(|r| r.2 == scan).map(|r| r.0)
}
pub fn win_from_hid(hid: u16) -> Option<u16> {
    lookup(|r| r.0 == hid).map(|r| r.2).filter(|&c| c != NONE)
}
pub fn hid_from_mac(code: u16) -> Option<u16> {
    lookup(|r| r.3 == code).map(|r| r.0)
}
pub fn mac_from_hid(hid: u16) -> Option<u16> {
    lookup(|r| r.0 == hid).map(|r| r.3).filter(|&c| c != NONE)
}

pub fn is_modifier(hid: u16) -> bool {
    (0xE0..=0xE7).contains(&hid)
}

/// Swap Ctrl <-> Cmd/Win so shortcuts feel native when crossing between
/// macOS and Windows/Linux (Cmd+C on the Mac becomes Ctrl+C on the PC).
pub fn swap_ctrl_meta(hid: u16) -> u16 {
    match hid {
        HID_LCTRL => HID_LMETA,
        HID_LMETA => HID_LCTRL,
        HID_RCTRL => HID_RMETA,
        HID_RMETA => HID_RCTRL,
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn hid_codes_unique() {
        let mut seen = HashSet::new();
        for r in TABLE {
            assert!(seen.insert(r.0), "duplicate hid {:#x}", r.0);
        }
    }

    #[test]
    fn native_codes_unique() {
        for col in 1..4 {
            let mut seen = HashSet::new();
            for r in TABLE {
                let c = [r.0, r.1, r.2, r.3][col];
                if c != NONE {
                    assert!(seen.insert(c), "duplicate code {c:#x} in column {col}");
                }
            }
        }
    }

    #[test]
    fn roundtrips() {
        for r in TABLE {
            assert_eq!(hid_from_evdev(evdev_from_hid(r.0).unwrap()), Some(r.0));
            assert_eq!(hid_from_win(win_from_hid(r.0).unwrap()), Some(r.0));
            if let Some(m) = mac_from_hid(r.0) {
                assert_eq!(hid_from_mac(m), Some(r.0));
            }
        }
    }

    #[test]
    fn swap() {
        assert_eq!(swap_ctrl_meta(HID_LCTRL), HID_LMETA);
        assert_eq!(swap_ctrl_meta(0x06), 0x06);
    }
}
