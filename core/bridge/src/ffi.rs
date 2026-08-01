use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::ffi::{c_char, c_void, CStr};
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{LazyLock, Mutex, MutexGuard};

use mycelium_board::{
    peripherals_powered, BoardConfig, Tp4054State, VirtualBoard, SLEEP_WAKE_CAUSE_EXT1,
    SLEEP_WAKE_CAUSE_TIMER, SLEEP_WAKE_CAUSE_UNKNOWN,
};
use mycelium_gps::GpsManager;
use mycelium_input::i2c_keyboard::I2cKeyboardBus;
use mycelium_input::wire_shim::{SharedI2cKeyboard, WireShim};
use mycelium_input::{get_input_manager, register_input_manager, SharedInputManager};
use mycelium_storage::StorageManager;
use radio_bus::{propagation, RadioBus, RadioChannel, RxPacket, Sx1262State, TxEvent};
use sdl2::keyboard::Keycode;
use tracing::warn;

struct BusState {
    bus: RadioBus,
    node_ids: HashSet<String>,
    now_ms: u64,
    sleep_requests: HashMap<String, (u64, u64, u32, u64, bool)>,
    last_wake_causes: HashMap<String, u8>,
}

impl BusState {
    fn new() -> Self {
        Self {
            bus: RadioBus::new(),
            node_ids: HashSet::new(),
            now_ms: 0,
            sleep_requests: HashMap::new(),
            last_wake_causes: HashMap::new(),
        }
    }
}

struct RadioHandle {
    node_id: String,
    radio: Mutex<Sx1262State>,
    pending: Mutex<VecDeque<RxPacket>>,
    last_rx: Mutex<Option<(f32, f32)>>,
}

struct GpsHandle {
    instance_id: String,
    manager: GpsManager,
}

struct SdFileHandle {
    instance_id: String,
    path: String,
    position: usize,
    writable: bool,
    append: bool,
}

static BUS: LazyLock<Mutex<BusState>> = LazyLock::new(|| Mutex::new(BusState::new()));
static STORAGE: LazyLock<Mutex<HashMap<String, StorageManager>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
static SD_FILE_HANDLES: LazyLock<Mutex<HashMap<u32, SdFileHandle>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
static SDCARD_REQUIRES_SLOW_INIT: AtomicBool = AtomicBool::new(false);
static SDCARD_WAKE_DELAY_MS: AtomicU32 = AtomicU32::new(0);

thread_local! {
    /// Compatibility context for legacy no-ID board/input APIs. The actual
    /// retained state is always stored in an instance-keyed registry.
    static CURRENT_INSTANCE: RefCell<Option<String>> = const { RefCell::new(None) };
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn set_current_instance(instance_id: &str) {
    CURRENT_INSTANCE.with(|current| {
        *current.borrow_mut() = Some(instance_id.to_owned());
    });
    mycelium_display::shared_spi::set_current_instance(instance_id);
}

fn current_instance() -> Option<String> {
    CURRENT_INSTANCE.with(|current| current.borrow().clone())
}

#[track_caller]
unsafe fn handle_ref<'a>(radio: *mut c_void) -> Option<&'a RadioHandle> {
    let Some(handle) = (radio as *const RadioHandle).as_ref() else {
        warn!(caller = %std::panic::Location::caller(), "FFI call with NULL or dangling radio handle");
        return None;
    };
    Some(handle)
}

#[track_caller]
unsafe fn keyboard_ref<'a>(keyboard: *mut c_void) -> Option<&'a SharedI2cKeyboard> {
    let Some(keyboard) = (unsafe { (keyboard as *const SharedI2cKeyboard).as_ref() }) else {
        warn!(caller = %std::panic::Location::caller(), "FFI call with NULL or dangling keyboard handle");
        return None;
    };
    Some(keyboard)
}

#[track_caller]
unsafe fn wire_mut<'a>(wire: *mut c_void) -> Option<&'a mut WireShim> {
    let Some(wire) = (unsafe { (wire as *mut WireShim).as_mut() }) else {
        warn!(caller = %std::panic::Location::caller(), "FFI call with NULL or dangling Wire shim handle");
        return None;
    };
    Some(wire)
}

#[track_caller]
unsafe fn ffi_string<'a>(value: *const c_char) -> Option<&'a str> {
    if value.is_null() {
        warn!(caller = %std::panic::Location::caller(), "FFI string argument is NULL");
        return None;
    }
    let Ok(value) = (unsafe { CStr::from_ptr(value) }).to_str() else {
        warn!(caller = %std::panic::Location::caller(), "FFI string argument is not valid UTF-8");
        return None;
    };
    if value.is_empty() {
        warn!(caller = %std::panic::Location::caller(), "FFI string argument is empty");
        return None;
    }
    Some(value)
}

#[track_caller]
unsafe fn storage_file_args<'a>(
    instance_id: *const c_char,
    path: *const c_char,
) -> Option<(&'a str, &'a str)> {
    let Some(instance_id) = (unsafe { ffi_string(instance_id) }) else {
        warn!(caller = %std::panic::Location::caller(), "FFI call with NULL or invalid instance_id");
        return None;
    };
    let Some(path) = (unsafe { ffi_string(path) }) else {
        warn!(caller = %std::panic::Location::caller(), %instance_id, "FFI call with NULL or invalid path");
        return None;
    };
    set_current_instance(instance_id);
    Some((instance_id, path))
}

/// Runs one SD transaction on the arbiter belonging to the virtual instance.
/// The display and SD paths therefore cannot silently drive the same instance
/// SPI pins at the same time.
fn with_sd_spi<T>(instance_id: &str, operation: impl FnOnce() -> T) -> Option<T> {
    mycelium_display::shared_spi::spi_bus_for_instance(instance_id)
        .transaction(mycelium_display::shared_spi::SpiDevice::SdCard, operation)
        .map_err(|error| warn!(%instance_id, %error, "SD SPI transaction rejected"))
        .ok()
}

/// Static sentinel handed to callers for empty reads. Returning a stable
/// non-null pointer with `*out_len = 0` avoids a wasted `malloc(1)` while
/// still satisfying callers that treat null as an error.
static EMPTY_READ_SENTINEL: [u8; 1] = [0];

fn copy_for_caller(data: &[u8], out_len: *mut usize) -> *mut u8 {
    if out_len.is_null() {
        return ptr::null_mut();
    }
    unsafe { *out_len = 0 };
    if data.is_empty() {
        return EMPTY_READ_SENTINEL.as_ptr().cast_mut();
    }
    let output = unsafe { libc::malloc(data.len()) }.cast::<u8>();
    if output.is_null() {
        return ptr::null_mut();
    }
    unsafe { ptr::copy_nonoverlapping(data.as_ptr(), output, data.len()) };
    unsafe { *out_len = data.len() };
    output
}

#[track_caller]
unsafe fn gps_mut<'a>(gps: *mut c_void) -> Option<&'a mut GpsHandle> {
    let Some(gps) = (unsafe { (gps as *mut GpsHandle).as_mut() }) else {
        warn!(caller = %std::panic::Location::caller(), "FFI call with NULL or dangling GPS handle");
        return None;
    };
    Some(gps)
}

#[track_caller]
unsafe fn board_ref<'a>(board: *mut c_void) -> Option<&'a VirtualBoard> {
    let Some(board) = (unsafe { (board as *const VirtualBoard).as_ref() }) else {
        warn!(caller = %std::panic::Location::caller(), "FFI call with NULL or dangling board handle");
        return None;
    };
    Some(board)
}

#[track_caller]
unsafe fn board_mut<'a>(board: *mut c_void) -> Option<&'a mut VirtualBoard> {
    let Some(board) = (unsafe { (board as *mut VirtualBoard).as_mut() }) else {
        warn!(caller = %std::panic::Location::caller(), "FFI call with NULL or dangling board handle");
        return None;
    };
    Some(board)
}

#[track_caller]
unsafe fn instance_id(instance_id: *const c_char) -> Option<String> {
    if instance_id.is_null() {
        warn!(caller = %std::panic::Location::caller(), "FFI instance_id argument is NULL");
        return None;
    }
    let Ok(instance_id) = (unsafe { CStr::from_ptr(instance_id) }).to_str() else {
        warn!(caller = %std::panic::Location::caller(), "FFI instance_id argument is not valid UTF-8");
        return None;
    };
    if instance_id.is_empty() {
        warn!(caller = %std::panic::Location::caller(), "FFI instance_id argument is empty");
        return None;
    }
    Some(instance_id.to_owned())
}

fn valid_position(lat: f64, lon: f64) -> bool {
    lat.is_finite()
        && lon.is_finite()
        && (-90.0..=90.0).contains(&lat)
        && (-180.0..=180.0).contains(&lon)
}

fn sx1262_state(
    freq_mhz: f64,
    bandwidth_khz: u16,
    spreading_factor: u8,
    coding_rate: u8,
    tx_power_dbm: f64,
) -> Option<Sx1262State> {
    // SX1262 operating limits and the LoRa modulation settings supported by
    // the bridge. Reject rather than passing unchecked values into airtime
    // arithmetic across the FFI boundary.
    let channel = RadioChannel::new(freq_mhz, bandwidth_khz, spreading_factor, coding_rate)?;
    Sx1262State::new(channel, tx_power_dbm)
}

/// Mounts the virtual SPIFFS partition for an emulator instance.
///
/// # Safety
///
/// `instance_id` must point to a valid NUL-terminated string for this call.
#[no_mangle]
pub unsafe extern "C" fn meshemu_spiffs_init(instance_id: *const c_char) -> bool {
    let Some(instance_id) = (unsafe { ffi_string(instance_id) }) else {
        return false;
    };
    let mut storage = lock(&STORAGE);
    let mounted = storage
        .entry(instance_id.to_owned())
        .or_insert_with(|| StorageManager::new(instance_id))
        .spiffs
        .mount()
        .is_ok();
    if !mounted {
        warn!(%instance_id, "SPIFFS mount failed");
    }
    mounted
}

/// Reads a SPIFFS file into a caller-owned allocation.
///
/// The returned buffer must be released with [`meshemu_storage_data_free`].
///
/// # Safety
///
/// String pointers must be valid NUL-terminated strings. `out_len` must be null
/// or point to writable memory for one `usize`.
#[no_mangle]
pub unsafe extern "C" fn meshemu_spiffs_read(
    instance_id: *const c_char,
    path: *const c_char,
    out_len: *mut usize,
) -> *mut u8 {
    if !out_len.is_null() {
        unsafe { *out_len = 0 };
    }
    let Some((instance_id, path)) = (unsafe { storage_file_args(instance_id, path) }) else {
        return ptr::null_mut();
    };
    let storage = lock(&STORAGE);
    let Some(data) = storage
        .get(instance_id)
        .and_then(|manager| manager.spiffs.read_file(path).ok())
    else {
        warn!(%instance_id, %path, "SPIFFS read failed: instance missing or file unreadable");
        return ptr::null_mut();
    };
    copy_for_caller(&data, out_len)
}

/// Writes a complete SPIFFS file.
///
/// # Safety
///
/// String pointers must be valid NUL-terminated strings. `data` must reference
/// `len` readable bytes, or may be null when `len` is zero.
#[no_mangle]
pub unsafe extern "C" fn meshemu_spiffs_write(
    instance_id: *const c_char,
    path: *const c_char,
    data: *const u8,
    len: usize,
) -> bool {
    let Some((instance_id, path)) = (unsafe { storage_file_args(instance_id, path) }) else {
        return false;
    };
    if data.is_null() && len != 0 {
        warn!(%instance_id, %path, "SPIFFS write rejected: NULL data with nonzero length");
        return false;
    }
    let bytes = if len == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(data, len) }
    };
    let written = lock(&STORAGE)
        .get_mut(instance_id)
        .is_some_and(|manager| manager.spiffs.write_file(path, bytes).is_ok());
    if !written {
        warn!(%instance_id, %path, "SPIFFS write failed: instance missing or write rejected");
    }
    written
}

