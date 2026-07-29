//! Runtime-bound LVGL v9 partial display driver.
//!
//! Firmware shared libraries bring their own LVGL build. Mycelium registers a
//! flush callback in that runtime and owns a persistent logical RGB565
//! framebuffer. Capture therefore reads valid driver-owned memory before or
//! after host presentation and never touches SDL's invalidated backbuffer.

use std::cell::RefCell;
use std::ffi::{c_int, c_void};
use std::ptr;

use libloading::Library;

use crate::shared_spi::SharedSpiBus;
use crate::st7789::{St7789Controller, ST7789_CASET, ST7789_RAMWR, ST7789_RASET};
use crate::{host_rgb565_to_st7789_wire, DisplayBackendOptions, Rect, BYTES_PER_PIXEL};

type LvInit = unsafe extern "C" fn();
type LvDisplayCreate = unsafe extern "C" fn(c_int, c_int) -> *mut c_void;
type LvDisplayDelete = unsafe extern "C" fn(*mut c_void);
type LvDisplaySetColorFormat = unsafe extern "C" fn(*mut c_void, u32);
type LvDisplaySetBuffers = unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void, u32, u32);
type LvDisplaySetFlushCallback = unsafe extern "C" fn(*mut c_void, Option<FlushCallbackV9>);
type LvDisplayFlushReady = unsafe extern "C" fn(*mut c_void);
type FlushCallbackV9 = unsafe extern "C" fn(*mut c_void, *const LvArea, *mut u8);

// `lv_color_format_t` encodes RGB565 as 0x12 in LVGL v9.
const LV_COLOR_FORMAT_RGB565: u32 = 0x12;
// `lv_display_render_mode_t::LV_DISPLAY_RENDER_MODE_PARTIAL`.
const LV_DISPLAY_RENDER_MODE_PARTIAL: u32 = 0;

#[repr(C)]
#[derive(Clone, Copy, Debug)]
struct LvArea {
    x1: i32,
    y1: i32,
    x2: i32,
    y2: i32,
}

#[derive(Clone, Copy)]
struct LvglV9Api {
    init: LvInit,
    create: LvDisplayCreate,
    delete: LvDisplayDelete,
    set_color_format: LvDisplaySetColorFormat,
    set_buffers: LvDisplaySetBuffers,
    set_flush_callback: LvDisplaySetFlushCallback,
    flush_ready: LvDisplayFlushReady,
}

impl LvglV9Api {
    unsafe fn load(library: &Library) -> Option<Self> {
        Some(Self {
            init: *unsafe { library.get::<LvInit>(b"lv_init\0").ok()? },
            create: *unsafe {
                library
                    .get::<LvDisplayCreate>(b"lv_display_create\0")
                    .ok()?
            },
            delete: *unsafe {
                library
                    .get::<LvDisplayDelete>(b"lv_display_delete\0")
                    .ok()?
            },
            set_color_format: *unsafe {
                library
                    .get::<LvDisplaySetColorFormat>(b"lv_display_set_color_format\0")
                    .ok()?
            },
            set_buffers: *unsafe {
                library
                    .get::<LvDisplaySetBuffers>(b"lv_display_set_buffers\0")
                    .ok()?
            },
            set_flush_callback: *unsafe {
                library
                    .get::<LvDisplaySetFlushCallback>(b"lv_display_set_flush_cb\0")
                    .ok()?
            },
            flush_ready: *unsafe {
                library
                    .get::<LvDisplayFlushReady>(b"lv_display_flush_ready\0")
                    .ok()?
            },
        })
    }
}

struct LvglV9Display {
    display: *mut c_void,
    width: u32,
    height: u32,
    framebuffer: Vec<u8>,
    _draw_buffer: Vec<u8>,
    controller: Option<St7789Controller>,
    api: LvglV9Api,
}

thread_local! {
    static DISPLAYS: RefCell<Vec<LvglV9Display>> = const { RefCell::new(Vec::new()) };
}

