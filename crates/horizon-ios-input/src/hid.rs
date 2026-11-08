//! Keys by their USB HID usage (the keyboard page), the numbers `GCKeyCode` uses.
use winit::keyboard::{KeyCode, NamedKey};

const LETTERS: [KeyCode; 26] = {
    use KeyCode::*;
    [
        KeyA, KeyB, KeyC, KeyD, KeyE, KeyF, KeyG, KeyH, KeyI, KeyJ, KeyK, KeyL, KeyM, KeyN, KeyO,
        KeyP, KeyQ, KeyR, KeyS, KeyT, KeyU, KeyV, KeyW, KeyX, KeyY, KeyZ,
    ]
};

/// The physical key a HID usage stands for, by its place on a US keyboard.
pub fn key_code(usage: u16) -> Option<KeyCode> {
    use KeyCode::*;
    const DIGITS: [KeyCode; 10] = [
        Digit1, Digit2, Digit3, Digit4, Digit5, Digit6, Digit7, Digit8, Digit9, Digit0,
    ];
    const F1_F12: [KeyCode; 12] = [F1, F2, F3, F4, F5, F6, F7, F8, F9, F10, F11, F12];
    const F13_F24: [KeyCode; 12] = [F13, F14, F15, F16, F17, F18, F19, F20, F21, F22, F23, F24];
    const NUMPAD: [KeyCode; 10] = [
        Numpad1, Numpad2, Numpad3, Numpad4, Numpad5, Numpad6, Numpad7, Numpad8, Numpad9, Numpad0,
    ];
    let at = |table: &[KeyCode], first: u16| table.get(usize::from(usage - first)).copied();
    Some(match usage {
        0x04..=0x1d => return at(&LETTERS, 0x04),
        0x1e..=0x27 => return at(&DIGITS, 0x1e),
        0x28 => Enter,
        0x29 => Escape,
        0x2a => Backspace,
        0x2b => Tab,
        0x2c => Space,
        0x2d => Minus,
        0x2e => Equal,
        0x2f => BracketLeft,
        0x30 => BracketRight,
        // 0x32 is the ISO keyboard's key beside Return, where the US one has Backslash.
        0x31 | 0x32 => Backslash,
        0x33 => Semicolon,
        0x34 => Quote,
        0x35 => Backquote,
        0x36 => Comma,
        0x37 => Period,
        0x38 => Slash,
        0x39 => CapsLock,
        0x3a..=0x45 => return at(&F1_F12, 0x3a),
        0x46 => PrintScreen,
        0x47 => ScrollLock,
        0x48 => Pause,
        0x49 => Insert,
        0x4a => Home,
        0x4b => PageUp,
        0x4c => Delete,
        0x4d => End,
        0x4e => PageDown,
        0x4f => ArrowRight,
        0x50 => ArrowLeft,
        0x51 => ArrowDown,
        0x52 => ArrowUp,
        0x53 => NumLock,
        0x54 => NumpadDivide,
        0x55 => NumpadMultiply,
        0x56 => NumpadSubtract,
        0x57 => NumpadAdd,
        0x58 => NumpadEnter,
        0x59..=0x62 => return at(&NUMPAD, 0x59),
        0x63 => NumpadDecimal,
        0x64 => IntlBackslash,
        0x65 => ContextMenu,
        0x67 => NumpadEqual,
        0x68..=0x73 => return at(&F13_F24, 0x68),
        0x85 => NumpadComma,
        0x87 => IntlRo,
        0x88 => KanaMode,
        0x89 => IntlYen,
        0x8a => Convert,
        0x8b => NonConvert,
        0x90 => Lang1,
        0x91 => Lang2,
        0xe0 => ControlLeft,
        0xe1 => ShiftLeft,
        0xe2 => AltLeft,
        0xe3 => SuperLeft,
        0xe4 => ControlRight,
        0xe5 => ShiftRight,
        0xe6 => AltRight,
        0xe7 => SuperRight,
        _ => return None,
    })
}