/// Mounts the virtual SD card for an emulator instance.
///
/// # Safety
///
/// `instance_id` must point to a valid NUL-terminated string for this call.
#[no_mangle]
pub unsafe extern "C" fn meshemu_sdcard_init(instance_id: *const c_char) -> bool {
    let Some(instance_id) = (unsafe { ffi_string(instance_id) }) else {
        return false;
    };
    set_current_instance(instance_id);
    if !peripherals_powered(instance_id) {
        warn!(%instance_id, "SD card init skipped: peripheral power rail is off");
        return false;
    }
    let requires_slow_init = SDCARD_REQUIRES_SLOW_INIT.load(Ordering::Relaxed);
    let wake_delay_ms = SDCARD_WAKE_DELAY_MS.load(Ordering::Relaxed);
    let Some(mounted) = with_sd_spi(instance_id, || {
        let mut storage = lock(&STORAGE);
        let sdcard = &mut storage
            .entry(instance_id.to_owned())
            .or_insert_with(|| StorageManager::new(instance_id))
            .sdcard;
        sdcard.set_behavior(requires_slow_init, wake_delay_ms);
        sdcard.mount_with_retry_ladder().unwrap_or(false)
    }) else {
        return false;
    };
    if !mounted {
        warn!(%instance_id, "SD card mount failed");
    }
    mounted
}

/// Configures the process-wide virtual SD-card personality.
///
/// Slow cards reject 4 MHz and 1 MHz initialization and only mount on a
/// 400 kHz ladder step after at least `wake_delay_ms` of cumulative settling.
#[no_mangle]
pub extern "C" fn meshemu_sdcard_set_behavior(slow_init: bool, wake_delay_ms: u32) {
    SDCARD_REQUIRES_SLOW_INIT.store(slow_init, Ordering::Relaxed);
    SDCARD_WAKE_DELAY_MS.store(wake_delay_ms, Ordering::Relaxed);
    for manager in lock(&STORAGE).values_mut() {
        manager.sdcard.set_behavior(slow_init, wake_delay_ms);
    }
}

/// Returns the Arduino SD card type (`3` for the emulated SDHC card).
///
/// # Safety
///
/// `instance_id` must point to a valid NUL-terminated string for this call.
#[no_mangle]
pub unsafe extern "C" fn meshemu_sdcard_card_type(instance_id: *const c_char) -> u32 {
    let Some(instance_id) = (unsafe { ffi_string(instance_id) }) else {
        return 0;
    };
    set_current_instance(instance_id);
    if !peripherals_powered(instance_id) {
        return 0;
    }
    if with_sd_spi(instance_id, || {
        lock(&STORAGE)
            .get(instance_id)
            .is_some_and(|manager| manager.sdcard.is_mounted())
    })
    .unwrap_or(false)
    {
        3
    } else {
        0
    }
}

/// Returns the emulated SDHC capacity, or zero while unmounted.
///
/// # Safety
///
/// `instance_id` must point to a valid NUL-terminated string for this call.
#[no_mangle]
pub unsafe extern "C" fn meshemu_sdcard_total_bytes(instance_id: *const c_char) -> u64 {
    unsafe { sdcard_info(instance_id) }
        .map(|info| info.total_bytes)
        .unwrap_or(0)
}

/// Returns the bytes currently occupied by files on the emulated SD card.
///
/// # Safety
///
/// `instance_id` must point to a valid NUL-terminated string for this call.
#[no_mangle]
pub unsafe extern "C" fn meshemu_sdcard_used_bytes(instance_id: *const c_char) -> u64 {
    unsafe { sdcard_info(instance_id) }
        .map(|info| info.used_bytes)
        .unwrap_or(0)
}

unsafe fn sdcard_info(instance_id: *const c_char) -> Option<mycelium_storage::SdCardInfo> {
    let instance_id = unsafe { ffi_string(instance_id) }?;
    set_current_instance(instance_id);
    if !peripherals_powered(instance_id) {
        return None;
    }
    with_sd_spi(instance_id, || {
        lock(&STORAGE)
            .get(instance_id)
            .and_then(|manager| manager.sdcard.info().ok())
    })
    .flatten()
}

/// Creates a directory, including missing parent directories.
///
/// # Safety
///
/// String pointers must be valid NUL-terminated strings.
#[no_mangle]
pub unsafe extern "C" fn meshemu_sdcard_mkdir(
    instance_id: *const c_char,
    path: *const c_char,
) -> bool {
    let Some((instance_id, path)) = (unsafe { storage_file_args(instance_id, path) }) else {
        return false;
    };
    if !peripherals_powered(instance_id) {
        return false;
    }
    with_sd_spi(instance_id, || {
        lock(&STORAGE)
            .get(instance_id)
            .is_some_and(|manager| manager.sdcard.create_dir(path).is_ok())
    })
    .unwrap_or(false)
}

/// Returns whether a file or directory exists on a mounted card.
///
/// # Safety
///
/// String pointers must be valid NUL-terminated strings.
#[no_mangle]
pub unsafe extern "C" fn meshemu_sdcard_exists(
    instance_id: *const c_char,
    path: *const c_char,
) -> bool {
    let Some((instance_id, path)) = (unsafe { storage_file_args(instance_id, path) }) else {
        return false;
    };
    if !peripherals_powered(instance_id) {
        return false;
    }
    with_sd_spi(instance_id, || {
        lock(&STORAGE)
            .get(instance_id)
            .and_then(|manager| manager.sdcard.exists(path).ok())
            .unwrap_or(false)
    })
    .unwrap_or(false)
}

/// Opens a file and returns a handle in the range 1..=255.
///
/// Modes are 0=read, 1=write (create/truncate), and 2=append (create).
///
/// # Safety
///
/// String pointers must be valid NUL-terminated strings.
#[no_mangle]
pub unsafe extern "C" fn meshemu_sdcard_open(
    instance_id: *const c_char,
    path: *const c_char,
    mode: u8,
) -> u32 {
    let Some((instance_id, path)) = (unsafe { storage_file_args(instance_id, path) }) else {
        return 0;
    };
    if !peripherals_powered(instance_id) {
        return 0;
    }

    let mut handles = lock(&SD_FILE_HANDLES);
    let Some(handle) = (1..=255).find(|candidate| !handles.contains_key(candidate)) else {
        return 0;
    };
    let Some((position, writable, append)) = with_sd_spi(instance_id, || {
        let storage = lock(&STORAGE);
        let sdcard = storage.get(instance_id).map(|manager| &manager.sdcard)?;
        let result = match mode {
            0 => match sdcard.read_file(path) {
                Ok(_) => (0, false, false),
                Err(_) => return None,
            },
            1 => {
                if sdcard.write_file(path, &[]).is_err() {
                    return None;
                }
                (0, true, false)
            }
            2 => {
                let position = match sdcard.read_file(path) {
                    Ok(data) => data.len(),
                    Err(_) if !sdcard.exists(path).unwrap_or(false) => {
                        if sdcard.write_file(path, &[]).is_err() {
                            return None;
                        }
                        0
                    }
                    Err(_) => return None,
                };
                (position, true, true)
            }
            _ => {
                warn!(%instance_id, %path, mode, "SD card open rejected: unknown mode");
                return None;
            }
        };
        Some(result)
    })
    .flatten() else {
        return 0;
    };

    handles.insert(
        handle,
        SdFileHandle {
            instance_id: instance_id.to_owned(),
            path: path.to_owned(),
            position,
            writable,
            append,
        },
    );
    handle
}

/// Writes bytes at the current file position.
///
/// Returns `-1` for invalid handles, read-only files, bad pointers, or storage
/// errors.
///
/// # Safety
///
/// `data` must reference `len` readable bytes, or may be null when `len` is
/// zero.
#[no_mangle]
pub unsafe extern "C" fn meshemu_sdcard_write_file(handle: u32, data: *const u8, len: u32) -> i32 {
    if len > i32::MAX as u32 || (data.is_null() && len != 0) {
        return -1;
    }
    let bytes = if len == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(data, len as usize) }
    };
    let mut handles = lock(&SD_FILE_HANDLES);
    let Some(file) = handles.get_mut(&handle) else {
        return -1;
    };
    if !file.writable || !peripherals_powered(&file.instance_id) {
        return -1;
    }
    with_sd_spi(&file.instance_id, || {
        let mut storage = lock(&STORAGE);
        let Some(sdcard) = storage
            .get_mut(&file.instance_id)
            .map(|manager| &mut manager.sdcard)
        else {
            return -1;
        };
        let Ok(mut contents) = sdcard.read_file(&file.path) else {
            return -1;
        };
        if file.append {
            file.position = contents.len();
        }
        if file.position > contents.len() {
            return -1;
        }
        let Some(end) = file.position.checked_add(bytes.len()) else {
            return -1;
        };
        if end > contents.len() {
            contents.resize(end, 0);
        }
        contents[file.position..end].copy_from_slice(bytes);
        if sdcard.write_file(&file.path, &contents).is_err() {
            return -1;
        }
        file.position = end;
        len as i32
    })
    .unwrap_or(-1)
}

/// Reads up to `max_len` bytes from the current file position.
///
/// # Safety
///
/// `buf` must reference `max_len` writable bytes, or may be null when
/// `max_len` is zero.
#[no_mangle]
pub unsafe extern "C" fn meshemu_sdcard_read_file(handle: u32, buf: *mut u8, max_len: u32) -> i32 {
    if max_len > i32::MAX as u32 || (buf.is_null() && max_len != 0) {
        return -1;
    }
    let mut handles = lock(&SD_FILE_HANDLES);
    let Some(file) = handles.get_mut(&handle) else {
        return -1;
    };
    if file.writable || !peripherals_powered(&file.instance_id) {
        return -1;
    }
    with_sd_spi(&file.instance_id, || {
        let storage = lock(&STORAGE);
        let Some(contents) = storage
            .get(&file.instance_id)
            .and_then(|manager| manager.sdcard.read_file(&file.path).ok())
        else {
            return -1;
        };
        let read_len = (contents.len().saturating_sub(file.position)).min(max_len as usize);
        if read_len != 0 {
            unsafe {
                ptr::copy_nonoverlapping(contents[file.position..].as_ptr(), buf, read_len);
            }
        }
        file.position += read_len;
        read_len as i32
    })
    .unwrap_or(-1)
}

/// Closes an open SD file handle.
#[no_mangle]
pub extern "C" fn meshemu_sdcard_close_file(handle: u32) -> bool {
    lock(&SD_FILE_HANDLES).remove(&handle).is_some()
}

/// Removes a regular file from a mounted SD card.
///
/// # Safety
///
/// String pointers must be valid NUL-terminated strings.
#[no_mangle]
pub unsafe extern "C" fn meshemu_sdcard_remove(
    instance_id: *const c_char,
    path: *const c_char,
) -> bool {
    let Some((instance_id, path)) = (unsafe { storage_file_args(instance_id, path) }) else {
        return false;
    };
    if !peripherals_powered(instance_id) {
        return false;
    }
    with_sd_spi(instance_id, || {
        lock(&STORAGE)
            .get(instance_id)
            .is_some_and(|manager| manager.sdcard.remove_file(path).is_ok())
    })
    .unwrap_or(false)
}

