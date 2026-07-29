use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Instant;

use sdl2::event::Event;
use sdl2::keyboard::Keycode;

use crate::{
    Gt911TouchEvent, KeyEvent, KeyboardEmulator, TouchEmulator, TrackballEmulator, TrackballEvent,
};

static START_TIME: LazyLock<Instant> = LazyLock::new(Instant::now);
static INPUT_MANAGERS: LazyLock<Mutex<HashMap<String, SharedInputManager>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

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
            instance_id: instance_id.to_owned(),
            last_activity_ms: monotonic_ms(),
            touch_events: VecDeque::new(),
            keyboard_events: VecDeque::new(),
            trackball_events: VecDeque::new(),
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
                self.wake();
                if let Some(event) = self.touch.handle_mouse_motion(x, y) {
                    self.touch_events.push_back(event);
                    events.push(InputEvent::Touch(event));
                }
            }
            Event::MouseButtonDown {
                mouse_btn, x, y, ..
            } => {
                self.wake();
                self.touch.handle_mouse_motion(x, y);
                if let Some(event) = self.touch.handle_mouse_button(mouse_btn, true) {
                    self.touch_events.push_back(event);
                    events.push(InputEvent::Touch(event));
                }
            }
            Event::MouseButtonUp {
                mouse_btn, x, y, ..
            } => {
                self.wake();
                self.touch.handle_mouse_motion(x, y);
                if let Some(event) = self.touch.handle_mouse_button(mouse_btn, false) {
                    self.touch_events.push_back(event);
                    events.push(InputEvent::Touch(event));
                }
            }
            Event::KeyDown {
                keycode: Some(keycode),
                ..
            } => {
                self.wake();
                self.handle_keycode(keycode, true, &mut events);
            }
            Event::KeyUp {
                keycode: Some(keycode),
                ..
            } => {
                self.wake();
                self.handle_keycode(keycode, false, &mut events);
            }
            _ => {}
        }
        events
    }

    pub fn inject_touch(&mut self, x: u16, y: u16, pressed: bool) -> bool {
        self.wake();
        let Some(event) = self.touch.inject(x, y, pressed) else {
            return false;
        };
        self.touch_events.push_back(event);
        true
    }

    pub fn inject_key(&mut self, keycode: Keycode, pressed: bool) -> Vec<InputEvent> {
        self.wake();
        let mut events = Vec::with_capacity(2);
        self.handle_keycode(keycode, pressed, &mut events);
        events
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
            self.keyboard_events.push_back(event);
            events.push(InputEvent::Keyboard(event));
        }
        if let Some(mut event) = self.trackball.handle_key(keycode, pressed) {
            event.timestamp_ms = monotonic_ms();
            self.trackball_events.push_back(event);
            events.push(InputEvent::Trackball(event));
        }
    }
}

pub fn register_input_manager(instance_id: &str, scale: f32) -> SharedInputManager {
    let manager = Arc::new(Mutex::new(InputManager::new(instance_id, scale)));
    INPUT_MANAGERS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(instance_id.to_owned(), Arc::clone(&manager));
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
    fn keyboard_and_trackball_events_are_queued_independently() {
        let mut manager = InputManager::new("node", 1.0);
        let events = manager.handle_sdl_event(&key_event(1, Keycode::Return, true));
        assert_eq!(events.len(), 2);
        assert!(manager.poll_keyboard().is_some());
        assert_eq!(
            manager.poll_trackball().unwrap().direction,
            crate::TrackballDirection::Center
        );
    }
}
