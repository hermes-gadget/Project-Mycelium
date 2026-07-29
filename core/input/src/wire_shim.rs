use std::sync::{Arc, Mutex, MutexGuard};

use crate::gt911::{Gt911Controller, GT911_I2C_ADDRESS};
use crate::i2c_keyboard::{I2cKeyboardBus, KEYBOARD_I2C_ADDRESS};

pub const KEYBOARD_I2C_CLOCK_HZ: u32 = 100_000;

pub type SharedI2cKeyboard = Arc<Mutex<I2cKeyboardBus>>;
pub type SharedGt911 = Arc<Mutex<Gt911Controller>>;

/// Arduino `Wire`-compatible bus shared by the T-Deck keyboard and GT911.
pub struct WireShim {
    keyboard_bus: SharedI2cKeyboard,
    gt911: SharedGt911,
    begun: bool,
    clock_hz: u32,
    transmit_address: Option<u8>,
    transmit_buffer: Vec<u8>,
    gt911_register: u16,
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
            begun: false,
            clock_hz: KEYBOARD_I2C_CLOCK_HZ,
            transmit_address: None,
            transmit_buffer: Vec::new(),
            gt911_register: 0,
            read_buffer: Vec::new(),
            read_position: 0,
        }
    }

    pub fn begin(&mut self) -> bool {
        self.begun = true;
        true
    }

    pub fn set_clock(&mut self, clock_hz: u32) {
        self.clock_hz = clock_hz;
    }

    pub fn clock_hz(&self) -> u32 {
        self.clock_hz
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
        self.transmit_address = Some(address);
        self.transmit_buffer.clear();
    }

    pub fn write_byte(&mut self, byte: u8) -> usize {
        if self.transmit_address.is_none() {
            return 0;
        }
        self.transmit_buffer.push(byte);
        1
    }

    pub fn end_transmission(&mut self) -> u8 {
        let Some(address) = self.transmit_address.take() else {
            return 4;
        };
        if !self.begun {
            self.transmit_buffer.clear();
            return 4;
        }

        let bytes = std::mem::take(&mut self.transmit_buffer);
        match address {
            KEYBOARD_I2C_ADDRESS => {
                let mut keyboard = lock(&self.keyboard_bus);
                for command in bytes {
                    keyboard.write_command(command);
                }
            }
            GT911_I2C_ADDRESS => {
                if bytes.len() < 2 {
                    return 4;
                }
                self.gt911_register = u16::from_be_bytes([bytes[0], bytes[1]]);
                if bytes.len() > 2 {
                    lock(&self.gt911).i2c_write(self.gt911_register, &bytes[2..]);
                }
            }
            _ => return 2,
        }
        0
    }

    pub fn request_from(&mut self, address: u8, count: u8) -> u8 {
        self.clear_read_buffer();
        if !self.begun || self.clock_hz != KEYBOARD_I2C_CLOCK_HZ || count == 0 {
            return 0;
        }

        match address {
            KEYBOARD_I2C_ADDRESS => self
                .read_buffer
                .push(lock(&self.keyboard_bus).read_key_byte()),
            GT911_I2C_ADDRESS => {
                self.read_buffer.resize(count as usize, 0);
                lock(&self.gt911).i2c_read(self.gt911_register, &mut self.read_buffer);
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

    fn configured_wire(keyboard: SharedI2cKeyboard) -> WireShim {
        let mut wire = WireShim::with_keyboard(keyboard);
        assert!(wire.begin());
        wire.set_clock(KEYBOARD_I2C_CLOCK_HZ);
        wire
    }

    #[test]
    fn real_wire_sequence_activates_key_mode_and_reads_fifo_bytes() {
        let keyboard = Arc::new(Mutex::new(I2cKeyboardBus::new()));
        lock(&keyboard).inject_key_byte(b'q');
        lock(&keyboard).inject_key_byte(b'W');
        let mut wire = configured_wire(keyboard);
        wire.begin_transmission(KEYBOARD_I2C_ADDRESS);
        wire.write_byte(0x04);
        assert_eq!(wire.end_transmission(), 0);
        assert_eq!(wire.request_from(KEYBOARD_I2C_ADDRESS, 1), 1);
        assert_eq!(wire.read(), i32::from(b'q'));
        assert_eq!(wire.request_from(KEYBOARD_I2C_ADDRESS, 1), 1);
        assert_eq!(wire.read(), i32::from(b'W'));
    }

    #[test]
    fn writes_are_buffered_until_end_transmission() {
        let keyboard = Arc::new(Mutex::new(I2cKeyboardBus::new()));
        lock(&keyboard).inject_key_byte(b'a');
        let mut wire = configured_wire(keyboard);
        wire.begin_transmission(KEYBOARD_I2C_ADDRESS);
        wire.write_byte(0x04);
        assert_eq!(wire.request_from(KEYBOARD_I2C_ADDRESS, 1), 1);
        assert_eq!(wire.read(), 0);
        assert_eq!(wire.end_transmission(), 0);
        assert_eq!(wire.request_from(KEYBOARD_I2C_ADDRESS, 1), 1);
        assert_eq!(wire.read(), i32::from(b'a'));
    }

    #[test]
    fn gt911_uses_two_byte_registers_and_clears_interrupt_via_wire() {
        let mut wire = configured_wire(Arc::new(Mutex::new(I2cKeyboardBus::new())));
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
    fn bad_addresses_nack_and_keyboard_returns_one_byte_per_poll() {
        let mut wire = WireShim::new();
        wire.begin();
        wire.begin_transmission(0x42);
        wire.write_byte(0x04);
        assert_eq!(wire.end_transmission(), 2);
        assert_eq!(wire.request_from(0x42, 1), 0);

        wire.begin_transmission(KEYBOARD_I2C_ADDRESS);
        wire.write_byte(0x04);
        assert_eq!(wire.end_transmission(), 0);
        assert_eq!(wire.request_from(KEYBOARD_I2C_ADDRESS, 8), 1);
    }

    #[test]
    fn devices_require_begin_and_a_100_khz_clock() {
        let mut wire = WireShim::new();
        assert_eq!(wire.request_from(KEYBOARD_I2C_ADDRESS, 1), 0);
        wire.begin();
        wire.set_clock(400_000);
        assert_eq!(wire.request_from(KEYBOARD_I2C_ADDRESS, 1), 0);
        wire.set_clock(KEYBOARD_I2C_CLOCK_HZ);
        assert_eq!(wire.request_from(KEYBOARD_I2C_ADDRESS, 1), 1);
    }
}