/// Closes instance file handles and unmounts its SD card.
///
/// # Safety
///
/// `instance_id` must point to a valid NUL-terminated string for this call.
#[no_mangle]
pub unsafe extern "C" fn meshemu_sdcard_end(instance_id: *const c_char) {
    let Some(instance_id) = (unsafe { ffi_string(instance_id) }) else {
        return;
    };
    set_current_instance(instance_id);
    lock(&SD_FILE_HANDLES).retain(|_, file| file.instance_id != instance_id);
    let _ = with_sd_spi(instance_id, || {
        if let Some(manager) = lock(&STORAGE).get_mut(instance_id) {
            manager.sdcard.unmount();
        }
    });
}

/// Reads an SD card file into a caller-owned allocation.
///
/// The returned buffer must be released with [`meshemu_storage_data_free`].
///
/// # Safety
///
/// String pointers must be valid NUL-terminated strings. `out_len` must be null
/// or point to writable memory for one `usize`.
#[no_mangle]
pub unsafe extern "C" fn meshemu_sdcard_read(
    instance_id: *const c_char,
    path: *const c_char,
    out_len: *mut usize,
) -> *mut u8 {
    if !out_len.is_null() {
        unsafe { *out_len = 0 };
    }
    let Some((instance_id, path)) = (unsafe { storage_file_args(instance_id, path) }) else {
        return ptr::null_mut();
    };
    if !peripherals_powered(instance_id) {
        return ptr::null_mut();
    }
    let Some(data) = with_sd_spi(instance_id, || {
        lock(&STORAGE)
            .get(instance_id)
            .and_then(|manager| manager.sdcard.read_file(path).ok())
    })
    .flatten() else {
        return ptr::null_mut();
    };
    copy_for_caller(&data, out_len)
}

/// Writes a complete SD card file.
///
/// # Safety
///
/// String pointers must be valid NUL-terminated strings. `data` must reference
/// `len` readable bytes, or may be null when `len` is zero.
#[no_mangle]
pub unsafe extern "C" fn meshemu_sdcard_write(
    instance_id: *const c_char,
    path: *const c_char,
    data: *const u8,
    len: usize,
) -> bool {
    let Some((instance_id, path)) = (unsafe { storage_file_args(instance_id, path) }) else {
        return false;
    };
    if !peripherals_powered(instance_id) {
        return false;
    }
    if data.is_null() && len != 0 {
        return false;
    }
    let bytes = if len == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(data, len) }
    };
    with_sd_spi(instance_id, || {
        lock(&STORAGE)
            .get(instance_id)
            .is_some_and(|manager| manager.sdcard.write_file(path, bytes).is_ok())
    })
    .unwrap_or(false)
}

/// Unmounts and removes all storage state for an emulator instance.
///
/// Returns whether an instance existed.
///
/// # Safety
///
/// `instance_id` must point to a valid NUL-terminated string for this call.
#[no_mangle]
pub unsafe extern "C" fn meshemu_storage_destroy(instance_id: *const c_char) -> bool {
    let Some(instance_id) = (unsafe { ffi_string(instance_id) }) else {
        return false;
    };
    lock(&SD_FILE_HANDLES).retain(|_, file| file.instance_id != instance_id);
    let Some(mut manager) = lock(&STORAGE).remove(instance_id) else {
        warn!(%instance_id, "storage_destroy: no storage state for instance");
        return false;
    };
    manager.unmount_all();
    mycelium_display::shared_spi::remove_instance(instance_id);
    true
}

/// Releases a buffer returned by a storage read function.
///
/// The empty-read sentinel from [`copy_for_caller`] and null are accepted as
/// no-ops; every other pointer must have been returned by a storage read
/// function.
///
/// # Safety
///
/// `data` must be null, the empty-read sentinel, or a pointer returned by a
/// storage read function.
#[no_mangle]
pub unsafe extern "C" fn meshemu_storage_data_free(data: *mut u8) {
    if data.is_null() || std::ptr::eq(data, EMPTY_READ_SENTINEL.as_ptr().cast_mut()) {
        return;
    }
    unsafe { libc::free(data.cast()) };
}

/// Creates an independently owned virtual GPS.
///
/// # Safety
///
/// `instance_id` must point to a valid NUL-terminated string for this call.
#[no_mangle]
pub unsafe extern "C" fn meshemu_gps_create(
    instance_id: *const c_char,
    lat: f64,
    lon: f64,
) -> *mut c_void {
    let Some(instance_id) = (unsafe { self::instance_id(instance_id) }) else {
        return ptr::null_mut();
    };
    if !valid_position(lat, lon) {
        warn!(%instance_id, lat, lon, "GPS create rejected: position out of range");
        return ptr::null_mut();
    }
    Box::into_raw(Box::new(GpsHandle {
        instance_id,
        manager: GpsManager::new(lat, lon),
    }))
    .cast()
}

/// Updates a virtual GPS position and altitude.
///
/// # Safety
///
/// `gps` must be a live GPS handle returned by [`meshemu_gps_create`].
#[no_mangle]
pub unsafe extern "C" fn meshemu_gps_set_position(gps: *mut c_void, lat: f64, lon: f64, alt: f64) {
    let Some(gps) = (unsafe { gps_mut(gps) }) else {
        return;
    };
    if valid_position(lat, lon) && alt.is_finite() {
        gps.manager.state_mut().latitude = lat;
        gps.manager.state_mut().longitude = lon;
        gps.manager.state_mut().altitude_m = alt;
    }
}

/// Copies the next NMEA bytes into `buf`.
///
/// # Safety
///
/// `gps` must be a live GPS handle and `buf` must reference at least
/// `max_len` writable bytes when `max_len` is positive.
#[no_mangle]
pub unsafe extern "C" fn meshemu_gps_read(gps: *mut c_void, buf: *mut u8, max_len: i32) -> i32 {
    let Some(gps) = (unsafe { gps_mut(gps) }) else {
        return 0;
    };
    if buf.is_null() || max_len <= 0 || !peripherals_powered(&gps.instance_id) {
        return 0;
    }
    let output = unsafe { std::slice::from_raw_parts_mut(buf, max_len as usize) };
    gps.manager.read(output).min(i32::MAX as usize) as i32
}

/// Advances the virtual GPS clock and movement model.
///
/// # Safety
///
/// `gps` must be a live GPS handle returned by [`meshemu_gps_create`].
#[no_mangle]
pub unsafe extern "C" fn meshemu_gps_tick(gps: *mut c_void, delta_ms: u64) {
    if let Some(gps) = unsafe { gps_mut(gps) } {
        if peripherals_powered(&gps.instance_id) {
            gps.manager.tick(delta_ms);
        }
    }
}

/// Enables or disables NMEA output.
///
/// # Safety
///
/// `gps` must be a live GPS handle returned by [`meshemu_gps_create`].
#[no_mangle]
pub unsafe extern "C" fn meshemu_gps_set_enabled(gps: *mut c_void, enabled: bool) {
    if let Some(gps) = unsafe { gps_mut(gps) } {
        gps.manager.state_mut().enabled = enabled;
    }
}

/// Configures the virtual UART baud rate.
///
/// The real L76K cycles between 9600 and 38400 baud. Returns `false` for
/// rates the receiver cannot use, leaving the current rate unchanged.
///
/// # Safety
///
/// `gps` must be a live GPS handle returned by [`meshemu_gps_create`].
#[no_mangle]
pub unsafe extern "C" fn meshemu_gps_set_baud_rate(gps: *mut c_void, baud_rate: u32) -> bool {
    unsafe { gps_mut(gps) }.is_some_and(|gps| gps.manager.set_baud_rate(baud_rate))
}

/// Pins the GPS clock to a Unix timestamp for deterministic NMEA replay.
///
/// Pass `unix_seconds > 0` to lock the clock; pass `0` to fall back to
/// the system clock.  Useful for reproducible integration tests.
///
/// # Safety
///
/// `gps` must be a live GPS handle returned by [`meshemu_gps_create`].
#[no_mangle]
pub unsafe extern "C" fn meshemu_gps_set_time(gps: *mut c_void, unix_seconds: i64) {
    if let Some(gps) = unsafe { gps_mut(gps) } {
        gps.manager
            .set_time((unix_seconds > 0).then_some(unix_seconds));
    }
}

/// Destroys a GPS handle.
///
/// # Safety
///
/// `gps` must be null or a live GPS handle, passed at most once.
#[no_mangle]
pub unsafe extern "C" fn meshemu_gps_destroy(gps: *mut c_void) {
    if !gps.is_null() {
        unsafe { drop(Box::from_raw(gps.cast::<GpsHandle>())) };
    }
}

/// Creates an independently owned virtual main board.
///
/// # Safety
///
/// `instance_id` must point to a valid NUL-terminated string for this call.
#[no_mangle]
pub unsafe extern "C" fn meshemu_board_create(
    instance_id: *const c_char,
    mv: u16,
    temp: f32,
) -> *mut c_void {
    let Some(instance_id) = (unsafe { self::instance_id(instance_id) }) else {
        return ptr::null_mut();
    };
    set_current_instance(&instance_id);
    if !temp.is_finite() {
        warn!(%instance_id, "Board create rejected: non-finite temperature");
        return ptr::null_mut();
    }
    let config = BoardConfig {
        battery_mv: mv,
        mcu_temperature: temp,
        ..BoardConfig::default()
    };
    Box::into_raw(Box::new(VirtualBoard::new(&instance_id, config))).cast()
}

/// Creates a virtual T-Deck with an explicitly configured PSRAM size.
/// A size of zero models a board without external PSRAM.
///
/// # Safety
///
/// `instance_id` must point to a valid NUL-terminated string for this call.
#[no_mangle]
pub unsafe extern "C" fn meshemu_board_create_ex(
    instance_id: *const c_char,
    mv: u16,
    temp: f32,
    psram_size_bytes: u32,
) -> *mut c_void {
    let Some(instance_id) = (unsafe { self::instance_id(instance_id) }) else {
        return ptr::null_mut();
    };
    set_current_instance(&instance_id);
    if !temp.is_finite() {
        warn!(%instance_id, "Board create rejected: non-finite temperature");
        return ptr::null_mut();
    }
    let config = BoardConfig {
        battery_mv: mv,
        mcu_temperature: temp,
        psram_size_bytes,
        ..BoardConfig::default()
    };
    Box::into_raw(Box::new(VirtualBoard::new(&instance_id, config))).cast()
}

/// # Safety
///
/// `board` must be a live board handle returned by [`meshemu_board_create`].
#[no_mangle]
pub unsafe extern "C" fn meshemu_board_get_battery(board: *mut c_void) -> u16 {
    unsafe { board_ref(board) }
        .map(VirtualBoard::get_battery_mv)
        .unwrap_or(0)
}

/// Reads a raw 12-bit ADC count from a board GPIO.
///
/// # Safety
///
/// `board` must be a live board handle returned by [`meshemu_board_create`].
#[no_mangle]
pub unsafe extern "C" fn meshemu_board_get_adc(board: *mut c_void, gpio: u8) -> u16 {
    unsafe { board_ref(board) }
        .map(|board| board.get_adc(gpio))
        .unwrap_or(0)
}

/// # Safety
///
/// `board` must be a live board handle returned by [`meshemu_board_create`].
#[no_mangle]
pub unsafe extern "C" fn meshemu_board_get_temp(board: *mut c_void) -> f32 {
    unsafe { board_ref(board) }
        .map(VirtualBoard::get_temperature)
        .unwrap_or(0.0)
}

