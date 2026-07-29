use std::sync::{Arc, Mutex, MutexGuard};

use crate::i2c_keyboard::{I2cKeyboardBus, KEYBOARD_I2C_ADDRESS};

pub type SharedI2cKeyboard = Arc<Mutex<I2cKeyboardBus>>;

/// Provides a virtual I2C bus that replaces the physical Arduino `Wire`
/// instance for keyboard access.
pub struct WireShim {
    keyboard_bus: SharedI2cKeyboard,
    current_address: u8,
    current_register: u8,
    read_buffer: Vec<u8>,
    read_position: usize,
}

impl WireShim {
    pub fn new() -> Self {
        Self::with_keyboard(Arc::new(Mutex::new(I2cKeyboardBus::new())))
    }

    pub fn with_keyboard(keyboard_bus: SharedI2cKeyboard) -> Self {
        Self {
            keyboard_bus,
            current_address: 0,
            current_register: 0,
            read_buffer: Vec::new(),
            read_position: 0,
        }
    }

    pub fn set_keyboard(&mut self, keyboard_bus: SharedI2cKeyboard) {
        self.keyboard_bus = keyboard_bus;
        self.clear_read_buffer();
    }

    pub fn keyboard(&self) -> SharedI2cKeyboard {
        Arc::clone(&self.keyboard_bus)
    }

    pub fn begin_transmission(&mut self, address: u8) {
        self.current_address = address;
    }

    pub fn write_byte(&mut self, byte: u8) -> usize {
        self.current_register = byte;
        1
    }

    pub fn end_transmission(&mut self) -> u8 {
        0
    }

    pub fn request_from(&mut self, address: u8, count: u8) -> u8 {
        self.clear_read_buffer();
        if address != KEYBOARD_I2C_ADDRESS || count == 0 {
            return 0;
        }

        let data = lock(&self.keyboard_bus).read_register(self.current_register);
        self.read_buffer
            .extend(data.into_iter().take(count as usize));
        self.read_buffer.len() as u8
    }

    pub fn read(&mut self) -> i32 {
        let Some(byte) = self.read_buffer.get(self.read_position).copied() else {
            return -1;
        };
        self.read_position += 1;
        byte as i32
    }

    pub fn available(&self) -> i32 {
        self.read_buffer.len().saturating_sub(self.read_position) as i32
    }

    fn clear_read_buffer(&mut self) {
        self.read_buffer.clear();
        self.read_position = 0;
    }
}

impl Default for WireShim {
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
    fn wire_sequence_selects_and_reads_a_keyboard_register() {
        let keyboard = Arc::new(Mutex::new(I2cKeyboardBus::new()));
        lock(&keyboard).inject_key(3, 5, true);
        let mut wire = WireShim::with_keyboard(keyboard);

        wire.begin_transmission(KEYBOARD_I2C_ADDRESS);
        assert_eq!(wire.write_byte(0x03), 1);
        assert_eq!(wire.end_transmission(), 0);
        assert_eq!(wire.request_from(KEYBOARD_I2C_ADDRESS, 1), 1);
        assert_eq!(wire.available(), 1);
        assert_eq!(wire.read(), 0b0010_0000);
        assert_eq!(wire.available(), 0);
        assert_eq!(wire.read(), -1);
    }

    #[test]
    fn non_keyboard_addresses_return_no_data_and_clear_stale_reads() {
        let mut wire = WireShim::new();
        wire.begin_transmission(KEYBOARD_I2C_ADDRESS);
        wire.write_byte(0x04);
        assert_eq!(wire.request_from(KEYBOARD_I2C_ADDRESS, 1), 1);

        assert_eq!(wire.request_from(0x42, 1), 0);
        assert_eq!(wire.available(), 0);
        assert_eq!(wire.read(), -1);
    }
}
