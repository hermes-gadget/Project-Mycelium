use std::collections::{HashMap, HashSet};

use sdl2::keyboard::Keycode;

/// Position and labels of one key in the T-Deck keyboard matrix.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TDeckKeyMapping {
    pub row: u8,
    pub col: u8,
    pub label: char,
    pub shifted: char,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct KeyEvent {
    pub row: u8,
    pub col: u8,
    pub pressed: bool,
    /// Exact byte returned by the T-Deck ESP32-C3 for this key.
    ///
    /// Shift keys have no byte of their own and use `0x00`.
    pub key_byte: u8,
}

pub struct KeyboardEmulator {
    keymap: HashMap<Keycode, TDeckKeyMapping>,
    pressed_keys: HashSet<Keycode>,
    last_event: Option<KeyEvent>,
}

impl KeyboardEmulator {
    pub fn new() -> Self {
        let mut emu = Self {
            keymap: HashMap::new(),
            pressed_keys: HashSet::new(),
            last_event: None,
        };

        for (col, (key, label)) in [
            (Keycode::Q, 'q'),
            (Keycode::W, 'w'),
            (Keycode::E, 'e'),
            (Keycode::R, 'r'),
            (Keycode::T, 't'),
            (Keycode::Y, 'y'),
            (Keycode::U, 'u'),
            (Keycode::I, 'i'),
            (Keycode::O, 'o'),
            (Keycode::P, 'p'),
        ]
        .into_iter()
        .enumerate()
        {
            emu.map_key(key, 0, col as u8, label, label.to_ascii_uppercase());
        }
        for (col, (key, label)) in [
            (Keycode::A, 'a'),
            (Keycode::S, 's'),
            (Keycode::D, 'd'),
            (Keycode::F, 'f'),
            (Keycode::G, 'g'),
            (Keycode::H, 'h'),
            (Keycode::J, 'j'),
            (Keycode::K, 'k'),
            (Keycode::L, 'l'),
        ]
        .into_iter()
        .enumerate()
        {
            emu.map_key(key, 1, col as u8, label, label.to_ascii_uppercase());
        }
        for (col, (key, label, shifted)) in [
            (Keycode::Z, 'z', 'Z'),
            (Keycode::X, 'x', 'X'),
            (Keycode::C, 'c', 'C'),
            (Keycode::V, 'v', 'V'),
            (Keycode::B, 'b', 'B'),
            (Keycode::N, 'n', 'N'),
            (Keycode::M, 'm', 'M'),
            (Keycode::Comma, ',', '<'),
            (Keycode::Period, '.', '>'),
        ]
        .into_iter()
        .enumerate()
        {
            emu.map_key(key, 2, col as u8, label, shifted);
        }
        emu.map_key(Keycode::LShift, 3, 0, '\0', '\0');
        emu.map_key(Keycode::RShift, 3, 0, '\0', '\0');
        emu.map_key(Keycode::Space, 3, 1, ' ', ' ');
        emu.map_key(Keycode::Return, 3, 2, '\r', '\r');
        emu.map_key(Keycode::Backspace, 3, 3, '\u{8}', '\u{8}');
        emu
    }

    pub fn handle_key(&mut self, keycode: Keycode, pressed: bool) -> Option<KeyEvent> {
        let mapping = *self.keymap.get(&keycode)?;
        let changed = if pressed {
            self.pressed_keys.insert(keycode)
        } else {
            self.pressed_keys.remove(&keycode)
        };
        if !changed {
            return None;
        }
        let shift_active = self.pressed_keys.contains(&Keycode::LShift)
            || self.pressed_keys.contains(&Keycode::RShift);
        let event = KeyEvent {
            row: mapping.row,
            col: mapping.col,
            pressed,
            key_byte: if shift_active {
                mapping.shifted as u8
            } else {
                mapping.label as u8
            },
        };
        self.last_event = Some(event);
        Some(event)
    }

    pub fn get_last(&self) -> Option<KeyEvent> {
        self.last_event
    }

    fn map_key(&mut self, keycode: Keycode, row: u8, col: u8, label: char, shifted: char) {
        self.keymap.insert(
            keycode,
            TDeckKeyMapping {
                row,
                col,
                label,
                shifted,
            },
        );
    }
}

impl Default for KeyboardEmulator {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_each_keyboard_row() {
        let mut keyboard = KeyboardEmulator::new();
        assert_eq!(
            keyboard.handle_key(Keycode::Q, true),
            Some(KeyEvent {
                row: 0,
                col: 0,
                pressed: true,
                key_byte: b'q',
            })
        );
        assert_eq!(keyboard.handle_key(Keycode::L, true).unwrap().col, 8);
        assert_eq!(keyboard.handle_key(Keycode::Period, true).unwrap().row, 2);
        assert_eq!(
            keyboard.handle_key(Keycode::Backspace, true).unwrap(),
            KeyEvent {
                row: 3,
                col: 3,
                pressed: true,
                key_byte: 0x08,
            }
        );
    }

    #[test]
    fn suppresses_repeats_and_spurious_releases() {
        let mut keyboard = KeyboardEmulator::new();
        assert!(keyboard.handle_key(Keycode::A, true).is_some());
        assert!(keyboard.handle_key(Keycode::A, true).is_none());
        assert!(keyboard.handle_key(Keycode::A, false).is_some());
        assert!(keyboard.handle_key(Keycode::A, false).is_none());
        assert!(keyboard.handle_key(Keycode::F1, true).is_none());
    }

    #[test]
    fn shift_keys_are_tracked_independently_and_uppercase_letters() {
        let mut keyboard = KeyboardEmulator::new();
        assert_eq!(
            keyboard.handle_key(Keycode::LShift, true).unwrap().key_byte,
            0x00
        );
        keyboard.handle_key(Keycode::RShift, true);
        assert_eq!(
            keyboard.handle_key(Keycode::Q, true).unwrap().key_byte,
            b'Q'
        );
        keyboard.handle_key(Keycode::Q, false);
        keyboard.handle_key(Keycode::LShift, false);
        assert_eq!(
            keyboard.handle_key(Keycode::W, true).unwrap().key_byte,
            b'W'
        );
        keyboard.handle_key(Keycode::W, false);
        keyboard.handle_key(Keycode::RShift, false);
        assert_eq!(
            keyboard.handle_key(Keycode::E, true).unwrap().key_byte,
            b'e'
        );
    }

    #[test]
    fn special_keys_match_the_c3_protocol() {
        let mut keyboard = KeyboardEmulator::new();

        assert_eq!(
            keyboard.handle_key(Keycode::Return, true).unwrap().key_byte,
            0x0d
        );
        assert_ne!(keyboard.get_last().unwrap().key_byte, 0x0a);
        assert_eq!(
            keyboard
                .handle_key(Keycode::Backspace, true)
                .unwrap()
                .key_byte,
            0x08
        );
        assert_eq!(
            keyboard.handle_key(Keycode::Space, true).unwrap().key_byte,
            0x20
        );
    }
}