/// Sets the value returned by the emulated ESP32 `temperatureRead()`.
///
/// # Safety
///
/// `board` must be a live board handle returned by [`meshemu_board_create`].
#[no_mangle]
pub unsafe extern "C" fn meshemu_board_set_mcu_temperature(board: *mut c_void, celsius: f32) {
    if let Some(board) = unsafe { board_mut(board) } {
        board.set_temperature(celsius);
    }
}

/// Returns the emulated ESP32 MCU temperature.
///
/// # Safety
///
/// `board` must be a live board handle returned by [`meshemu_board_create`].
#[no_mangle]
pub unsafe extern "C" fn meshemu_board_get_mcu_temperature(board: *mut c_void) -> f32 {
    unsafe { board_ref(board) }
        .map(VirtualBoard::get_temperature)
        .unwrap_or(0.0)
}

/// Writes bytes into per-instance RTC memory retained across board recreation.
///
/// # Safety
///
/// `instance_id` must be a valid C string. For nonzero `len`, `data` must
/// reference at least `len` readable bytes.
#[no_mangle]
pub unsafe extern "C" fn meshemu_board_set_rtc_noinit(
    instance_id: *const c_char,
    offset: usize,
    data: *const u8,
    len: usize,
) -> bool {
    let Some(instance_id) = (unsafe { ffi_string(instance_id) }) else {
        return false;
    };
    if data.is_null() && len != 0 {
        return false;
    }
    let bytes = if len == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(data, len) }
    };
    mycelium_board::set_rtc_noinit(instance_id, offset, bytes)
}

/// Reads bytes from per-instance RTC memory retained across board recreation.
///
/// # Safety
///
/// `instance_id` must be a valid C string. For nonzero `len`, `data` must
/// reference at least `len` writable bytes.
#[no_mangle]
pub unsafe extern "C" fn meshemu_board_get_rtc_noinit(
    instance_id: *const c_char,
    offset: usize,
    data: *mut u8,
    len: usize,
) -> bool {
    let Some(instance_id) = (unsafe { ffi_string(instance_id) }) else {
        return false;
    };
    if data.is_null() && len != 0 {
        return false;
    }
    let bytes = if len == 0 {
        &mut []
    } else {
        unsafe { std::slice::from_raw_parts_mut(data, len) }
    };
    mycelium_board::get_rtc_noinit(instance_id, offset, bytes)
}

/// Clears all retained RTC memory for an instance.
///
/// # Safety
///
/// `instance_id` must point to a valid NUL-terminated string for this call.
#[no_mangle]
pub unsafe extern "C" fn meshemu_board_clear_rtc_noinit(instance_id: *const c_char) {
    if let Some(instance_id) = unsafe { ffi_string(instance_id) } {
        mycelium_board::clear_rtc_noinit(instance_id);
    }
}

/// Reports whether external PSRAM is installed.
///
/// # Safety
///
/// `board` must be a live board handle returned by [`meshemu_board_create`].
#[no_mangle]
pub unsafe extern "C" fn meshemu_board_psram_found(board: *mut c_void) -> bool {
    unsafe { board_ref(board) }.is_some_and(VirtualBoard::psram_found)
}

/// Returns bytes still allocatable from the simulated external PSRAM heap.
///
/// # Safety
///
/// `board` must be a live board handle returned by [`meshemu_board_create`].
#[no_mangle]
pub unsafe extern "C" fn meshemu_board_get_psram_free(board: *mut c_void) -> u32 {
    unsafe { board_ref(board) }
        .map(VirtualBoard::psram_free_bytes)
        .unwrap_or(0)
}

/// Changes the PSRAM capacity of a board handle. A zero size models absent
/// PSRAM; shrinking below already-reserved memory is rejected.
///
/// # Safety
///
/// `board` must be a live board handle returned by [`meshemu_board_create`]
/// or [`meshemu_board_create_ex`].
#[no_mangle]
pub unsafe extern "C" fn meshemu_board_set_psram_size(
    board: *mut c_void,
    psram_size_bytes: u32,
) -> bool {
    unsafe { board_mut(board) }.is_some_and(|board| board.set_psram_size(psram_size_bytes))
}

/// Writes and verifies a deterministic pattern in simulated PSRAM.
///
/// # Safety
///
/// `board` must be a live board handle returned by [`meshemu_board_create`].
#[no_mangle]
pub unsafe extern "C" fn meshemu_board_psram_readback_test(board: *mut c_void) -> bool {
    unsafe { board_mut(board) }.is_some_and(VirtualBoard::psram_readback_test)
}

/// Reserves simulated PSRAM for a firmware allocation.
///
/// # Safety
///
/// `board` must be a live board handle returned by [`meshemu_board_create`].
#[no_mangle]
pub unsafe extern "C" fn meshemu_board_psram_reserve(board: *mut c_void, bytes: u32) -> bool {
    unsafe { board_mut(board) }.is_some_and(|board| board.reserve_psram(bytes))
}

/// Releases a previous simulated PSRAM reservation.
///
/// # Safety
///
/// `board` must be a live board handle returned by [`meshemu_board_create`].
#[no_mangle]
pub unsafe extern "C" fn meshemu_board_psram_release(board: *mut c_void, bytes: u32) {
    if let Some(board) = unsafe { board_mut(board) } {
        board.release_psram(bytes);
    }
}

/// # Safety
///
/// `board` must be a live board handle returned by [`meshemu_board_create`].
#[no_mangle]
pub unsafe extern "C" fn meshemu_board_set_battery(board: *mut c_void, mv: u16) {
    if let Some(board) = unsafe { board_mut(board) } {
        board.set_battery(mv);
    }
}

/// Selects calibrated `analogReadMilliVolts()`-equivalent ADC behavior.
///
/// # Safety
///
/// `board` must be a live board handle returned by [`meshemu_board_create`].
#[no_mangle]
pub unsafe extern "C" fn meshemu_board_set_adc_calibration(board: *mut c_void, calibrated: bool) {
    if let Some(board) = unsafe { board_mut(board) } {
        board.set_adc_calibration(calibrated);
    }
}

/// Drives one board GPIO. GPIO10 is the active-HIGH peripheral power rail.
///
/// # Safety
///
/// `board` must be a live board handle returned by [`meshemu_board_create`].
#[no_mangle]
pub unsafe extern "C" fn meshemu_board_digital_write(board: *mut c_void, gpio: u8, high: bool) {
    if let Some(board) = unsafe { board_mut(board) } {
        board.digital_write(gpio, high);
    }
}

/// Drives GPIO10, the active-HIGH T-Deck peripheral power rail.
///
/// # Safety
///
/// `board` must be a live board handle returned by [`meshemu_board_create`].
#[no_mangle]
pub unsafe extern "C" fn meshemu_board_set_periph_power(board: *mut c_void, enabled: bool) {
    if let Some(board) = unsafe { board_mut(board) } {
        board.digital_write(mycelium_board::PERIPH_PWR_EN_GPIO, enabled);
    }
}

/// Attaches a GPIO to an emulated LEDC channel.
///
/// # Safety
///
/// `board` must be a live board handle returned by [`meshemu_board_create`].
#[no_mangle]
pub unsafe extern "C" fn meshemu_board_ledc_attach(board: *mut c_void, channel: u8, gpio: u8) {
    if let Some(board) = unsafe { board_mut(board) } {
        board.ledc_attach(channel, gpio);
    }
}

/// Writes a PWM period and HIGH time to an emulated LEDC channel.
///
/// # Safety
///
/// `board` must be a live board handle returned by [`meshemu_board_create`].
#[no_mangle]
pub unsafe extern "C" fn meshemu_board_ledc_write(
    board: *mut c_void,
    channel: u8,
    period_us: u32,
    high_time_us: u32,
) -> bool {
    unsafe { board_mut(board) }
        .is_some_and(|board| board.ledc_write(channel, period_us, high_time_us))
}

/// Updates external-power detection for the TP4054 charger.
///
/// # Safety
///
/// `board` must be a live board handle returned by [`meshemu_board_create`].
#[no_mangle]
pub unsafe extern "C" fn meshemu_board_set_external_power(board: *mut c_void, powered: bool) {
    if let Some(board) = unsafe { board_mut(board) } {
        board.set_external_power(powered);
    }
}

/// Returns the current TP4054 state as a `MESHEMU_TP4054_*` value.
///
/// # Safety
///
/// `board` must be a live board handle returned by [`meshemu_board_create`].
#[no_mangle]
pub unsafe extern "C" fn meshemu_board_get_charger_state(board: *mut c_void) -> u8 {
    unsafe { board_ref(board) }
        .map(|board| board.charger_state() as u8)
        .unwrap_or(Tp4054State::NoBattery as u8)
}

/// Holds one RTC-capable GPIO at `level` across deep sleep.
///
/// # Safety
///
/// `board` must be a live board handle returned by [`meshemu_board_create`].
#[no_mangle]
pub unsafe extern "C" fn meshemu_board_rtc_gpio_hold(board: *mut c_void, gpio: u8, level: bool) {
    if let Some(board) = unsafe { board_mut(board) } {
        board.rtc_gpio_hold(gpio, level);
    }
}

/// Sets the reset cause reported for this emulated ESP32.
///
/// # Safety
///
/// `board` must be a live board handle returned by [`meshemu_board_create`].
#[no_mangle]
pub unsafe extern "C" fn meshemu_board_set_reset_reason(board: *mut c_void, reason: u8) -> bool {
    unsafe { board_mut(board) }.is_some_and(|board| board.set_reset_reason(reason))
}

/// Returns the reset cause reported for this emulated ESP32.
///
/// # Safety
///
/// `board` must be a live board handle returned by [`meshemu_board_create`].
#[no_mangle]
pub unsafe extern "C" fn meshemu_board_get_reset_reason(board: *mut c_void) -> u8 {
    unsafe { board_ref(board) }
        .map(VirtualBoard::reset_reason)
        .unwrap_or(mycelium_board::RESET_REASON_UNKNOWN)
}

/// Configures the emulated task watchdog.
///
/// # Safety
///
/// `board` must be a live board handle returned by [`meshemu_board_create`].
#[no_mangle]
pub unsafe extern "C" fn meshemu_board_wdt_init(
    board: *mut c_void,
    timeout_sec: u32,
    panic_on_timeout: bool,
) {
    if let Some(board) = unsafe { board_mut(board) } {
        board.wdt_init(timeout_sec, panic_on_timeout);
    }
}

/// Feeds the task watchdog, returning false when disabled or already expired.
///
/// # Safety
///
/// `board` must be a live board handle returned by [`meshemu_board_create`].
#[no_mangle]
pub unsafe extern "C" fn meshemu_board_wdt_feed(board: *mut c_void) -> bool {
    unsafe { board_mut(board) }.is_some_and(VirtualBoard::wdt_feed)
}

/// Returns `MESHEMU_WDT_*` for the task watchdog.
///
/// # Safety
///
/// `board` must be a live board handle returned by [`meshemu_board_create`].
#[no_mangle]
pub unsafe extern "C" fn meshemu_board_wdt_get_status(board: *mut c_void) -> u8 {
    unsafe { board_mut(board) }
        .map(VirtualBoard::wdt_status)
        .unwrap_or(mycelium_board::WDT_STATUS_DISABLED)
}

/// Disables the task watchdog.
///
/// # Safety
///
/// `board` must be a live board handle returned by [`meshemu_board_create`].
#[no_mangle]
pub unsafe extern "C" fn meshemu_board_wdt_disable(board: *mut c_void) {
    if let Some(board) = unsafe { board_mut(board) } {
        board.wdt_disable();
    }
}

