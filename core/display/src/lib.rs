//! SDL2-backed display emulation for virtual T-Deck instances.

mod config;
mod lvgl_v8;
pub mod lvgl_v9;
pub mod manager;
mod version;
pub mod window;

use std::cell::Cell;
use std::ffi::{c_char, c_void, CStr};

use libloading::Library;
use sdl2::render::Canvas;
use sdl2::video::Window;

pub use config::{DisplayConfig, T_DECK_HEIGHT, T_DECK_WIDTH};
pub use lvgl_v8::lvgl_v8_init_sdl;
pub use lvgl_v9::lvgl_v9_init_sdl;
pub use manager::DisplayManager;
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

pub(crate) enum BackendState {
    V8(lvgl_v8::LvglV8State),
}

impl BackendState {
    fn is_valid(&self, width: u32, height: u32) -> bool {
        match self {
            Self::V8(state) => state.is_valid(width, height),
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
        if !backend.is_valid(width, height) {
            return None;
        }

        let sdl = sdl2::init().ok()?;
        let video = sdl.video().ok()?;
        let window = video
            .window(instance_id, width, height)
            .position_centered()
            .hidden()
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
        LvglVersion::V8 => lvgl_v8_init_sdl(instance_id, width, height),
        LvglVersion::V9 | LvglVersion::Unknown => lvgl_v9_init_sdl(instance_id, width, height),
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
    let header = unsafe { display.cast::<version::DisplayHeader>().as_ref()? };
    if !header.is_mycelium_handle() {
        return None;
    }
    let display = unsafe { &*display.cast::<DisplayHandle>() };
    Some(display.framebuffer.clone())
}

/// Destroy a host-managed LVGL v8 compatibility display.
///
/// # Safety
///
/// `display` must be null or a live handle returned by [`lvgl_v8_init_sdl`].
pub unsafe fn destroy_managed_display(display: *mut c_void) {
    let Some(header) = (unsafe { display.cast::<version::DisplayHeader>().as_ref() }) else {
        return;
    };
    if header.is_mycelium_handle() {
        unsafe { drop(Box::from_raw(display.cast::<DisplayHandle>())) };
    }
}

/// Destroy a Mycelium-managed compatibility display.
///
/// Native LVGL v9 displays remain owned by the firmware's LVGL runtime.
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
    fn v8_compatibility_initializer_returns_a_versioned_handle() {
        let _serial = SDL_TEST_LOCK.lock().unwrap();
        std::env::set_var("SDL_VIDEODRIVER", "dummy");
        let display = lvgl_v8_init_sdl("v8 test", 320, 240);

        assert!(!display.is_null());
        assert_eq!(LvglVersion::detect(display), LvglVersion::V8);
        let display_ref = unsafe { &*display.cast::<DisplayHandle>() };
        assert_eq!(display_ref.framebuffer.len(), 320 * 240 * BYTES_PER_PIXEL);
        assert_eq!(
            unsafe { capture_managed_rgb565(display) }.unwrap().len(),
            320 * 240 * BYTES_PER_PIXEL
        );

        unsafe { destroy_managed_display(display) };
    }

    #[test]
    fn versioned_create_preserves_v8_compatibility() {
        let _serial = SDL_TEST_LOCK.lock().unwrap();
        std::env::set_var("SDL_VIDEODRIVER", "dummy");
        let title = std::ffi::CString::new("v8 versioned").unwrap();
        let display = unsafe { meshemu_display_create_v(320, 240, title.as_ptr(), 8) };

        assert_eq!(LvglVersion::detect(display), LvglVersion::V8);
        unsafe { meshemu_display_destroy(display) };
    }

    #[test]
    fn t_deck_initializers_reject_non_native_geometry() {
        assert!(meshemu_display_init("wide", 640, 480, LvglVersion::V8).is_null());
        assert!(meshemu_display_init("short", 320, 200, LvglVersion::V9).is_null());
    }
}
