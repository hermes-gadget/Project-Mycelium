use std::mem;

pub const KEYBOARD_I2C_ADDRESS: u8 = 0x55;
pub const KEYBOARD_ROWS: usize = 4;
pub const KEYBOARD_COLS: usize = 10;

/// Represents an I2C read from the keyboard co-processor.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct I2cTransaction {
    pub address: u8,
    pub register: u8,
    pub data: Vec<u8>,
}

/// Simulates the ESP32-C3 keyboard co-processor at I2C address `0x55`.
#[derive(Debug)]
pub struct I2cKeyboardBus {
    /// Current key matrix state: rows x columns bitmap.
    key_state: [[bool; KEYBOARD_COLS]; KEYBOARD_ROWS],
    /// I2C register data that the co-processor reports.
    registers: [u8; 32],
    /// Completed I2C reads, retained for inspection by the emulator.
    pending_reads: Vec<I2cTransaction>,
}

impl I2cKeyboardBus {
    pub fn new() -> Self {
        let mut registers = [0; 32];
        registers[0x04] = 0x01;
        Self {
            key_state: [[false; KEYBOARD_COLS]; KEYBOARD_ROWS],
            registers,
            pending_reads: Vec::new(),
        }
    }

    /// Inject a key event from the host keyboard emulator.
    pub fn inject_key(&mut self, row: u8, col: u8, pressed: bool) {
        if row < KEYBOARD_ROWS as u8 && col < KEYBOARD_COLS as u8 {
            self.key_state[row as usize][col as usize] = pressed;
            self.registers[row as usize] = self.encode_row(row as usize);
        }
    }

    /// Read a register as if it were requested from I2C address `0x55`.
    pub fn read_register(&mut self, reg: u8) -> Vec<u8> {
        let data = match reg {
            0x00..=0x03 => vec![self.registers[reg as usize]],
            0x04 => vec![self.keyboard_present()],
            _ => vec![0x00],
        };
        self.pending_reads.push(I2cTransaction {
            address: KEYBOARD_I2C_ADDRESS,
            register: reg,
            data: data.clone(),
        });
        data
    }

    /// Return all reads recorded since the bus was created or last drained.
    pub fn pending_reads(&self) -> &[I2cTransaction] {
        &self.pending_reads
    }

    /// Drain the recorded I2C reads.
    pub fn take_pending_reads(&mut self) -> Vec<I2cTransaction> {
        mem::take(&mut self.pending_reads)
    }

    /// Encode the columns representable by the controller's one-byte row
    /// register. Columns 8 and 9 remain part of the matrix for host/LVGL input,
    /// but cannot be represented in this legacy register format.
    fn encode_row(&self, row: usize) -> u8 {
        let mut byte = 0_u8;
        for col in 0..u8::BITS as usize {
            if self.key_state[row][col] {
                byte |= 1 << col;
            }
        }
        byte
    }

    fn keyboard_present(&self) -> u8 {
        0x01
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
    fn injected_key_is_exposed_in_its_row_register() {
        let mut bus = I2cKeyboardBus::new();

        bus.inject_key(2, 3, true);

        assert_eq!(bus.read_register(0x02), [0b0000_1000]);
        assert_eq!(bus.read_register(0x00), [0]);
        assert_eq!(bus.read_register(0x04), [1]);
    }

    #[test]
    fn multiple_pressed_keys_are_encoded_and_releases_clear_bits() {
        let mut bus = I2cKeyboardBus::new();
        bus.inject_key(1, 0, true);
        bus.inject_key(1, 3, true);
        bus.inject_key(1, 7, true);

        assert_eq!(bus.read_register(0x01), [0b1000_1001]);

        bus.inject_key(1, 3, false);
        assert_eq!(bus.read_register(0x01), [0b1000_0001]);
    }

    #[test]
    fn out_of_range_keys_are_ignored_and_reads_are_recorded() {
        let mut bus = I2cKeyboardBus::new();
        bus.inject_key(4, 0, true);
        bus.inject_key(0, 10, true);

        assert_eq!(bus.read_register(0), [0]);
        assert_eq!(
            bus.take_pending_reads(),
            [I2cTransaction {
                address: KEYBOARD_I2C_ADDRESS,
                register: 0,
                data: vec![0],
            }]
        );
        assert!(bus.pending_reads().is_empty());
    }
}
