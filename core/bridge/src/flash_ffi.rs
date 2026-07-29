use std::ffi::{c_char, CStr};
use std::ptr;
use std::sync::{Mutex, MutexGuard};

use mycelium_board::partition::{
    active_partition_table, get_partition_table, register_partition_table,
};
use mycelium_board::{get_nvs, register_nvs, remove_nvs, LAUNCHER_NVS_SIZE, STANDALONE_NVS_SIZE};

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

unsafe fn ffi_string<'a>(value: *const c_char) -> Option<&'a str> {
    if value.is_null() {
        return None;
    }
    let value = unsafe { CStr::from_ptr(value) }.to_str().ok()?;
    (!value.is_empty()).then_some(value)
}

unsafe fn ffi_string_allow_empty<'a>(value: *const c_char) -> Option<&'a str> {
    if value.is_null() {
        return None;
    }
    unsafe { CStr::from_ptr(value) }.to_str().ok()
}

unsafe fn nvs_args<'a>(
    instance_id: *const c_char,
    namespace: *const c_char,
    key: *const c_char,
) -> Option<(&'a str, &'a str, &'a str)> {
    Some((
        unsafe { ffi_string(instance_id) }?,
        unsafe { ffi_string(namespace) }?,
        unsafe { ffi_string(key) }?,
    ))
}

/// Opens or reconfigures persistent NVS for one emulator instance.
///
/// # Safety
///
/// `instance_id` must point to a valid NUL-terminated string for this call.
#[no_mangle]
pub unsafe extern "C" fn meshemu_nvs_init(instance_id: *const c_char, size_bytes: u32) -> bool {
    let Some(instance_id) = (unsafe { ffi_string(instance_id) }) else {
        return false;
    };
    register_nvs(instance_id, size_bytes).is_ok()
}

/// Reports whether a key exists in an NVS namespace.
///
/// # Safety
///
/// All string pointers must be valid NUL-terminated strings for this call.
#[no_mangle]
pub unsafe extern "C" fn meshemu_nvs_exists(
    instance_id: *const c_char,
    namespace: *const c_char,
    key: *const c_char,
) -> bool {
    let Some((instance_id, namespace, key)) = (unsafe { nvs_args(instance_id, namespace, key) })
    else {
        return false;
    };
    let Some(nvs) = get_nvs(instance_id) else {
        return false;
    };
    let mut nvs = lock(&nvs);
    let found = nvs.begin(namespace, true) && nvs.exists(key);
    nvs.end();
    found
}

/// Reads a bool, returning `default_value` for a missing key or type mismatch.
///
/// # Safety
///
/// All string pointers must be valid NUL-terminated strings for this call.
#[no_mangle]
pub unsafe extern "C" fn meshemu_nvs_get_bool(
    instance_id: *const c_char,
    namespace: *const c_char,
    key: *const c_char,
    default_value: bool,
) -> bool {
    let Some((instance_id, namespace, key)) = (unsafe { nvs_args(instance_id, namespace, key) })
    else {
        return default_value;
    };
    let Some(nvs) = get_nvs(instance_id) else {
        return default_value;
    };
    let mut nvs = lock(&nvs);
    let value = if nvs.begin(namespace, true) {
        nvs.get_bool(key, default_value)
    } else {
        default_value
    };
    nvs.end();
    value
}

/// Writes a bool to a namespace, creating that namespace when needed.
///
/// # Safety
///
/// All string pointers must be valid NUL-terminated strings for this call.
#[no_mangle]
pub unsafe extern "C" fn meshemu_nvs_put_bool(
    instance_id: *const c_char,
    namespace: *const c_char,
    key: *const c_char,
    value: bool,
) -> bool {
    let Some((instance_id, namespace, key)) = (unsafe { nvs_args(instance_id, namespace, key) })
    else {
        return false;
    };
    let Some(nvs) = get_nvs(instance_id) else {
        return false;
    };
    let mut nvs = lock(&nvs);
    let written = nvs.begin(namespace, false) && nvs.try_put_bool(key, value);
    nvs.end();
    written
}

