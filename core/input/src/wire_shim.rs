use std::sync::{Arc, Mutex, MutexGuard};

use crate::i2c_keyboard::{I2cKeyboardBus, KEYBOARD_I2C_ADDRESS};

pub const KEYBOARD_I2C_CLOCK_HZ: u32 = 100_000;

pub type SharedI2cKeyboard = Arc<Mutex<I2cKeyboardBus>>;

/// Arduino `Wire`-compatible virtual bus for the T-Deck keyboard.
pub struct WireShim {
    keyboard_bus: SharedI2cKeyboard,
    begun: bool,
    clock_hz: u32,
    transmit_address: Option<u8>,
    transmit_buffer: Vec<u8>,
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
            begun: false,
            clock_hz: KEYBOARD_I2C_CLOCK_HZ,
            transmit_address: None,
            transmit_buffer: Vec::new(),
            read_buffer: Vec::new(),
            read_position: 0,
        }
    }

    /// Initialize the virtual I2C controller.
    pub fn begin(&mut self) -> bool {
        self.begun = true;
        true
    }

    /// Configure the I2C clock, as with Arduino `Wire.setClock()`.
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

    pub fn begin_transmission(&mut self, address: u8) {
        self.transmit_address = Some(address);
        self.transmit_buffer.clear();
    }

    /// Buffer a byte until `end_transmission`, matching Arduino `Wire.write()`.
    pub fn write_byte(&mut self, byte: u8) -> usize {
        if self.transmit_address.is_none() {
            return 0;
        }
        self.transmit_buffer.push(byte);
        1
    }

    /// Finish a transmission.
    ///
    /// Arduino-compatible results used here are `0` for success, `2` for an
    /// address NACK, and `4` for an invalid controller/transaction state.
    pub fn end_transmission(&mut self) -> u8 {
        let Some(address) = self.transmit_address.take() else {
            return 4;
        };
        if !self.begun {
            self.transmit_buffer.clear();
            return 4;
        }
        if address != KEYBOARD_I2C_ADDRESS {
            self.transmit_buffer.clear();
            return 2;
        }

        let commands = std::mem::take(&mut self.transmit_buffer);
        let mut keyboard = lock(&self.keyboard_bus);
        for command in commands {
            keyboard.write_command(command);
        }
        0
    }

    /// Poll the keyboard. The real C3 supplies exactly one byte per request.
    pub fn request_from(&mut self, address: u8, count: u8) -> u8 {
        self.clear_read_buffer();
        if !self.begun || self.clock_hz != KEYBOARD_I2C_CLOCK_HZ {
            return 0;
        }
        if address != KEYBOARD_I2C_ADDRESS || count == 0 {
            return 0;
        }

        self.read_buffer
            .push(lock(&self.keyboard_bus).read_key_byte());
        1
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
        assert_eq!(wire.write_byte(0x04), 1);
        assert_eq!(wire.end_transmission(), 0);

        assert_eq!(wire.request_from(KEYBOARD_I2C_ADDRESS, 1), 1);
        assert_eq!(wire.available(), 1);
        assert_eq!(wire.read(), i32::from(b'q'));
        assert_eq!(wire.available(), 0);
        assert_eq!(wire.request_from(KEYBOARD_I2C_ADDRESS, 1), 1);
        assert_eq!(wire.read(), i32::from(b'W'));
    }

    #[test]
    fn idle_keyboard_poll_returns_one_zero_byte() {
        let keyboard = Arc::new(Mutex::new(I2cKeyboardBus::new()));
        let mut wire = configured_wire(keyboard);
        wire.begin_transmission(KEYBOARD_I2C_ADDRESS);
        wire.write_byte(0x04);
        assert_eq!(wire.end_transmission(), 0);

        assert_eq!(wire.request_from(KEYBOARD_I2C_ADDRESS, 1), 1);
        assert_eq!(wire.read(), 0x00);
        assert_eq!(wire.read(), -1);
    }

    #[test]
    fn writes_are_buffered_until_end_transmission() {
        let keyboard = Arc::new(Mutex::new(I2cKeyboardBus::new()));
        lock(&keyboard).inject_key_byte(b'a');
        let mut wire = configured_wire(Arc::clone(&keyboard));
        wire.begin_transmission(KEYBOARD_I2C_ADDRESS);
        wire.write_byte(0x04);

        assert_eq!(wire.request_from(KEYBOARD_I2C_ADDRESS, 1), 1);
        assert_eq!(wire.read(), 0x00);
        assert_eq!(wire.end_transmission(), 0);
        assert_eq!(wire.request_from(KEYBOARD_I2C_ADDRESS, 1), 1);
        assert_eq!(wire.read(), i32::from(b'a'));
    }

    #[test]
    fn bad_addresses_nack_and_only_one_byte_is_returned_per_poll() {
        let mut wire = WireShim::new();
        wire.begin();
        wire.set_clock(KEYBOARD_I2C_CLOCK_HZ);

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
    fn keyboard_requires_begin_and_a_100_khz_clock() {
        let mut wire = WireShim::new();
        assert_eq!(wire.clock_hz(), KEYBOARD_I2C_CLOCK_HZ);
        assert_eq!(wire.request_from(KEYBOARD_I2C_ADDRESS, 1), 0);

        wire.begin();
        wire.set_clock(400_000);
        assert_eq!(wire.request_from(KEYBOARD_I2C_ADDRESS, 1), 0);

        wire.set_clock(KEYBOARD_I2C_CLOCK_HZ);
        assert_eq!(wire.request_from(KEYBOARD_I2C_ADDRESS, 1), 1);
    }
}
