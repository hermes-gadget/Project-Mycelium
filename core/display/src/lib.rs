//! SDL2-backed display emulation for virtual T-Deck instances.

pub mod lvgl_v9;
pub mod manager;
pub mod window;

pub use manager::DisplayManager;
pub use window::{DisplayConfig, DisplayEvent, DisplayWindow, Rect};

#[cfg(test)]
pub(crate) static SDL_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
