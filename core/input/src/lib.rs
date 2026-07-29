//! Host input emulation for the T-Deck peripherals.

mod keyboard;
mod manager;
mod touch;
mod trackball;

pub use keyboard::{KeyEvent, KeyboardEmulator, TDeckKeyMapping};
pub use manager::{
    get_input_manager, register_input_manager, remove_input_manager, InputEvent, InputManager,
    SharedInputManager,
};
pub use touch::{Gt911TouchEvent, TouchEmulator};
pub use trackball::{TrackballDirection, TrackballEmulator, TrackballEvent};
