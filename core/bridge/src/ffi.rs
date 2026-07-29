use std::collections::{HashMap, HashSet, VecDeque};
use std::ffi::{c_char, c_void, CStr};
use std::ptr;
use std::sync::{LazyLock, Mutex, MutexGuard};

use mycelium_board::{BoardConfig, VirtualBoard};
use mycelium_gps::GpsManager;
use mycelium_input::i2c_keyboard::I2cKeyboardBus;
use mycelium_input::wire_shim::{SharedI2cKeyboard, WireShim};
use mycelium_input::{get_input_manager, register_input_manager, SharedInputManager};
use mycelium_storage::StorageManager;
use radio_bus::{propagation, RadioBus, RadioChannel, RxPacket, TxEvent};
use sdl2::keyboard::Keycode;

struct BusState {
    bus: RadioBus,
    node_ids: HashSet<String>,
    now_ms: u64,
}

impl BusState {
    fn new() -> Self {
        Self {
            bus: RadioBus::new(),
            node_ids: HashSet::new(),
            now_ms: 0,
        }
    }
}

struct RadioHandle {
    node_id: String,
    channel: RadioChannel,
    tx_power_dbm: f64,
    pending: Mutex<VecDeque<RxPacket>>,
    last_rx: Mutex<Option<(f32, f32)>>,
}

static BUS: LazyLock<Mutex<BusState>> = LazyLock::new(|| Mutex::new(BusState::new()));
static STORAGE: LazyLock<Mutex<HashMap<String, StorageManager>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

unsafe fn handle_ref<'a>(radio: *mut c_void) -> Option<&'a RadioHandle> {
    (radio as *const RadioHandle).as_ref()
}

unsafe fn keyboard_ref<'a>(keyboard: *mut c_void) -> Option<&'a SharedI2cKeyboard> {
    unsafe { (keyboard as *const SharedI2cKeyboard).as_ref() }
}

unsafe fn wire_mut<'a>(wire: *mut c_void) -> Option<&'a mut WireShim> {
    unsafe { (wire as *mut WireShim).as_mut() }
}

unsafe fn ffi_string<'a>(value: *const c_char) -> Option<&'a str> {
    if value.is_null() {
        return None;
    }
    let value = unsafe { CStr::from_ptr(value) }.to_str().ok()?;
    (!value.is_empty()).then_some(value)
}

unsafe fn storage_file_args<'a>(
    instance_id: *const c_char,
    path: *const c_char,
) -> Option<(&'a str, &'a str)> {
    Some((unsafe { ffi_string(instance_id) }?, unsafe {
        ffi_string(path)
    }?))
}

fn copy_for_caller(data: &[u8], out_len: *mut usize) -> *mut u8 {
    if out_len.is_null() {
        return ptr::null_mut();
    }
    unsafe { *out_len = 0 };
    let allocation_len = data.len().max(1);
    let output = unsafe { libc::malloc(allocation_len) }.cast::<u8>();
    if output.is_null() {
        return ptr::null_mut();
    }
    if !data.is_empty() {
        unsafe { ptr::copy_nonoverlapping(data.as_ptr(), output, data.len()) };
    }
    unsafe { *out_len = data.len() };
    output
}

unsafe fn gps_mut<'a>(gps: *mut c_void) -> Option<&'a mut GpsManager> {
    unsafe { (gps as *mut GpsManager).as_mut() }
}

unsafe fn board_ref<'a>(board: *mut c_void) -> Option<&'a VirtualBoard> {
    unsafe { (board as *const VirtualBoard).as_ref() }
}

unsafe fn board_mut<'a>(board: *mut c_void) -> Option<&'a mut VirtualBoard> {
    unsafe { (board as *mut VirtualBoard).as_mut() }
}

unsafe fn instance_id(instance_id: *const c_char) -> Option<String> {
    if instance_id.is_null() {
        return None;
    }
    let instance_id = unsafe { CStr::from_ptr(instance_id) }.to_str().ok()?;
    (!instance_id.is_empty()).then(|| instance_id.to_owned())
}

