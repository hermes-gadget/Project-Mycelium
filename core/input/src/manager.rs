use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Instant;

use sdl2::event::Event;
use sdl2::keyboard::Keycode;

use crate::{
    i2c_keyboard::I2cKeyboardBus,
    wire_shim::{SharedGt911, SharedI2cKeyboard, WireShim},
};
use crate::{
    Gt911TouchEvent, KeyEvent, KeyboardEmulator, TouchEmulator, TrackballEmulator, TrackballEvent,
};

static START_TIME: LazyLock<Instant> = LazyLock::new(Instant::now);
static INPUT_MANAGERS: LazyLock<Mutex<HashMap<String, SharedInputManager>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Maximum number of pending events exposed by one emulated input FIFO.
/// Hardware input producers must not be able to grow host memory without
/// bound when firmware stops polling.
pub const INPUT_EVENT_QUEUE_CAPACITY: usize = 64;

pub type SharedInputManager = Arc<Mutex<InputManager>>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InputEvent {
    Touch(Gt911TouchEvent),
    Keyboard(KeyEvent),
    Trackball(TrackballEvent),
}

pub struct InputManager {
    pub touch: TouchEmulator,
    pub keyboard: KeyboardEmulator,
    pub trackball: TrackballEmulator,
    i2c_keyboard: SharedI2cKeyboard,
    gt911: SharedGt911,
    instance_id: String,
    last_activity_ms: u64,
    touch_events: VecDeque<Gt911TouchEvent>,
    keyboard_events: VecDeque<KeyEvent>,
    trackball_events: VecDeque<TrackballEvent>,
}

impl InputManager {
    pub fn new(instance_id: &str, scale: f32) -> Self {
        Self {
            touch: TouchEmulator::new(320, 240, scale),
            keyboard: KeyboardEmulator::new(),
            trackball: TrackballEmulator::new(),
            i2c_keyboard: Arc::new(Mutex::new(I2cKeyboardBus::new_for_instance(instance_id))),
            gt911: crate::gt911::new_shared_gt911_for_instance(instance_id),
            instance_id: instance_id.to_owned(),
            last_activity_ms: monotonic_ms(),
            touch_events: VecDeque::with_capacity(INPUT_EVENT_QUEUE_CAPACITY),
            keyboard_events: VecDeque::with_capacity(INPUT_EVENT_QUEUE_CAPACITY),
            trackball_events: VecDeque::with_capacity(INPUT_EVENT_QUEUE_CAPACITY),
        }
    }

    pub fn instance_id(&self) -> &str {
        &self.instance_id
    }

    /// Process one SDL event and queue any resulting T-Deck peripheral events.
    pub fn handle_sdl_event(&mut self, event: &Event) -> Vec<InputEvent> {
        let mut events = Vec::with_capacity(2);
        match *event {
            Event::MouseMotion { x, y, .. } => {
                if let Some(event) = self.touch.handle_mouse_motion(x, y) {
                    self.update_gt911(true);
                    self.enqueue_touch_event(event);
                    events.push(InputEvent::Touch(event));
                }
            }
            Event::MouseButtonDown {
                mouse_btn, x, y, ..
            } => {
                if let Some(event) = self.touch.handle_mouse_button_at(mouse_btn, true, x, y) {
                    self.update_gt911(true);
                    self.enqueue_touch_event(event);
                    events.push(InputEvent::Touch(event));
                }
            }
            Event::MouseButtonUp {
                mouse_btn, x, y, ..
            } => {
                if let Some(event) = self.touch.handle_mouse_button_at(mouse_btn, false, x, y) {
                    self.update_gt911(false);
                    self.enqueue_touch_event(event);
                    events.push(InputEvent::Touch(event));
                }
            }
            Event::KeyDown {
                keycode: Some(keycode),
                ..
            } => {
                self.handle_keycode(keycode, true, &mut events);
            }
            Event::KeyUp {
                keycode: Some(keycode),
                ..
            } => {
                self.handle_keycode(keycode, false, &mut events);
            }
            _ => {}
        }
        if !events.is_empty() {
            self.wake();
        }
        events
    }

    pub fn inject_touch(&mut self, x: u16, y: u16, pressed: bool) -> bool {
        let Some(event) = self.touch.inject(x, y, pressed) else {
            return false;
        };
        self.update_gt911(pressed);
        self.enqueue_touch_event(event);
        self.wake();
        true
    }

