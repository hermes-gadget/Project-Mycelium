use std::sync::{Arc, LazyLock, Mutex, MutexGuard, Weak};

pub const GT911_I2C_ADDRESS: u8 = 0x5d;
pub const GT911_CONFIG_X_REGISTER: u16 = 0x8048;
pub const GT911_CONFIG_Y_REGISTER: u16 = 0x804a;
pub const GT911_PRODUCT_ID_REGISTER: u16 = 0x8140;
pub const GT911_STATUS_REGISTER: u16 = 0x814e;
pub const GT911_INT_GPIO: u8 = 16;
pub const GT911_MAX_TOUCHES: usize = 5;
pub const DEFAULT_GT911_CONTACT_SIZE: u16 = 50;
pub const DEFAULT_GT911_MAX_X: u16 = 320;
pub const DEFAULT_GT911_MAX_Y: u16 = 240;

pub const GT911_FAILURE_MODE_BUS: u8 = 1;
pub const GT911_FAILURE_MODE_FRAME_STALL: u8 = 2;
pub const GT911_FAILURE_MODE_PHANTOM_LATCH: u8 = 3;

pub const GT911_STATUS_BUS_WATCHDOG_FIRED: u64 = 1 << 0;
pub const GT911_STATUS_FRAME_WATCHDOG_FIRED: u64 = 1 << 1;
pub const GT911_STATUS_PHANTOM_WATCHDOG_FIRED: u64 = 1 << 2;

const GT911_PRODUCT_ID: [u8; 4] = *b"911\0";
const GT911_SCAN_INTERVAL_MS: u64 = 10;
const GT911_BUS_WATCHDOG_FAILURES: u8 = 8;
const GT911_FRAME_WATCHDOG_MS: u64 = 250;
const GT911_PHANTOM_WATCHDOG_MS: u64 = 10_000;

pub type SharedGt911 = Arc<Mutex<Gt911Controller>>;

#[derive(Clone, Copy, Debug, Default)]
struct FailureConfiguration {
    i2c_failure_rate: u8,
    frame_stall_ms: u32,
    phantom_latch: bool,
}

#[derive(Default)]
struct ControllerRegistry {
    configuration: FailureConfiguration,
    controllers: Vec<Weak<Mutex<Gt911Controller>>>,
}

static CONTROLLERS: LazyLock<Mutex<ControllerRegistry>> =
    LazyLock::new(|| Mutex::new(ControllerRegistry::default()));

/// One contact reported by the GT911 controller.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Gt911Point {
    pub track_id: u8,
    pub x: u16,
    pub y: u16,
    pub size: u16,
}

/// Register-level model of the T-Deck GT911 touch controller.
pub struct Gt911Controller {
    touch_points: [Option<Gt911Point>; GT911_MAX_TOUCHES],
    contact_size: u16,
    max_x: u16,
    max_y: u16,
    int_asserted: bool,
    frame_ready: bool,
    now_ms: u64,
    last_frame_ms: Option<u64>,
    last_raw_position: Option<(u16, u16)>,
    last_mapped_position: Option<(u16, u16)>,
    physical_primary: Option<((u16, u16), Gt911Point)>,
    i2c_failure_rate: u8,
    failure_accumulator: u16,
    consecutive_status_failures: u8,
    frame_stall_ms: u32,
    stall_started_ms: Option<u64>,
    stall_until_ms: Option<u64>,
    phantom_latch: bool,
    phantom_point: Option<Gt911Point>,
    phantom_started_ms: Option<u64>,
    watchdog_status: u64,
}

impl Gt911Controller {
    pub fn new() -> Self {
        Self {
            touch_points: [None; GT911_MAX_TOUCHES],
            contact_size: DEFAULT_GT911_CONTACT_SIZE,
            max_x: DEFAULT_GT911_MAX_X,
            max_y: DEFAULT_GT911_MAX_Y,
            int_asserted: false,
            frame_ready: false,
            now_ms: 0,
            last_frame_ms: None,
            last_raw_position: None,
            last_mapped_position: None,
            physical_primary: None,
            i2c_failure_rate: 0,
            failure_accumulator: 0,
            consecutive_status_failures: 0,
            frame_stall_ms: 0,
            stall_started_ms: None,
            stall_until_ms: None,
            phantom_latch: false,
            phantom_point: None,
            phantom_started_ms: None,
            watchdog_status: 0,
        }
    }

