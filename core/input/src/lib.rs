//! SDL2-backed input and virtual T-Deck keyboard emulation.

pub mod i2c_keyboard;
pub mod wire_shim;

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard};

use i2c_keyboard::I2cKeyboardBus;
use sdl2::event::Event;
use sdl2::keyboard::Keycode;
use wire_shim::{SharedI2cKeyboard, WireShim};

pub use i2c_keyboard::{I2cTransaction, KEYBOARD_COLS, KEYBOARD_I2C_ADDRESS, KEYBOARD_ROWS};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MatrixPosition {
    pub row: u8,
    pub col: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LvglKeyEvent {
    pub keycode: Keycode,
    pub pressed: bool,
}

/// Maps host keyboard keys onto the T-Deck's 4x10 matrix.
#[derive(Clone, Copy, Debug, Default)]
pub struct KeyboardEmulator;

impl KeyboardEmulator {
    pub fn matrix_position(&self, keycode: Keycode) -> Option<MatrixPosition> {
        let (row, col) = match keycode {
            Keycode::Q => (0, 0),
            Keycode::W => (0, 1),
            Keycode::E => (0, 2),
            Keycode::R => (0, 3),
            Keycode::T => (0, 4),
            Keycode::Y => (0, 5),
            Keycode::U => (0, 6),
            Keycode::I => (0, 7),
            Keycode::O => (0, 8),
            Keycode::P => (0, 9),
            Keycode::A => (1, 0),
            Keycode::S => (1, 1),
            Keycode::D => (1, 2),
            Keycode::F => (1, 3),
            Keycode::G => (1, 4),
            Keycode::H => (1, 5),
            Keycode::J => (1, 6),
            Keycode::K => (1, 7),
            Keycode::L => (1, 8),
            Keycode::Return => (1, 9),
            Keycode::Z => (2, 0),
            Keycode::X => (2, 1),
            Keycode::C => (2, 2),
            Keycode::V => (2, 3),
            Keycode::B => (2, 4),
            Keycode::N => (2, 5),
            Keycode::M => (2, 6),
            Keycode::Comma => (2, 7),
            Keycode::Period => (2, 8),
            Keycode::Backspace => (2, 9),
            Keycode::Num1 => (3, 0),
            Keycode::Num2 => (3, 1),
            Keycode::Num3 => (3, 2),
            Keycode::Num4 => (3, 3),
            Keycode::Num5 => (3, 4),
            Keycode::Num6 => (3, 5),
            Keycode::Num7 => (3, 6),
            Keycode::Num8 => (3, 7),
            Keycode::Num9 => (3, 8),
            Keycode::Num0 => (3, 9),
            _ => return None,
        };
        Some(MatrixPosition { row, col })
    }
}

/// Routes every mapped host key to both raw-I2C state and the LVGL input
/// queue, so firmware can consume either interface.
pub struct InputManager {
    keyboard_emulator: KeyboardEmulator,
    keyboard_bus: SharedI2cKeyboard,
    lvgl_events: VecDeque<LvglKeyEvent>,
}

impl InputManager {
    pub fn new() -> Self {
        Self {
            keyboard_emulator: KeyboardEmulator,
            keyboard_bus: Arc::new(Mutex::new(I2cKeyboardBus::new())),
            lvgl_events: VecDeque::new(),
        }
    }

    pub fn keyboard_bus(&self) -> SharedI2cKeyboard {
        Arc::clone(&self.keyboard_bus)
    }

    pub fn wire_shim(&self) -> WireShim {
        WireShim::with_keyboard(self.keyboard_bus())
    }

    /// Handle an SDL host key event and route it to both firmware input paths.
    ///
    /// Repeated key-down events are ignored because the matrix is state-based;
    /// firmware can implement its own repeat behavior while a key remains down.
    pub fn handle_event(&mut self, event: &Event) -> bool {
        match *event {
            Event::KeyDown {
                keycode: Some(keycode),
                repeat: false,
                ..
            } => self.inject_host_key(keycode, true),
            Event::KeyUp {
                keycode: Some(keycode),
                ..
            } => self.inject_host_key(keycode, false),
            _ => false,
        }
    }

    pub fn inject_host_key(&mut self, keycode: Keycode, pressed: bool) -> bool {
        let Some(position) = self.keyboard_emulator.matrix_position(keycode) else {
            return false;
        };
        lock(&self.keyboard_bus).inject_key(position.row, position.col, pressed);
        self.lvgl_events
            .push_back(LvglKeyEvent { keycode, pressed });
        true
    }

    pub fn next_lvgl_event(&mut self) -> Option<LvglKeyEvent> {
        self.lvgl_events.pop_front()
    }
}

impl Default for InputManager {
    fn default() -> Self {
        Self::new()
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_key_updates_i2c_and_lvgl_paths_together() {
        let mut manager = InputManager::new();
        let mut wire = manager.wire_shim();

        assert!(manager.inject_host_key(Keycode::D, true));
        wire.begin_transmission(KEYBOARD_I2C_ADDRESS);
        wire.write_byte(1);
        assert_eq!(wire.request_from(KEYBOARD_I2C_ADDRESS, 1), 1);
        assert_eq!(wire.read(), 0b0000_0100);
        assert_eq!(
            manager.next_lvgl_event(),
            Some(LvglKeyEvent {
                keycode: Keycode::D,
                pressed: true,
            })
        );
    }

    #[test]
    fn unmapped_host_keys_do_not_reach_either_input_path() {
        let mut manager = InputManager::new();

        assert!(!manager.inject_host_key(Keycode::F12, true));
        assert_eq!(manager.next_lvgl_event(), None);
    }

    #[test]
    fn sdl_key_events_drive_press_and_release_state() {
        let mut manager = InputManager::new();
        let mut wire = manager.wire_shim();
        wire.write_byte(0);

        let pressed = Event::KeyDown {
            timestamp: 1,
            window_id: 7,
            keycode: Some(Keycode::Q),
            scancode: None,
            keymod: sdl2::keyboard::Mod::NOMOD,
            repeat: false,
        };
        let released = Event::KeyUp {
            timestamp: 2,
            window_id: 7,
            keycode: Some(Keycode::Q),
            scancode: None,
            keymod: sdl2::keyboard::Mod::NOMOD,
            repeat: false,
        };

        assert!(manager.handle_event(&pressed));
        assert_eq!(wire.request_from(KEYBOARD_I2C_ADDRESS, 1), 1);
        assert_eq!(wire.read(), 1);
        assert!(manager.handle_event(&released));
        assert_eq!(wire.request_from(KEYBOARD_I2C_ADDRESS, 1), 1);
        assert_eq!(wire.read(), 0);
    }
}
