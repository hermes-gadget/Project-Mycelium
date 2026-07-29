use std::ffi::c_void;
use std::ptr;

use crate::{BackendState, DisplayHandle, LvglVersion};

const LV_COLOR_FORMAT_RGB565_BYTES_PER_PIXEL: usize = 2;

type FlushCallbackV9 = unsafe extern "C" fn(*mut LvDisplay);

/// Minimal host-side representation of LVGL v9's `lv_draw_buf_t`.
#[derive(Debug)]
pub(crate) struct LvDrawBuf {
    pub(crate) size_in_bytes: usize,
}

/// Minimal host-side representation of LVGL v9's `lv_display_t`.
#[derive(Debug)]
pub(crate) struct LvDisplay {
    pub(crate) hor_res: i32,
    pub(crate) ver_res: i32,
    pub(crate) flush_cb: Option<FlushCallbackV9>,
}

#[derive(Debug)]
pub(crate) struct LvglV9State {
    pub(crate) draw_buffer: LvDrawBuf,
    pub(crate) display: LvDisplay,
}

impl LvglV9State {
    pub(crate) fn is_valid(&self, width: u32, height: u32, framebuffer_len: usize) -> bool {
        self.draw_buffer.size_in_bytes == framebuffer_len
            && self.display.hor_res == width as i32
            && self.display.ver_res == height as i32
            && self.display.flush_cb.is_some()
    }
}

unsafe extern "C" fn sdl_flush_callback_v9(_display: *mut LvDisplay) {
    // The SDL canvas is owned by DisplayHandle. A real v9 flush completes by
    // calling lv_display_flush_ready(); the host callback is synchronous.
}

/// Initializes the LVGL v9-style draw buffer and display over SDL2.
pub fn lvgl_v9_init_sdl(instance_id: &str, width: i32, height: i32) -> *mut c_void {
    let Some(buffer_size) = width.try_into().ok().and_then(|width: usize| {
        height.try_into().ok().and_then(|height: usize| {
            width
                .checked_mul(height)?
                .checked_mul(LV_COLOR_FORMAT_RGB565_BYTES_PER_PIXEL)
        })
    }) else {
        return ptr::null_mut();
    };
    if buffer_size == 0 {
        return ptr::null_mut();
    }

    let state = LvglV9State {
        draw_buffer: LvDrawBuf {
            size_in_bytes: buffer_size,
        },
        display: LvDisplay {
            hor_res: width,
            ver_res: height,
            flush_cb: Some(sdl_flush_callback_v9),
        },
    };
    DisplayHandle::new(
        instance_id,
        width,
        height,
        LvglVersion::V9,
        BackendState::V9(state),
    )
    .map_or(ptr::null_mut(), |display| Box::into_raw(display).cast())
}
