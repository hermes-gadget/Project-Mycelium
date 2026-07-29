use std::sync::{Arc, Mutex, MutexGuard};

use crate::gt911::{Gt911Controller, GT911_I2C_ADDRESS};
use crate::i2c_keyboard::{I2cKeyboardBus, KEYBOARD_I2C_ADDRESS};

pub type SharedI2cKeyboard = Arc<Mutex<I2cKeyboardBus>>;
pub type SharedGt911 = Arc<Mutex<Gt911Controller>>;

/// Virtual Arduino `Wire` bus shared by the keyboard and GT911.
pub struct WireShim {
    keyboard_bus: SharedI2cKeyboard,
    gt911: SharedGt911,
    current_address: u8,
    current_register: u16,
    write_buffer: Vec<u8>,
    read_buffer: Vec<u8>,
    read_position: usize,
}

impl WireShim {
    pub fn new() -> Self {
        Self::with_devices(
            Arc::new(Mutex::new(I2cKeyboardBus::new())),
            Arc::new(Mutex::new(Gt911Controller::new())),
        )
    }

    pub fn with_keyboard(keyboard_bus: SharedI2cKeyboard) -> Self {
        Self::with_devices(keyboard_bus, Arc::new(Mutex::new(Gt911Controller::new())))
    }

    pub fn with_devices(keyboard_bus: SharedI2cKeyboard, gt911: SharedGt911) -> Self {
        Self {
            keyboard_bus,
            gt911,
            current_address: 0,
            current_register: 0,
            write_buffer: Vec::new(),
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

    pub fn gt911(&self) -> SharedGt911 {
        Arc::clone(&self.gt911)
    }

    pub fn begin_transmission(&mut self, address: u8) {
        self.current_address = address;
        self.write_buffer.clear();
    }

    pub fn write_byte(&mut self, byte: u8) -> usize {
        self.write_buffer.push(byte);
        // Preserve the convenient register-only sequence used by callers that
        // omit endTransmission before requestFrom.
        match self.current_address {
            GT911_I2C_ADDRESS if self.write_buffer.len() >= 2 => {
                self.current_register =
                    u16::from_be_bytes([self.write_buffer[0], self.write_buffer[1]]);
            }
            _ if self.write_buffer.len() == 1 => self.current_register = u16::from(byte),
            _ => {}
        }
        1
    }

    pub fn end_transmission(&mut self) -> u8 {
        match self.current_address {
            KEYBOARD_I2C_ADDRESS => {
                let Some(register) = self.write_buffer.first() else {
                    return 4;
                };
                self.current_register = u16::from(*register);
            }
            GT911_I2C_ADDRESS => {
                if self.write_buffer.len() < 2 {
                    return 4;
                }
                self.current_register =
                    u16::from_be_bytes([self.write_buffer[0], self.write_buffer[1]]);
                if self.write_buffer.len() > 2 {
                    lock(&self.gt911).i2c_write(self.current_register, &self.write_buffer[2..]);
                }
            }
            _ => return 2,
        }
        0
    }

    pub fn request_from(&mut self, address: u8, count: u8) -> u8 {
        self.clear_read_buffer();
        if count == 0 {
            return 0;
        }

        match address {
            KEYBOARD_I2C_ADDRESS => {
                let data = lock(&self.keyboard_bus).read_register(self.current_register as u8);
                self.read_buffer
                    .extend(data.into_iter().take(count as usize));
            }
            GT911_I2C_ADDRESS => {
                self.read_buffer.resize(count as usize, 0);
                lock(&self.gt911).i2c_read(self.current_register, &mut self.read_buffer);
            }
            _ => return 0,
        }
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
    use crate::gt911::GT911_STATUS_REGISTER;

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
    fn gt911_uses_two_byte_registers_and_clears_interrupt_via_wire() {
        let mut wire = WireShim::new();
        lock(&wire.gt911()).inject_touch(100, 40, true);

        wire.begin_transmission(GT911_I2C_ADDRESS);
        for byte in GT911_STATUS_REGISTER.to_be_bytes() {
            wire.write_byte(byte);
        }
        assert_eq!(wire.end_transmission(), 0);
        assert_eq!(wire.request_from(GT911_I2C_ADDRESS, 9), 9);
        let bytes: Vec<_> = (0..9).map(|_| wire.read() as u8).collect();
        assert_eq!(bytes[0], 0x81);
        assert_eq!(u16::from_le_bytes([bytes[2], bytes[3]]), 40);
        assert_eq!(u16::from_le_bytes([bytes[4], bytes[5]]), 219);

        wire.begin_transmission(GT911_I2C_ADDRESS);
        for byte in GT911_STATUS_REGISTER.to_be_bytes() {
            wire.write_byte(byte);
        }
        wire.write_byte(0);
        assert_eq!(wire.end_transmission(), 0);
        assert!(lock(&wire.gt911()).gpio16_level());
    }

    #[test]
    fn unknown_addresses_return_no_data_and_clear_stale_reads() {
        let mut wire = WireShim::new();
        wire.begin_transmission(KEYBOARD_I2C_ADDRESS);
        wire.write_byte(0x04);
        assert_eq!(wire.request_from(KEYBOARD_I2C_ADDRESS, 1), 1);

        assert_eq!(wire.request_from(0x42, 1), 0);
        assert_eq!(wire.available(), 0);
        assert_eq!(wire.read(), -1);
    }
}