unsafe extern "C" fn flush_callback_v9(display: *mut c_void, area: *const LvArea, pixels: *mut u8) {
    if display.is_null() {
        return;
    }
    DISPLAYS.with(|displays| {
        let mut displays = displays.borrow_mut();
        let Some(state) = displays
            .iter_mut()
            .find(|candidate| candidate.display == display)
        else {
            return;
        };
        if let (Some(area), false) = (unsafe { area.as_ref() }, pixels.is_null()) {
            state.flush(*area, pixels);
        }
        // LVGL requires completion exactly once even when an invalid area was
        // rejected, otherwise its render pipeline remains permanently busy.
        unsafe { (state.api.flush_ready)(display) };
    });
}

impl LvglV9Display {
    fn flush(&mut self, area: LvArea, pixels: *const u8) {
        if area.x1 < 0
            || area.y1 < 0
            || area.x2 < area.x1
            || area.y2 < area.y1
            || area.x2 as u32 >= self.width
            || area.y2 as u32 >= self.height
        {
            return;
        }
        let width = (area.x2 - area.x1 + 1) as u32;
        let height = (area.y2 - area.y1 + 1) as u32;
        let Some(source_len) = crate::framebuffer_size(width, height) else {
            return;
        };
        // SAFETY: LVGL guarantees the inclusive flush area is tightly packed
        // in `pixels` for the duration of this callback.
        let source = unsafe { std::slice::from_raw_parts(pixels, source_len) };

        if let Some(controller) = self.controller.as_mut() {
            let columns = [
                (area.x1 as u16).to_be_bytes(),
                (area.x2 as u16).to_be_bytes(),
            ]
            .concat();
            let rows = [
                (area.y1 as u16).to_be_bytes(),
                (area.y2 as u16).to_be_bytes(),
            ]
            .concat();
            let result = controller
                .write_command(ST7789_CASET, &columns)
                .and_then(|_| controller.write_command(ST7789_RASET, &rows))
                .and_then(|_| controller.write_command(ST7789_RAMWR, &[]))
                .and_then(|_| {
                    controller.write_pixels(
                        &host_rgb565_to_st7789_wire(source).expect("even RGB565 source"),
                    )
                });
            if result.is_ok() {
                self.framebuffer
                    .copy_from_slice(controller.framebuffer_host_rgb565());
            }
        } else {
            let _ = crate::framebuffer::update_rgb565(
                &mut self.framebuffer,
                self.width,
                self.height,
                source,
                Rect {
                    x: area.x1 as u32,
                    y: area.y1 as u32,
                    width,
                    height,
                },
            );
        }
    }
}

/// Initialize a v9 partial display in the active firmware's LVGL runtime.
pub fn lvgl_v9_init_sdl(instance_id: &str, width: i32, height: i32) -> *mut c_void {
    lvgl_v9_init_sdl_with_options(instance_id, width, height, DisplayBackendOptions::default())
}

pub(crate) fn lvgl_v9_init_sdl_with_options(
    instance_id: &str,
    width: i32,
    height: i32,
    options: DisplayBackendOptions,
) -> *mut c_void {
    crate::with_active_firmware_library(|library| unsafe {
        lvgl_v9_init_sdl_with_library_and_options(library, instance_id, width, height, options)
    })
    .unwrap_or(ptr::null_mut())
}

/// Initialize a v9 display by resolving every symbol from one firmware library.
///
/// # Safety
///
/// `library` must contain LVGL v9 functions with the declared ABIs.
pub unsafe fn lvgl_v9_init_sdl_with_library(
    library: &Library,
    instance_id: &str,
    width: i32,
    height: i32,
) -> *mut c_void {
    unsafe {
        lvgl_v9_init_sdl_with_library_and_options(
            library,
            instance_id,
            width,
            height,
            DisplayBackendOptions::default(),
        )
    }
}