/// Runs the T-Deck peripheral shutdown sequence and latches GPIO10 LOW.
///
/// # Safety
///
/// `board` must be a live board handle returned by [`meshemu_board_create`].
#[no_mangle]
pub unsafe extern "C" fn meshemu_board_quiesce_peripherals(board: *mut c_void) -> bool {
    let Some(board) = (unsafe { board_mut(board) }) else {
        return false;
    };
    if let Some(storage) = lock(&STORAGE).get_mut(&board.instance_id) {
        storage.sdcard.unmount();
    }
    board.quiesce_peripherals();
    true
}

/// Enters and completes one synchronous virtual deep-sleep interval.
///
/// Bus time advances by `sleep_secs`; the instance radio drops packets that
/// finish during that interval. A nonzero wake mask models an EXT1 wake
/// source, so the reported wake cause is a bitwise combination of timer and
/// EXT1 flags.
///
/// # Safety
///
/// `instance_id` must point to a valid NUL-terminated string for this call.
#[no_mangle]
pub unsafe extern "C" fn meshemu_board_deep_sleep(
    instance_id: *const c_char,
    sleep_secs: u32,
    wake_pin_mask: u64,
) -> u64 {
    let Some(instance_id) = (unsafe { ffi_string(instance_id) }) else {
        return lock(&BUS).now_ms;
    };
    set_current_instance(instance_id);
    mycelium_board::set_reset_reason(instance_id, mycelium_board::RESET_REASON_DEEPSLEEP);

    let wake_cause = if sleep_secs > 0 {
        SLEEP_WAKE_CAUSE_TIMER
    } else {
        SLEEP_WAKE_CAUSE_UNKNOWN
    } | if wake_pin_mask != 0 {
        SLEEP_WAKE_CAUSE_EXT1
    } else {
        SLEEP_WAKE_CAUSE_UNKNOWN
    };

    let mut state = lock(&BUS);
    let requested_at_ms = state.now_ms;
    let wake_at_ms = requested_at_ms.saturating_add(u64::from(sleep_secs).saturating_mul(1_000));
    state.sleep_requests.insert(
        instance_id.to_owned(),
        (requested_at_ms, wake_at_ms, sleep_secs, wake_pin_mask, true),
    );
    state.bus.set_receive_enabled(instance_id, false);
    state.bus.tick(wake_at_ms);
    state.now_ms = wake_at_ms;
    state.bus.set_receive_enabled(instance_id, true);
    if let Some(request) = state.sleep_requests.get_mut(instance_id) {
        request.4 = false;
    }
    state
        .last_wake_causes
        .insert(instance_id.to_owned(), wake_cause);
    wake_at_ms
}

/// Returns the wake-cause flags from the most recently completed sleep.
#[no_mangle]
pub extern "C" fn meshemu_board_get_sleep_wake_cause() -> u8 {
    let Some(instance_id) = current_instance() else {
        return SLEEP_WAKE_CAUSE_UNKNOWN;
    };
    lock(&BUS)
        .last_wake_causes
        .get(&instance_id)
        .copied()
        .unwrap_or(SLEEP_WAKE_CAUSE_UNKNOWN)
}

/// Returns the wake-cause flags for one explicit virtual board instance.
///
/// # Safety
///
/// `instance_id` must point to a valid NUL-terminated string for this call.
#[no_mangle]
pub unsafe extern "C" fn meshemu_board_get_sleep_wake_cause_for_instance(
    instance_id: *const c_char,
) -> u8 {
    let Some(instance_id) = (unsafe { ffi_string(instance_id) }) else {
        return SLEEP_WAKE_CAUSE_UNKNOWN;
    };
    lock(&BUS)
        .last_wake_causes
        .get(instance_id)
        .copied()
        .unwrap_or(SLEEP_WAKE_CAUSE_UNKNOWN)
}

/// Persists the latest boot checkpoint across board-handle restarts.
#[no_mangle]
pub extern "C" fn meshemu_board_set_boot_phase(phase: u8) {
    if let Some(instance_id) = current_instance() {
        mycelium_board::set_boot_phase_for_instance(&instance_id, phase);
    } else {
        mycelium_board::set_boot_phase(phase);
    }
}

/// Returns the latest persistent boot checkpoint.
#[no_mangle]
pub extern "C" fn meshemu_board_get_last_boot_phase() -> u8 {
    current_instance().map_or_else(mycelium_board::last_boot_phase, |instance_id| {
        mycelium_board::last_boot_phase_for_instance(&instance_id)
    })
}

/// Persists a boot checkpoint for one explicit virtual board instance.
///
/// # Safety
///
/// `instance_id` must point to a valid NUL-terminated string for this call.
#[no_mangle]
pub unsafe extern "C" fn meshemu_board_set_boot_phase_for_instance(
    instance_id: *const c_char,
    phase: u8,
) {
    let Some(instance_id) = (unsafe { ffi_string(instance_id) }) else {
        return;
    };
    set_current_instance(instance_id);
    mycelium_board::set_boot_phase_for_instance(instance_id, phase);
}

/// Returns the boot checkpoint for one explicit virtual board instance.
///
/// # Safety
///
/// `instance_id` must point to a valid NUL-terminated string for this call.
#[no_mangle]
pub unsafe extern "C" fn meshemu_board_get_last_boot_phase_for_instance(
    instance_id: *const c_char,
) -> u8 {
    let Some(instance_id) = (unsafe { ffi_string(instance_id) }) else {
        return mycelium_board::BD_STARTUP_NORMAL;
    };
    mycelium_board::last_boot_phase_for_instance(instance_id)
}

/// Destroys a board handle.
///
/// # Safety
///
/// `board` must be null or a live board handle, passed at most once.
#[no_mangle]
pub unsafe extern "C" fn meshemu_board_destroy(board: *mut c_void) {
    if !board.is_null() {
        unsafe { drop(Box::from_raw(board.cast::<VirtualBoard>())) };
    }
}

/// Creates an independently owned virtual T-Deck keyboard.
#[no_mangle]
pub extern "C" fn meshemu_i2c_keyboard_create() -> *mut c_void {
    let keyboard = std::sync::Arc::new(Mutex::new(I2cKeyboardBus::new()));
    Box::into_raw(Box::new(keyboard)).cast()
}

/// Creates a keyboard whose retained C3 backlight belongs to one instance.
///
/// # Safety
///
/// `instance_id` must point to a valid NUL-terminated string for this call.
#[no_mangle]
pub unsafe extern "C" fn meshemu_i2c_keyboard_create_for_instance(
    instance_id: *const c_char,
) -> *mut c_void {
    let Some(instance_id) = (unsafe { ffi_string(instance_id) }) else {
        return ptr::null_mut();
    };
    set_current_instance(instance_id);
    let keyboard = std::sync::Arc::new(Mutex::new(I2cKeyboardBus::new_for_instance(instance_id)));
    Box::into_raw(Box::new(keyboard)).cast()
}

/// Injects the exact key byte returned by the T-Deck ESP32-C3.
///
/// # Safety
///
/// `keyboard` must be null or a live keyboard handle returned by
/// [`meshemu_i2c_keyboard_create`].
#[no_mangle]
pub unsafe extern "C" fn meshemu_i2c_keyboard_inject_key_byte(keyboard: *mut c_void, key_byte: u8) {
    let Some(keyboard) = (unsafe { keyboard_ref(keyboard) }) else {
        return;
    };
    lock(keyboard).inject_key_byte(key_byte);
}

/// Configure whether the emulated C3 retains its backlight across host resets.
///
/// # Safety
///
/// `keyboard` must be null or a live keyboard handle returned by
/// [`meshemu_i2c_keyboard_create`].
#[no_mangle]
pub unsafe extern "C" fn meshemu_i2c_keyboard_set_cross_reset(
    keyboard: *mut c_void,
    persist: bool,
) {
    let Some(keyboard) = (unsafe { keyboard_ref(keyboard) }) else {
        return;
    };
    lock(keyboard).set_cross_reset_persist(persist);
}

/// Destroys a keyboard handle.
///
/// Wire shims attached to this keyboard retain shared ownership and remain
/// valid until they are destroyed or assigned another keyboard.
///
/// # Safety
///
/// `keyboard` must be null or a live keyboard handle, passed at most once.
#[no_mangle]
pub unsafe extern "C" fn meshemu_i2c_keyboard_destroy(keyboard: *mut c_void) {
    if !keyboard.is_null() {
        unsafe { drop(Box::from_raw(keyboard.cast::<SharedI2cKeyboard>())) };
    }
}

/// Creates a virtual Arduino Wire shim with its own keyboard bus.
#[no_mangle]
pub extern "C" fn meshemu_wire_shim_create() -> *mut c_void {
    Box::into_raw(Box::new(WireShim::new())).cast()
}

/// Creates a Wire shim attached to the same peripherals as an SDL input manager.
///
/// # Safety
///
/// `instance_id` must point to a valid NUL-terminated string for this call.
#[no_mangle]
pub unsafe extern "C" fn meshemu_wire_shim_create_for_instance(
    instance_id: *const c_char,
) -> *mut c_void {
    let Some(manager) = (unsafe { input_manager(instance_id, true) }) else {
        return ptr::null_mut();
    };
    let (mut wire, instance_id) = {
        let manager = manager
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        (manager.wire_shim(), manager.instance_id().to_owned())
    };
    wire.set_peripheral_power_check(move || peripherals_powered(&instance_id));
    Box::into_raw(Box::new(wire)).cast()
}

/// Assigns a keyboard to a Wire shim.
///
/// # Safety
///
/// Both arguments must be null or live handles of their corresponding types.
#[no_mangle]
pub unsafe extern "C" fn meshemu_wire_shim_set_keyboard(wire: *mut c_void, keyboard: *mut c_void) {
    let Some(wire) = (unsafe { wire_mut(wire) }) else {
        return;
    };
    let Some(keyboard) = (unsafe { keyboard_ref(keyboard) }) else {
        return;
    };
    wire.set_keyboard(std::sync::Arc::clone(keyboard));
}

/// Initializes a Wire shim.
///
/// # Safety
///
/// `wire` must be a live Wire shim handle.
#[no_mangle]
pub unsafe extern "C" fn meshemu_wire_begin(wire: *mut c_void) -> bool {
    unsafe { wire_mut(wire) }
        .map(WireShim::begin)
        .unwrap_or(false)
}

/// Configures the Wire clock.
///
/// # Safety
///
/// `wire` must be a live Wire shim handle.
#[no_mangle]
pub unsafe extern "C" fn meshemu_wire_set_clock(wire: *mut c_void, clock_hz: u32) {
    if let Some(wire) = unsafe { wire_mut(wire) } {
        wire.set_clock(clock_hz);
    }
}

/// Return whether a device ACKs an address-only I2C probe.
///
/// # Safety
///
/// `wire` must be a live Wire shim handle.
#[no_mangle]
pub unsafe extern "C" fn meshemu_wire_probe_address(wire: *mut c_void, address: u8) -> bool {
    unsafe { wire_mut(wire) }.is_some_and(|wire| wire.probe_address(address))
}

/// Read the externally pulled-up idle levels of SDA and SCL.
///
/// Invalid handles report LOW to non-null outputs.
///
/// # Safety
///
/// `wire` must be null or a live Wire shim handle. Output pointers may be null,
/// otherwise each must be valid for one `u8` write.
#[no_mangle]
pub unsafe extern "C" fn meshemu_wire_read_idle_levels(
    wire: *mut c_void,
    sda: *mut u8,
    scl: *mut u8,
) {
    if !sda.is_null() {
        unsafe { *sda = 0 };
    }
    if !scl.is_null() {
        unsafe { *scl = 0 };
    }
    let Some(wire) = (unsafe { wire_mut(wire) }) else {
        return;
    };
    let (sda_level, scl_level) = wire.idle_levels();
    if !sda.is_null() {
        unsafe { *sda = sda_level };
    }
    if !scl.is_null() {
        unsafe { *scl = scl_level };
    }
}

