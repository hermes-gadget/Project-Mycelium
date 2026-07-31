//! SDL2-backed display emulation for virtual T-Deck instances.

mod config;
mod framebuffer;
mod lvgl_v8;
pub mod lvgl_v9;
pub mod manager;
pub mod shared_spi;
pub mod st7789;
mod version;
pub mod window;

use std::cell::Cell;
use std::ffi::{c_char, c_void, CStr};

use libloading::Library;

pub use config::{
    DisplayBackendOptions, DisplayConfig, DEFAULT_DRAW_BUFFER_ROWS, T_DECK_HEIGHT, T_DECK_WIDTH,
};
pub use framebuffer::{host_rgb565_to_st7789_wire, st7789_wire_to_host_rgb565, Rgb565ByteOrder};
pub use lvgl_v8::lvgl_v8_init_sdl;
pub use lvgl_v9::lvgl_v9_init_sdl;
pub use manager::DisplayManager;
pub use shared_spi::{global_spi_bus, SharedSpiBus, SpiDevice};
pub use version::LvglVersion;
pub use window::{DisplayEvent, DisplayWindow, Rect};

pub(crate) const BYTES_PER_PIXEL: usize = 2;

thread_local! {
    static ACTIVE_FIRMWARE_LIBRARY: Cell<*const Library> = const { Cell::new(std::ptr::null()) };
}

struct ActiveLibraryGuard(*const Library);

impl Drop for ActiveLibraryGuard {
    fn drop(&mut self) {
        ACTIVE_FIRMWARE_LIBRARY.with(|active| active.set(self.0));
    }
}

/// Make a firmware library the symbol-resolution scope for one firmware call.
///
/// The loader wraps setup, loop, display lookup, and capture calls with this
/// function so LVGL symbols always come from the firmware being advanced.
pub fn with_firmware_library<T>(library: &Library, call: impl FnOnce() -> T) -> T {
    ACTIVE_FIRMWARE_LIBRARY.with(|active| {
        let previous = active.replace(library);
        let _guard = ActiveLibraryGuard(previous);
        call()
    })
}

pub(crate) fn with_active_firmware_library<T>(call: impl FnOnce(&Library) -> T) -> Option<T> {
    ACTIVE_FIRMWARE_LIBRARY.with(|active| {
        // SAFETY: `with_firmware_library` installs this pointer only for the
        // dynamic extent of a call while the owning `Library` is borrowed.
        unsafe { active.get().as_ref() }.map(call)
    })
}

pub fn is_t_deck_resolution(width: i32, height: i32) -> bool {
    width == T_DECK_WIDTH as i32 && height == T_DECK_HEIGHT as i32
}
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
    if !is_t_deck_resolution(width, height) {
        return std::ptr::null_mut();
    }
    match version {
        LvglVersion::V8 => lvgl_v8::lvgl_v8_init_sdl_with_options(
            instance_id,
            width,
            height,
            DisplayBackendOptions::default(),
        ),
        LvglVersion::V9 | LvglVersion::Unknown => lvgl_v9::lvgl_v9_init_sdl_with_options(
            instance_id,
            width,
            height,
            DisplayBackendOptions::default(),
        ),
    }
}

/// Create a display using explicit partial-buffer and controller-fidelity
/// options.
///
/// # Safety
///
/// `window_title` and `options` must be null or point to readable values for
/// the duration of this call.
pub unsafe extern "C" fn meshemu_display_create_ex(
    width: i32,
    height: i32,
    window_title: *const c_char,
    lvgl_version: i32,
    options: *const DisplayBackendOptions,
) -> *mut c_void {
    let title = if window_title.is_null() {
        "T-Deck".into()
    } else {
        unsafe { CStr::from_ptr(window_title) }
            .to_string_lossy()
            .into_owned()
    };
    let Some(options) = (if options.is_null() {
        Some(DisplayBackendOptions::default())
    } else {
        unsafe { options.as_ref() }.copied()
    })
    .and_then(|options| options.validated(height.max(0) as u32)) else {
        return std::ptr::null_mut();
    };
    match lvgl_version {
        8 => lvgl_v8::lvgl_v8_init_sdl_with_options(&title, width, height, options),
        9 => lvgl_v9::lvgl_v9_init_sdl_with_options(&title, width, height, options),
        _ => std::ptr::null_mut(),
    }
}

/// Create an SDL display for an explicit LVGL major version.
///
/// # Safety
///
/// `window_title` must be null or point to a valid NUL-terminated string.
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
        .or_else(|| unsafe { lvgl_v9::capture_managed_rgb565(display) })
}

/// Destroy a host-managed LVGL v8 or v9 display with its owning backend API.
///
/// # Safety
///
/// `display` must be null or a live handle returned by a display initializer.
pub unsafe fn destroy_managed_display(display: *mut c_void) {
    if !unsafe { lvgl_v8::destroy_display(display) } {
        unsafe { lvgl_v9::destroy_display(display) };
    }
}

/// Destroy a Mycelium-managed compatibility display.
///
/// # Safety
///
/// `display` must be null or point to readable display-handle memory.
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

    #[test]
    fn t_deck_initializers_reject_non_native_geometry() {
        assert!(meshemu_display_init("wide", 640, 480, LvglVersion::V8).is_null());
        assert!(meshemu_display_init("short", 320, 200, LvglVersion::V9).is_null());
    }
}