    /// Configure the GT911 contact size/weight used for primary touch injection.
    pub fn set_contact_size(&mut self, contact_size: u16) {
        self.contact_size = contact_size;
    }

    pub fn contact_size(&self) -> u16 {
        self.contact_size
    }

    pub fn set_resolution(&mut self, max_x: u16, max_y: u16) {
        self.max_x = max_x;
        self.max_y = max_y;
    }

    pub fn resolution(&self) -> (u16, u16) {
        (self.max_x, self.max_y)
    }

    /// Serialise the calibration state for NVS persistence.
    ///
    /// Returns `(max_x, max_y, contact_size)` — everything needed to restore
    /// the touch controller after a virtual restart.
    pub fn calibration(&self) -> (u16, u16, u16) {
        (self.max_x, self.max_y, self.contact_size)
    }

    /// Restore calibration from previously-saved NVS data.
    pub fn apply_calibration(&mut self, max_x: u16, max_y: u16, contact_size: u16) {
        self.max_x = max_x;
        self.max_y = max_y;
        self.contact_size = contact_size;
    }

    /// Make an exact percentage of status reads fail using a deterministic
    /// accumulator. Reconfiguring the mode clears its sticky fired bit.
    pub fn set_i2c_failure_rate(&mut self, pct: u8) {
        self.i2c_failure_rate = pct.min(100);
        self.failure_accumulator = 0;
        self.consecutive_status_failures = 0;
        self.watchdog_status &= !GT911_STATUS_BUS_WATCHDOG_FIRED;
    }

    /// Suppress periodic held-touch frames for `ms` after a press.
    ///
    /// The initial press frame is still produced so firmware can enter its
    /// pressed state before observing the stall. Reconfiguring clears the
    /// corresponding sticky fired bit.
    pub fn set_frame_stall_ms(&mut self, ms: u32) {
        self.frame_stall_ms = ms;
        self.watchdog_status &= !GT911_STATUS_FRAME_WATCHDOG_FIRED;
        self.arm_frame_stall();
    }

    /// Freeze the first pressed point and continue reporting it until disabled.
    ///
    /// Movements and releases are ignored while latched, matching a controller
    /// wedged on one stale frame. Reconfiguring clears the sticky fired bit.
    pub fn set_phantom_latch(&mut self, enabled: bool) {
        let was_enabled = self.phantom_latch;
        self.phantom_latch = enabled;
        self.watchdog_status &= !GT911_STATUS_PHANTOM_WATCHDOG_FIRED;
        if enabled {
            self.phantom_point = self.primary_point();
            self.phantom_started_ms = self.phantom_point.map(|_| self.now_ms);
        } else {
            self.phantom_point = None;
            self.phantom_started_ms = None;
            if was_enabled {
                match self.physical_primary {
                    Some((raw, point)) => {
                        self.touch_points[0] = Some(point);
                        self.last_raw_position = Some(raw);
                        self.last_mapped_position = Some((point.x, point.y));
                    }
                    None => self.touch_points[0] = None,
                }
                self.mark_frame_ready();
            }
        }
    }

    pub fn watchdog_status(&self) -> u64 {
        self.watchdog_status
    }