/// Clocks SCL nine times to release a slave holding SDA LOW.
///
/// Returns 0=already free, 1=recovered, or 2=still stuck.
///
/// # Safety
///
/// `wire` must be a live Wire shim handle.
#[no_mangle]
pub unsafe extern "C" fn meshemu_wire_clock_out_recovery(wire: *mut c_void) -> u8 {
    unsafe { wire_mut(wire) }
        .map(WireShim::clock_out_recovery)
        .unwrap_or(2)
}

/// Emits a STOP condition and clears incomplete Wire transaction state.
///
/// # Safety
///
/// `wire` must be a live Wire shim handle.
#[no_mangle]
pub unsafe extern "C" fn meshemu_wire_emit_stop(wire: *mut c_void) {
    if let Some(wire) = unsafe { wire_mut(wire) } {
        wire.emit_stop();
    }
}

/// Simulates a slave holding SDA LOW.
///
/// # Safety
///
/// `wire` must be a live Wire shim handle.
#[no_mangle]
pub unsafe extern "C" fn meshemu_wire_set_sda_stuck(wire: *mut c_void, stuck: bool) {
    if let Some(wire) = unsafe { wire_mut(wire) } {
        wire.set_sda_stuck(stuck);
    }
}

/// Selects the I2C target for a Wire transaction.
///
/// # Safety
///
/// `wire` must be a live Wire shim handle.
#[no_mangle]
pub unsafe extern "C" fn meshemu_wire_begin_transmission(wire: *mut c_void, address: u8) {
    if let Some(wire) = unsafe { wire_mut(wire) } {
        wire.begin_transmission(address);
    }
}

/// Writes one byte to the current Wire transaction.
///
/// # Safety
///
/// `wire` must be a live Wire shim handle.
#[no_mangle]
pub unsafe extern "C" fn meshemu_wire_write(wire: *mut c_void, byte: u8) -> usize {
    unsafe { wire_mut(wire) }
        .map(|wire| wire.write_byte(byte))
        .unwrap_or(0)
}

/// Finishes the current Wire transmission.
///
/// # Safety
///
/// `wire` must be a live Wire shim handle.
#[no_mangle]
pub unsafe extern "C" fn meshemu_wire_end_transmission(wire: *mut c_void) -> u8 {
    unsafe { wire_mut(wire) }
        .map(WireShim::end_transmission)
        .unwrap_or(4)
}

/// Requests bytes from an I2C target.
///
/// # Safety
///
/// `wire` must be a live Wire shim handle.
#[no_mangle]
pub unsafe extern "C" fn meshemu_wire_request_from(
    wire: *mut c_void,
    address: u8,
    count: u8,
) -> u8 {
    unsafe { wire_mut(wire) }
        .map(|wire| wire.request_from(address, count))
        .unwrap_or(0)
}

/// Returns the number of unread bytes buffered by the Wire shim.
///
/// # Safety
///
/// `wire` must be a live Wire shim handle.
#[no_mangle]
pub unsafe extern "C" fn meshemu_wire_available(wire: *mut c_void) -> i32 {
    unsafe { wire_mut(wire) }
        .map(|wire| wire.available())
        .unwrap_or(0)
}

/// Reads the next buffered byte or `-1` when none is available.
///
/// # Safety
///
/// `wire` must be a live Wire shim handle.
#[no_mangle]
pub unsafe extern "C" fn meshemu_wire_read(wire: *mut c_void) -> i32 {
    unsafe { wire_mut(wire) }.map(WireShim::read).unwrap_or(-1)
}

/// Destroys a Wire shim.
///
/// # Safety
///
/// `wire` must be null or a live Wire shim handle, passed at most once.
#[no_mangle]
pub unsafe extern "C" fn meshemu_wire_shim_destroy(wire: *mut c_void) {
    if !wire.is_null() {
        unsafe { drop(Box::from_raw(wire.cast::<WireShim>())) };
    }
}

#[track_caller]
unsafe fn input_manager(instance_id: *const c_char, create: bool) -> Option<SharedInputManager> {
    if instance_id.is_null() {
        warn!(caller = %std::panic::Location::caller(), "FFI instance_id argument is NULL");
        return None;
    }
    let Ok(instance_id) = (unsafe { CStr::from_ptr(instance_id) }).to_str() else {
        warn!(caller = %std::panic::Location::caller(), "FFI instance_id argument is not valid UTF-8");
        return None;
    };
    if instance_id.is_empty() {
        warn!(caller = %std::panic::Location::caller(), "FFI instance_id argument is empty");
        return None;
    }
    set_current_instance(instance_id);
    get_input_manager(instance_id)
        .or_else(|| create.then(|| register_input_manager(instance_id, 1.0)))
}

/// Injects a logical GT911 touch coordinate into an instance's input queue.
///
/// # Safety
///
/// `instance_id` must point to a valid NUL-terminated string for this call.
#[no_mangle]
pub unsafe extern "C" fn meshemu_input_inject_touch(
    instance_id: *const c_char,
    x: u16,
    y: u16,
    pressed: bool,
) {
    let Some(manager) = (unsafe { input_manager(instance_id, true) }) else {
        return;
    };
    manager
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .inject_touch(x, y, pressed);
}

/// Injects an SDL keycode into an instance's input queue.
///
/// # Safety
///
/// `instance_id` must point to a valid NUL-terminated string for this call.
#[no_mangle]
pub unsafe extern "C" fn meshemu_input_inject_key(
    instance_id: *const c_char,
    keycode: u32,
    pressed: bool,
) {
    let Some(keycode) = Keycode::from_i32(keycode as i32) else {
        return;
    };
    let Some(manager) = (unsafe { input_manager(instance_id, true) }) else {
        return;
    };
    manager
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .inject_key(keycode, pressed);
}

/// Polls one touch event, packed as x[0..15], y[16..31], pressure[32..39].
///
/// Returns zero when no event is queued.
///
/// # Safety
///
/// `instance_id` must point to a valid NUL-terminated string for this call.
#[no_mangle]
pub unsafe extern "C" fn meshemu_input_poll_touch(instance_id: *const c_char) -> u64 {
    let Some(manager) = (unsafe { input_manager(instance_id, false) }) else {
        return 0;
    };
    let event = manager
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .poll_touch();
    let Some(event) = event else {
        return 0;
    };
    u64::from(event.x) | (u64::from(event.y) << 16) | (u64::from(event.pressure) << 32)
}

/// Return the latest touch coordinate before the historical portrait mapping.
///
/// Missing instances or touches report `(0, 0)`.
///
/// # Safety
///
/// `instance_id` must point to a valid NUL-terminated string for this call.
/// Output pointers may be null, otherwise each must be valid for one `u16`
/// write.
#[no_mangle]
pub unsafe extern "C" fn meshemu_input_get_touch_raw(
    instance_id: *const c_char,
    x: *mut u16,
    y: *mut u16,
) {
    unsafe { write_touch_position(instance_id, x, y, false) };
}

/// Return the latest touch coordinate after the historical portrait mapping.
///
/// This is the same coordinate space returned by `meshemu_input_poll_touch`.
/// Missing instances or touches report `(0, 0)`.
///
/// # Safety
///
/// `instance_id` must point to a valid NUL-terminated string for this call.
/// Output pointers may be null, otherwise each must be valid for one `u16`
/// write.
#[no_mangle]
pub unsafe extern "C" fn meshemu_input_get_touch_mapped(
    instance_id: *const c_char,
    x: *mut u16,
    y: *mut u16,
) {
    unsafe { write_touch_position(instance_id, x, y, true) };
}

unsafe fn write_touch_position(instance_id: *const c_char, x: *mut u16, y: *mut u16, mapped: bool) {
    if !x.is_null() {
        unsafe { *x = 0 };
    }
    if !y.is_null() {
        unsafe { *y = 0 };
    }
    let Some(manager) = (unsafe { input_manager(instance_id, false) }) else {
        return;
    };
    let manager = manager
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let position = if mapped {
        manager.touch_mapped_position()
    } else {
        manager.touch_raw_position()
    };
    let Some((touch_x, touch_y)) = position else {
        return;
    };
    if !x.is_null() {
        unsafe { *x = touch_x };
    }
    if !y.is_null() {
        unsafe { *y = touch_y };
    }
}

/// Configure a GT911 failure mode for all live and future controllers.
#[no_mangle]
pub extern "C" fn meshemu_input_gt911_set_failure_mode(mode: u8, value: u32) {
    mycelium_input::set_global_failure_mode(mode, value);
}

/// Return sticky watchdog-fired flags across live GT911 controllers.
#[no_mangle]
pub extern "C" fn meshemu_input_gt911_get_status() -> u64 {
    mycelium_input::global_watchdog_status()
}

/// Configure a GT911 failure mode for one input instance only.
///
/// # Safety
///
/// `instance_id` must point to a valid NUL-terminated string for this call.
#[no_mangle]
pub unsafe extern "C" fn meshemu_input_gt911_set_failure_mode_for_instance(
    instance_id: *const c_char,
    mode: u8,
    value: u32,
) {
    let Some(manager) = (unsafe { input_manager(instance_id, true) }) else {
        return;
    };
    manager
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .set_gt911_failure_mode(mode, value);
}

/// Return sticky GT911 watchdog flags for one input instance only.
///
/// # Safety
///
/// `instance_id` must point to a valid NUL-terminated string for this call.
#[no_mangle]
pub unsafe extern "C" fn meshemu_input_gt911_get_status_for_instance(
    instance_id: *const c_char,
) -> u64 {
    let Some(manager) = (unsafe { input_manager(instance_id, false) }) else {
        return 0;
    };
    let status = manager
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .gt911_watchdog_status();
    status
}

/// Persist the current GT911 calibration for an instance.
///
/// Stores `(max_x, max_y, contact_size)` into NVS under the "touch" namespace
/// so the controller can be restored after a virtual restart.
///
/// # Safety
///
/// `instance_id` must point to a valid NUL-terminated string for this call.
#[no_mangle]
pub unsafe extern "C" fn meshemu_input_gt911_save_calibration(
    instance_id: *const c_char,
    max_x: u16,
    max_y: u16,
    contact_size: u16,
) -> bool {
    let Some(instance_id) = (unsafe { ffi_string(instance_id) }) else {
        return false;
    };
    let Some(nvs) = mycelium_board::get_nvs(instance_id) else {
        warn!(%instance_id, "GT911 save_calibration: NVS not initialized");
        return false;
    };
    let mut nvs = nvs.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if !nvs.begin("touch", false) {
        warn!(%instance_id, "GT911 save_calibration: could not open touch namespace");
        return false;
    }
    let blob = format!("{max_x}:{max_y}:{contact_size}");
    nvs.put_string("gt911_cal", &blob);
    nvs.end();
    true
}

