//! Host input emulation for the T-Deck peripherals.

pub mod i2c_keyboard;
mod keyboard;
mod manager;
mod touch;
mod trackball;
pub mod wire_shim;

pub use i2c_keyboard::{I2cTransaction, KEYBOARD_COLS, KEYBOARD_I2C_ADDRESS, KEYBOARD_ROWS};
pub use keyboard::{KeyEvent, KeyboardEmulator, TDeckKeyMapping};
pub use manager::{
    get_input_manager, register_input_manager, remove_input_manager, InputEvent, InputManager,
    SharedInputManager,
};
pub use touch::{Gt911TouchEvent, TouchEmulator};
pub use trackball::{TrackballDirection, TrackballEmulator, TrackballEvent};
