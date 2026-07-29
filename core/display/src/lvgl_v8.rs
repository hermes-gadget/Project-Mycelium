//! Runtime-bound LVGL v8 display driver backed by SDL2.
//!
//! Firmware shared libraries bring their own LVGL build. Resolving the v8
//! symbols from the active firmware library avoids linking a second LVGL copy
//! while still registering a real `lv_disp_drv_t` with that firmware runtime.

use std::cell::RefCell;
use std::ffi::{c_int, c_void, CString};
use std::ptr;

use libloading::Library;
use sdl2::pixels::PixelFormatEnum;

use crate::shared_spi::SharedSpiBus;
use crate::st7789::{St7789Controller, ST7789_CASET, ST7789_RAMWR, ST7789_RASET};
use crate::{framebuffer_size, host_rgb565_to_st7789_wire, DisplayBackendOptions, Rect};

const LV_COLOR_DEPTH: u32 = 16;

/// LVGL v8's default `lv_coord_t` for displays smaller than 32768 pixels.
type LvCoord = i16;

/// ABI-compatible representation of LVGL v8's `lv_area_t`.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub(crate) struct LvArea {
    pub(crate) x1: LvCoord,
    pub(crate) y1: LvCoord,
    pub(crate) x2: LvCoord,
    pub(crate) y2: LvCoord,
}

/// ABI-compatible RGB565 representation of LVGL v8's `lv_color_t`.
#[repr(transparent)]
#[derive(Clone, Copy)]
pub(crate) struct LvColor(u16);

/// ABI-compatible representation of LVGL v8's `lv_disp_draw_buf_t`.
#[repr(C)]
pub(crate) struct LvDispDrawBuf {
    buf1: *mut c_void,
    buf2: *mut c_void,
    buf_act: *mut c_void,
    size: u32,
    flushing: c_int,
    flushing_last: c_int,
    flags: u32,
}

type FlushCallbackV8 = unsafe extern "C" fn(*mut LvDispDrv, *const LvArea, *mut LvColor);
type RounderCallbackV8 = unsafe extern "C" fn(*mut LvDispDrv, *mut LvArea);
type SetPixelCallbackV8 =
    unsafe extern "C" fn(*mut LvDispDrv, *mut u8, LvCoord, LvCoord, LvCoord, LvColor, u8);
type ClearCallbackV8 = unsafe extern "C" fn(*mut LvDispDrv, *mut u8, u32);
type MonitorCallbackV8 = unsafe extern "C" fn(*mut LvDispDrv, u32, u32);
type DriverCallbackV8 = unsafe extern "C" fn(*mut LvDispDrv);
type DrawContextCallbackV8 = unsafe extern "C" fn(*mut LvDispDrv, *mut c_void);

/// ABI-compatible representation of the canonical LVGL v8.3
/// `lv_disp_drv_t` with 16-bit coordinates and colors.
///
/// `user_data` is included as trailing storage even when LVGL was compiled
/// with `LV_USE_USER_DATA=0`. All preceding offsets are identical, and the
/// extra trailing word makes the allocation large enough for either setting.
#[repr(C)]
pub(crate) struct LvDispDrv {
    hor_res: LvCoord,
    ver_res: LvCoord,
    physical_hor_res: LvCoord,
    physical_ver_res: LvCoord,
    offset_x: LvCoord,
    offset_y: LvCoord,
    draw_buf: *mut LvDispDrawBuf,
    flags_and_dpi: u32,
    flush_cb: Option<FlushCallbackV8>,
    rounder_cb: Option<RounderCallbackV8>,
    set_px_cb: Option<SetPixelCallbackV8>,
    clear_cb: Option<ClearCallbackV8>,
    monitor_cb: Option<MonitorCallbackV8>,
    wait_cb: Option<DriverCallbackV8>,
    clean_dcache_cb: Option<DriverCallbackV8>,
    drv_update_cb: Option<DriverCallbackV8>,
    render_start_cb: Option<DriverCallbackV8>,
    color_chroma_key: LvColor,
    draw_ctx: *mut c_void,
    draw_ctx_init: Option<DrawContextCallbackV8>,
    draw_ctx_deinit: Option<DrawContextCallbackV8>,
    draw_ctx_size: usize,
    user_data: *mut c_void,
}

