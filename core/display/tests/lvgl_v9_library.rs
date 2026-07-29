#![cfg(target_os = "linux")]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use libloading::{Library, Symbol};
use mycelium_display::lvgl_v9::{capture_lvgl_rgb565_with_library, lvgl_v9_init_sdl_with_library};
use mycelium_display::with_firmware_library;

type CounterFn = unsafe extern "C" fn() -> i32;

fn build_mock(build_dir: &Path, name: &str, color_format: u32) -> PathBuf {
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/mock_lvgl_v9.c");
    let library_path = build_dir.join(format!("{name}.so"));
    let status = Command::new("cc")
        .args(["-shared", "-fPIC"])
        .arg(format!("-DMOCK_COLOR_FORMAT={color_format}"))
        .arg(fixture)
        .arg("-o")
        .arg(&library_path)
        .status()
        .expect("run C compiler");
    assert!(status.success(), "compile mock LVGL firmware");
    library_path
}

#[test]
fn v9_uses_the_active_firmware_library_and_requires_native_rgb565() {
    let build_dir =
        std::env::temp_dir().join(format!("mycelium-lvgl-v9-test-{}", std::process::id()));
    fs::create_dir_all(&build_dir).expect("create fixture build directory");
    let rgb565_path = build_mock(&build_dir, "rgb565", 0x12);
    let incompatible_path = build_mock(&build_dir, "xrgb8888", 0x20);

    // SAFETY: Both test libraries are live for every symbol call and export
    // the exact mock signatures declared above.
    unsafe {
        let rgb565 = Library::new(rgb565_path).expect("load RGB565 mock");
        let incompatible = Library::new(incompatible_path).expect("load incompatible mock");

        let rejected = with_firmware_library(&incompatible, || {
            mycelium_display::lvgl_v9_init_sdl("node-bad", 320, 240)
        });
        assert!(rejected.is_null());
        let incompatible_deletes: Symbol<CounterFn> =
            incompatible.get(b"mock_delete_calls\0").unwrap();
        let incompatible_hides: Symbol<CounterFn> = incompatible.get(b"mock_hide_calls\0").unwrap();
        assert_eq!(incompatible_deletes(), 1);
        assert_eq!(incompatible_hides(), 0);

        let display = with_firmware_library(&rgb565, || {
            mycelium_display::lvgl_v9_init_sdl("node-good", 320, 240)
        });
        assert!(!display.is_null());
        let rgb565_deletes: Symbol<CounterFn> = rgb565.get(b"mock_delete_calls\0").unwrap();
        let rgb565_hides: Symbol<CounterFn> = rgb565.get(b"mock_hide_calls\0").unwrap();
        assert_eq!(rgb565_deletes(), 0);
        assert_eq!(rgb565_hides(), 1);

        let pixels = capture_lvgl_rgb565_with_library(&rgb565, display).unwrap();
        assert_eq!(pixels.len(), 320 * 240 * 2);
        assert!(pixels.iter().all(|pixel| *pixel == 0x5a));

        assert!(lvgl_v9_init_sdl_with_library(&rgb565, "wrong-size", 640, 480).is_null());
    }

    fs::remove_dir_all(build_dir).expect("remove fixture build directory");
}