/// Restore GT911 calibration from NVS for an instance.
///
/// Reads the calibration string written by `meshemu_input_gt911_save_calibration`
/// and applies it to the instance's touch controller.
///
/// # Safety
///
/// `instance_id` must point to a valid NUL-terminated string for this call.
#[no_mangle]
pub unsafe extern "C" fn meshemu_input_gt911_load_calibration(instance_id: *const c_char) -> bool {
    let Some(instance_id) = (unsafe { ffi_string(instance_id) }) else {
        return false;
    };
    let Some(nvs) = mycelium_board::get_nvs(instance_id) else {
        return false;
    };
    let mut nvs = nvs.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if !nvs.begin("touch", true) {
        return false;
    }
    let cal = nvs.get_string("gt911_cal", "");
    nvs.end();
    let parts: Vec<&str> = cal.split(':').collect();
    if parts.len() != 3 {
        return false;
    }
    let Ok(max_x) = parts[0].parse::<u16>() else {
        return false;
    };
    let Ok(max_y) = parts[1].parse::<u16>() else {
        return false;
    };
    let Ok(contact_size) = parts[2].parse::<u16>() else {
        return false;
    };
    // Apply to live controller if present
    if let Some(manager) = unsafe { input_manager(instance_id.as_ptr() as *const _, false) } {
        let controller = manager
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .gt911();
        let mut c = controller
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        c.set_resolution(max_x, max_y);
        c.set_contact_size(contact_size);
    }
    true
}

/// Returns the device that currently owns the global shared SPI bus.
///
/// Returns 0=none, 1=Display, 2=Sx1262, 3=SdCard.
#[no_mangle]
pub extern "C" fn meshemu_spi_bus_owner() -> u8 {
    match mycelium_display::shared_spi::global_spi_bus().owner() {
        None => 0,
        Some(mycelium_display::shared_spi::SpiDevice::Display) => 1,
        Some(mycelium_display::shared_spi::SpiDevice::Sx1262) => 2,
        Some(mycelium_display::shared_spi::SpiDevice::SdCard) => 3,
    }
}

/// Polls one keyboard event, packed as row[0..7], col[8..15], pressed[16].
///
/// Returns zero when no event is queued.
///
/// # Safety
///
/// `instance_id` must point to a valid NUL-terminated string for this call.
#[no_mangle]
pub unsafe extern "C" fn meshemu_input_poll_key(instance_id: *const c_char) -> u64 {
    let Some(manager) = (unsafe { input_manager(instance_id, false) }) else {
        return 0;
    };
    let event = manager
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .poll_keyboard();
    let Some(event) = event else {
        return 0;
    };
    u64::from(event.row) | (u64::from(event.col) << 8) | (u64::from(event.pressed) << 16)
}

/// Reads a T-Deck input GPIO, including GT911 INT on GPIO16.
///
/// # Safety
///
/// `instance_id` must point to a valid NUL-terminated string for this call.
#[no_mangle]
pub unsafe extern "C" fn meshemu_input_digital_read(instance_id: *const c_char, gpio: u8) -> bool {
    let Some(manager) = (unsafe { input_manager(instance_id, false) }) else {
        return true;
    };
    let level = manager
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .digital_read(gpio);
    level
}

/// Consumes pending FALLING interrupts for one trackball GPIO.
///
/// # Safety
///
/// `instance_id` must point to a valid NUL-terminated string for this call.
#[no_mangle]
pub unsafe extern "C" fn meshemu_input_take_falling_edges(
    instance_id: *const c_char,
    gpio: u8,
) -> u32 {
    let Some(manager) = (unsafe { input_manager(instance_id, false) }) else {
        return 0;
    };
    let edges = manager
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take_falling_edges(gpio);
    edges
}

/// Sets or clears the sticky hardware interrupt-enable bit for a trackball pin.
///
/// # Safety
///
/// `instance_id` must point to a valid NUL-terminated string for this call.
#[no_mangle]
pub unsafe extern "C" fn meshemu_input_set_gpio_intr_enabled(
    instance_id: *const c_char,
    gpio: u8,
    enabled: bool,
) {
    let Some(manager) = (unsafe { input_manager(instance_id, true) }) else {
        return;
    };
    manager
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .set_gpio_intr_enabled(gpio, enabled);
}

/// Reads the hardware interrupt-enable bit for a trackball pin.
///
/// # Safety
///
/// `instance_id` must point to a valid NUL-terminated string for this call.
#[no_mangle]
pub unsafe extern "C" fn meshemu_input_get_gpio_intr_enabled(
    instance_id: *const c_char,
    gpio: u8,
) -> bool {
    let Some(manager) = (unsafe { input_manager(instance_id, false) }) else {
        return false;
    };
    let enabled = manager
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .gpio_intr_enabled(gpio);
    enabled
}

/// Creates an SDL display for the requested LVGL major version.
///
/// Returns null for non-T-Deck geometry, an unsupported ABI, or when v9 was
/// requested but the active firmware does not export a compatible RGB565 LVGL
/// SDL driver.
///
/// # Safety
///
/// `window_title` must be null or point to a valid NUL-terminated string.
#[no_mangle]
pub unsafe extern "C" fn meshemu_display_create_v(
    width: i32,
    height: i32,
    window_title: *const c_char,
    lvgl_version: i32,
) -> *mut c_void {
    if !matches!(lvgl_version, 8 | 9) {
        warn!(
            lvgl_version,
            "Display create rejected: unsupported LVGL version"
        );
        return ptr::null_mut();
    }
    unsafe { mycelium_display::meshemu_display_create_v(width, height, window_title, lvgl_version) }
}

/// Creates an LVGL display with explicit partial-buffer and fidelity options.
///
/// # Safety
///
/// `window_title` and `options` must be null or point to readable values for
/// this call.
#[no_mangle]
pub unsafe extern "C" fn meshemu_display_create_ex(
    width: i32,
    height: i32,
    window_title: *const c_char,
    lvgl_version: i32,
    options: *const mycelium_display::DisplayBackendOptions,
) -> *mut c_void {
    unsafe {
        mycelium_display::meshemu_display_create_ex(
            width,
            height,
            window_title,
            lvgl_version,
            options,
        )
    }
}

/// Creates a default LVGL v9 display.
///
/// # Safety
///
/// `window_title` must be null or point to a valid NUL-terminated string.
#[no_mangle]
pub unsafe extern "C" fn meshemu_display_create(
    width: i32,
    height: i32,
    window_title: *const c_char,
) -> *mut c_void {
    unsafe { meshemu_display_create_v(width, height, window_title, 9) }
}

/// Captures an LVGL display as native-endian packed RGB565.
///
/// The caller owns the returned allocation and must release it with
/// [`meshemu_display_capture_free`], passing the value written to `size_out`.
///
/// # Safety
///
/// `display` must be null or a live LVGL SDL display. `size_out` must be null
/// or point to writable memory for one `usize`.
#[no_mangle]
pub unsafe extern "C" fn meshemu_display_capture(
    display: *mut c_void,
    size_out: *mut usize,
) -> *mut u8 {
    if !size_out.is_null() {
        unsafe { *size_out = 0 };
    }
    if display.is_null() || size_out.is_null() {
        return ptr::null_mut();
    }
    let pixels = unsafe { mycelium_display::capture_managed_rgb565(display) }
        .or_else(|| unsafe { mycelium_display::lvgl_v9::capture_lvgl_rgb565(display) });
    let Some(pixels) = pixels else {
        return ptr::null_mut();
    };
    let len = pixels.len();
    let data = unsafe { libc::malloc(len) }.cast::<u8>();
    if data.is_null() {
        return ptr::null_mut();
    }
    unsafe {
        ptr::copy_nonoverlapping(pixels.as_ptr(), data, len);
    }
    unsafe { *size_out = len };
    data
}

/// Releases a buffer returned by [`meshemu_display_capture`].
///
/// # Safety
///
/// `data` must be null or a pointer returned by `meshemu_display_capture` with
/// the exact corresponding `size`.
#[no_mangle]
pub unsafe extern "C" fn meshemu_display_capture_free(data: *mut u8, size: usize) {
    let _ = size;
    unsafe { libc::free(data.cast()) };
}

/// Destroys a Mycelium-managed compatibility display.
///
/// # Safety
///
/// `display` must be null or a live handle returned by a display creator.
#[no_mangle]
pub unsafe extern "C" fn meshemu_display_destroy(display: *mut c_void) {
    unsafe { mycelium_display::meshemu_display_destroy(display) };
}

/// Creates a radio and registers its node with the process-wide radio bus.
///
/// Returns null when the identifier or radio configuration is invalid, or when
/// another live radio already uses the same identifier.
///
/// # Safety
///
/// `instance_id` must point to a valid NUL-terminated string for this call.
#[no_mangle]
pub unsafe extern "C" fn meshemu_radio_create(
    instance_id: *const c_char,
    freq_mhz: f64,
    bandwidth_khz: u16,
    spreading_factor: u8,
    coding_rate: u8,
    tx_power_dbm: f64,
    lat: f64,
    lon: f64,
) -> *mut c_void {
    let Some(radio_state) = sx1262_state(
        freq_mhz,
        bandwidth_khz,
        spreading_factor,
        coding_rate,
        tx_power_dbm,
    ) else {
        warn!(
            freq_mhz,
            bandwidth_khz,
            spreading_factor,
            coding_rate,
            tx_power_dbm,
            "Radio create rejected: invalid SX1262 configuration"
        );
        return ptr::null_mut();
    };
    if instance_id.is_null() || !valid_position(lat, lon) {
        warn!(
            lat,
            lon, "Radio create rejected: NULL instance_id or position out of range"
        );
        return ptr::null_mut();
    }

    let node_id = unsafe { CStr::from_ptr(instance_id) }
        .to_string_lossy()
        .into_owned();
    if node_id.is_empty() {
        warn!("Radio create rejected: empty instance_id");
        return ptr::null_mut();
    }

    let mut state = lock(&BUS);
    if !state.node_ids.insert(node_id.clone()) {
        warn!(%node_id, "Radio create rejected: instance_id already registered");
        return ptr::null_mut();
    }
    state.bus.register_node(
        node_id.clone(),
        (lat, lon),
        radio_state.tx_power_dbm,
        radio_state.channel.clone(),
    );
    drop(state);

    Box::into_raw(Box::new(RadioHandle {
        node_id,
        radio: Mutex::new(radio_state),
        pending: Mutex::new(VecDeque::new()),
        last_rx: Mutex::new(None),
    })) as *mut c_void
}

/// Starts a virtual transmission, returning false while the SX1262 is busy.
///
/// # Safety
///
/// `radio` must be a live bridge handle. When `len` is nonzero, `data` must
/// reference at least `len` readable bytes.
#[no_mangle]
pub unsafe extern "C" fn meshemu_radio_start_send(
    radio: *mut c_void,
    data: *const u8,
    len: u32,
) -> bool {
    let Some(handle) = (unsafe { handle_ref(radio) }) else {
        return false;
    };
    if (data.is_null() && len != 0) || len > 255 {
        warn!(node_id = %handle.node_id, len, "Radio start_send rejected: NULL data or oversized packet");
        return false;
    }

    let bytes = if len == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(data, len as usize) }
    };
    let radio = lock(&handle.radio);
    let airtime_ms = propagation::airtime_ms(
        bytes.len(),
        radio.channel.spreading_factor,
        radio.channel.bandwidth_khz,
        radio.channel.coding_rate,
        8,
        true,
    );
    let mut state = lock(&BUS);
    let timestamp_ms = state.now_ms;
    state.bus.broadcast(TxEvent {
        node_id: handle.node_id.clone(),
        channel: radio.channel.clone(),
        data: bytes.to_vec(),
        tx_power_dbm: radio.tx_power_dbm,
        airtime_ms,
        position: (0.0, 0.0),
        timestamp_ms,
    })
}