type LvInit = unsafe extern "C" fn();
type LvDispDrawBufInit = unsafe extern "C" fn(*mut LvDispDrawBuf, *mut c_void, *mut c_void, u32);
type LvDispDrvInit = unsafe extern "C" fn(*mut LvDispDrv);
type LvDispDrvRegister = unsafe extern "C" fn(*mut LvDispDrv) -> *mut c_void;
type LvDispFlushReady = unsafe extern "C" fn(*mut LvDispDrv);
type LvDispRemove = unsafe extern "C" fn(*mut c_void);

#[derive(Clone, Copy)]
struct LvglV8Api {
    init: LvInit,
    draw_buf_init: LvDispDrawBufInit,
    drv_init: LvDispDrvInit,
    drv_register: LvDispDrvRegister,
    flush_ready: LvDispFlushReady,
    disp_remove: Option<LvDispRemove>,
}

impl LvglV8Api {
    unsafe fn load(library: &Library) -> Option<Self> {
        Some(Self {
            init: *unsafe { library.get::<LvInit>(b"lv_init\0").ok()? },
            draw_buf_init: *unsafe {
                library
                    .get::<LvDispDrawBufInit>(b"lv_disp_draw_buf_init\0")
                    .ok()?
            },
            drv_init: *unsafe { library.get::<LvDispDrvInit>(b"lv_disp_drv_init\0").ok()? },
            drv_register: *unsafe {
                library
                    .get::<LvDispDrvRegister>(b"lv_disp_drv_register\0")
                    .ok()?
            },
            flush_ready: *unsafe {
                library
                    .get::<LvDispFlushReady>(b"lv_disp_flush_ready\0")
                    .ok()?
            },
            disp_remove: unsafe {
                library
                    .get::<LvDispRemove>(b"lv_disp_remove\0")
                    .ok()
                    .map(|symbol| *symbol)
            },
        })
    }
}

struct SdlDisplay {
    _sdl: sdl2::Sdl,
    window: *mut sdl2::sys::SDL_Window,
    renderer: *mut sdl2::sys::SDL_Renderer,
    texture: *mut sdl2::sys::SDL_Texture,
}

impl SdlDisplay {
    fn new(instance_id: &str, width: i32, height: i32) -> Option<Self> {
        let title = CString::new(format!("T-Deck — {instance_id}")).ok()?;
        let sdl = sdl2::init().ok()?;

        // SAFETY: SDL is initialized above; each successful allocation is
        // retained by SdlDisplay and released in reverse order by Drop.
        unsafe {
            let window = sdl2::sys::SDL_CreateWindow(
                title.as_ptr(),
                sdl2::sys::SDL_WINDOWPOS_CENTERED_MASK as i32,
                sdl2::sys::SDL_WINDOWPOS_CENTERED_MASK as i32,
                width,
                height,
                sdl2::sys::SDL_WindowFlags::SDL_WINDOW_HIDDEN as u32,
            );
            if window.is_null() {
                return None;
            }

            let renderer = sdl2::sys::SDL_CreateRenderer(
                window,
                -1,
                sdl2::sys::SDL_RendererFlags::SDL_RENDERER_SOFTWARE as u32,
            );
            if renderer.is_null() {
                sdl2::sys::SDL_DestroyWindow(window);
                return None;
            }

            let texture = sdl2::sys::SDL_CreateTexture(
                renderer,
                PixelFormatEnum::RGB565 as u32,
                sdl2::sys::SDL_TextureAccess::SDL_TEXTUREACCESS_STREAMING as c_int,
                width,
                height,
            );
            if texture.is_null() {
                sdl2::sys::SDL_DestroyRenderer(renderer);
                sdl2::sys::SDL_DestroyWindow(window);
                return None;
            }

            Some(Self {
                _sdl: sdl,
                window,
                renderer,
                texture,
            })
        }
    }
}

impl Drop for SdlDisplay {
    fn drop(&mut self) {
        // SAFETY: These handles were created together in `new`, remain owned
        // by this value, and are destroyed exactly once in dependency order.
        unsafe {
            sdl2::sys::SDL_DestroyTexture(self.texture);
            sdl2::sys::SDL_DestroyRenderer(self.renderer);
            sdl2::sys::SDL_DestroyWindow(self.window);
        }
    }
}

struct LvglV8Display {
    display: *mut c_void,
    width: usize,
    height: usize,
    framebuffer: Vec<u8>,
    controller: Option<St7789Controller>,
    _pixels: Vec<u8>,
    _draw_buf: Box<LvDispDrawBuf>,
    driver: Box<LvDispDrv>,
    sdl: SdlDisplay,
    api: LvglV8Api,
}

