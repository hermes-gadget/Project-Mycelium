use sdl2::keyboard::Keycode;

/// T-Deck five-direction trackball.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum TrackballDirection {
    Up = 0,
    Down = 1,
    Left = 2,
    Right = 3,
    Center = 4,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TrackballEvent {
    pub direction: TrackballDirection,
    pub pressed: bool,
    pub timestamp_ms: u64,
}

#[derive(Default)]
pub struct TrackballEmulator {
    last_event: Option<TrackballEvent>,
}

impl TrackballEmulator {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn handle_key(&mut self, keycode: Keycode, pressed: bool) -> Option<TrackballEvent> {
        let direction = match keycode {
            Keycode::Up => TrackballDirection::Up,
            Keycode::Down => TrackballDirection::Down,
            Keycode::Left => TrackballDirection::Left,
            Keycode::Right => TrackballDirection::Right,
            Keycode::Return => TrackballDirection::Center,
            _ => return None,
        };
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_arrows_and_return_to_trackball_directions() {
        let mut trackball = TrackballEmulator::new();
        assert_eq!(
            trackball.handle_key(Keycode::Left, true).unwrap().direction,
            TrackballDirection::Left
        );
        assert_eq!(
            trackball.handle_key(Keycode::Return, false).unwrap(),
            TrackballEvent {
                direction: TrackballDirection::Center,
                pressed: false,
                timestamp_ms: 0,
            }
        );
        assert!(trackball.handle_key(Keycode::A, true).is_none());
    }
}