/// Copies one queued packet into `buffer`, returning zero when none is ready.
///
/// If the buffer is too small, `truncated` is set, the packet remains queued,
/// and its required length is returned as a negative value.
///
/// # Safety
///
/// `radio` must be a live bridge handle, and `buffer` must reference at least
/// `max_len` writable bytes when `max_len` is positive. When non-null,
/// `truncated` must point to writable storage.
#[no_mangle]
pub unsafe extern "C" fn meshemu_radio_recv_raw(
    radio: *mut c_void,
    buffer: *mut u8,
    max_len: i32,
    truncated: *mut bool,
) -> i32 {
    if !truncated.is_null() {
        unsafe { *truncated = false };
    }
    let Some(handle) = (unsafe { handle_ref(radio) }) else {
        return 0;
    };
    if buffer.is_null() || max_len <= 0 {
        return 0;
    }

    let mut pending = lock(&handle.pending);
    if pending.is_empty() {
        let packets = lock(&BUS).bus.poll(&handle.node_id);
        pending.extend(packets);
    }
    let Some(packet) = pending.front() else {
        return 0;
    };
    if packet.data.len() > max_len as usize {
        if !truncated.is_null() {
            unsafe { *truncated = true };
        }
        return -(packet.data.len().min(i32::MAX as usize) as i32);
    }
    let packet = pending.pop_front().expect("front was checked above");
    let len = packet.data.len();
    unsafe {
        ptr::copy_nonoverlapping(packet.data.as_ptr(), buffer, len);
    }
    *lock(&handle.last_rx) = Some((packet.rssi_dbm as f32, packet.snr_db as f32));
    len as i32
}

/// # Safety
///
/// `radio` must be a live bridge handle.
#[no_mangle]
pub unsafe extern "C" fn meshemu_radio_get_est_airtime(radio: *mut c_void, len: i32) -> u32 {
    let Some(handle) = (unsafe { handle_ref(radio) }) else {
        return 0;
    };
    if len < 0 {
        return 0;
    }
    let radio = lock(&handle.radio);
    propagation::airtime_ms(
        len as usize,
        radio.channel.spreading_factor,
        radio.channel.bandwidth_khz,
        radio.channel.coding_rate,
        8,
        true,
    )
}

/// # Safety
///
/// `radio` must be a live bridge handle.
#[no_mangle]
pub unsafe extern "C" fn meshemu_radio_get_rssi(radio: *mut c_void) -> f32 {
    let Some(handle) = (unsafe { handle_ref(radio) }) else {
        return 0.0;
    };
    lock(&handle.last_rx).map(|(rssi, _)| rssi).unwrap_or(0.0)
}

/// # Safety
///
/// `radio` must be a live bridge handle.
#[no_mangle]
pub unsafe extern "C" fn meshemu_radio_get_snr(radio: *mut c_void) -> f32 {
    let Some(handle) = (unsafe { handle_ref(radio) }) else {
        return 0.0;
    };
    lock(&handle.last_rx).map(|(_, snr)| snr).unwrap_or(0.0)
}

/// # Safety
///
/// `radio` must be a live bridge handle.
#[no_mangle]
pub unsafe extern "C" fn meshemu_radio_is_send_complete(radio: *mut c_void) -> bool {
    let Some(handle) = (unsafe { handle_ref(radio) }) else {
        return false;
    };
    let state = lock(&BUS);
    state.bus.is_send_complete(&handle.node_id, state.now_ms)
}

/// # Safety
///
/// `radio` must be a live bridge handle.
#[no_mangle]
pub unsafe extern "C" fn meshemu_radio_set_position(radio: *mut c_void, lat: f64, lon: f64) {
    let Some(handle) = (unsafe { handle_ref(radio) }) else {
        return;
    };
    if valid_position(lat, lon) {
        lock(&BUS).bus.update_position(&handle.node_id, (lat, lon));
    }
}

/// Configures whether DIO2 drives the T-Deck's external SX1262 RF switch.
///
/// Upstream-compatible radio handles start with this disabled. Wadamesh,
/// Meshtastic, and other T-Deck firmware must set it to `true` for the normal
/// antenna path; leaving it off applies 16 dB TX and 3 dB RX loss.
///
/// # Safety
///
/// `radio` must be a live bridge handle.
#[no_mangle]
pub unsafe extern "C" fn meshemu_radio_set_dio2_config(radio: *mut c_void, as_rf_switch: bool) {
    let Some(handle) = (unsafe { handle_ref(radio) }) else {
        return;
    };
    lock(&handle.radio).dio2_rf_switch_enabled = as_rf_switch;
    lock(&BUS)
        .bus
        .set_dio2_as_rf_switch(&handle.node_id, as_rf_switch);
}

/// Returns whether DIO2 currently drives the external RF switch.
///
/// # Safety
///
/// `radio` must be a live bridge handle.
#[no_mangle]
pub unsafe extern "C" fn meshemu_radio_get_dio2_config(radio: *mut c_void) -> bool {
    let Some(handle) = (unsafe { handle_ref(radio) }) else {
        return false;
    };
    lock(&handle.radio).dio2_rf_switch_enabled
}

/// Destroys a radio handle. The caller must pass each non-null handle once.
///
/// # Safety
///
/// `radio` must be null or a live bridge handle. A non-null handle must be
/// passed exactly once and must not be used afterward.
#[no_mangle]
pub unsafe extern "C" fn meshemu_radio_destroy(radio: *mut c_void) {
    if radio.is_null() {
        return;
    }

    let handle = unsafe { Box::from_raw(radio as *mut RadioHandle) };
    let mut state = lock(&BUS);
    state.bus.unregister_node(&handle.node_id);
    state.node_ids.remove(&handle.node_id);
}

/// Advances virtual radio time and expires completed transmissions.
#[no_mangle]
pub extern "C" fn meshemu_bus_tick(now_ms: u64) {
    mycelium_input::tick_all_gt911(now_ms);
    let mut state = lock(&BUS);
    let monotonic_now_ms = state.now_ms.max(now_ms);
    state.now_ms = monotonic_now_ms;
    state.bus.tick(monotonic_now_ms);
}

#[cfg(test)]
pub(crate) fn reset_bus() {
    *lock(&BUS) = BusState::new();
}

#[cfg(test)]
pub(crate) unsafe fn radio_state(radio: *mut c_void) -> Option<Sx1262State> {
    Some(lock(&(unsafe { handle_ref(radio) })?.radio).clone())
}

#[cfg(test)]
pub(crate) fn sleep_request(instance_id: &str) -> Option<(u64, u64, u32, u64, bool)> {
    lock(&BUS).sleep_requests.get(instance_id).copied()
}

#[cfg(test)]
mod input_ffi_tests {
    use super::*;
    use mycelium_input::KEYBOARD_BRIGHTNESS_COMMAND;

    #[test]
    fn c3_backlight_survives_keyboard_handle_recreation() {
        let keyboard = meshemu_i2c_keyboard_create();
        let wire = meshemu_wire_shim_create();
        unsafe {
            meshemu_wire_shim_set_keyboard(wire, keyboard);
            assert!(meshemu_wire_begin(wire));
            meshemu_wire_begin_transmission(wire, mycelium_input::KEYBOARD_I2C_ADDRESS);
            assert_eq!(meshemu_wire_write(wire, KEYBOARD_BRIGHTNESS_COMMAND), 1);
            assert_eq!(meshemu_wire_write(wire, 128), 1);
            assert_eq!(meshemu_wire_end_transmission(wire), 0);
            meshemu_wire_shim_destroy(wire);
            meshemu_i2c_keyboard_destroy(keyboard);
        }

        let fresh = meshemu_i2c_keyboard_create();
        let fresh_keyboard = unsafe { keyboard_ref(fresh) }.unwrap();
        assert!(lock(fresh_keyboard).cross_reset_persist);
        assert_eq!(lock(fresh_keyboard).backlight(), 128);
        unsafe {
            meshemu_i2c_keyboard_set_cross_reset(fresh, false);
            meshemu_i2c_keyboard_destroy(fresh);
        }
    }

    #[test]
    fn instance_explicit_board_and_keyboard_apis_isolate_state() {
        let first_id = std::ffi::CString::new("ffi-explicit-first").unwrap();
        let second_id = std::ffi::CString::new("ffi-explicit-second").unwrap();
        let first = unsafe { meshemu_board_create_ex(first_id.as_ptr(), 3_900, 35.0, 0) };
        let second = unsafe { meshemu_board_create_ex(second_id.as_ptr(), 3_900, 35.0, 2_048) };
        assert!(!unsafe { meshemu_board_psram_found(first) });
        assert!(unsafe { meshemu_board_psram_found(second) });
        assert!(unsafe { meshemu_board_set_psram_size(first, 512) });
        assert_eq!(unsafe { meshemu_board_get_psram_free(first) }, 512);

        let first_keyboard = unsafe { meshemu_i2c_keyboard_create_for_instance(first_id.as_ptr()) };
        let first_wire = meshemu_wire_shim_create();
        unsafe {
            meshemu_wire_shim_set_keyboard(first_wire, first_keyboard);
            assert!(meshemu_wire_begin(first_wire));
            meshemu_wire_begin_transmission(first_wire, mycelium_input::KEYBOARD_I2C_ADDRESS);
            meshemu_wire_write(first_wire, KEYBOARD_BRIGHTNESS_COMMAND);
            meshemu_wire_write(first_wire, 99);
            assert_eq!(meshemu_wire_end_transmission(first_wire), 0);
            meshemu_wire_shim_destroy(first_wire);
            meshemu_i2c_keyboard_destroy(first_keyboard);
        }
        let second_keyboard =
            unsafe { meshemu_i2c_keyboard_create_for_instance(second_id.as_ptr()) };
        let second_keyboard_ref = unsafe { keyboard_ref(second_keyboard) }.unwrap();
        assert_eq!(lock(second_keyboard_ref).backlight(), 0);

        unsafe {
            meshemu_i2c_keyboard_destroy(second_keyboard);
            meshemu_board_destroy(first);
            meshemu_board_destroy(second);
        }
    }

    #[test]
    fn instance_explicit_gt911_failure_api_isolates_watchdog_status() {
        let first_id = std::ffi::CString::new("ffi-gt911-first").unwrap();
        let second_id = std::ffi::CString::new("ffi-gt911-second").unwrap();
        unsafe {
            meshemu_input_gt911_set_failure_mode_for_instance(
                first_id.as_ptr(),
                mycelium_input::GT911_FAILURE_MODE_BUS,
                100,
            );
        }
        let first = get_input_manager("ffi-gt911-first").unwrap();
        let second = register_input_manager("ffi-gt911-second", 1.0);
        let first_controller = first.lock().unwrap().gt911();
        let second_controller = second.lock().unwrap().gt911();
        lock(&first_controller).inject_touch(10, 20, true);
        lock(&second_controller).inject_touch(10, 20, true);
        let mut status = [0];
        for _ in 0..8 {
            assert_eq!(
                lock(&first_controller)
                    .i2c_read(mycelium_input::GT911_STATUS_REGISTER, &mut status),
                0
            );
            assert_ne!(
                lock(&second_controller)
                    .i2c_read(mycelium_input::GT911_STATUS_REGISTER, &mut status),
                0
            );
        }
        assert_eq!(
            unsafe { meshemu_input_gt911_get_status_for_instance(first_id.as_ptr()) },
            mycelium_input::GT911_STATUS_BUS_WATCHDOG_FIRED
        );
        assert_eq!(
            unsafe { meshemu_input_gt911_get_status_for_instance(second_id.as_ptr()) },
            0
        );
        mycelium_input::remove_input_manager("ffi-gt911-first");
        mycelium_input::remove_input_manager("ffi-gt911-second");
    }
}
