use std::ffi::c_void;
use std::ptr;

use crate::{BackendState, DisplayHandle, LvglVersion};

const LV_COLOR_DEPTH: u32 = 16;

type FlushCallbackV8 = unsafe extern "C" fn(*mut LvDispDrv);

/// Minimal host-side representation of LVGL v8's `lv_disp_draw_buf_t`.
#[derive(Debug)]
pub(crate) struct LvDispDrawBuf {
    pub(crate) size_in_pixels: usize,
}

/// Minimal host-side representation of LVGL v8's `lv_disp_drv_t`.
#[derive(Debug)]
pub(crate) struct LvDispDrv {
    pub(crate) hor_res: i32,
    pub(crate) ver_res: i32,
    pub(crate) flush_cb: Option<FlushCallbackV8>,
}

#[derive(Debug)]
pub(crate) struct LvglV8State {
    pub(crate) draw_buffer: LvDispDrawBuf,
    pub(crate) driver: LvDispDrv,
}

impl LvglV8State {
    pub(crate) fn is_valid(&self, width: u32, height: u32) -> bool {
        self.draw_buffer.size_in_pixels
            == (width as usize).checked_mul(height as usize).unwrap_or(0)
            && self.driver.hor_res == width as i32
            && self.driver.ver_res == height as i32
            && self.driver.flush_cb.is_some()
    }
}

unsafe extern "C" fn sdl_flush_callback_v8(_driver: *mut LvDispDrv) {
    // The SDL canvas is owned by DisplayHandle. A real v8 flush completes by
    // calling lv_disp_flush_ready(); the host callback is synchronous.
}

/// Initializes the LVGL v8-style draw buffer and display driver over SDL2.
pub fn lvgl_v8_init_sdl(instance_id: &str, width: i32, height: i32) -> *mut c_void {
    let Some(pixel_count) = width.try_into().ok().and_then(|width: usize| {
        height
            .try_into()
            .ok()
            .and_then(|height: usize| width.checked_mul(height))
    }) else {
        return ptr::null_mut();
    };
    if pixel_count == 0 || LV_COLOR_DEPTH != 16 {
        return ptr::null_mut();
    }

    let state = LvglV8State {
        draw_buffer: LvDispDrawBuf {
            size_in_pixels: pixel_count,
        },
        driver: LvDispDrv {
            hor_res: width,
            ver_res: height,
            flush_cb: Some(sdl_flush_callback_v8),
        },
    };
    DisplayHandle::new(
        instance_id,
        width,
        height,
        LvglVersion::V8,
        BackendState::V8(state),
    )
    .map_or(ptr::null_mut(), |display| Box::into_raw(display).cast())
}
