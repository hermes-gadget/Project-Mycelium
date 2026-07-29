use sdl2::mouse::MouseButton;

/// GT911 touch event (matches the T-Deck capacitive touch controller).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Gt911TouchEvent {
    pub x: u16,
    pub y: u16,
    pub pressure: u8,
    pub touch_id: u8,
}

pub struct TouchEmulator {
    display_width: u32,
    display_height: u32,
    window_scale: f32,
    last_touch: Option<Gt911TouchEvent>,
    pressing: bool,
}

impl TouchEmulator {
    pub fn new(display_width: u32, display_height: u32, window_scale: f32) -> Self {
        Self {
            display_width,
            display_height,
            window_scale: if window_scale.is_finite() && window_scale > 0.0 {
                window_scale
            } else {
                1.0
            },
            last_touch: None,
            pressing: false,
        }
    }

    /// Convert SDL mouse coordinates to GT911 touch coordinates.
    pub fn handle_mouse_motion(&mut self, window_x: i32, window_y: i32) -> Option<Gt911TouchEvent> {
        if window_x < 0 || window_y < 0 {
            return None;
        }
        let tx = (window_x as f32 / self.window_scale) as u16;
        let ty = (window_y as f32 / self.window_scale) as u16;
        if tx >= self.display_width as u16 || ty >= self.display_height as u16 {
            return None;
        }
        let event = Gt911TouchEvent {
            x: tx,
            y: ty,
            pressure: if self.pressing { 255 } else { 0 },
            touch_id: 0,
        };
        self.last_touch = Some(event);
        Some(event)
    }

    pub fn handle_mouse_button(
        &mut self,
        button: MouseButton,
        pressed: bool,
    ) -> Option<Gt911TouchEvent> {
        if button != MouseButton::Left {
            return None;
        }
        self.pressing = pressed;
        let mut event = self.last_touch?;
        event.pressure = if pressed { 255 } else { 0 };
        self.last_touch = Some(event);
        Some(event)
    }

    /// Inject logical display coordinates without applying the window scale.
    pub fn inject(&mut self, x: u16, y: u16, pressed: bool) -> Option<Gt911TouchEvent> {
        if x >= self.display_width as u16 || y >= self.display_height as u16 {
            return None;
        }
        self.pressing = pressed;
        let event = Gt911TouchEvent {
            x,
            y,
            pressure: if pressed { 255 } else { 0 },
            touch_id: 0,
        };
        self.last_touch = Some(event);
        Some(event)
    }

    pub fn get_last(&self) -> Option<Gt911TouchEvent> {
        self.last_touch
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_scaled_window_coordinates_and_rejects_outside_points() {
        let mut touch = TouchEmulator::new(320, 240, 2.0);
        assert_eq!(
            touch.handle_mouse_motion(318, 238),
            Some(Gt911TouchEvent {
                x: 159,
                y: 119,
                pressure: 0,
                touch_id: 0,
            })
        );
        assert_eq!(touch.handle_mouse_motion(640, 10), None);
        assert_eq!(touch.handle_mouse_motion(-1, 10), None);
    }

    #[test]
    fn left_button_changes_pressure_at_the_last_position() {
        let mut touch = TouchEmulator::new(320, 240, 1.0);
        touch.handle_mouse_motion(12, 34);
        assert_eq!(
            touch
                .handle_mouse_button(MouseButton::Left, true)
                .unwrap()
                .pressure,
            255
        );
        assert_eq!(
            touch
                .handle_mouse_button(MouseButton::Left, false)
                .unwrap()
                .pressure,
            0
        );
        assert_eq!(touch.handle_mouse_button(MouseButton::Right, true), None);
    }
}