    /// Inject a primary contact from the existing landscape host-coordinate API.
    ///
    /// The mapped coordinate retains the historical portrait swap/reversal used
    /// by `meshemu_input_poll_touch`; `last_raw_position` preserves the value
    /// before that mapping for calibration diagnostics.
    pub fn inject_touch(&mut self, mouse_x: i32, mouse_y: i32, pressed: bool) {
        let raw = (
            mouse_x.clamp(0, i32::from(u16::MAX)) as u16,
            mouse_y.clamp(0, i32::from(u16::MAX)) as u16,
        );
        let point = Gt911Point {
            track_id: 0,
            x: mouse_y.clamp(0, 239) as u16,
            y: (319 - mouse_x).clamp(0, 319) as u16,
            size: self.contact_size,
        };
        let was_pressed = self.is_pressed();
        self.physical_primary = pressed.then_some((raw, point));

        if pressed {
            if self.phantom_latch {
                let latched = *self.phantom_point.get_or_insert(point);
                self.touch_points[0] = Some(latched);
                if self.phantom_started_ms.is_none() {
                    self.phantom_started_ms = Some(self.now_ms);
                    self.last_raw_position = Some(raw);
                    self.last_mapped_position = Some((latched.x, latched.y));
                }
            } else {
                self.touch_points[0] = Some(point);
                self.last_raw_position = Some(raw);
                self.last_mapped_position = Some((point.x, point.y));
            }

            if !was_pressed {
                self.arm_frame_stall();
                self.mark_frame_ready();
            } else if !self.frame_is_stalled() {
                self.mark_frame_ready();
            }
        } else if self.phantom_latch && self.phantom_point.is_some() {
            self.touch_points[0] = self.phantom_point;
            if !self.frame_is_stalled() {
                self.mark_frame_ready();
            }
        } else {
            self.touch_points[0] = None;
            self.stall_started_ms = None;
            self.stall_until_ms = None;
            self.phantom_point = None;
            self.phantom_started_ms = None;
            // A zero-contact frame is the real GT911 lift notification.
            self.mark_frame_ready();
        }
    }

    /// Inject or release one of the five hardware contact slots.
    pub fn inject_point(&mut self, point: Gt911Point, pressed: bool) -> bool {
        let slot = usize::from(point.track_id);
        if slot >= GT911_MAX_TOUCHES {
            return false;
        }
        let was_pressed = self.is_pressed();
        self.touch_points[slot] = pressed.then_some(point);
        self.last_raw_position = Some((point.x, point.y));
        self.last_mapped_position = Some((point.x, point.y));
        if pressed && !was_pressed {
            self.arm_frame_stall();
        }
        self.mark_frame_ready();
        true
    }

    pub fn raw_position(&self) -> Option<(u16, u16)> {
        self.last_raw_position
    }

    pub fn mapped_position(&self) -> Option<(u16, u16)> {
        self.last_mapped_position
    }

    /// Advance the controller's deterministic clock and generate scan frames.
    pub fn tick(&mut self, now_ms: u64) {
        self.now_ms = now_ms;

        if !self.is_pressed() {
            return;
        }

        if let (Some(started), Some(until)) = (self.stall_started_ms, self.stall_until_ms) {
            let watchdog_at = started.saturating_add(GT911_FRAME_WATCHDOG_MS);
            if until > watchdog_at && now_ms > watchdog_at && !self.frame_ready {
                self.watchdog_status |= GT911_STATUS_FRAME_WATCHDOG_FIRED;
            }
        }

        if self.phantom_latch
            && self
                .phantom_started_ms
                .is_some_and(|started| now_ms.saturating_sub(started) > GT911_PHANTOM_WATCHDOG_MS)
        {
            self.watchdog_status |= GT911_STATUS_PHANTOM_WATCHDOG_FIRED;
        }

        if self.frame_ready || self.frame_is_stalled() {
            return;
        }

        if self
            .last_frame_ms
            .is_none_or(|last_frame| now_ms.saturating_sub(last_frame) >= GT911_SCAN_INTERVAL_MS)
        {
            self.mark_frame_ready();
        }
    }

    /// Read GPIO16. The GT911 interrupt output is active LOW.
    pub fn gpio16_level(&self) -> bool {
        !self.int_asserted
    }

    pub fn frame_ready(&self) -> bool {
        self.frame_ready
    }

    pub fn touch_points(&self) -> &[Option<Gt911Point>; GT911_MAX_TOUCHES] {
        &self.touch_points
    }

    /// Read a GT911 register window. Zero means the I2C read failed.
    pub fn i2c_read(&mut self, register: u16, buf: &mut [u8]) -> usize {
        if register == GT911_STATUS_REGISTER && self.fail_status_read() {
            return 0;
        }

        for (offset, output) in buf.iter_mut().enumerate() {
            let Some(address) = register.checked_add(offset as u16) else {
                return offset;
            };
            *output = self.register_byte(address);
        }
        buf.len()
    }

    /// Handle controller writes. Writing zero to 0x814E acknowledges the frame.
    pub fn i2c_write(&mut self, register: u16, data: &[u8]) {
        if register == GT911_STATUS_REGISTER && data.first() == Some(&0) {
            self.frame_ready = false;
            self.int_asserted = false;
        }
    }

