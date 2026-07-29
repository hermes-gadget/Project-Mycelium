//! Runtime-bound LVGL v9 SDL integration.
//!
//! Firmware shared libraries bring their own LVGL build. Resolving its symbols
//! at runtime keeps the emulator usable for headless firmware and avoids
//! pinning the Rust workspace to a second LVGL copy.

use std::ffi::{c_char, c_int, c_void, CString};
use std::ptr;

#[cfg(unix)]
use libloading::os::unix::{Library, Symbol};
#[cfg(windows)]
use libloading::os::windows::{Library, Symbol};
use sdl2::pixels::PixelFormatEnum;

type LvInit = unsafe extern "C" fn();
type LvSdlWindowCreate = unsafe extern "C" fn(c_int, c_int) -> *mut c_void;
type LvDisplaySetColorFormat = unsafe extern "C" fn(*mut c_void, u32);
type LvSdlWindowSetTitle = unsafe extern "C" fn(*mut c_void, *const c_char);
type LvSdlWindowGetRenderer = unsafe extern "C" fn(*mut c_void) -> *mut c_void;
type LvDisplayGetResolution = unsafe extern "C" fn(*mut c_void) -> c_int;

// `lv_color_format_t` encodes RGB565 as 0x12 in LVGL v9.
const LV_COLOR_FORMAT_RGB565: u32 = 0x12;

#[cfg(unix)]
fn current_process_library() -> Option<Library> {
    Some(Library::this())
}

#[cfg(windows)]
fn current_process_library() -> Option<Library> {
    Library::this().ok()
}

/// Initialize LVGL v9 with its built-in SDL2 display driver.
///
/// Returns null if LVGL's required SDL symbols are not exported by the process
/// or if the supplied dimensions are invalid.
pub fn lvgl_v9_init_sdl(instance_id: &str, width: i32, height: i32) -> *mut c_void {
    if instance_id.is_empty() || width <= 0 || height <= 0 {
        return ptr::null_mut();
    }
    let Ok(title) = CString::new(format!("T-Deck — {instance_id}")) else {
        return ptr::null_mut();
    };

    // SAFETY: Symbols are resolved from the already-loaded firmware/LVGL image
    // with their documented LVGL v9 signatures and called immediately.
    unsafe {
        let Some(library) = current_process_library() else {
            return ptr::null_mut();
        };
        let Ok(lv_init) = library.get::<LvInit>(b"lv_init\0") else {
            return ptr::null_mut();
        };
        let Ok(create) = library.get::<LvSdlWindowCreate>(b"lv_sdl_window_create\0") else {
            return ptr::null_mut();
        };
        let Ok(set_color_format) =
            library.get::<LvDisplaySetColorFormat>(b"lv_display_set_color_format\0")
        else {
            return ptr::null_mut();
        };
        let set_title: Option<Symbol<LvSdlWindowSetTitle>> =
            library.get(b"lv_sdl_window_set_title\0").ok();

        lv_init();
        let display = create(width, height);
        if display.is_null() {
            return ptr::null_mut();
        }
        set_color_format(display, LV_COLOR_FORMAT_RGB565);
        if let Some(set_title) = set_title {
            set_title(display, title.as_ptr());
        }
        display
    }
}

/// Capture an LVGL SDL display's logical framebuffer as packed RGB565.
///
/// Returns `None` if the handle is null, required LVGL symbols are unavailable,
/// the display has invalid dimensions, or SDL cannot read the renderer.
///
/// # Safety
///
/// `display` must be a live `lv_display_t` created by LVGL's SDL driver.
pub unsafe fn capture_lvgl_rgb565(display: *mut c_void) -> Option<Vec<u8>> {
    if display.is_null() {
        return None;
    }

    // SAFETY: The caller guarantees a live LVGL display. Symbols are resolved
    // with their LVGL v9 signatures and SDL writes into an exactly sized buffer.
    unsafe {
        let library = current_process_library()?;
        let renderer = library
            .get::<LvSdlWindowGetRenderer>(b"lv_sdl_window_get_renderer\0")
            .ok()?(display);
        let width = library
            .get::<LvDisplayGetResolution>(b"lv_display_get_horizontal_resolution\0")
            .ok()?(display);
        let height = library
            .get::<LvDisplayGetResolution>(b"lv_display_get_vertical_resolution\0")
            .ok()?(display);
        if renderer.is_null() || width <= 0 || height <= 0 {
            return None;
        }
        let len = (width as usize)
            .checked_mul(height as usize)?
            .checked_mul(2)?;
        let mut pixels = vec![0_u8; len];
        let rect = sdl2::sys::SDL_Rect {
            x: 0,
            y: 0,
            w: width,
            h: height,
        };
        let result = sdl2::sys::SDL_RenderReadPixels(
            renderer.cast(),
            &rect,
            PixelFormatEnum::RGB565 as u32,
            pixels.as_mut_ptr().cast(),
            width * 2,
        );
        (result == 0).then_some(pixels)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_invalid_configuration_without_loading_lvgl() {
        assert!(lvgl_v9_init_sdl("", 320, 240).is_null());
        assert!(lvgl_v9_init_sdl("node1", 0, 240).is_null());
        assert!(lvgl_v9_init_sdl("node1", 320, -1).is_null());
    }
}