    pub fn inject_key(&mut self, keycode: Keycode, pressed: bool) -> Vec<InputEvent> {
        let mut events = Vec::with_capacity(2);
        self.handle_keycode(keycode, pressed, &mut events);
        if !events.is_empty() {
            self.wake();
        }
        events
    }

    /// Configure the contact size used by both queued touch events and GT911 registers.
    pub fn set_touch_contact_size(&mut self, contact_size: u16) {
        self.touch.set_contact_size(contact_size);
        self.gt911
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .set_contact_size(contact_size);
    }

    pub fn touch_contact_size(&self) -> u16 {
        self.touch.contact_size()
    }

    pub fn i2c_keyboard(&self) -> SharedI2cKeyboard {
        Arc::clone(&self.i2c_keyboard)
    }

    pub fn gt911(&self) -> SharedGt911 {
        Arc::clone(&self.gt911)
    }

    /// Configure failure injection for this manager's GT911 instance.
    pub fn set_gt911_failure_mode(&mut self, mode: u8, value: u32) {
        crate::gt911::set_failure_mode_for_instance(&self.instance_id, mode, value);
    }

    pub fn gt911_watchdog_status(&self) -> u64 {
        crate::gt911::watchdog_status_for_instance(&self.instance_id)
    }

    pub fn touch_raw_position(&self) -> Option<(u16, u16)> {
        self.gt911
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .raw_position()
    }

    pub fn touch_mapped_position(&self) -> Option<(u16, u16)> {
        self.gt911
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .mapped_position()
    }

    pub fn wire_shim(&self) -> WireShim {
        WireShim::with_devices(self.i2c_keyboard(), self.gt911())
    }

    /// Read trackball pins or the GT911 interrupt pin.
    pub fn digital_read(&self, gpio: u8) -> bool {
        if gpio == crate::GT911_INT_GPIO {
            return self
                .gt911
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .gpio16_level();
        }
        self.trackball.digital_read(gpio)
    }

    pub fn take_falling_edges(&mut self, gpio: u8) -> u32 {
        self.trackball.take_falling_edges(gpio)
    }

    pub fn set_gpio_intr_enabled(&mut self, gpio: u8, enabled: bool) {
        self.trackball.set_gpio_intr_enabled(gpio, enabled);
    }

    pub fn gpio_intr_enabled(&self, gpio: u8) -> bool {
        self.trackball.gpio_intr_enabled(gpio)
    }

    pub fn poll_touch(&mut self) -> Option<Gt911TouchEvent> {
        self.touch_events.pop_front()
    }

    pub fn poll_keyboard(&mut self) -> Option<KeyEvent> {
        self.keyboard_events.pop_front()
    }

    pub fn poll_trackball(&mut self) -> Option<TrackballEvent> {
        self.trackball_events.pop_front()
    }

    /// Reset the display auto-off timer.
    pub fn wake(&mut self) {
        self.last_activity_ms = monotonic_ms();
    }

    /// Check whether the input inactivity timeout has elapsed.
    pub fn should_auto_off(&self, now_ms: u64, timeout_ms: u64) -> bool {
        now_ms.saturating_sub(self.last_activity_ms) >= timeout_ms
    }

    pub fn last_activity_ms(&self) -> u64 {
        self.last_activity_ms
    }

    fn handle_keycode(&mut self, keycode: Keycode, pressed: bool, events: &mut Vec<InputEvent>) {
        if let Some(event) = self.keyboard.handle_key(keycode, pressed) {
            if event.pressed && event.key_byte != 0 {
                self.i2c_keyboard
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .inject_key_byte(event.key_byte);
            }
            self.enqueue_keyboard_event(event);
            events.push(InputEvent::Keyboard(event));
        }
        if let Some(mut event) = self.trackball.handle_key(keycode, pressed) {
            event.timestamp_ms = monotonic_ms();
            self.enqueue_trackball_event(event);
            events.push(InputEvent::Trackball(event));
        }
    }

    fn update_gt911(&mut self, pressed: bool) {
        let Some((host_x, host_y)) = self.touch.last_host_position() else {
            return;
        };
        self.gt911
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .inject_touch(host_x, host_y, pressed);
    }

    fn enqueue_touch_event(&mut self, event: Gt911TouchEvent) -> bool {
        enqueue_bounded(&mut self.touch_events, event)
    }

    fn enqueue_keyboard_event(&mut self, event: KeyEvent) -> bool {
        enqueue_bounded(&mut self.keyboard_events, event)
    }

