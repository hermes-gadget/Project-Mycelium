#![cfg(target_os = "linux")]

use std::fs;
use std::path::PathBuf;
use std::process::Command;

use libloading::{Library, Symbol};
use mycelium_core::instance::{InstanceConfig, InstanceManager};

type CounterFn = unsafe extern "C" fn() -> i32;

#[test]
fn manager_loads_starts_ticks_and_kills_firmware() {
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/test_firmware.c");
    let build_dir =
        std::env::temp_dir().join(format!("mycelium-loader-test-{}", std::process::id()));
    fs::create_dir_all(&build_dir).expect("create fixture build directory");
    let library_path = build_dir.join("test_firmware.so");

    let status = Command::new("cc")
        .args(["-shared", "-fPIC"])
        .arg(&fixture)
        .arg("-o")
        .arg(&library_path)
        .status()
        .expect("run C compiler");
    assert!(status.success(), "compile test firmware");

    let mut manager = InstanceManager::new();
    let id = manager
        .spawn(&library_path, InstanceConfig::default())
        .expect("spawn firmware");
    assert_eq!(id, "node1");
    assert!(manager.get(&id).expect("loaded instance").has_display());

    manager.tick_all();

    // SAFETY: This test fixture defines each counter function with the exact
    // declared signature and remains loaded while the symbols are called.
    unsafe {
        let inspector = Library::new(&library_path).expect("open fixture for inspection");
        let setup_calls: Symbol<CounterFn> =
            inspector.get(b"test_setup_calls\0").expect("setup counter");
        let loop_calls: Symbol<CounterFn> =
            inspector.get(b"test_loop_calls\0").expect("loop counter");
        let bus_tick_calls: Symbol<CounterFn> = inspector
            .get(b"test_bus_tick_calls\0")
            .expect("bus tick counter");

        assert_eq!(setup_calls(), 1);
        assert_eq!(loop_calls(), 1);
        assert_eq!(bus_tick_calls(), 1);
    }

    manager.kill(&id).expect("kill firmware");
    assert!(manager.list().is_empty());
    fs::remove_dir_all(build_dir).expect("remove fixture build directory");
}
