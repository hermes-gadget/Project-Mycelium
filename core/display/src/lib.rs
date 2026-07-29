//! SDL2-backed LVGL v8 and v9 display emulation.

mod config;
mod lvgl_v8;
mod lvgl_v9;
mod version;

use std::ffi::{c_char, c_void, CStr};
use std::ptr;

use sdl2::render::Canvas;
use sdl2::video::Window;

pub use config::DisplayConfig;
pub use lvgl_v8::lvgl_v8_init_sdl;
pub use lvgl_v9::lvgl_v9_init_sdl;
pub use version::LvglVersion;

pub(crate) const BYTES_PER_PIXEL: usize = 2;

pub(crate) enum BackendState {
    V8(lvgl_v8::LvglV8State),
    V9(lvgl_v9::LvglV9State),
}

impl BackendState {
    fn is_valid(&self, width: u32, height: u32, framebuffer_len: usize) -> bool {
        match self {
            Self::V8(state) => state.is_valid(width, height),
            Self::V9(state) => state.is_valid(width, height, framebuffer_len),
        }
    }
}

#[repr(C)]
pub(crate) struct DisplayHandle {
    _header: version::DisplayHeader,
    width: u32,
    height: u32,
    framebuffer: Vec<u8>,
    _backend: BackendState,
    _sdl: sdl2::Sdl,
    _canvas: Canvas<Window>,
}

impl DisplayHandle {
    pub(crate) fn new(
        instance_id: &str,
        width: i32,
        height: i32,
        version: LvglVersion,
        backend: BackendState,
    ) -> Option<Box<Self>> {
        let width = u32::try_from(width).ok().filter(|width| *width > 0)?;
        let height = u32::try_from(height).ok().filter(|height| *height > 0)?;
        let framebuffer_len = framebuffer_size(width, height)?;
        if !backend.is_valid(width, height, framebuffer_len) {
            return None;
        }

        let sdl = sdl2::init().ok()?;
        let video = sdl.video().ok()?;
        let window = video
            .window(instance_id, width, height)
            .position_centered()
            .build()
            .ok()?;
        let canvas = window.into_canvas().software().build().ok()?;

        Some(Box::new(Self {
            _header: version::DisplayHeader::new(version),
            width,
            height,
            framebuffer: vec![0; framebuffer_len],
            _backend: backend,
            _sdl: sdl,
            _canvas: canvas,
        }))
    }

    fn framebuffer(&self) -> &[u8] {
        debug_assert_eq!(
            Some(self.framebuffer.len()),
            framebuffer_size(self.width, self.height)
        );
        &self.framebuffer
    }
}

pub(crate) fn framebuffer_size(width: u32, height: u32) -> Option<usize> {
    (width as usize)
        .checked_mul(height as usize)?
        .checked_mul(BYTES_PER_PIXEL)
}

/// Initializes an SDL display using the requested LVGL ABI.
///
/// `Unknown` deliberately uses v9 because it is Mycelium's primary target.
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

/// Creates an SDL-backed display for a specific LVGL major version.
///
/// Unsupported version numbers follow the default v9 path.
///
/// # Safety
///
/// When non-null, `window_title` must point to a valid NUL-terminated string
/// for the duration of this call.
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

/// Creates an SDL-backed LVGL v9 display.
///
/// # Safety
///
/// When non-null, `window_title` must point to a valid NUL-terminated string
/// for the duration of this call.
#[no_mangle]
pub unsafe extern "C" fn meshemu_display_create(
    width: i32,
    height: i32,
    window_title: *const c_char,
) -> *mut c_void {
    unsafe { meshemu_display_create_v(width, height, window_title, 9) }
}

/// Copies the current RGB565 framebuffer into a C-allocated buffer.
///
/// The caller owns the returned allocation and may release it with `free()`.
///
/// # Safety
///
/// `display` must be a live Mycelium display handle. When non-null, `size_out`
/// must be writable. The display must not be concurrently destroyed.
#[no_mangle]
pub unsafe extern "C" fn meshemu_display_capture(
    display: *mut c_void,
    size_out: *mut usize,
) -> *mut u8 {
    if !size_out.is_null() {
        unsafe { *size_out = 0 };
    }
    let Some(display) = (display as *const DisplayHandle).as_ref() else {
        return ptr::null_mut();
    };
    let framebuffer = display.framebuffer();
    let captured = unsafe { libc::malloc(framebuffer.len()) }.cast::<u8>();
    if captured.is_null() {
        return ptr::null_mut();
    }
    unsafe {
        ptr::copy_nonoverlapping(framebuffer.as_ptr(), captured, framebuffer.len());
        if !size_out.is_null() {
            *size_out = framebuffer.len();
        }
    }
    captured
}

/// Destroys a display handle. Null is accepted as a no-op.
///
/// # Safety
///
/// A non-null handle must have been returned by a Mycelium display creation
/// function and must be passed exactly once.
#[no_mangle]
pub unsafe extern "C" fn meshemu_display_destroy(display: *mut c_void) {
    if !display.is_null() {
        drop(unsafe { Box::from_raw(display as *mut DisplayHandle) });
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::CString;
    use std::sync::Mutex;

    use super::*;

    static SDL_TEST: Mutex<()> = Mutex::new(());

    fn with_dummy_sdl(test: impl FnOnce()) {
        let _serial = SDL_TEST.lock().unwrap();
        std::env::set_var("SDL_VIDEODRIVER", "dummy");
        test();
    }

    #[test]
    fn v8_and_v9_initializers_return_versioned_handles() {
        with_dummy_sdl(|| {
            let v8 = lvgl_v8_init_sdl("v8 test", 320, 240);
            let v9 = lvgl_v9_init_sdl("v9 test", 320, 240);

            assert!(!v8.is_null());
            assert!(!v9.is_null());
            assert_eq!(LvglVersion::detect(v8), LvglVersion::V8);
            assert_eq!(LvglVersion::detect(v9), LvglVersion::V9);

            unsafe {
                meshemu_display_destroy(v8);
                meshemu_display_destroy(v9);
            }
        });
    }

    #[test]
    fn both_versions_allocate_full_rgb565_framebuffers() {
        with_dummy_sdl(|| {
            for version in [LvglVersion::V8, LvglVersion::V9] {
                let display = meshemu_display_init("buffer test", 17, 11, version);
                assert!(!display.is_null());

                let mut size = 0;
                let capture = unsafe { meshemu_display_capture(display, &mut size) };
                assert!(!capture.is_null());
                assert_eq!(size, 17 * 11 * BYTES_PER_PIXEL);

                unsafe {
                    libc::free(capture.cast());
                    meshemu_display_destroy(display);
                }
            }
        });
    }

    #[test]
    fn legacy_create_defaults_to_v9_and_rejects_invalid_dimensions() {
        with_dummy_sdl(|| {
            let title = CString::new("legacy").unwrap();
            let display = unsafe { meshemu_display_create(320, 240, title.as_ptr()) };
            assert_eq!(LvglVersion::detect(display), LvglVersion::V9);
            unsafe { meshemu_display_destroy(display) };

            assert!(meshemu_display_init("invalid", 0, 240, LvglVersion::V8).is_null());
        });
    }
}