    fn apply_failure_mode(&mut self, mode: u8, value: u32) {
        match mode {
            GT911_FAILURE_MODE_BUS => self.set_i2c_failure_rate(value.min(100) as u8),
            GT911_FAILURE_MODE_FRAME_STALL => self.set_frame_stall_ms(value),
            GT911_FAILURE_MODE_PHANTOM_LATCH => self.set_phantom_latch(value != 0),
            _ => {}
        }
    }

    fn primary_point(&self) -> Option<Gt911Point> {
        self.touch_points.iter().flatten().next().copied()
    }

    fn is_pressed(&self) -> bool {
        self.touch_points.iter().any(Option::is_some)
    }

    fn arm_frame_stall(&mut self) {
        if self.frame_stall_ms == 0 || !self.is_pressed() {
            self.stall_started_ms = None;
            self.stall_until_ms = None;
            return;
        }
        self.stall_started_ms = Some(self.now_ms);
        self.stall_until_ms = Some(self.now_ms.saturating_add(u64::from(self.frame_stall_ms)));
    }

    fn frame_is_stalled(&self) -> bool {
        self.stall_until_ms.is_some_and(|until| self.now_ms < until)
    }

    fn mark_frame_ready(&mut self) {
        self.frame_ready = true;
        self.int_asserted = true;
        self.last_frame_ms = Some(self.now_ms);
    }

    fn fail_status_read(&mut self) -> bool {
        self.failure_accumulator = self
            .failure_accumulator
            .saturating_add(u16::from(self.i2c_failure_rate));
        if self.failure_accumulator < 100 {
            self.consecutive_status_failures = 0;
            return false;
        }

        self.failure_accumulator -= 100;
        if self.is_pressed() {
            self.consecutive_status_failures = self.consecutive_status_failures.saturating_add(1);
            if self.consecutive_status_failures >= GT911_BUS_WATCHDOG_FAILURES {
                self.watchdog_status |= GT911_STATUS_BUS_WATCHDOG_FIRED;
            }
        } else {
            self.consecutive_status_failures = 0;
        }
        true
    }

    fn register_byte(&self, address: u16) -> u8 {
        if let Some(offset) = address.checked_sub(GT911_PRODUCT_ID_REGISTER) {
            if let Some(byte) = GT911_PRODUCT_ID.get(usize::from(offset)) {
                return *byte;
            }
        }
        if (GT911_CONFIG_X_REGISTER..GT911_CONFIG_X_REGISTER + 2).contains(&address) {
            return self.max_x.to_le_bytes()[usize::from(address - GT911_CONFIG_X_REGISTER)];
        }
        if (GT911_CONFIG_Y_REGISTER..GT911_CONFIG_Y_REGISTER + 2).contains(&address) {
            return self.max_y.to_le_bytes()[usize::from(address - GT911_CONFIG_Y_REGISTER)];
        }
        if address == GT911_STATUS_REGISTER {
            if !self.frame_ready {
                return 0;
            }
            let count = self.touch_points.iter().flatten().count() as u8;
            return 0x80 | count;
        }
        if address < GT911_STATUS_REGISTER + 1 {
            return 0;
        }

        let offset = usize::from(address - (GT911_STATUS_REGISTER + 1));
        let point_index = offset / 8;
        let byte_index = offset % 8;
        let Some(point) = self.touch_points.iter().flatten().nth(point_index).copied() else {
            return 0;
        };
        match byte_index {
            0 => point.track_id,
            1 => point.x as u8,
            2 => (point.x >> 8) as u8,
            3 => point.y as u8,
            4 => (point.y >> 8) as u8,
            5 => point.size as u8,
            6 => (point.size >> 8) as u8,
            _ => 0,
        }
    }
}

impl Default for Gt911Controller {
    fn default() -> Self {
        Self::new()
    }
}

/// Construct and register a shared controller for global FFI failure controls.
pub fn new_shared_gt911() -> SharedGt911 {
    let mut registry = lock(&CONTROLLERS);
    let configuration = registry.configuration;
    let mut controller = Gt911Controller::new();
    controller.set_i2c_failure_rate(configuration.i2c_failure_rate);
    controller.set_frame_stall_ms(configuration.frame_stall_ms);
    controller.set_phantom_latch(configuration.phantom_latch);
    let controller = Arc::new(Mutex::new(controller));
    registry.controllers.push(Arc::downgrade(&controller));
    controller
}