/// Copies a string value into `buffer` and returns its full byte length.
///
/// The output is always NUL-terminated when `buffer_len` is nonzero. A small
/// buffer receives a truncated string while the return value still reports the
/// full size required, excluding the terminator.
///
/// # Safety
///
/// Input strings must be valid and NUL-terminated. `buffer` must reference
/// `buffer_len` writable bytes, or may be null when `buffer_len` is zero.
#[no_mangle]
pub unsafe extern "C" fn meshemu_nvs_get_string(
    instance_id: *const c_char,
    namespace: *const c_char,
    key: *const c_char,
    default_value: *const c_char,
    buffer: *mut c_char,
    buffer_len: usize,
) -> usize {
    if buffer.is_null() && buffer_len != 0 {
        return 0;
    }
    if !buffer.is_null() && buffer_len != 0 {
        unsafe { *buffer = 0 };
    }
    let default_value = if default_value.is_null() {
        ""
    } else {
        let Ok(value) = (unsafe { CStr::from_ptr(default_value) }).to_str() else {
            return 0;
        };
        value
    };
    let Some((instance_id, namespace, key)) = (unsafe { nvs_args(instance_id, namespace, key) })
    else {
        return 0;
    };
    let value = get_nvs(instance_id)
        .map(|nvs| {
            let mut nvs = lock(&nvs);
            let value = if nvs.begin(namespace, true) {
                nvs.get_string(key, default_value)
            } else {
                default_value.to_owned()
            };
            nvs.end();
            value
        })
        .unwrap_or_else(|| default_value.to_owned());
    if !buffer.is_null() && buffer_len != 0 {
        let copied = value.len().min(buffer_len - 1);
        unsafe {
            ptr::copy_nonoverlapping(value.as_ptr(), buffer.cast::<u8>(), copied);
            *buffer.add(copied) = 0;
        }
    }
    value.len()
}

/// Writes a UTF-8 string to a namespace.
///
/// # Safety
///
/// All string pointers must be valid NUL-terminated strings for this call.
#[no_mangle]
pub unsafe extern "C" fn meshemu_nvs_put_string(
    instance_id: *const c_char,
    namespace: *const c_char,
    key: *const c_char,
    value: *const c_char,
) -> bool {
    let Some((instance_id, namespace, key)) = (unsafe { nvs_args(instance_id, namespace, key) })
    else {
        return false;
    };
    let Some(value) = (unsafe { ffi_string_allow_empty(value) }) else {
        return false;
    };
    let Some(nvs) = get_nvs(instance_id) else {
        return false;
    };
    let mut nvs = lock(&nvs);
    let written = nvs.begin(namespace, false) && nvs.try_put_string(key, value);
    nvs.end();
    written
}

/// Removes one key from an NVS namespace.
///
/// # Safety
///
/// All string pointers must be valid NUL-terminated strings for this call.
#[no_mangle]
pub unsafe extern "C" fn meshemu_nvs_remove(
    instance_id: *const c_char,
    namespace: *const c_char,
    key: *const c_char,
) -> bool {
    let Some((instance_id, namespace, key)) = (unsafe { nvs_args(instance_id, namespace, key) })
    else {
        return false;
    };
    let Some(nvs) = get_nvs(instance_id) else {
        return false;
    };
    let mut nvs = lock(&nvs);
    let removed = nvs.begin(namespace, false) && nvs.remove(key);
    nvs.end();
    removed
}

/// Drops the live NVS handle while preserving its crash-durable JSON image.
///
/// # Safety
///
/// `instance_id` must point to a valid NUL-terminated string for this call.
#[no_mangle]
pub unsafe extern "C" fn meshemu_nvs_destroy(instance_id: *const c_char) -> bool {
    let Some(instance_id) = (unsafe { ffi_string(instance_id) }) else {
        return false;
    };
    remove_nvs(instance_id).is_some()
}

/// Switches one instance between standalone and Launcher flash geometry.
///
/// # Safety
///
/// `instance_id` must point to a valid NUL-terminated string for this call.
#[no_mangle]
pub unsafe extern "C" fn meshemu_partition_set_launcher_mode(
    instance_id: *const c_char,
    enabled: bool,
) -> bool {
    let Some(instance_id) = (unsafe { ffi_string(instance_id) }) else {
        return false;
    };
    let nvs_size = if enabled {
        LAUNCHER_NVS_SIZE
    } else {
        STANDALONE_NVS_SIZE
    };
    if register_nvs(instance_id, nvs_size).is_err() {
        return false;
    }
    register_partition_table(instance_id, enabled);
    true
}

