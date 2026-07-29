use sdl2::keyboard::Keycode;

pub const TRACKBALL_UP_GPIO: u8 = 3;
pub const TRACKBALL_DOWN_GPIO: u8 = 15;
pub const TRACKBALL_LEFT_GPIO: u8 = 1;
pub const TRACKBALL_RIGHT_GPIO: u8 = 2;
pub const TRACKBALL_CLICK_GPIO: u8 = 0;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TrackballDirection {
    Up,
    Down,
    Left,
    Right,
    Center,
}

impl TrackballDirection {
    pub const fn gpio(self) -> u8 {
        match self {
            Self::Up => TRACKBALL_UP_GPIO,
            Self::Down => TRACKBALL_DOWN_GPIO,
            Self::Left => TRACKBALL_LEFT_GPIO,
            Self::Right => TRACKBALL_RIGHT_GPIO,
            Self::Center => TRACKBALL_CLICK_GPIO,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TrackballEvent {
    pub direction: TrackballDirection,
    pub pressed: bool,
    pub timestamp_ms: u64,
}

/// Active-LOW GPIO model of the T-Deck five-way trackball.
pub struct TrackballGpio {
    up: bool,
    down: bool,
    left: bool,
    right: bool,
    click: bool,
    boot_state: bool,
    falling_edges: [u32; 5],
    last_event: Option<TrackballEvent>,
}

impl TrackballGpio {
    pub fn new() -> Self {
        Self {
            up: true,
            down: true,
            left: true,
            right: true,
            click: true,
            boot_state: true,
            falling_edges: [0; 5],
            last_event: None,
        }
    }

    /// Read a virtual pin as firmware `digitalRead` would.
    pub fn digital_read(&self, gpio: u8) -> bool {
        match gpio {
            TRACKBALL_CLICK_GPIO => self.click && self.boot_state,
            TRACKBALL_LEFT_GPIO => self.left,
            TRACKBALL_RIGHT_GPIO => self.right,
            TRACKBALL_UP_GPIO => self.up,
            TRACKBALL_DOWN_GPIO => self.down,
            _ => true,
        }
    }

    /// Drive the BOOT side of shared GPIO0.
    pub fn set_boot_pressed(&mut self, pressed: bool) {
        let was_high = self.digital_read(TRACKBALL_CLICK_GPIO);
        self.boot_state = !pressed;
        if was_high && !self.digital_read(TRACKBALL_CLICK_GPIO) {
            self.falling_edges[direction_index(TrackballDirection::Center)] += 1;
        }
    }

    pub fn press(&mut self, direction: TrackballDirection) {
        let gpio = direction.gpio();
        let was_high = self.digital_read(gpio);
        *self.pin(direction) = false;
        if was_high && !self.digital_read(gpio) {
            self.falling_edges[direction_index(direction)] += 1;
        }
    }

    pub fn release(&mut self, direction: TrackballDirection) {
        *self.pin(direction) = true;
    }

    /// Consume pending FALLING interrupts for a trackball GPIO.
    pub fn take_falling_edges(&mut self, gpio: u8) -> u32 {
        let Some(index) = gpio_index(gpio) else {
            return 0;
        };
        let edges = self.falling_edges[index];
        self.falling_edges[index] = 0;
        edges
    }

    pub fn handle_key(&mut self, keycode: Keycode, pressed: bool) -> Option<TrackballEvent> {
        let direction = match keycode {
            Keycode::Up => TrackballDirection::Up,
            Keycode::Down => TrackballDirection::Down,
            Keycode::Left => TrackballDirection::Left,
            Keycode::Right => TrackballDirection::Right,
            Keycode::KpEnter => TrackballDirection::Center,
            _ => return None,
        };
        if self.is_pressed(direction) == pressed {
            return None;
        }
        if pressed {
            self.press(direction);
        } else {
            self.release(direction);
        }
        let event = TrackballEvent {
            direction,
            pressed,
            timestamp_ms: 0,
        };
        self.last_event = Some(event);
        Some(event)
    }

    pub fn get_last(&self) -> Option<TrackballEvent> {
        self.last_event
    }

    fn is_pressed(&self, direction: TrackballDirection) -> bool {
        !match direction {
            TrackballDirection::Up => self.up,
            TrackballDirection::Down => self.down,
            TrackballDirection::Left => self.left,
            TrackballDirection::Right => self.right,
            TrackballDirection::Center => self.click,
        }
    }

    fn pin(&mut self, direction: TrackballDirection) -> &mut bool {
        match direction {
            TrackballDirection::Up => &mut self.up,
            TrackballDirection::Down => &mut self.down,
            TrackballDirection::Left => &mut self.left,
            TrackballDirection::Right => &mut self.right,
            TrackballDirection::Center => &mut self.click,
        }
    }
}

impl Default for TrackballGpio {
    fn default() -> Self {
        Self::new()
    }
}

pub type TrackballEmulator = TrackballGpio;

const fn direction_index(direction: TrackballDirection) -> usize {
    match direction {
        TrackballDirection::Up => 0,
        TrackballDirection::Down => 1,
        TrackballDirection::Left => 2,
        TrackballDirection::Right => 3,
        TrackballDirection::Center => 4,
    }
}

const fn gpio_index(gpio: u8) -> Option<usize> {
    match gpio {
        TRACKBALL_UP_GPIO => Some(0),
        TRACKBALL_DOWN_GPIO => Some(1),
        TRACKBALL_LEFT_GPIO => Some(2),
        TRACKBALL_RIGHT_GPIO => Some(3),
        TRACKBALL_CLICK_GPIO => Some(4),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn up_press_is_a_gpio3_falling_edge_and_release_restores_high() {
        let mut trackball = TrackballGpio::new();
        assert!(trackball.digital_read(TRACKBALL_UP_GPIO));

        trackball.press(TrackballDirection::Up);
        assert!(!trackball.digital_read(TRACKBALL_UP_GPIO));
        assert_eq!(trackball.take_falling_edges(TRACKBALL_UP_GPIO), 1);

        trackball.release(TrackballDirection::Up);
        assert!(trackball.digital_read(TRACKBALL_UP_GPIO));
        assert_eq!(trackball.take_falling_edges(TRACKBALL_UP_GPIO), 0);
    }

    #[test]
    fn gpio0_is_low_for_either_trackball_click_or_boot_button() {
        let mut trackball = TrackballGpio::new();
        trackball.press(TrackballDirection::Center);
        assert!(!trackball.digital_read(TRACKBALL_CLICK_GPIO));
        assert_eq!(trackball.take_falling_edges(TRACKBALL_CLICK_GPIO), 1);
        trackball.release(TrackballDirection::Center);
        assert!(trackball.digital_read(TRACKBALL_CLICK_GPIO));

        trackball.set_boot_pressed(true);
        assert!(!trackball.digital_read(TRACKBALL_CLICK_GPIO));
        assert_eq!(trackball.take_falling_edges(TRACKBALL_CLICK_GPIO), 1);
        trackball.press(TrackballDirection::Center);
        assert_eq!(trackball.take_falling_edges(TRACKBALL_CLICK_GPIO), 0);
        trackball.set_boot_pressed(false);
        assert!(!trackball.digital_read(TRACKBALL_CLICK_GPIO));
        trackball.release(TrackballDirection::Center);
        assert!(trackball.digital_read(TRACKBALL_CLICK_GPIO));
    }

    #[test]
    fn keypad_enter_is_click_but_return_is_not() {
        let mut trackball = TrackballGpio::new();
        assert!(trackball.handle_key(Keycode::Return, true).is_none());
        assert_eq!(
            trackball
                .handle_key(Keycode::KpEnter, true)
                .unwrap()
                .direction,
            TrackballDirection::Center
        );
        assert!(!trackball.digital_read(TRACKBALL_CLICK_GPIO));
    }

    #[test]
    fn suppresses_duplicate_transitions() {
        let mut trackball = TrackballGpio::new();
        assert!(trackball.handle_key(Keycode::Up, true).is_some());
        assert!(trackball.handle_key(Keycode::Up, true).is_none());
        assert!(trackball.handle_key(Keycode::Up, false).is_some());
        assert!(trackball.handle_key(Keycode::Up, false).is_none());
        assert_eq!(trackball.take_falling_edges(TRACKBALL_UP_GPIO), 1);
    }
}
