#![cfg(target_os = "linux")]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use libloading::{Library, Symbol};
use mycelium_display::lvgl_v9::{capture_lvgl_rgb565_with_library, lvgl_v9_init_sdl_with_library};
use mycelium_display::{destroy_managed_display, with_firmware_library};

type CounterFn = unsafe extern "C" fn() -> i32;
type U32Fn = unsafe extern "C" fn() -> u32;
type FlushFn = unsafe extern "C" fn(i32, i32, i32, i32, *mut u16);

fn build_mock(build_dir: &Path) -> PathBuf {
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/mock_lvgl_v9.c");
    let library_path = build_dir.join("mock-lvgl-v9.so");
    let status = Command::new("cc")
        .args(["-shared", "-fPIC"])
        .arg(fixture)
        .arg("-o")
        .arg(&library_path)
        .status()
        .expect("run C compiler");
    assert!(status.success(), "compile mock LVGL firmware");
    library_path
}

#[test]
fn v9_uses_persistent_partial_flush_memory_and_backend_aware_destroy() {
    let build_dir =
        std::env::temp_dir().join(format!("mycelium-lvgl-v9-test-{}", std::process::id()));
    fs::create_dir_all(&build_dir).expect("create fixture build directory");
    let library_path = build_mock(&build_dir);

    // SAFETY: The test library remains live for every symbol call and exports
    // the exact mock signatures declared above.
    unsafe {
        let library = Library::new(library_path).expect("load LVGL mock");
        let display = with_firmware_library(&library, || {
            mycelium_display::lvgl_v9_init_sdl("node-good", 320, 240)
        });
        assert!(!display.is_null());
        assert_eq!(
            library.get::<U32Fn>(b"mock_color_format\0").unwrap()(),
            0x12
        );
        assert_eq!(
            library.get::<U32Fn>(b"mock_buffer_size\0").unwrap()(),
            320 * 24 * 2
        );
        assert_eq!(library.get::<U32Fn>(b"mock_render_mode\0").unwrap()(), 0);

        let flush: Symbol<FlushFn> = library.get(b"mock_flush_area\0").unwrap();
        let mut single = [0xf800_u16];
        flush(0, 0, 0, 0, single.as_mut_ptr());
        let mut edge = [0x07e0_u16];
        flush(319, 239, 319, 239, edge.as_mut_ptr());
        let mut rows = [1_u16, 2, 3, 4, 5, 6];
        flush(10, 20, 12, 21, rows.as_mut_ptr());

        let pixels = capture_lvgl_rgb565_with_library(&library, display).unwrap();
        let pixel = |x: usize, y: usize| {
            let offset = (y * 320 + x) * 2;
            u16::from_ne_bytes([pixels[offset], pixels[offset + 1]])
        };
        assert_eq!(pixel(0, 0), 0xf800);
        assert_eq!(pixel(319, 239), 0x07e0);
        assert_eq!((pixel(10, 20), pixel(12, 20)), (1, 3));
        assert_eq!((pixel(10, 21), pixel(12, 21)), (4, 6));
        assert_eq!(pixel(100, 100), 0);
        assert_eq!(
            library
                .get::<CounterFn>(b"mock_flush_ready_calls\0")
                .unwrap()(),
            3
        );

        with_firmware_library(&library, || destroy_managed_display(display));
        assert_eq!(
            library.get::<CounterFn>(b"mock_delete_calls\0").unwrap()(),
            1
        );
        assert!(capture_lvgl_rgb565_with_library(&library, display).is_none());

        assert!(lvgl_v9_init_sdl_with_library(&library, "wrong-size", 640, 480).is_null());
    }

    fs::remove_dir_all(build_dir).expect("remove fixture build directory");
}