thread_local! {
    static DISPLAYS: RefCell<Vec<LvglV8Display>> = const { RefCell::new(Vec::new()) };
}

unsafe extern "C" fn sdl_flush_callback_v8(
    driver: *mut LvDispDrv,
    area: *const LvArea,
    color_p: *mut LvColor,
) {
    if driver.is_null() {
        return;
    }

    DISPLAYS.with(|displays| {
        let mut displays = displays.borrow_mut();
        let Some(display) = displays
            .iter_mut()
            .find(|display| ptr::eq(display.driver.as_ref(), driver))
        else {
            return;
        };

        if let (Some(area), false) = (unsafe { area.as_ref() }, color_p.is_null()) {
            display.flush(area, color_p.cast());
        }

        // SAFETY: `driver` is the live driver registered with this API and
        // LVGL requires ready to be signalled exactly once per callback.
        unsafe { (display.api.flush_ready)(driver) };
    });
}

impl LvglV8Display {
    fn flush(&mut self, area: &LvArea, pixels: *const u8) {
        let x1 = i32::from(area.x1);
        let y1 = i32::from(area.y1);
        let x2 = i32::from(area.x2);
        let y2 = i32::from(area.y2);
        if x1 < 0
            || y1 < 0
            || x2 < x1
            || y2 < y1
            || x2 as usize >= self.width
            || y2 as usize >= self.height
        {
            return;
        }

        let width = (x2 - x1 + 1) as usize;
        let height = (y2 - y1 + 1) as usize;
        let Some(source_len) = width.checked_mul(height).and_then(|px| px.checked_mul(2)) else {
            return;
        };
        // SAFETY: LVGL guarantees `color_p` contains the packed pixels for the
        // inclusive flush area for the duration of this callback.
        let source = unsafe { std::slice::from_raw_parts(pixels, source_len) };
        if let Some(controller) = self.controller.as_mut() {
            let column = [(x1 as u16).to_be_bytes(), (x2 as u16).to_be_bytes()].concat();
            let row = [(y1 as u16).to_be_bytes(), (y2 as u16).to_be_bytes()].concat();
            let result = controller
                .write_command(ST7789_CASET, &column)
                .and_then(|_| controller.write_command(ST7789_RASET, &row))
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
                self.width as u32,
                self.height as u32,
                source,
                Rect {
                    x: x1 as u32,
                    y: y1 as u32,
                    width: width as u32,
                    height: height as u32,
                },
            );
        }

        let rect = sdl2::sys::SDL_Rect {
            x: x1,
            y: y1,
            w: width as i32,
            h: height as i32,
        };
        // SAFETY: The source slice is contiguous with the stated pitch and the
        // SDL handles remain live for this display.
        unsafe {
            if sdl2::sys::SDL_UpdateTexture(
                self.sdl.texture,
                &rect,
                source.as_ptr().cast(),
                (width * 2) as c_int,
            ) == 0
            {
                sdl2::sys::SDL_RenderClear(self.sdl.renderer);
                sdl2::sys::SDL_RenderCopy(
                    self.sdl.renderer,
                    self.sdl.texture,
                    ptr::null(),
                    ptr::null(),
                );
                sdl2::sys::SDL_RenderPresent(self.sdl.renderer);
            }
        }
    }
}

/// Initialize LVGL v8 and register an SDL2-backed RGB565 display driver.
///
/// Returns the real `lv_disp_t *` returned by `lv_disp_drv_register`, or null
/// when the dimensions are invalid, SDL setup fails, required LVGL v8 symbols
/// are unavailable, or LVGL rejects the driver.
///
/// The display and its SDL resources are thread-affine and must be used,
/// captured, and destroyed on the thread that creates them.
pub fn lvgl_v8_init_sdl(instance_id: &str, width: i32, height: i32) -> *mut c_void {
    lvgl_v8_init_sdl_with_options(instance_id, width, height, DisplayBackendOptions::default())
}

pub(crate) fn lvgl_v8_init_sdl_with_options(
    instance_id: &str,
    width: i32,
    height: i32,
    options: DisplayBackendOptions,
) -> *mut c_void {
    if instance_id.is_empty() || !crate::is_t_deck_resolution(width, height) {
        return ptr::null_mut();
    }
    let Some(api) =
        crate::with_active_firmware_library(|library| unsafe { LvglV8Api::load(library) })
            .flatten()
    else {
        return ptr::null_mut();
    };
    unsafe { lvgl_v8_init_sdl_with_api(instance_id, width, height, api, options) }
}

