//! Host input emulation for the T-Deck peripherals.

pub mod gt911;
pub mod i2c_keyboard;
mod keyboard;
mod manager;
mod touch;
mod trackball;
pub mod wire_shim;

pub use gt911::{
    Gt911Controller, Gt911Point, GT911_I2C_ADDRESS, GT911_INT_GPIO, GT911_MAX_TOUCHES,
    GT911_STATUS_REGISTER,
};
pub use i2c_keyboard::{I2cTransaction, KEYBOARD_COLS, KEYBOARD_I2C_ADDRESS, KEYBOARD_ROWS};
pub use keyboard::{KeyEvent, KeyboardEmulator, TDeckKeyMapping};
pub use manager::{
    get_input_manager, register_input_manager, remove_input_manager, InputEvent, InputManager,
    SharedInputManager,
};
pub use touch::{Gt911TouchEvent, TouchEmulator};
pub use trackball::{
    TrackballDirection, TrackballEmulator, TrackballEvent, TrackballGpio, TRACKBALL_CLICK_GPIO,
    TRACKBALL_DOWN_GPIO, TRACKBALL_LEFT_GPIO, TRACKBALL_RIGHT_GPIO, TRACKBALL_UP_GPIO,
};