/// Finds the first matching entry in the currently active firmware table.
///
/// # Safety
///
/// Both output pointers must point to writable `u32` values.
#[no_mangle]
pub unsafe extern "C" fn meshemu_partition_find_first(
    partition_type: u8,
    subtype: u8,
    address_out: *mut u32,
    size_out: *mut u32,
) -> bool {
    unsafe {
        write_partition_result(
            active_partition_table(),
            partition_type,
            subtype,
            address_out,
            size_out,
        )
    }
}

/// Finds a partition for a specific virtual node without changing activation.
///
/// # Safety
///
/// `instance_id` and both output pointers must be valid for this call.
#[no_mangle]
pub unsafe extern "C" fn meshemu_partition_find_first_for_instance(
    instance_id: *const c_char,
    partition_type: u8,
    subtype: u8,
    address_out: *mut u32,
    size_out: *mut u32,
) -> bool {
    let Some(instance_id) = (unsafe { ffi_string(instance_id) }) else {
        return false;
    };
    let Some(table) = get_partition_table(instance_id) else {
        return false;
    };
    let table = lock(&table).clone();
    unsafe { write_partition_result(table, partition_type, subtype, address_out, size_out) }
}

/// Returns the otadata address in the currently active firmware table.
#[no_mangle]
pub extern "C" fn meshemu_get_otadata_address() -> u32 {
    active_partition_table().otadata_address().unwrap_or(0)
}

/// Applies the same dual-signal Launcher detection used by real firmware.
///
/// # Safety
///
/// `instance_id` must point to a valid NUL-terminated string for this call.
#[no_mangle]
pub unsafe extern "C" fn meshemu_is_under_launcher(instance_id: *const c_char) -> bool {
    let Some(instance_id) = (unsafe { ffi_string(instance_id) }) else {
        return false;
    };
    get_partition_table(instance_id).is_some_and(|table| lock(&table).is_under_launcher())
}

unsafe fn write_partition_result(
    table: mycelium_board::VirtualPartitionTable,
    partition_type: u8,
    subtype: u8,
    address_out: *mut u32,
    size_out: *mut u32,
) -> bool {
    if address_out.is_null() || size_out.is_null() {
        return false;
    }
    unsafe {
        *address_out = 0;
        *size_out = 0;
    }
    let Some(partition) = table.find_first(partition_type, subtype) else {
        return false;
    };
    unsafe {
        *address_out = partition.address;
        *size_out = partition.size;
    }
    true
}

#[cfg(test)]
mod tests {
    use std::ffi::{CStr, CString};
    use std::sync::Mutex;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    static TEST_FLASH: Mutex<()> = Mutex::new(());

    fn unique_id(label: &str) -> (String, CString) {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let id = format!("{label}-{}-{nonce}", std::process::id());
        let c_id = CString::new(id.clone()).unwrap();
        (id, c_id)
    }

    fn remove_backing_file(instance_id: &str) {
        let Some(nvs) = mycelium_board::get_nvs(instance_id) else {
            return;
        };
        let path = lock(&nvs).backing_path().to_owned();
        mycelium_board::remove_nvs(instance_id);
        mycelium_board::remove_partition_table(instance_id);
        if path.exists() {
            std::fs::remove_file(path).unwrap();
        }
    }

