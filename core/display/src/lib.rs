//! SDL2-backed display emulation for virtual T-Deck instances.

mod config;
mod lvgl_v8;
pub mod lvgl_v9;
pub mod manager;
mod version;
pub mod window;

use std::ffi::{c_char, c_void, CStr};

pub use config::DisplayConfig;
pub use lvgl_v8::lvgl_v8_init_sdl;
pub use lvgl_v9::lvgl_v9_init_sdl;
pub use manager::DisplayManager;
pub use version::LvglVersion;
pub use window::{DisplayEvent, DisplayWindow, Rect};

pub(crate) const BYTES_PER_PIXEL: usize = 2;

pub(crate) fn framebuffer_size(width: u32, height: u32) -> Option<usize> {
    (width as usize)
        .checked_mul(height as usize)?
        .checked_mul(BYTES_PER_PIXEL)
}

/// Initialize an SDL display using the requested LVGL ABI.
///
/// Unknown versions deliberately use v9 because it is Mycelium's primary
/// target.
pub fn meshemu_display_init(
    instance_id: &str,
    width: i32,
    height: i32,
    version: LvglVersion,
) -> *mut c_void {
    match version {
        LvglVersion::V8 => lvgl_v8_init_sdl(instance_id, width, height),
        LvglVersion::V9 | LvglVersion::Unknown => lvgl_v9_init_sdl(instance_id, width, height),
    }
}

/// Create an SDL display for an explicit LVGL major version.
///
/// # Safety
///
/// `window_title` must be null or point to a valid NUL-terminated string.
#[no_mangle]
pub unsafe extern "C" fn meshemu_display_create_v(
    width: i32,
    height: i32,
    window_title: *const c_char,
    lvgl_version: i32,
) -> *mut c_void {
    let title = if window_title.is_null() {
        "T-Deck".into()
    } else {
        unsafe { CStr::from_ptr(window_title) }
            .to_string_lossy()
            .into_owned()
    };
    let version = match lvgl_version {
        8 => LvglVersion::V8,
        9 => LvglVersion::V9,
        _ => LvglVersion::Unknown,
    };
    meshemu_display_init(&title, width, height, version)
}

/// Copy a host-managed compatibility display's RGB565 framebuffer.
///
/// Returns `None` when the pointer is null or is not a Mycelium-managed handle.
///
/// # Safety
///
/// `display` must be null or point to readable display-handle memory.
pub unsafe fn capture_managed_rgb565(display: *mut c_void) -> Option<Vec<u8>> {
    unsafe { lvgl_v8::capture_rgb565(display) }
}

/// Destroy a host-managed LVGL v8 compatibility display.
///
/// # Safety
///
/// `display` must be null or a live handle returned by [`lvgl_v8_init_sdl`].
pub unsafe fn destroy_managed_display(display: *mut c_void) {
    unsafe { lvgl_v8::destroy_display(display) };
}

/// Destroy a Mycelium-managed compatibility display.
///
/// Native LVGL v9 displays remain owned by the firmware's LVGL runtime.
///
/// # Safety
///
/// `display` must be null or point to readable display-handle memory.
#[no_mangle]
pub unsafe extern "C" fn meshemu_display_destroy(display: *mut c_void) {
    unsafe { destroy_managed_display(display) };
}

#[cfg(test)]
pub(crate) static SDL_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn framebuffer_size_is_full_screen_rgb565() {
        assert_eq!(framebuffer_size(320, 240), Some(320 * 240 * 2));
        assert_eq!(framebuffer_size(u32::MAX, u32::MAX), None);
    }
}