unsafe fn lvgl_v9_init_sdl_with_library_and_options(
    library: &Library,
    instance_id: &str,
    width: i32,
    height: i32,
    options: DisplayBackendOptions,
) -> *mut c_void {
    if instance_id.is_empty()
        || !crate::is_t_deck_resolution(width, height)
        || options.validated(height as u32).is_none()
    {
        return ptr::null_mut();
    }
    let Some(api) = (unsafe { LvglV9Api::load(library) }) else {
        return ptr::null_mut();
    };
    let Some(framebuffer_len) = crate::framebuffer_size(width as u32, height as u32) else {
        return ptr::null_mut();
    };
    let Some(draw_buffer_len) = (width as usize)
        .checked_mul(options.draw_buffer_rows as usize)
        .and_then(|pixels| pixels.checked_mul(BYTES_PER_PIXEL))
    else {
        return ptr::null_mut();
    };
    let Ok(draw_buffer_len_u32) = u32::try_from(draw_buffer_len) else {
        return ptr::null_mut();
    };

    unsafe { (api.init)() };
    let display = unsafe { (api.create)(width, height) };
    if display.is_null() {
        return ptr::null_mut();
    }
    let mut draw_buffer = vec![0; draw_buffer_len];
    let controller = if options.st7789_fidelity {
        let mut controller =
            St7789Controller::new(width as u16, height as u16, SharedSpiBus::default());
        controller.set_reset(false);
        controller.set_reset(true);
        if controller.initialize_t_deck().is_err() {
            unsafe { (api.delete)(display) };
            return ptr::null_mut();
        }
        controller.set_backlight(u8::MAX);
        Some(controller)
    } else {
        None
    };

    unsafe {
        (api.set_color_format)(display, LV_COLOR_FORMAT_RGB565);
        (api.set_buffers)(
            display,
            draw_buffer.as_mut_ptr().cast(),
            ptr::null_mut(),
            draw_buffer_len_u32,
            LV_DISPLAY_RENDER_MODE_PARTIAL,
        );
    }
    DISPLAYS.with(|displays| {
        displays.borrow_mut().push(LvglV9Display {
            display,
            width: width as u32,
            height: height as u32,
            framebuffer: vec![0; framebuffer_len],
            _draw_buffer: draw_buffer,
            controller,
            api,
        });
    });
    unsafe { (api.set_flush_callback)(display, Some(flush_callback_v9)) };
    display
}

/// Capture the persistent logical framebuffer owned by the v9 driver.
///
/// # Safety
///
/// `display` must be null or a live display returned by this module.
pub unsafe fn capture_lvgl_rgb565(display: *mut c_void) -> Option<Vec<u8>> {
    unsafe { capture_managed_rgb565(display) }
}

/// Compatibility entry point retaining the prior library-scoped signature.
///
/// Capture no longer uses symbols or SDL renderer state from `library`.
///
/// # Safety
///
/// `display` must be null or a live display returned by this module.
pub unsafe fn capture_lvgl_rgb565_with_library(
    _library: &Library,
    display: *mut c_void,
) -> Option<Vec<u8>> {
    unsafe { capture_managed_rgb565(display) }
}

pub(crate) unsafe fn capture_managed_rgb565(display: *mut c_void) -> Option<Vec<u8>> {
    DISPLAYS.with(|displays| {
        displays
            .borrow()
            .iter()
            .find(|candidate| candidate.display == display)
            .map(|candidate| candidate.framebuffer.clone())
    })
}

pub(crate) unsafe fn destroy_display(display: *mut c_void) -> bool {
    if display.is_null() {
        return false;
    }
    let removed = DISPLAYS.with(|displays| {
        let mut displays = displays.borrow_mut();
        displays
            .iter()
            .position(|candidate| candidate.display == display)
            .map(|index| displays.remove(index))
    });
    let Some(removed) = removed else {
        return false;
    };
    unsafe { (removed.api.delete)(display) };
    true
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