/// The named key a physical key is, when it types no character.
pub fn named_key(code: KeyCode) -> Option<NamedKey> {
    use KeyCode::*;
    Some(match code {
        Enter | NumpadEnter => NamedKey::Enter,
        Escape => NamedKey::Escape,
        Backspace => NamedKey::Backspace,
        Tab => NamedKey::Tab,
        Space => NamedKey::Space,
        CapsLock => NamedKey::CapsLock,
        F1 => NamedKey::F1,
        F2 => NamedKey::F2,
        F3 => NamedKey::F3,
        F4 => NamedKey::F4,
        F5 => NamedKey::F5,
        F6 => NamedKey::F6,
        F7 => NamedKey::F7,
        F8 => NamedKey::F8,
        F9 => NamedKey::F9,
        F10 => NamedKey::F10,
        F11 => NamedKey::F11,
        F12 => NamedKey::F12,
        PrintScreen => NamedKey::PrintScreen,
        ScrollLock => NamedKey::ScrollLock,
        Pause => NamedKey::Pause,
        Insert => NamedKey::Insert,
        Home => NamedKey::Home,
        PageUp => NamedKey::PageUp,
        Delete => NamedKey::Delete,
        End => NamedKey::End,
        PageDown => NamedKey::PageDown,
        ArrowRight => NamedKey::ArrowRight,
        ArrowLeft => NamedKey::ArrowLeft,
        ArrowDown => NamedKey::ArrowDown,
        ArrowUp => NamedKey::ArrowUp,
        NumLock => NamedKey::NumLock,
        ContextMenu => NamedKey::ContextMenu,
        ControlLeft | ControlRight => NamedKey::Control,
        ShiftLeft | ShiftRight => NamedKey::Shift,
        AltLeft | AltRight => NamedKey::Alt,
        SuperLeft | SuperRight => NamedKey::Super,
        _ => return None,
    })
}

/// The character a key types on a US layout, with Shift or without.
pub fn us_char(code: KeyCode, shift: bool) -> Option<char> {
    use KeyCode::*;
    if let Some(i) = LETTERS.iter().position(|&k| k == code) {
        let c = char::from(b'a' + i as u8);
        return Some(if shift { c.to_ascii_uppercase() } else { c });
    }
    let (plain, shifted) = match code {
        Digit1 => ('1', '!'),
        Digit2 => ('2', '@'),
        Digit3 => ('3', '#'),
        Digit4 => ('4', '$'),
        Digit5 => ('5', '%'),
        Digit6 => ('6', '^'),
        Digit7 => ('7', '&'),
        Digit8 => ('8', '*'),
        Digit9 => ('9', '('),
        Digit0 => ('0', ')'),
        Minus => ('-', '_'),
        Equal => ('=', '+'),
        BracketLeft => ('[', '{'),
        BracketRight => (']', '}'),
        Backslash => ('\\', '|'),
        Semicolon => (';', ':'),
        Quote => ('\'', '"'),
        Backquote => ('`', '~'),
        Comma => (',', '<'),
        Period => ('.', '>'),
        Slash => ('/', '?'),
        Space => (' ', ' '),
        _ => return None,
    };
    Some(if shift { shifted } else { plain })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hid_usages_name_the_keys_at_their_us_places() {
        for (usage, key) in [
            (0x04, KeyCode::KeyA),
            (0x1a, KeyCode::KeyW),
            (0x1d, KeyCode::KeyZ),
            (0x1e, KeyCode::Digit1),
            (0x27, KeyCode::Digit0),
            (0x29, KeyCode::Escape),
            (0x2c, KeyCode::Space),
            (0x3a, KeyCode::F1),
            (0x3c, KeyCode::F3),
            (0x45, KeyCode::F12),
            (0x4f, KeyCode::ArrowRight),
            (0x52, KeyCode::ArrowUp),
            (0x59, KeyCode::Numpad1),
            (0x62, KeyCode::Numpad0),
            (0x68, KeyCode::F13),
            (0x73, KeyCode::F24),
            (0xe1, KeyCode::ShiftLeft),
            (0xe3, KeyCode::SuperLeft),
            (0xe7, KeyCode::SuperRight),
        ] {
            assert_eq!(key_code(usage), Some(key), "usage {usage:#x}");
        }
        for usage in [0x00, 0x01, 0x03, 0x66, 0xa5, 0xe8, 0xffff] {
            assert_eq!(key_code(usage), None, "usage {usage:#x}");
        }
    }

    #[test]
    fn us_layout_types_shifted_and_plain_characters() {
        assert_eq!(us_char(KeyCode::KeyW, false), Some('w'));
        assert_eq!(us_char(KeyCode::KeyW, true), Some('W'));
        assert_eq!(us_char(KeyCode::Digit2, true), Some('@'));
        assert_eq!(us_char(KeyCode::Slash, false), Some('/'));
        assert_eq!(us_char(KeyCode::Enter, false), None);
        assert_eq!(us_char(KeyCode::F3, true), None);
    }
}
