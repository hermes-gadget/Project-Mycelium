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
    last_host_position: Option<(i32, i32)>,
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
            last_host_position: None,
            last_touch: None,
            pressing: false,
        }
    }

    /// Track SDL mouse coordinates and emit only while a contact is active.
    pub fn handle_mouse_motion(&mut self, window_x: i32, window_y: i32) -> Option<Gt911TouchEvent> {
        let host_position = self.scale_position(window_x, window_y)?;
        self.last_host_position = Some(host_position);
        if !self.pressing {
            return None;
        }
        let event = portrait_event(host_position.0, host_position.1, true);
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
        let (host_x, host_y) = self.last_host_position?;
        let event = portrait_event(host_x, host_y, pressed);
        self.last_touch = Some(event);
        Some(event)
    }

    /// Inject landscape host coordinates without applying window scale.
    pub fn inject(&mut self, x: u16, y: u16, pressed: bool) -> Option<Gt911TouchEvent> {
        if x >= self.display_width as u16 || y >= self.display_height as u16 {
            return None;
        }
        self.pressing = pressed;
        self.last_host_position = Some((i32::from(x), i32::from(y)));
        let event = portrait_event(i32::from(x), i32::from(y), pressed);
        self.last_touch = Some(event);
        Some(event)
    }

    pub fn get_last(&self) -> Option<Gt911TouchEvent> {
        self.last_touch
    }

    pub fn set_window_scale(&mut self, window_scale: f32) {
        self.window_scale = if window_scale.is_finite() && window_scale > 0.0 {
            window_scale
        } else {
            1.0
        };
    }

    pub fn last_host_position(&self) -> Option<(i32, i32)> {
        self.last_host_position
    }

    fn scale_position(&self, window_x: i32, window_y: i32) -> Option<(i32, i32)> {
        if window_x < 0 || window_y < 0 {
            return None;
        }
        let x = (window_x as f32 / self.window_scale) as i32;
        let y = (window_y as f32 / self.window_scale) as i32;
        (x < self.display_width as i32 && y < self.display_height as i32).then_some((x, y))
    }
}

fn portrait_event(host_x: i32, host_y: i32, pressed: bool) -> Gt911TouchEvent {
    Gt911TouchEvent {
        x: host_y.clamp(0, 239) as u16,
        y: (319 - host_x).clamp(0, 319) as u16,
        pressure: if pressed { 255 } else { 0 },
        touch_id: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hover_is_silent_and_pressed_motion_uses_portrait_coordinates() {
        let mut touch = TouchEmulator::new(320, 240, 2.0);
        assert_eq!(touch.handle_mouse_motion(318, 238), None);
        touch.handle_mouse_button(MouseButton::Left, true);
        assert_eq!(
            touch.handle_mouse_motion(318, 238),
            Some(Gt911TouchEvent {
                x: 119,
                y: 160,
                pressure: 255,
                touch_id: 0,
            })
        );
        assert_eq!(touch.handle_mouse_motion(640, 10), None);
        assert_eq!(touch.handle_mouse_motion(-1, 10), None);
    }

    #[test]
    fn left_button_changes_pressure_at_the_last_position() {
        let mut touch = TouchEmulator::new(320, 240, 1.0);
        assert_eq!(touch.handle_mouse_motion(12, 34), None);
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
