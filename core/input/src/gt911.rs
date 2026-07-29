pub const GT911_I2C_ADDRESS: u8 = 0x5d;
pub const GT911_STATUS_REGISTER: u16 = 0x814e;
pub const GT911_INT_GPIO: u8 = 16;
pub const GT911_MAX_TOUCHES: usize = 5;

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
    int_asserted: bool,
    frame_ready: bool,
}

impl Gt911Controller {
    pub fn new() -> Self {
        Self {
            touch_points: [None; GT911_MAX_TOUCHES],
            int_asserted: false,
            frame_ready: false,
        }
    }

    /// Inject a primary contact from landscape host coordinates.
    ///
    /// The T-Deck portrait driver swaps and reverses the hardware axes.
    pub fn inject_touch(&mut self, mouse_x: i32, mouse_y: i32, pressed: bool) {
        if pressed {
            let point = Gt911Point {
                track_id: 0,
                x: mouse_y.clamp(0, 239) as u16,
                y: (319 - mouse_x).clamp(0, 319) as u16,
                size: 50,
            };
            self.touch_points[0] = Some(point);
            self.frame_ready = true;
            self.int_asserted = true;
        } else {
            self.touch_points[0] = None;
            self.frame_ready = false;
            self.int_asserted = false;
        }
    }

    /// Inject or release one of the five hardware contact slots.
    pub fn inject_point(&mut self, point: Gt911Point, pressed: bool) -> bool {
        let slot = usize::from(point.track_id);
        if slot >= GT911_MAX_TOUCHES {
            return false;
        }
        self.touch_points[slot] = pressed.then_some(point);
        self.frame_ready = true;
        self.int_asserted = true;
        true
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

    /// Read the GT911 status/contact register window.
    pub fn i2c_read(&mut self, register: u16, buf: &mut [u8]) -> usize {
        for (offset, output) in buf.iter_mut().enumerate() {
            let Some(address) = register.checked_add(offset as u16) else {
                break;
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

    fn register_byte(&self, address: u16) -> u8 {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn touch_is_encoded_in_gt911_register_format_with_portrait_swap() {
        let mut controller = Gt911Controller::new();
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
        assert_eq!(u16::from_le_bytes([registers[6], registers[7]]), 50);
        assert_eq!(registers[8], 0);
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
}