    fn enqueue_trackball_event(&mut self, event: TrackballEvent) -> bool {
        enqueue_bounded(&mut self.trackball_events, event)
    }
}

fn enqueue_bounded<T>(queue: &mut VecDeque<T>, event: T) -> bool {
    if queue.len() >= INPUT_EVENT_QUEUE_CAPACITY {
        return false;
    }
    queue.push_back(event);
    true
}

/// Register an instance route, or return its existing shared manager.
///
/// Duplicate registration updates only the window scale and never replaces the
/// live `Arc`, so existing producers and later lookups cannot diverge.
pub fn register_input_manager(instance_id: &str, scale: f32) -> SharedInputManager {
    let (manager, existed) = {
        let mut managers = INPUT_MANAGERS
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(manager) = managers.get(instance_id) {
            (Arc::clone(manager), true)
        } else {
            let manager = Arc::new(Mutex::new(InputManager::new(instance_id, scale)));
            managers.insert(instance_id.to_owned(), Arc::clone(&manager));
            (manager, false)
        }
    };
    if existed {
        manager
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .touch
            .set_window_scale(scale);
    }
    manager
}

pub fn get_input_manager(instance_id: &str) -> Option<SharedInputManager> {
    INPUT_MANAGERS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(instance_id)
        .cloned()
}

pub fn remove_input_manager(instance_id: &str) {
    INPUT_MANAGERS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .remove(instance_id);
}