    #[test]
    fn nvs_round_trips_types_namespaces_and_restart_persistence() {
        let _serial = TEST_FLASH.lock().unwrap();
        let (instance, id) = unique_id("ffi-nvs");
        let touch = CString::new("touch").unwrap();
        let other = CString::new("other").unwrap();
        let busy = CString::new("sd_mig_busy").unwrap();
        let label = CString::new("label").unwrap();
        let value = CString::new("Wadamesh").unwrap();
        let empty = CString::new("").unwrap();
        let fallback = CString::new("fallback").unwrap();

        unsafe {
            assert!(meshemu_nvs_init(id.as_ptr(), STANDALONE_NVS_SIZE));
            assert!(!meshemu_nvs_exists(
                id.as_ptr(),
                touch.as_ptr(),
                busy.as_ptr()
            ));
            assert!(meshemu_nvs_put_bool(
                id.as_ptr(),
                touch.as_ptr(),
                busy.as_ptr(),
                true
            ));
            assert!(meshemu_nvs_put_string(
                id.as_ptr(),
                touch.as_ptr(),
                label.as_ptr(),
                value.as_ptr()
            ));
            assert!(meshemu_nvs_put_string(
                id.as_ptr(),
                other.as_ptr(),
                label.as_ptr(),
                empty.as_ptr()
            ));
            assert!(!meshemu_nvs_exists(
                id.as_ptr(),
                other.as_ptr(),
                busy.as_ptr()
            ));

            let mut small = [0_i8; 5];
            assert_eq!(
                meshemu_nvs_get_string(
                    id.as_ptr(),
                    touch.as_ptr(),
                    label.as_ptr(),
                    fallback.as_ptr(),
                    small.as_mut_ptr(),
                    small.len()
                ),
                8
            );
            assert_eq!(CStr::from_ptr(small.as_ptr()).to_bytes(), b"Wada");

            assert!(meshemu_nvs_destroy(id.as_ptr()));
            assert!(meshemu_nvs_init(id.as_ptr(), LAUNCHER_NVS_SIZE));
            assert!(meshemu_nvs_get_bool(
                id.as_ptr(),
                touch.as_ptr(),
                busy.as_ptr(),
                false
            ));
            assert!(meshemu_nvs_remove(
                id.as_ptr(),
                touch.as_ptr(),
                busy.as_ptr()
            ));
            assert!(meshemu_nvs_destroy(id.as_ptr()));
        }
        // Reopen solely to recover the deterministic backing path for cleanup.
        assert!(unsafe { meshemu_nvs_init(id.as_ptr(), STANDALONE_NVS_SIZE) });
        remove_backing_file(&instance);
    }

    #[test]
    fn nvs_rejects_invalid_arguments_and_uninitialized_instances() {
        let namespace = CString::new("touch").unwrap();
        let key = CString::new("key").unwrap();
        let value = CString::new("value").unwrap();
        unsafe {
            assert!(!meshemu_nvs_init(std::ptr::null(), STANDALONE_NVS_SIZE));
            assert!(!meshemu_nvs_init(value.as_ptr(), 0));
            assert!(!meshemu_nvs_exists(
                value.as_ptr(),
                namespace.as_ptr(),
                key.as_ptr()
            ));
            assert!(!meshemu_nvs_put_bool(
                value.as_ptr(),
                namespace.as_ptr(),
                key.as_ptr(),
                true
            ));
            assert!(!meshemu_nvs_put_string(
                value.as_ptr(),
                namespace.as_ptr(),
                key.as_ptr(),
                std::ptr::null()
            ));
        }
    }

    #[test]
    fn partition_switches_exact_geometry_and_launcher_detection() {
        let _serial = TEST_FLASH.lock().unwrap();
        let (instance, id) = unique_id("ffi-partitions");
        let mut address = u32::MAX;
        let mut size = u32::MAX;

        unsafe {
            assert!(meshemu_partition_set_launcher_mode(id.as_ptr(), false));
            assert!(!meshemu_is_under_launcher(id.as_ptr()));
            assert_eq!(meshemu_get_otadata_address(), 0xE000);
            assert!(meshemu_partition_find_first(
                mycelium_board::partition::ESP_PARTITION_TYPE_DATA,
                mycelium_board::partition::ESP_PARTITION_SUBTYPE_DATA_NVS,
                &mut address,
                &mut size
            ));
            assert_eq!((address, size), (0x9000, 0x5000));

            assert!(meshemu_partition_set_launcher_mode(id.as_ptr(), true));
            assert!(meshemu_is_under_launcher(id.as_ptr()));
            assert_eq!(meshemu_get_otadata_address(), 0xD000);
            assert!(meshemu_partition_find_first(
                mycelium_board::partition::ESP_PARTITION_TYPE_APP,
                mycelium_board::partition::ESP_PARTITION_SUBTYPE_APP_TEST,
                &mut address,
                &mut size
            ));
            assert_eq!((address, size), (0x10000, 0x180000));
            assert!(meshemu_partition_find_first_for_instance(
                id.as_ptr(),
                mycelium_board::partition::ESP_PARTITION_TYPE_DATA,
                mycelium_board::partition::ESP_PARTITION_SUBTYPE_DATA_OTA,
                &mut address,
                &mut size
            ));
            assert_eq!((address, size), (0xD000, 0x2000));
        }
        remove_backing_file(&instance);
    }
}