fn valid_position(lat: f64, lon: f64) -> bool {
    lat.is_finite()
        && lon.is_finite()
        && (-90.0..=90.0).contains(&lat)
        && (-180.0..=180.0).contains(&lon)
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
    storage
        .entry(instance_id.to_owned())
        .or_insert_with(|| StorageManager::new(instance_id))
        .spiffs
        .mount()
        .is_ok()
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
        return false;
    }
    let bytes = if len == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(data, len) }
    };
    lock(&STORAGE)
        .get(instance_id)
        .is_some_and(|manager| manager.spiffs.write_file(path, bytes).is_ok())
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
    let mut storage = lock(&STORAGE);
    storage
        .entry(instance_id.to_owned())
        .or_insert_with(|| StorageManager::new(instance_id))
        .sdcard
        .mount()
        .is_ok()
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
    let storage = lock(&STORAGE);
    let Some(data) = storage
        .get(instance_id)
        .and_then(|manager| manager.sdcard.read_file(path).ok())
    else {
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
    if data.is_null() && len != 0 {
        return false;
    }
    let bytes = if len == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(data, len) }
    };
    lock(&STORAGE)
        .get(instance_id)
        .is_some_and(|manager| manager.sdcard.write_file(path, bytes).is_ok())
}

/// Releases a buffer returned by a storage read function.
///
/// # Safety
///
/// `data` must be null or a pointer returned by a storage read function.
#[no_mangle]
pub unsafe extern "C" fn meshemu_storage_data_free(data: *mut u8) {
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
    if (unsafe { self::instance_id(instance_id) }).is_none() || !valid_position(lat, lon) {
        return ptr::null_mut();
    }
    Box::into_raw(Box::new(GpsManager::new(lat, lon))).cast()
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
        gps.state_mut().latitude = lat;
        gps.state_mut().longitude = lon;
        gps.state_mut().altitude_m = alt;
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
    if buf.is_null() || max_len <= 0 {
        return 0;
    }
    let output = unsafe { std::slice::from_raw_parts_mut(buf, max_len as usize) };
    gps.read(output).min(i32::MAX as usize) as i32
}

