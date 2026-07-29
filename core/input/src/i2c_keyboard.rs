use std::collections::VecDeque;

pub const KEYBOARD_I2C_ADDRESS: u8 = 0x55;
pub const KEYBOARD_KEY_MODE_COMMAND: u8 = 0x04;

/// Simulates the ESP32-C3 keyboard co-processor at I2C address `0x55`.
///
/// The real protocol is polled: each I2C read returns one key byte, with
/// `0x00` indicating that no key is currently waiting.
#[derive(Debug)]
pub struct I2cKeyboardBus {
    /// Queue of key bytes waiting to be read (FIFO).
    key_queue: VecDeque<u8>,
    /// Whether key mode (`CMD 0x04`) has been sent.
    key_mode_active: bool,
    /// Last command byte written.
    last_command: u8,
}

impl I2cKeyboardBus {
    pub fn new() -> Self {
        Self {
            key_queue: VecDeque::new(),
            key_mode_active: false,
            last_command: 0,
        }
    }

    /// Inject the exact byte that the ESP32-C3 would return for a key press.
    pub fn inject_key_byte(&mut self, key_byte: u8) {
        self.key_queue.push_back(key_byte);
    }

    /// Record and apply a command written to the keyboard co-processor.
    pub fn write_command(&mut self, byte: u8) {
        self.last_command = byte;
        if byte == KEYBOARD_KEY_MODE_COMMAND {
            self.key_mode_active = true;
        }
    }

    /// Return one queued key byte, or `0x00` when key mode is inactive or idle.
    pub fn read_key_byte(&mut self) -> u8 {
        if !self.key_mode_active {
            return 0x00;
        }
        self.key_queue.pop_front().unwrap_or(0x00)
    }

    /// Return how many key bytes the next poll can expose.
    pub fn available(&self) -> usize {
        if !self.key_mode_active {
            return 0;
        }
        self.key_queue.len().min(1)
    }

    pub fn reset(&mut self) {
        self.key_queue.clear();
        self.key_mode_active = false;
        self.last_command = 0;
    }
}

impl Default for I2cKeyboardBus {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_mode_command_activates_polled_key_bytes() {
        let mut bus = I2cKeyboardBus::new();
        bus.inject_key_byte(b'q');

        assert_eq!(bus.available(), 0);
        assert_eq!(bus.read_key_byte(), 0x00);

        bus.write_command(KEYBOARD_KEY_MODE_COMMAND);

        assert!(bus.key_mode_active);
        assert_eq!(bus.last_command, 0x04);
        assert_eq!(bus.available(), 1);
        assert_eq!(bus.read_key_byte(), b'q');
    }

    #[test]
    fn injected_key_bytes_are_returned_fifo_one_per_poll() {
        let mut bus = I2cKeyboardBus::new();
        bus.write_command(KEYBOARD_KEY_MODE_COMMAND);
        bus.inject_key_byte(b'q');
        bus.inject_key_byte(b'W');
        bus.inject_key_byte(0x0d);

        assert_eq!(bus.available(), 1);
        assert_eq!(bus.read_key_byte(), b'q');
        assert_eq!(bus.available(), 1);
        assert_eq!(bus.read_key_byte(), b'W');
        assert_eq!(bus.read_key_byte(), 0x0d);
        assert_eq!(bus.available(), 0);
    }

    #[test]
    fn idle_poll_returns_zero_and_reset_disables_key_mode() {
        let mut bus = I2cKeyboardBus::new();
        bus.write_command(KEYBOARD_KEY_MODE_COMMAND);

        assert_eq!(bus.read_key_byte(), 0x00);

        bus.inject_key_byte(0x08);
        bus.reset();
        assert_eq!(bus.available(), 0);
        assert_eq!(bus.read_key_byte(), 0x00);
        assert!(!bus.key_mode_active);
        assert_eq!(bus.last_command, 0);
    }
}