unsafe fn lvgl_v8_init_sdl_with_api(
    instance_id: &str,
    width: i32,
    height: i32,
    api: LvglV8Api,
    options: DisplayBackendOptions,
) -> *mut c_void {
    if instance_id.is_empty()
        || width <= 0
        || height <= 0
        || width > LvCoord::MAX.into()
        || height > LvCoord::MAX.into()
        || LV_COLOR_DEPTH != 16
        || options.validated(height as u32).is_none()
    {
        return ptr::null_mut();
    }
    let Some(pixel_count) = (width as usize).checked_mul(options.draw_buffer_rows as usize) else {
        return ptr::null_mut();
    };
    let Ok(pixel_count_u32) = u32::try_from(pixel_count) else {
        return ptr::null_mut();
    };
    let Some(buffer_len) = framebuffer_size(width as u32, height as u32) else {
        return ptr::null_mut();
    };
    unsafe { (api.init)() };
    let Some(sdl) = SdlDisplay::new(instance_id, width, height) else {
        return ptr::null_mut();
    };

    let mut pixels = vec![0_u8; buffer_len];
    // SAFETY: These C-layout values are immediately initialized by LVGL before
    // any field is read.
    let mut draw_buf = Box::new(unsafe { std::mem::zeroed::<LvDispDrawBuf>() });
    let mut driver = Box::new(unsafe { std::mem::zeroed::<LvDispDrv>() });
    let controller = if options.st7789_fidelity {
        let mut controller =
            St7789Controller::new(width as u16, height as u16, SharedSpiBus::default());
        controller.set_reset(false);
        controller.set_reset(true);
        if controller.initialize_t_deck().is_err() {
            return ptr::null_mut();
        }
        controller.set_backlight(u8::MAX);
        Some(controller)
    } else {
        None
    };

    unsafe {
        (api.draw_buf_init)(
            draw_buf.as_mut(),
            pixels.as_mut_ptr().cast(),
            ptr::null_mut(),
            pixel_count_u32,
        );
        (api.drv_init)(driver.as_mut());
    }
    driver.hor_res = width as LvCoord;
    driver.ver_res = height as LvCoord;
    driver.flush_cb = Some(sdl_flush_callback_v8);
    driver.draw_buf = draw_buf.as_mut();

    let driver_ptr = driver.as_mut() as *mut LvDispDrv;
    DISPLAYS.with(|displays| {
        displays.borrow_mut().push(LvglV8Display {
            display: ptr::null_mut(),
            width: width as usize,
            height: height as usize,
            framebuffer: vec![0; buffer_len],
            controller,
            _pixels: pixels,
            _draw_buf: draw_buf,
            driver,
            sdl,
            api,
        });
    });

    let display = unsafe { (api.drv_register)(driver_ptr) };
    DISPLAYS.with(|displays| {
        let mut displays = displays.borrow_mut();
        let index = displays
            .iter()
            .position(|candidate| ptr::eq(candidate.driver.as_ref(), driver_ptr))
            .expect("newly installed LVGL v8 driver state disappeared");
        if display.is_null() {
            displays.remove(index);
        } else {
            displays[index].display = display;
        }
    });
    display
}

pub(crate) fn is_v8_display(display: *mut c_void) -> bool {
    !display.is_null()
        && DISPLAYS.with(|displays| {
            displays
                .borrow()
                .iter()
                .any(|candidate| candidate.display == display)
        })
}