/// Apply one failure mode to all live controllers and to future controllers.
pub fn set_global_failure_mode(mode: u8, value: u32) {
    let controllers = {
        let mut registry = lock(&CONTROLLERS);
        match mode {
            GT911_FAILURE_MODE_BUS => {
                registry.configuration.i2c_failure_rate = value.min(100) as u8;
            }
            GT911_FAILURE_MODE_FRAME_STALL => registry.configuration.frame_stall_ms = value,
            GT911_FAILURE_MODE_PHANTOM_LATCH => {
                registry.configuration.phantom_latch = value != 0;
            }
            _ => return,
        }
        live_controllers(&mut registry)
    };
    for controller in controllers {
        lock(&controller).apply_failure_mode(mode, value);
    }
}

/// Return the union of sticky watchdog-fired bits across live controllers.
pub fn global_watchdog_status() -> u64 {
    let controllers = {
        let mut registry = lock(&CONTROLLERS);
        live_controllers(&mut registry)
    };
    controllers.iter().fold(0, |status, controller| {
        status | lock(controller).watchdog_status()
    })
}

/// Advance all shared GT911 controllers using the emulator's virtual time.
pub fn tick_all_gt911(now_ms: u64) {
    let controllers = {
        let mut registry = lock(&CONTROLLERS);
        live_controllers(&mut registry)
    };
    for controller in controllers {
        lock(&controller).tick(now_ms);
    }
}

