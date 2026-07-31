use std::sync::{Arc, Mutex, MutexGuard};

pub use crate::gt911::SharedGt911;
use crate::gt911::{new_shared_gt911, GT911_I2C_ADDRESS};
use crate::i2c_keyboard::{I2cKeyboardBus, KEYBOARD_I2C_ADDRESS};

pub const KEYBOARD_I2C_CLOCK_HZ: u32 = 100_000;
pub const FAST_I2C_CLOCK_HZ: u32 = 400_000;

pub type SharedI2cKeyboard = Arc<Mutex<I2cKeyboardBus>>;
type PeripheralPowerCheck = Arc<dyn Fn() -> bool + Send + Sync>;

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
    peripheral_power_check: Option<PeripheralPowerCheck>,
    sda_stuck: bool,
}

impl WireShim {
    pub fn new() -> Self {
        Self::with_devices(
            Arc::new(Mutex::new(I2cKeyboardBus::new())),
            new_shared_gt911(),
        )
    }

    pub fn with_keyboard(keyboard_bus: SharedI2cKeyboard) -> Self {
        Self::with_devices(keyboard_bus, new_shared_gt911())
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
            peripheral_power_check: None,
            sda_stuck: false,
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

    /// Probe one address without changing the current transaction state.
    pub fn probe_address(&self, address: u8) -> bool {
        self.bus_ready() && matches!(address, KEYBOARD_I2C_ADDRESS | GT911_I2C_ADDRESS)
    }

    /// Both externally pulled-up T-Deck I2C lines idle HIGH.
    pub fn idle_levels(&self) -> (u8, u8) {
        (u8::from(!self.sda_stuck), 1)
    }

    /// Clock SCL nine times, as the firmware recovery path does.
    ///
    /// Returns 0 when SDA was already free, 1 when the pulses released it, and
    /// 2 when an unpowered peripheral rail leaves it stuck.
    pub fn clock_out_recovery(&mut self) -> u8 {
        if !self.sda_stuck {
            return 0;
        }
        if !self.peripherals_powered() {
            return 2;
        }
        self.sda_stuck = false;
        1
    }

    /// Emit an I2C STOP and discard any incomplete transaction state.
    pub fn emit_stop(&mut self) {
        self.transmit_address = None;
        self.transmit_buffer.clear();
        self.clear_read_buffer();
    }

    pub fn set_sda_stuck(&mut self, stuck: bool) {
        self.sda_stuck = stuck;
        if stuck {
            self.emit_stop();
        }
    }

    pub fn set_peripheral_power_check<F>(&mut self, check: F)
    where
        F: Fn() -> bool + Send + Sync + 'static,
    {
        self.peripheral_power_check = Some(Arc::new(check));
    }

    pub fn begin_transmission(&mut self, address: u8) {
        self.transmit_address = Some(address);
        self.transmit_buffer.clear();
    }

    pub fn write_byte(&mut self, byte: u8) -> usize {
        if self.transmit_address.is_none() || !self.peripherals_powered() || self.sda_stuck {
            return 0;
        }
        self.transmit_buffer.push(byte);
        1
    }

    pub fn end_transmission(&mut self) -> u8 {
        let Some(address) = self.transmit_address.take() else {
            return 4;
        };
        if !self.peripherals_powered() {
            self.transmit_buffer.clear();
            self.clear_read_buffer();
            return 2;
        }
        if self.sda_stuck {
            self.transmit_buffer.clear();
            self.clear_read_buffer();
            return 2;
        }
        if !self.begun {
            self.transmit_buffer.clear();
            return 4;
        }

        let bytes = std::mem::take(&mut self.transmit_buffer);
        match address {
            KEYBOARD_I2C_ADDRESS => {
                lock(&self.keyboard_bus).write_transaction(&bytes);
            }
            GT911_I2C_ADDRESS => {
                // An empty write is the address-only ACK used by an I2C scan.
                if bytes.is_empty() {
                    return 0;
                }
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
        if !self.bus_ready() || count == 0 {
            return 0;
        }

        match address {
            KEYBOARD_I2C_ADDRESS => {
                // The real ESP32-C3 keyboard only responds reliably at
                // 100 kHz; reads at faster clocks time out. Probes and
                // writes still reach the address, matching the T-Deck.
                if self.clock_hz > KEYBOARD_I2C_CLOCK_HZ {
                    return 0;
                }
                self.read_buffer
                    .push(lock(&self.keyboard_bus).read_key_byte())
            }
            GT911_I2C_ADDRESS => {
                self.read_buffer.resize(count as usize, 0);
                let read = lock(&self.gt911).i2c_read(self.gt911_register, &mut self.read_buffer);
                if read != usize::from(count) {
                    self.clear_read_buffer();
                    return 0;
                }
            }
            _ => return 0,
        }
        self.read_buffer.len() as u8
    }

    pub fn read(&mut self) -> i32 {
        if !self.peripherals_powered() {
            return -1;
        }
        let Some(byte) = self.read_buffer.get(self.read_position).copied() else {
            return -1;
        };
        self.read_position += 1;
        byte as i32
    }

    pub fn available(&self) -> i32 {
        if !self.peripherals_powered() {
            return 0;
        }
        self.read_buffer.len().saturating_sub(self.read_position) as i32
    }

    fn peripherals_powered(&self) -> bool {
        self.peripheral_power_check
            .as_ref()
            .is_none_or(|check| check())
    }

    fn clear_read_buffer(&mut self) {
        self.read_buffer.clear();
        self.read_position = 0;
    }

    fn bus_ready(&self) -> bool {
        self.peripherals_powered()
            && self.begun
            && !self.sda_stuck
            && matches!(self.clock_hz, KEYBOARD_I2C_CLOCK_HZ | FAST_I2C_CLOCK_HZ)
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
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::*;
    use crate::gt911::{GT911_CONFIG_X_REGISTER, GT911_PRODUCT_ID_REGISTER, GT911_STATUS_REGISTER};
    use crate::KEYBOARD_KEY_MODE_COMMAND;

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
    fn bus_scan_and_gt911_identity_diagnostics_match_the_tdeck() {
        let mut wire = configured_wire(Arc::new(Mutex::new(I2cKeyboardBus::new())));
        wire.set_clock(FAST_I2C_CLOCK_HZ);
        assert_eq!(wire.idle_levels(), (1, 1));
        assert!(wire.probe_address(KEYBOARD_I2C_ADDRESS));
        assert!(wire.probe_address(GT911_I2C_ADDRESS));
        assert!(!wire.probe_address(0x14));

        wire.begin_transmission(GT911_I2C_ADDRESS);
        assert_eq!(wire.end_transmission(), 0);

        wire.begin_transmission(GT911_I2C_ADDRESS);
        for byte in GT911_PRODUCT_ID_REGISTER.to_be_bytes() {
            wire.write_byte(byte);
        }
        assert_eq!(wire.end_transmission(), 0);
        assert_eq!(wire.request_from(GT911_I2C_ADDRESS, 4), 4);
        let product_id: Vec<_> = (0..4).map(|_| wire.read() as u8).collect();
        assert_eq!(&product_id, b"911\0");

        wire.begin_transmission(GT911_I2C_ADDRESS);
        for byte in GT911_CONFIG_X_REGISTER.to_be_bytes() {
            wire.write_byte(byte);
        }
        assert_eq!(wire.end_transmission(), 0);
        assert_eq!(wire.request_from(GT911_I2C_ADDRESS, 4), 4);
        let resolution: Vec<_> = (0..4).map(|_| wire.read() as u8).collect();
        assert_eq!(resolution, [64, 1, 240, 0]);
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
    fn devices_require_begin_and_a_supported_clock() {
        let mut wire = WireShim::new();
        assert_eq!(wire.request_from(KEYBOARD_I2C_ADDRESS, 1), 0);
        wire.begin();
        wire.set_clock(1_000_000);
        assert_eq!(wire.request_from(KEYBOARD_I2C_ADDRESS, 1), 0);
        wire.set_clock(FAST_I2C_CLOCK_HZ);
        // The keyboard times out at 400 kHz; only 100 kHz reads succeed.
        assert_eq!(wire.request_from(KEYBOARD_I2C_ADDRESS, 1), 0);
        wire.set_clock(KEYBOARD_I2C_CLOCK_HZ);
        assert_eq!(wire.request_from(KEYBOARD_I2C_ADDRESS, 1), 1);
    }

    #[test]
    fn fast_clock_times_out_keyboard_reads_but_not_gt911() {
        let keyboard = Arc::new(Mutex::new(I2cKeyboardBus::new()));
        lock(&keyboard).inject_key_byte(b'q');
        let mut wire = configured_wire(keyboard);
        wire.set_clock(FAST_I2C_CLOCK_HZ);

        // A 400 kHz keyboard read yields no data and nothing to read.
        wire.begin_transmission(KEYBOARD_I2C_ADDRESS);
        wire.write_byte(0x04);
        assert_eq!(wire.end_transmission(), 0);
        assert_eq!(wire.request_from(KEYBOARD_I2C_ADDRESS, 1), 0);
        assert_eq!(wire.read(), -1);

        // The GT911 touch controller tolerates the fast clock.
        lock(&wire.gt911()).inject_touch(10, 20, true);
        wire.begin_transmission(GT911_I2C_ADDRESS);
        for byte in GT911_STATUS_REGISTER.to_be_bytes() {
            wire.write_byte(byte);
        }
        assert_eq!(wire.end_transmission(), 0);
        assert_eq!(wire.request_from(GT911_I2C_ADDRESS, 9), 9);

        // Dropping back to 100 kHz restores keyboard reads immediately.
        wire.set_clock(KEYBOARD_I2C_CLOCK_HZ);
        wire.begin_transmission(KEYBOARD_I2C_ADDRESS);
        wire.write_byte(0x04);
        assert_eq!(wire.end_transmission(), 0);
        assert_eq!(wire.request_from(KEYBOARD_I2C_ADDRESS, 1), 1);
        assert_eq!(wire.read(), i32::from(b'q'));
    }

    #[test]
    fn stuck_sda_nacks_until_nine_clock_recovery_and_stop() {
        let mut wire = WireShim::new();
        wire.begin();
        wire.set_sda_stuck(true);

        assert_eq!(wire.idle_levels(), (0, 1));
        assert!(!wire.probe_address(KEYBOARD_I2C_ADDRESS));
        wire.begin_transmission(KEYBOARD_I2C_ADDRESS);
        assert_eq!(wire.write_byte(KEYBOARD_KEY_MODE_COMMAND), 0);
        assert_eq!(wire.end_transmission(), 2);
        assert_eq!(wire.request_from(KEYBOARD_I2C_ADDRESS, 1), 0);

        assert_eq!(wire.clock_out_recovery(), 1);
        wire.emit_stop();
        assert_eq!(wire.idle_levels(), (1, 1));
        assert_eq!(wire.clock_out_recovery(), 0);
        assert!(wire.probe_address(KEYBOARD_I2C_ADDRESS));
    }

    #[test]
    fn peripheral_power_loss_nacks_all_i2c_transactions() {
        let powered = Arc::new(AtomicBool::new(true));
        let power_check = Arc::clone(&powered);
        let mut wire = configured_wire(Arc::new(Mutex::new(I2cKeyboardBus::new())));
        wire.set_peripheral_power_check(move || power_check.load(Ordering::Relaxed));

        wire.begin_transmission(KEYBOARD_I2C_ADDRESS);
        assert_eq!(wire.write_byte(0x04), 1);
        assert_eq!(wire.end_transmission(), 0);

        powered.store(false, Ordering::Relaxed);
        wire.begin_transmission(KEYBOARD_I2C_ADDRESS);
        assert_eq!(wire.write_byte(0x04), 0);
        assert_eq!(wire.end_transmission(), 0x02);
        assert_eq!(wire.request_from(KEYBOARD_I2C_ADDRESS, 1), 0);
        assert_eq!(wire.available(), 0);
        assert_eq!(wire.read(), -1);
    }
}