/// Enables or disables NMEA output.
///
/// # Safety
///
/// `gps` must be a live GPS handle returned by [`meshemu_gps_create`].
#[no_mangle]
pub unsafe extern "C" fn meshemu_gps_set_enabled(gps: *mut c_void, enabled: bool) {
    if let Some(gps) = unsafe { gps_mut(gps) } {
        gps.state_mut().enabled = enabled;
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
        unsafe { drop(Box::from_raw(gps.cast::<GpsManager>())) };
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
    if !temp.is_finite() {
        return ptr::null_mut();
    }
    let config = BoardConfig {
        battery_mv: mv,
        mcu_temperature: temp,
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

/// # Safety
///
/// `board` must be a live board handle returned by [`meshemu_board_create`].
#[no_mangle]
pub unsafe extern "C" fn meshemu_board_get_temp(board: *mut c_void) -> f32 {
    unsafe { board_ref(board) }
        .map(VirtualBoard::get_temperature)
        .unwrap_or(0.0)
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

unsafe fn input_manager(instance_id: *const c_char, create: bool) -> Option<SharedInputManager> {
    if instance_id.is_null() {
        return None;
    }
    let instance_id = unsafe { CStr::from_ptr(instance_id) }.to_str().ok()?;
    if instance_id.is_empty() {
        return None;
    }
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

/// Creates an LVGL v9 SDL display for a firmware instance.
///
/// Returns null for invalid arguments or when the loaded firmware does not
/// export the LVGL v9 SDL driver API.
///
/// # Safety
///
/// `instance_id` must point to a valid NUL-terminated string for this call.
#[no_mangle]
pub unsafe extern "C" fn meshemu_display_create(
    instance_id: *const c_char,
    width: i32,
    height: i32,
) -> *mut c_void {
    if instance_id.is_null() || width <= 0 || height <= 0 {
        return ptr::null_mut();
    }
    let instance_id = unsafe { CStr::from_ptr(instance_id) }.to_string_lossy();
    mycelium_display::lvgl_v9::lvgl_v9_init_sdl(&instance_id, width, height)
}

/// Captures an LVGL SDL display as a newly allocated packed RGB565 buffer.
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
    if instance_id.is_null()
        || !freq_mhz.is_finite()
        || freq_mhz <= 0.0
        || bandwidth_khz == 0
        || !tx_power_dbm.is_finite()
        || !lat.is_finite()
        || !lon.is_finite()
    {
        return ptr::null_mut();
    }

    let node_id = unsafe { CStr::from_ptr(instance_id) }
        .to_string_lossy()
        .into_owned();
    if node_id.is_empty() {
        return ptr::null_mut();
    }

    let channel = RadioChannel {
        freq_mhz,
        bandwidth_khz,
        spreading_factor,
        coding_rate,
    };
    let mut state = lock(&BUS);
    if !state.node_ids.insert(node_id.clone()) {
        return ptr::null_mut();
    }
    state
        .bus
        .register_node(node_id.clone(), (lat, lon), tx_power_dbm, channel.clone());
    drop(state);

    Box::into_raw(Box::new(RadioHandle {
        node_id,
        channel,
        tx_power_dbm,
        pending: Mutex::new(VecDeque::new()),
        last_rx: Mutex::new(None),
    })) as *mut c_void
}

/// Starts an instantaneous virtual transmission.
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
    if data.is_null() && len != 0 {
        return false;
    }

    let bytes = if len == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(data, len as usize) }
    };
    let airtime_ms = propagation::airtime_ms(
        bytes.len(),
        handle.channel.spreading_factor,
        handle.channel.bandwidth_khz,
        handle.channel.coding_rate,
        8,
        true,
    );
    let mut state = lock(&BUS);
    let timestamp_ms = state.now_ms;
    state.bus.broadcast(TxEvent {
        node_id: handle.node_id.clone(),
        channel: handle.channel.clone(),
        data: bytes.to_vec(),
        tx_power_dbm: handle.tx_power_dbm,
        airtime_ms,
        position: (0.0, 0.0),
        timestamp_ms,
    });
    true
}

/// Copies one queued packet into `buffer`, returning zero when none is ready.
///
/// # Safety
///
/// `radio` must be a live bridge handle, and `buffer` must reference at least
/// `max_len` writable bytes when `max_len` is positive.
#[no_mangle]
pub unsafe extern "C" fn meshemu_radio_recv_raw(
    radio: *mut c_void,
    buffer: *mut u8,
    max_len: i32,
) -> i32 {
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
    let Some(packet) = pending.pop_front() else {
        return 0;
    };

    let len = packet.data.len().min(max_len as usize);
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
    propagation::airtime_ms(
        len as usize,
        handle.channel.spreading_factor,
        handle.channel.bandwidth_khz,
        handle.channel.coding_rate,
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

#[no_mangle]
pub extern "C" fn meshemu_radio_is_send_complete(radio: *mut c_void) -> bool {
    !radio.is_null()
}

/// # Safety
///
/// `radio` must be a live bridge handle.
#[no_mangle]
pub unsafe extern "C" fn meshemu_radio_set_position(radio: *mut c_void, lat: f64, lon: f64) {
    let Some(handle) = (unsafe { handle_ref(radio) }) else {
        return;
    };
    if lat.is_finite() && lon.is_finite() {
        lock(&BUS).bus.update_position(&handle.node_id, (lat, lon));
    }
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
    let mut state = lock(&BUS);
    state.now_ms = now_ms;
    state.bus.tick(now_ms);
}

#[cfg(test)]
pub(crate) fn reset_bus() {
    *lock(&BUS) = BusState::new();
}
