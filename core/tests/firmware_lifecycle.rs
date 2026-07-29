#![cfg(target_os = "linux")]

use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use libloading::{Library, Symbol};
use mycelium_core::instance::{InstanceConfig, InstanceManager};

type CounterFn = unsafe extern "C" fn() -> i32;

fn compile_fixture(name: &str) -> (PathBuf, PathBuf) {
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/test_firmware.c");
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let build_dir = std::env::temp_dir().join(format!(
        "mycelium-loader-{name}-{}-{nonce}",
        std::process::id()
    ));
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
    (build_dir, library_path)
}

#[test]
fn manager_loads_starts_ticks_and_kills_firmware() {
    let (build_dir, library_path) = compile_fixture("lifecycle");
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

        assert_eq!(setup_calls(), 1);
        assert_eq!(loop_calls(), 1);
    }

    manager.kill(&id).expect("kill firmware");
    assert!(manager.list().is_empty());
    fs::remove_dir_all(build_dir).expect("remove fixture build directory");
}

#[test]
fn migration_breadcrumb_survives_instance_kill_and_restart() {
    let (build_dir, library_path) = compile_fixture("nvs-restart");
    let instance_id = format!(
        "firmware-nvs-restart-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let config = InstanceConfig {
        instance_id: Some(instance_id.clone()),
        ..InstanceConfig::default()
    };

    let mut manager = InstanceManager::new();
    manager
        .spawn(&library_path, config.clone())
        .expect("spawn first instance");
    let backing_path = {
        let instance = manager.get(&instance_id).expect("first instance");
        let mut nvs = instance.nvs().lock().unwrap();
        assert!(nvs.begin("touch", false));
        assert_eq!(nvs.put_bool("sd_mig_busy", true), 1);
        nvs.backing_path().to_owned()
    };

    // The instance disappears without clearing the in-progress breadcrumb.
    manager.kill(&instance_id).expect("simulate crash");
    manager
        .spawn(&library_path, config)
        .expect("restart same stable instance ID");

    {
        let instance = manager.get(&instance_id).expect("restarted instance");
        let mut nvs = instance.nvs().lock().unwrap();
        assert!(nvs.begin("touch", true));
        let migration_should_run = !nvs.get_bool("sd_mig_busy", false);
        assert!(
            !migration_should_run,
            "a stale sd_mig_busy breadcrumb must skip SPIFFS-to-SD migration"
        );
    }

    manager.kill(&instance_id).expect("stop restarted instance");
    fs::remove_file(backing_path).expect("remove NVS test image");
    fs::remove_dir_all(build_dir).expect("remove fixture build directory");
}