fn live_controllers(registry: &mut ControllerRegistry) -> Vec<SharedGt911> {
    let mut controllers = Vec::with_capacity(registry.controllers.len());
    registry.controllers.retain(|controller| {
        if let Some(controller) = controller.upgrade() {
            controllers.push(controller);
            true
        } else {
            false
        }
    });
    controllers
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn touch_is_encoded_in_gt911_register_format_with_portrait_swap() {
        let mut controller = Gt911Controller::new();
        controller.set_contact_size(321);
        controller.inject_touch(100, 40, true);
        let mut registers = [0; 9];

        assert_eq!(
            controller.i2c_read(GT911_STATUS_REGISTER, &mut registers),
            9
        );
        assert_eq!(registers[0], 0x81);
        assert_eq!(registers[1], 0);
        assert_eq!(u16::from_le_bytes([registers[2], registers[3]]), 40);
        assert_eq!(u16::from_le_bytes([registers[4], registers[5]]), 219);
        assert_eq!(u16::from_le_bytes([registers[6], registers[7]]), 321);
        assert_eq!(registers[8], 0);
        assert_eq!(controller.raw_position(), Some((100, 40)));
        assert_eq!(controller.mapped_position(), Some((40, 219)));
    }

    #[test]
    fn product_id_and_configured_resolution_are_readable() {
        let mut controller = Gt911Controller::new();
        let mut product_id = [0; 4];
        let mut resolution = [0; 4];

        assert_eq!(
            controller.i2c_read(GT911_PRODUCT_ID_REGISTER, &mut product_id),
            4
        );
        assert_eq!(&product_id, b"911\0");
        assert_eq!(
            controller.i2c_read(GT911_CONFIG_X_REGISTER, &mut resolution),
            4
        );
        assert_eq!(
            u16::from_le_bytes([resolution[0], resolution[1]]),
            DEFAULT_GT911_MAX_X
        );
        assert_eq!(
            u16::from_le_bytes([resolution[2], resolution[3]]),
            DEFAULT_GT911_MAX_Y
        );

        controller.set_resolution(240, 320);
        controller.i2c_read(GT911_CONFIG_X_REGISTER, &mut resolution);
        assert_eq!(resolution, [240, 0, 64, 1]);
    }

    #[test]
    fn primary_contact_size_is_configurable() {
        let mut controller = Gt911Controller::new();
        assert_eq!(controller.contact_size(), DEFAULT_GT911_CONTACT_SIZE);

        controller.set_contact_size(u16::MAX);
        controller.inject_touch(10, 20, true);

        assert_eq!(controller.touch_points()[0].unwrap().size, u16::MAX);
    }

    #[test]
    fn interrupt_is_active_low_until_status_is_cleared() {
        let mut controller = Gt911Controller::new();
        assert!(controller.gpio16_level());

        controller.inject_touch(10, 20, true);
        assert!(!controller.gpio16_level());
        controller.i2c_write(GT911_STATUS_REGISTER, &[0]);

        assert!(controller.gpio16_level());
        assert!(!controller.frame_ready());
        let mut status = [0xff];
        controller.i2c_read(GT911_STATUS_REGISTER, &mut status);
        assert_eq!(status[0], 0);
    }

    #[test]
    fn release_and_periodic_held_touch_frames_are_generated() {
        let mut controller = Gt911Controller::new();
        controller.inject_touch(10, 20, true);
        controller.i2c_write(GT911_STATUS_REGISTER, &[0]);
        controller.tick(9);
        assert!(!controller.frame_ready());
        controller.tick(10);
        assert!(controller.frame_ready());

        controller.i2c_write(GT911_STATUS_REGISTER, &[0]);
        controller.inject_touch(10, 20, false);
        let mut status = [0];
        controller.i2c_read(GT911_STATUS_REGISTER, &mut status);
        assert_eq!(status[0], 0x80);
    }

    #[test]
    fn reports_up_to_five_contacts() {
        let mut controller = Gt911Controller::new();
        for track_id in 0..5 {
            assert!(controller.inject_point(
                Gt911Point {
                    track_id,
                    x: 10 + u16::from(track_id),
                    y: 20,
                    size: 30,
                },
                true,
            ));
        }
        assert!(!controller.inject_point(
            Gt911Point {
                track_id: 5,
                x: 0,
                y: 0,
                size: 0,
            },
            true,
        ));

        let mut status = [0];
        controller.i2c_read(GT911_STATUS_REGISTER, &mut status);
        assert_eq!(status[0], 0x85);
    }

    #[test]
    fn bus_failure_mode_sets_the_watchdog_bit_after_eight_failed_status_reads() {
        let mut controller = Gt911Controller::new();
        controller.inject_touch(10, 20, true);
        controller.set_i2c_failure_rate(100);
        let mut status = [0];

        for _ in 0..7 {
            assert_eq!(controller.i2c_read(GT911_STATUS_REGISTER, &mut status), 0);
        }
        assert_eq!(controller.watchdog_status(), 0);
        assert_eq!(controller.i2c_read(GT911_STATUS_REGISTER, &mut status), 0);
        assert_eq!(
            controller.watchdog_status(),
            GT911_STATUS_BUS_WATCHDOG_FIRED
        );
    }

    #[test]
    fn frame_stall_suppresses_scans_and_sets_the_timeout_bit() {
        let mut controller = Gt911Controller::new();
        controller.set_frame_stall_ms(500);
        controller.inject_touch(10, 20, true);
        controller.i2c_write(GT911_STATUS_REGISTER, &[0]);

        controller.tick(250);
        assert!(!controller.frame_ready());
        assert_eq!(controller.watchdog_status(), 0);
        controller.tick(251);
        assert_eq!(
            controller.watchdog_status(),
            GT911_STATUS_FRAME_WATCHDOG_FIRED
        );
        controller.tick(500);
        assert!(controller.frame_ready());
    }

    #[test]
    fn phantom_latch_freezes_the_point_and_sets_the_ten_second_bit() {
        let mut controller = Gt911Controller::new();
        controller.set_phantom_latch(true);
        controller.inject_touch(10, 20, true);
        let frozen = controller.touch_points()[0];

        controller.inject_touch(200, 100, true);
        controller.inject_touch(200, 100, false);
        assert_eq!(controller.touch_points()[0], frozen);
        controller.tick(10_000);
        assert_eq!(controller.watchdog_status(), 0);
        controller.tick(10_001);
        assert_eq!(
            controller.watchdog_status(),
            GT911_STATUS_PHANTOM_WATCHDOG_FIRED
        );

        controller.set_phantom_latch(false);
        assert!(controller.touch_points()[0].is_none());
        let mut status = [0];
        controller.i2c_read(GT911_STATUS_REGISTER, &mut status);
        assert_eq!(status[0], 0x80);
        assert_eq!(controller.watchdog_status(), 0);
    }
}