pub(crate) unsafe fn capture_rgb565(display: *mut c_void) -> Option<Vec<u8>> {
    DISPLAYS.with(|displays| {
        displays
            .borrow()
            .iter()
            .find(|candidate| candidate.display == display)
            .map(|display| display.framebuffer.clone())
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
    if let Some(remove) = removed.api.disp_remove {
        unsafe { remove(display) };
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicPtr, AtomicUsize, Ordering};

    static DRIVER: AtomicPtr<LvDispDrv> = AtomicPtr::new(ptr::null_mut());
    static INIT_CALLS: AtomicUsize = AtomicUsize::new(0);
    static DRAW_BUF_INIT_CALLS: AtomicUsize = AtomicUsize::new(0);
    static DRIVER_INIT_CALLS: AtomicUsize = AtomicUsize::new(0);
    static REGISTER_CALLS: AtomicUsize = AtomicUsize::new(0);
    static FLUSH_READY_CALLS: AtomicUsize = AtomicUsize::new(0);
    static REMOVE_CALLS: AtomicUsize = AtomicUsize::new(0);
    static mut FAKE_DISPLAY: u8 = 0;

    unsafe extern "C" fn fake_init() {
        INIT_CALLS.fetch_add(1, Ordering::SeqCst);
    }

    unsafe extern "C" fn fake_draw_buf_init(
        draw_buf: *mut LvDispDrawBuf,
        buf1: *mut c_void,
        buf2: *mut c_void,
        size: u32,
    ) {
        DRAW_BUF_INIT_CALLS.fetch_add(1, Ordering::SeqCst);
        unsafe {
            ptr::write(
                draw_buf,
                LvDispDrawBuf {
                    buf1,
                    buf2,
                    buf_act: buf1,
                    size,
                    flushing: 0,
                    flushing_last: 0,
                    flags: 0,
                },
            );
        }
    }

    unsafe extern "C" fn fake_driver_init(driver: *mut LvDispDrv) {
        DRIVER_INIT_CALLS.fetch_add(1, Ordering::SeqCst);
        unsafe { ptr::write_bytes(driver, 0, 1) };
    }

    unsafe extern "C" fn fake_register(driver: *mut LvDispDrv) -> *mut c_void {
        REGISTER_CALLS.fetch_add(1, Ordering::SeqCst);
        DRIVER.store(driver, Ordering::SeqCst);
        ptr::addr_of_mut!(FAKE_DISPLAY).cast()
    }

    unsafe extern "C" fn fake_flush_ready(_driver: *mut LvDispDrv) {
        FLUSH_READY_CALLS.fetch_add(1, Ordering::SeqCst);
    }

    unsafe extern "C" fn fake_remove(_display: *mut c_void) {
        REMOVE_CALLS.fetch_add(1, Ordering::SeqCst);
    }

    fn fake_api() -> LvglV8Api {
        LvglV8Api {
            init: fake_init,
            draw_buf_init: fake_draw_buf_init,
            drv_init: fake_driver_init,
            drv_register: fake_register,
            flush_ready: fake_flush_ready,
            disp_remove: Some(fake_remove),
        }
    }

    fn reset_calls() {
        DRIVER.store(ptr::null_mut(), Ordering::SeqCst);
        INIT_CALLS.store(0, Ordering::SeqCst);
        DRAW_BUF_INIT_CALLS.store(0, Ordering::SeqCst);
        DRIVER_INIT_CALLS.store(0, Ordering::SeqCst);
        REGISTER_CALLS.store(0, Ordering::SeqCst);
        FLUSH_READY_CALLS.store(0, Ordering::SeqCst);
        REMOVE_CALLS.store(0, Ordering::SeqCst);
    }

    #[test]
    fn registers_real_v8_driver_and_configurable_partial_draw_buffer() {
        let _serial = crate::SDL_TEST_LOCK.lock().unwrap();
        std::env::set_var("SDL_VIDEODRIVER", "dummy");
        reset_calls();

        let options = DisplayBackendOptions {
            draw_buffer_rows: 2,
            st7789_fidelity: false,
        };
        let display =
            unsafe { lvgl_v8_init_sdl_with_api("v8 registration", 8, 6, fake_api(), options) };

        assert!(!display.is_null());
        assert_eq!(crate::LvglVersion::detect(display), crate::LvglVersion::V8);
        assert_eq!(INIT_CALLS.load(Ordering::SeqCst), 1);
        assert_eq!(DRAW_BUF_INIT_CALLS.load(Ordering::SeqCst), 1);
        assert_eq!(DRIVER_INIT_CALLS.load(Ordering::SeqCst), 1);
        assert_eq!(REGISTER_CALLS.load(Ordering::SeqCst), 1);

        let driver = DRIVER.load(Ordering::SeqCst);
        assert!(!driver.is_null());
        let driver = unsafe { &*driver };
        assert_eq!(driver.hor_res, 8);
        assert_eq!(driver.ver_res, 6);
        assert_eq!(unsafe { (*driver.draw_buf).size }, 8 * 2);
        assert!(driver.flush_cb.is_some());

        assert!(unsafe { destroy_display(display) });
        assert_eq!(REMOVE_CALLS.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn canonical_v8_struct_layout_places_three_argument_flush_callback_correctly() {
        #[cfg(target_pointer_width = "64")]
        {
            assert_eq!(std::mem::size_of::<LvDispDrawBuf>(), 40);
            assert_eq!(std::mem::offset_of!(LvDispDrv, draw_buf), 16);
            assert_eq!(std::mem::offset_of!(LvDispDrv, flush_cb), 32);
            assert_eq!(std::mem::size_of::<LvDispDrv>(), 152);
        }
        #[cfg(target_pointer_width = "32")]
        {
            assert_eq!(std::mem::size_of::<LvDispDrawBuf>(), 28);
            assert_eq!(std::mem::offset_of!(LvDispDrv, draw_buf), 12);
            assert_eq!(std::mem::offset_of!(LvDispDrv, flush_cb), 20);
            assert_eq!(std::mem::size_of::<LvDispDrv>(), 80);
        }
    }

    #[test]
    fn flushes_known_rgb565_rectangle_captures_pixels_and_signals_ready() {
        let _serial = crate::SDL_TEST_LOCK.lock().unwrap();
        std::env::set_var("SDL_VIDEODRIVER", "dummy");
        reset_calls();
        let options = DisplayBackendOptions {
            draw_buffer_rows: 1,
            st7789_fidelity: false,
        };
        let display = unsafe { lvgl_v8_init_sdl_with_api("v8 render", 4, 3, fake_api(), options) };
        assert!(!display.is_null());

        let driver = DRIVER.load(Ordering::SeqCst);
        let area = LvArea {
            x1: 1,
            y1: 1,
            x2: 2,
            y2: 2,
        };
        // Red, green, blue, white in RGB565, little-endian host layout.
        let mut pattern = [
            LvColor(0xf800),
            LvColor(0x07e0),
            LvColor(0x001f),
            LvColor(0xffff),
        ];
        let flush = unsafe { (*driver).flush_cb.unwrap() };
        unsafe { flush(driver, &area, pattern.as_mut_ptr()) };

        assert_eq!(FLUSH_READY_CALLS.load(Ordering::SeqCst), 1);
        let captured = unsafe { capture_rgb565(display) }.unwrap();
        let pixel = |x: usize, y: usize| {
            u16::from_ne_bytes([captured[(y * 4 + x) * 2], captured[(y * 4 + x) * 2 + 1]])
        };
        assert_eq!(pixel(0, 0), 0);
        assert_eq!(pixel(1, 1), 0xf800);
        assert_eq!(pixel(2, 1), 0x07e0);
        assert_eq!(pixel(1, 2), 0x001f);
        assert_eq!(pixel(2, 2), 0xffff);

        assert!(unsafe { destroy_display(display) });
    }

    #[test]
    fn rejects_invalid_configuration_before_calling_lvgl() {
        let _serial = crate::SDL_TEST_LOCK.lock().unwrap();
        reset_calls();

        let options = DisplayBackendOptions {
            draw_buffer_rows: 1,
            st7789_fidelity: false,
        };
        assert!(unsafe { lvgl_v8_init_sdl_with_api("", 4, 3, fake_api(), options) }.is_null());
        assert!(unsafe { lvgl_v8_init_sdl_with_api("bad", 0, 3, fake_api(), options) }.is_null());
        assert!(unsafe { lvgl_v8_init_sdl_with_api("bad", 4, -1, fake_api(), options) }.is_null());
        assert_eq!(INIT_CALLS.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn registers_t_deck_realistic_partial_buffer_sizes() {
        let _serial = crate::SDL_TEST_LOCK.lock().unwrap();
        std::env::set_var("SDL_VIDEODRIVER", "dummy");
        for rows in [10, 24, 40] {
            reset_calls();
            let options = DisplayBackendOptions {
                draw_buffer_rows: rows,
                st7789_fidelity: false,
            };
            let display = unsafe {
                lvgl_v8_init_sdl_with_api(
                    "t-deck partial buffer",
                    crate::T_DECK_WIDTH as i32,
                    crate::T_DECK_HEIGHT as i32,
                    fake_api(),
                    options,
                )
            };
            assert!(!display.is_null());
            let driver = DRIVER.load(Ordering::SeqCst);
            assert_eq!(
                unsafe { (*(*driver).draw_buf).size },
                crate::T_DECK_WIDTH * rows
            );
            assert!(unsafe { destroy_display(display) });
        }
    }
}
