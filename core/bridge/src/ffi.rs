use std::collections::{HashSet, VecDeque};
use std::ffi::{c_char, c_void, CStr};
use std::ptr;
use std::sync::{LazyLock, Mutex, MutexGuard};

use radio_bus::{propagation, RadioBus, RadioChannel, RxPacket, TxEvent};

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

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

unsafe fn handle_ref<'a>(radio: *mut c_void) -> Option<&'a RadioHandle> {
    (radio as *const RadioHandle).as_ref()
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
