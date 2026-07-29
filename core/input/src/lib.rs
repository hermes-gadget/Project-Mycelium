//! Host input emulation for the T-Deck peripherals.

pub mod gt911;
pub mod i2c_keyboard;
mod keyboard;
mod manager;
mod touch;
mod trackball;
pub mod wire_shim;

pub use gt911::{
    global_watchdog_status, set_global_failure_mode, tick_all_gt911, Gt911Controller, Gt911Point,
    DEFAULT_GT911_CONTACT_SIZE, DEFAULT_GT911_MAX_X, DEFAULT_GT911_MAX_Y, GT911_CONFIG_X_REGISTER,
    GT911_CONFIG_Y_REGISTER, GT911_FAILURE_MODE_BUS, GT911_FAILURE_MODE_FRAME_STALL,
    GT911_FAILURE_MODE_PHANTOM_LATCH, GT911_I2C_ADDRESS, GT911_INT_GPIO, GT911_MAX_TOUCHES,
    GT911_PRODUCT_ID_REGISTER, GT911_STATUS_BUS_WATCHDOG_FIRED, GT911_STATUS_FRAME_WATCHDOG_FIRED,
    GT911_STATUS_PHANTOM_WATCHDOG_FIRED, GT911_STATUS_REGISTER,
};
pub use i2c_keyboard::{
    KEYBOARD_BRIGHTNESS_COMMAND, KEYBOARD_I2C_ADDRESS, KEYBOARD_KEY_MODE_COMMAND,
};
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
