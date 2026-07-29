use std::collections::VecDeque;
use std::sync::atomic::{AtomicU8, Ordering};

pub const KEYBOARD_I2C_ADDRESS: u8 = 0x55;
pub const KEYBOARD_BRIGHTNESS_COMMAND: u8 = 0x01;
pub const KEYBOARD_KEY_MODE_COMMAND: u8 = 0x04;

static PERSISTED_BACKLIGHT: AtomicU8 = AtomicU8::new(0);

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
    /// Keyboard backlight brightness (0 is off, 255 is maximum).
    backlight: u8,
    /// Whether C3 backlight state survives an S3-side handle recreation.
    pub cross_reset_persist: bool,
}

impl I2cKeyboardBus {
    pub fn new() -> Self {
        Self {
            key_queue: VecDeque::new(),
            key_mode_active: false,
            last_command: 0,
            backlight: PERSISTED_BACKLIGHT.load(Ordering::SeqCst),
            cross_reset_persist: true,
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

    /// Apply one complete host I2C write transaction.
    pub fn write_transaction(&mut self, bytes: &[u8]) {
        let Some(&command) = bytes.first() else {
            return;
        };
        self.last_command = command;
        match (command, bytes.get(1).copied()) {
            (KEYBOARD_BRIGHTNESS_COMMAND, Some(brightness)) => {
                self.backlight = brightness;
                if self.cross_reset_persist {
                    PERSISTED_BACKLIGHT.store(brightness, Ordering::SeqCst);
                }
            }
            (KEYBOARD_KEY_MODE_COMMAND, _) => self.key_mode_active = true,
            _ => {}
        }
    }

    pub fn backlight(&self) -> u8 {
        self.backlight
    }

    /// Configure whether this C3's brightness survives future host recreations.
    pub fn set_cross_reset_persist(&mut self, persist: bool) {
        self.cross_reset_persist = persist;
        if persist {
            PERSISTED_BACKLIGHT.store(self.backlight, Ordering::SeqCst);
        } else {
            // A non-persistent device presents power-on darkness after the next
            // host recreation, regardless of an older retained C3 value.
            PERSISTED_BACKLIGHT.store(0, Ordering::SeqCst);
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
        if !self.cross_reset_persist {
            self.backlight = 0;
        }
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
    use std::sync::Mutex;

    static PERSISTENCE_TEST: Mutex<()> = Mutex::new(());

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

    #[test]
    fn brightness_survives_host_recreation_by_default() {
        let _serial = PERSISTENCE_TEST.lock().unwrap();
        let mut first = I2cKeyboardBus::new();
        first.write_transaction(&[KEYBOARD_BRIGHTNESS_COMMAND, 128]);
        assert_eq!(first.backlight(), 128);
        drop(first);

        let mut fresh = I2cKeyboardBus::new();
        assert!(fresh.cross_reset_persist);
        assert_eq!(fresh.backlight(), 128);

        // Leave global state deterministic for unrelated tests.
        fresh.set_cross_reset_persist(false);
    }

    #[test]
    fn disabling_cross_reset_returns_the_next_c3_to_power_on_darkness() {
        let _serial = PERSISTENCE_TEST.lock().unwrap();
        let mut bus = I2cKeyboardBus::new();
        bus.write_transaction(&[KEYBOARD_BRIGHTNESS_COMMAND, 200]);
        bus.set_cross_reset_persist(false);
        assert_eq!(bus.backlight(), 200);

        assert_eq!(I2cKeyboardBus::new().backlight(), 0);
    }
}
