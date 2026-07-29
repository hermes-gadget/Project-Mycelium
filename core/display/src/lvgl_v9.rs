//! Runtime-bound LVGL v9 SDL integration.
//!
//! Firmware shared libraries bring their own LVGL build. Resolving its symbols
//! at runtime keeps the emulator usable for headless firmware and avoids
//! pinning the Rust workspace to a second LVGL copy.

use std::ffi::{c_char, c_int, c_void, CString};
use std::ptr;

use libloading::{Library, Symbol};
use sdl2::pixels::PixelFormatEnum;

type LvInit = unsafe extern "C" fn();
type LvSdlWindowCreate = unsafe extern "C" fn(c_int, c_int) -> *mut c_void;
type LvDisplayDelete = unsafe extern "C" fn(*mut c_void);
type LvDisplayGetColorFormat = unsafe extern "C" fn(*mut c_void) -> u32;
type LvSdlWindowSetTitle = unsafe extern "C" fn(*mut c_void, *const c_char);
type LvSdlWindowSetResizable = unsafe extern "C" fn(*mut c_void, bool);
type LvSdlWindowGetRenderer = unsafe extern "C" fn(*mut c_void) -> *mut c_void;
type LvSdlWindowGetWindow = unsafe extern "C" fn(*mut c_void) -> *mut c_void;
type LvDisplayGetResolution = unsafe extern "C" fn(*mut c_void) -> c_int;
type SdlHideWindow = unsafe extern "C" fn(*mut c_void);
type SdlRenderReadPixels =
    unsafe extern "C" fn(*mut c_void, *const sdl2::sys::SDL_Rect, u32, *mut c_void, c_int) -> c_int;

// `lv_color_format_t` encodes RGB565 as 0x12 in LVGL v9.
const LV_COLOR_FORMAT_RGB565: u32 = 0x12;

/// Initialize LVGL v9 with its built-in SDL2 display driver.
///
/// Returns null when no firmware library is active, the firmware's LVGL/SDL
/// symbols are incomplete, its native color depth is not RGB565, or the
/// supplied dimensions do not match the T-Deck panel.
pub fn lvgl_v9_init_sdl(instance_id: &str, width: i32, height: i32) -> *mut c_void {
    crate::with_active_firmware_library(|library| unsafe {
        lvgl_v9_init_sdl_with_library(library, instance_id, width, height)
    })
    .unwrap_or(ptr::null_mut())
}

/// Initialize LVGL v9 by resolving every symbol from one firmware library.
///
/// # Safety
///
/// `library` must contain LVGL v9 and SDL functions with the declared ABIs.
pub unsafe fn lvgl_v9_init_sdl_with_library(
    library: &Library,
    instance_id: &str,
    width: i32,
    height: i32,
) -> *mut c_void {
    if instance_id.is_empty() || !crate::is_t_deck_resolution(width, height) {
        return ptr::null_mut();
    }
    let Ok(title) = CString::new(format!("T-Deck — {instance_id}")) else {
        return ptr::null_mut();
    };

    // SAFETY: Symbols are resolved from the already-loaded firmware/LVGL image
    // with their documented LVGL v9 signatures and called immediately.
    unsafe {
        let Ok(lv_init) = library.get::<LvInit>(b"lv_init\0") else {
            return ptr::null_mut();
        };
        let Ok(create) = library.get::<LvSdlWindowCreate>(b"lv_sdl_window_create\0") else {
            return ptr::null_mut();
        };
        let Ok(get_color_format) =
            library.get::<LvDisplayGetColorFormat>(b"lv_display_get_color_format\0")
        else {
            return ptr::null_mut();
        };
        let Ok(delete) = library.get::<LvDisplayDelete>(b"lv_display_delete\0") else {
            return ptr::null_mut();
        };
        let Ok(get_window) = library.get::<LvSdlWindowGetWindow>(b"lv_sdl_window_get_window\0")
        else {
            return ptr::null_mut();
        };
        let Ok(hide_window) = library.get::<SdlHideWindow>(b"SDL_HideWindow\0") else {
            return ptr::null_mut();
        };
        let set_title: Option<Symbol<LvSdlWindowSetTitle>> =
            library.get(b"lv_sdl_window_set_title\0").ok();
        let set_resizable: Option<Symbol<LvSdlWindowSetResizable>> =
            library.get(b"lv_sdl_window_set_resizeable\0").ok();

        lv_init();
        let display = create(width, height);
        if display.is_null() {
            return ptr::null_mut();
        }
        // The SDL driver allocates its draw buffers in `create`. Changing the
        // format afterwards would leave those allocations at the old size, so
        // only accept firmware built natively for the T-Deck's RGB565 panel.
        if get_color_format(display) != LV_COLOR_FORMAT_RGB565 {
            delete(display);
            return ptr::null_mut();
        }
        if let Some(set_title) = set_title {
            set_title(display, title.as_ptr());
        }
        if let Some(set_resizable) = set_resizable {
            set_resizable(display, false);
        }
        let native_window = get_window(display);
        if native_window.is_null() {
            delete(display);
            return ptr::null_mut();
        }
        // DisplayManager owns the visible, resizable, letterboxed host window.
        // LVGL's SDL window remains only as its RGB565 rendering surface.
        hide_window(native_window);
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
    crate::with_active_firmware_library(|library| unsafe {
        capture_lvgl_rgb565_with_library(library, display)
    })
    .flatten()
}

/// Capture a v9 SDL framebuffer using the same library that owns the display.
///
/// # Safety
///
/// `display` must be a live LVGL display created by `library`.
pub unsafe fn capture_lvgl_rgb565_with_library(
    library: &Library,
    display: *mut c_void,
) -> Option<Vec<u8>> {
    if display.is_null() {
        return None;
    }

    // SAFETY: The caller guarantees a live LVGL display. Symbols are resolved
    // with their LVGL v9 signatures and SDL writes into an exactly sized buffer.
    unsafe {
        let renderer = library
            .get::<LvSdlWindowGetRenderer>(b"lv_sdl_window_get_renderer\0")
            .ok()?(display);
        let width = library
            .get::<LvDisplayGetResolution>(b"lv_display_get_horizontal_resolution\0")
            .ok()?(display);
        let height = library
            .get::<LvDisplayGetResolution>(b"lv_display_get_vertical_resolution\0")
            .ok()?(display);
        if renderer.is_null() || !crate::is_t_deck_resolution(width, height) {
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
        let render_read_pixels = library
            .get::<SdlRenderReadPixels>(b"SDL_RenderReadPixels\0")
            .ok()?;
        let result = render_read_pixels(
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
        assert!(lvgl_v9_init_sdl("node1", 640, 480).is_null());
    }
}