fn monotonic_ms() -> u64 {
    START_TIME.elapsed().as_millis().min(u128::from(u64::MAX)) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key_event(instance_window: u32, keycode: Keycode, pressed: bool) -> Event {
        if pressed {
            Event::KeyDown {
                timestamp: 0,
                window_id: instance_window,
                keycode: Some(keycode),
                scancode: None,
                keymod: sdl2::keyboard::Mod::NOMOD,
                repeat: false,
            }
        } else {
            Event::KeyUp {
                timestamp: 0,
                window_id: instance_window,
                keycode: Some(keycode),
                scancode: None,
                keymod: sdl2::keyboard::Mod::NOMOD,
                repeat: false,
            }
        }
    }

    #[test]
    fn routes_events_only_into_the_target_manager() {
        let mut first = InputManager::new("first", 1.0);
        let mut second = InputManager::new("second", 1.0);
        let event = key_event(7, Keycode::Q, true);
        first.handle_sdl_event(&event);

        assert_eq!(first.instance_id(), "first");
        assert_eq!(first.poll_keyboard().unwrap().row, 0);
        assert!(second.poll_keyboard().is_none());
    }

    #[test]
    fn auto_off_uses_last_user_activity() {
        let mut manager = InputManager::new("node", 1.0);
        manager.last_activity_ms = 100;
        assert!(!manager.should_auto_off(1_099, 1_000));
        assert!(manager.should_auto_off(1_100, 1_000));
        assert!(!manager.should_auto_off(50, 1_000));
    }

    #[test]
    fn ignored_sdl_input_does_not_reset_activity() {
        let mut manager = InputManager::new("node", 1.0);
        manager.last_activity_ms = 4_242;

        let ignored = [
            Event::MouseMotion {
                timestamp: 0,
                window_id: 1,
                which: 0,
                mousestate: sdl2::mouse::MouseState::from_sdl_state(0),
                x: 10,
                y: 10,
                xrel: 1,
                yrel: 1,
            },
            Event::MouseMotion {
                timestamp: 0,
                window_id: 1,
                which: 0,
                mousestate: sdl2::mouse::MouseState::from_sdl_state(0),
                x: 320,
                y: 10,
                xrel: 1,
                yrel: 0,
            },
            Event::MouseButtonDown {
                timestamp: 0,
                window_id: 1,
                which: 0,
                mouse_btn: sdl2::mouse::MouseButton::Right,
                clicks: 1,
                x: 10,
                y: 10,
            },
            key_event(1, Keycode::F1, true),
        ];

        for event in ignored {
            assert!(manager.handle_sdl_event(&event).is_empty());
            assert_eq!(manager.last_activity_ms(), 4_242);
        }

        assert_eq!(
            manager
                .handle_sdl_event(&key_event(1, Keycode::Q, true))
                .len(),
            1
        );
        manager.last_activity_ms = 4_242;
        assert!(manager
            .handle_sdl_event(&key_event(1, Keycode::Q, true))
            .is_empty());
        assert_eq!(manager.last_activity_ms(), 4_242);

        assert_eq!(
            manager
                .handle_sdl_event(&key_event(1, Keycode::Up, true))
                .len(),
            1
        );
        manager.last_activity_ms = 4_242;
        assert!(manager
            .handle_sdl_event(&key_event(1, Keycode::Up, true))
            .is_empty());
        assert_eq!(manager.last_activity_ms(), 4_242);
    }

    #[test]
    fn accepted_hardware_input_resets_activity() {
        let mut manager = InputManager::new("node", 1.0);
        manager.last_activity_ms = u64::MAX;

        assert_eq!(
            manager
                .handle_sdl_event(&key_event(1, Keycode::Q, true))
                .len(),
            1
        );
        assert_ne!(manager.last_activity_ms(), u64::MAX);

        manager.last_activity_ms = u64::MAX;
        assert!(manager.inject_touch(20, 30, true));
        assert_ne!(manager.last_activity_ms(), u64::MAX);
    }

    #[test]
    fn configured_contact_size_reaches_event_queue_and_gt911_registers() {
        let mut manager = InputManager::new("node", 1.0);
        manager.set_touch_contact_size(123);
        assert_eq!(manager.touch_contact_size(), 123);
        assert!(manager.inject_touch(100, 40, true));
        assert_eq!(manager.poll_touch().unwrap().pressure, 123);
        assert_eq!(
            manager.gt911().lock().unwrap().touch_points()[0]
                .unwrap()
                .size,
            123
        );
    }

    #[test]
    fn duplicate_registration_preserves_one_shared_route() {
        let instance_id = "duplicate-registration-preserves-route";
        remove_input_manager(instance_id);
        let original = register_input_manager(instance_id, 1.0);
        original.lock().unwrap().inject_key(Keycode::Q, true);

        let duplicate = register_input_manager(instance_id, 2.0);
        let lookup = get_input_manager(instance_id).unwrap();
        assert!(Arc::ptr_eq(&original, &duplicate));
        assert!(Arc::ptr_eq(&original, &lookup));
        assert_eq!(
            duplicate.lock().unwrap().poll_keyboard().unwrap().key_byte,
            b'q'
        );
        remove_input_manager(instance_id);
    }

    #[test]
    fn return_and_keypad_enter_drive_separate_hardware() {
        let mut manager = InputManager::new("node", 1.0);
        let events = manager.handle_sdl_event(&key_event(1, Keycode::Return, true));
        assert_eq!(events.len(), 1);
        assert!(manager.poll_keyboard().is_some());
        assert!(manager.poll_trackball().is_none());

        let events = manager.handle_sdl_event(&key_event(1, Keycode::KpEnter, true));
        assert_eq!(events.len(), 1);
        assert!(manager.poll_keyboard().is_none());
        assert_eq!(
            manager.poll_trackball().unwrap().direction,
            crate::TrackballDirection::Center
        );
        assert!(!manager.digital_read(crate::TRACKBALL_CLICK_GPIO));
    }

    #[test]
    fn keyboard_events_update_lvgl_queue_and_raw_i2c_key_bytes() {
        let mut manager = InputManager::new("node", 1.0);
        let mut wire = manager.wire_shim();
        wire.begin();
        wire.set_clock(crate::wire_shim::KEYBOARD_I2C_CLOCK_HZ);
        wire.begin_transmission(crate::KEYBOARD_I2C_ADDRESS);
        wire.write_byte(crate::KEYBOARD_KEY_MODE_COMMAND);
        assert_eq!(wire.end_transmission(), 0);

        assert_eq!(manager.inject_key(Keycode::D, true).len(), 1);
        assert_eq!(manager.poll_keyboard().unwrap().col, 2);
        assert_eq!(wire.request_from(crate::KEYBOARD_I2C_ADDRESS, 1), 1);
        assert_eq!(wire.read(), i32::from(b'd'));

        manager.inject_key(Keycode::D, false);
        assert_eq!(wire.request_from(crate::KEYBOARD_I2C_ADDRESS, 1), 1);
        assert_eq!(wire.read(), 0);
    }

    #[test]
    fn input_event_queues_drop_new_events_at_hardware_capacity() {
        let mut manager = InputManager::new("bounded-input", 1.0);
        for _ in 0..(INPUT_EVENT_QUEUE_CAPACITY * 2) {
            manager.inject_key(Keycode::Q, true);
            manager.inject_key(Keycode::Q, false);
        }

        let mut queued = 0;
        while manager.poll_keyboard().is_some() {
            queued += 1;
        }
        assert_eq!(queued, INPUT_EVENT_QUEUE_CAPACITY);
    }

    #[test]
    fn manager_keyboard_retention_is_scoped_to_the_input_instance() {
        let first_id = "manager-keyboard-retention-first";
        let second_id = "manager-keyboard-retention-second";
        let first = InputManager::new(first_id, 1.0);
        first
            .i2c_keyboard()
            .lock()
            .unwrap()
            .write_transaction(&[crate::KEYBOARD_BRIGHTNESS_COMMAND, 88]);

        let recreated = InputManager::new(first_id, 1.0);
        assert_eq!(recreated.i2c_keyboard().lock().unwrap().backlight(), 88);
        assert_eq!(
            InputManager::new(second_id, 1.0)
                .i2c_keyboard()
                .lock()
                .unwrap()
                .backlight(),
            0
        );
        recreated
            .i2c_keyboard()
            .lock()
            .unwrap()
            .set_cross_reset_persist(false);
    }

    #[test]
    fn shifted_host_key_reaches_i2c_as_uppercase_ascii() {
        let mut manager = InputManager::new("node", 1.0);
        let mut wire = manager.wire_shim();
        wire.begin();
        wire.begin_transmission(crate::KEYBOARD_I2C_ADDRESS);
        wire.write_byte(crate::KEYBOARD_KEY_MODE_COMMAND);
        assert_eq!(wire.end_transmission(), 0);
        manager.inject_key(Keycode::LShift, true);
        manager.inject_key(Keycode::Q, true);
        assert_eq!(wire.request_from(crate::KEYBOARD_I2C_ADDRESS, 1), 1);
        assert_eq!(wire.read(), i32::from(b'Q'));
    }

    #[test]
    fn sdl_touch_and_firmware_wire_share_the_gt911() {
        let mut manager = InputManager::new("node", 1.0);
        let mut wire = manager.wire_shim();
        wire.begin();
        manager.handle_sdl_event(&Event::MouseButtonDown {
            timestamp: 0,
            window_id: 1,
            which: 0,
            mouse_btn: sdl2::mouse::MouseButton::Left,
            clicks: 1,
            x: 100,
            y: 40,
        });

        assert!(!manager.digital_read(crate::GT911_INT_GPIO));
        wire.begin_transmission(crate::GT911_I2C_ADDRESS);
        for byte in crate::GT911_STATUS_REGISTER.to_be_bytes() {
            wire.write_byte(byte);
        }
        assert_eq!(wire.end_transmission(), 0);
        assert_eq!(wire.request_from(crate::GT911_I2C_ADDRESS, 9), 9);
        let bytes: Vec<_> = (0..9).map(|_| wire.read() as u8).collect();
        assert_eq!(bytes[0], 0x81);
        assert_eq!(u16::from_le_bytes([bytes[2], bytes[3]]), 40);
        assert_eq!(u16::from_le_bytes([bytes[4], bytes[5]]), 219);
    }

    #[test]
    fn arrow_key_drives_active_low_gpio_and_falling_interrupt() {
        let mut manager = InputManager::new("node", 1.0);
        manager.handle_sdl_event(&key_event(1, Keycode::Up, true));
        assert!(!manager.digital_read(crate::TRACKBALL_UP_GPIO));
        assert_eq!(manager.take_falling_edges(crate::TRACKBALL_UP_GPIO), 1);

        manager.handle_sdl_event(&key_event(1, Keycode::Up, false));
        assert!(manager.digital_read(crate::TRACKBALL_UP_GPIO));
    }

    #[test]
    fn gpio_interrupt_enable_survives_unrelated_injected_input() {
        let mut manager = InputManager::new("node", 1.0);
        manager.set_gpio_intr_enabled(crate::TRACKBALL_UP_GPIO, false);
        manager.inject_touch(20, 30, true);
        manager.inject_key(Keycode::Q, true);
        assert!(!manager.gpio_intr_enabled(crate::TRACKBALL_UP_GPIO));

        manager.inject_key(Keycode::Up, true);
        assert_eq!(manager.take_falling_edges(crate::TRACKBALL_UP_GPIO), 0);
        manager.inject_key(Keycode::Up, false);
        manager.set_gpio_intr_enabled(crate::TRACKBALL_UP_GPIO, true);
        manager.inject_key(Keycode::Up, true);
        assert_eq!(manager.take_falling_edges(crate::TRACKBALL_UP_GPIO), 1);
    }
}
