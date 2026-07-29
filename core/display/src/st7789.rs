//! Optional ST7789 command-path fidelity.
//!
//! The normal Mycelium backend consumes already-corrected logical LVGL pixels.
//! This model is opt-in for tests and firmware adapters that need reset,
//! MADCTL, inversion, address-window, backlight, wire-byte, and SPI transaction
//! behavior. It is not required for ordinary UI rendering.

use anyhow::{bail, ensure, Result};

use crate::framebuffer::st7789_wire_to_host_rgb565;
use crate::shared_spi::{SharedSpiBus, SpiDevice};

pub const ST7789_CASET: u8 = 0x2a;
pub const ST7789_RASET: u8 = 0x2b;
pub const ST7789_RAMWR: u8 = 0x2c;
pub const ST7789_MADCTL: u8 = 0x36;
pub const ST7789_INVON: u8 = 0x21;
pub const ST7789_INVOFF: u8 = 0x20;
pub const T_DECK_MADCTL: u8 = 0x55;

#[derive(Clone)]
pub struct St7789Controller {
    width: u16,
    height: u16,
    gram: Vec<u8>,
    bus: SharedSpiBus,
    reset_high: bool,
    reset_seen_low: bool,
    initialized: bool,
    inverted: bool,
    madctl: u8,
    backlight: u8,
    column: (u16, u16),
    row: (u16, u16),
    ram_write_selected: bool,
}

impl St7789Controller {
    pub fn new(width: u16, height: u16, bus: SharedSpiBus) -> Self {
        Self {
            width,
            height,
            gram: vec![0; usize::from(width) * usize::from(height) * 2],
            bus,
            reset_high: true,
            reset_seen_low: false,
            initialized: false,
            inverted: false,
            madctl: 0,
            backlight: 0,
            column: (0, width.saturating_sub(1)),
            row: (0, height.saturating_sub(1)),
            ram_write_selected: false,
        }
    }

    pub fn set_reset(&mut self, high: bool) {
        self.reset_high = high;
        if !high {
            self.reset_seen_low = true;
            self.initialized = false;
            self.inverted = false;
            self.madctl = 0;
            self.ram_write_selected = false;
        }
    }

    /// Model the T-Deck controller initialization used by the real board.
    pub fn initialize_t_deck(&mut self) -> Result<()> {
        ensure!(self.reset_high, "ST7789 is held in reset");
        ensure!(
            self.reset_seen_low,
            "ST7789 initialization requires a low-to-high reset sequence"
        );
        self.write_command(ST7789_MADCTL, &[T_DECK_MADCTL])?;
        self.write_command(ST7789_INVON, &[])?;
        self.initialized = true;
        Ok(())
    }

    /// Set the GPIO42/PWM duty cycle modeled as an 8-bit brightness.
    pub fn set_backlight(&mut self, duty: u8) {
        self.backlight = duty;
    }

    pub fn write_command(&mut self, command: u8, data: &[u8]) -> Result<()> {
        let bus = self.bus.clone();
        bus.transaction(SpiDevice::Display, || self.apply_command(command, data))?
    }

    pub fn write_pixels(&mut self, wire_bytes: &[u8]) -> Result<()> {
        let bus = self.bus.clone();
        bus.transaction(SpiDevice::Display, || self.apply_pixels(wire_bytes))?
    }

    pub fn framebuffer_host_rgb565(&self) -> &[u8] {
        &self.gram
    }

    pub fn madctl(&self) -> u8 {
        self.madctl
    }

    pub fn inverted(&self) -> bool {
        self.inverted
    }

    pub fn initialized(&self) -> bool {
        self.initialized
    }

    pub fn backlight(&self) -> u8 {
        self.backlight
    }

    fn apply_command(&mut self, command: u8, data: &[u8]) -> Result<()> {
        self.ram_write_selected = false;
        match command {
            ST7789_MADCTL => {
                ensure!(data.len() == 1, "MADCTL requires one byte");
                self.madctl = data[0];
            }
            ST7789_INVON => {
                ensure!(data.is_empty(), "INVON does not take data");
                self.inverted = true;
            }
            ST7789_INVOFF => {
                ensure!(data.is_empty(), "INVOFF does not take data");
                self.inverted = false;
            }
            ST7789_CASET => self.column = parse_window(data, self.width)?,
            ST7789_RASET => self.row = parse_window(data, self.height)?,
            ST7789_RAMWR => {
                ensure!(data.is_empty(), "RAMWR pixel data is a separate transfer");
                self.ram_write_selected = true;
            }
            _ => {}
        }
        Ok(())
    }

    fn apply_pixels(&mut self, wire_bytes: &[u8]) -> Result<()> {
        ensure!(self.reset_high, "ST7789 is held in reset");
        ensure!(self.initialized, "ST7789 is not initialized");
        ensure!(self.ram_write_selected, "RAMWR must precede pixel data");
        let pixels = st7789_wire_to_host_rgb565(wire_bytes)
            .ok_or_else(|| anyhow::anyhow!("ST7789 pixel transfer has an odd byte count"))?;
        let window_width = usize::from(self.column.1 - self.column.0 + 1);
        let window_height = usize::from(self.row.1 - self.row.0 + 1);
        ensure!(
            pixels.len() == window_width * window_height * 2,
            "pixel transfer does not fill the selected address window"
        );

        for index in 0..window_width * window_height {
            let x = self.column.0 + (index % window_width) as u16;
            let y = self.row.0 + (index / window_width) as u16;
            let (x, y) = self.map_orientation(x, y);
            if x >= self.width || y >= self.height {
                bail!("MADCTL maps the address window outside panel GRAM");
            }
            let target = (usize::from(y) * usize::from(self.width) + usize::from(x)) * 2;
            self.gram[target..target + 2].copy_from_slice(&pixels[index * 2..index * 2 + 2]);
        }
        Ok(())
    }

    fn map_orientation(&self, mut x: u16, mut y: u16) -> (u16, u16) {
        // The T-Deck board's 0x55 setup is treated as its documented X+Y
        // mirror. Other values use the standard ST7789 MX/MY/MV bits.
        if self.madctl == T_DECK_MADCTL {
            return (self.width - 1 - x, self.height - 1 - y);
        }
        if self.madctl & 0x20 != 0 {
            std::mem::swap(&mut x, &mut y);
        }
        if self.madctl & 0x40 != 0 {
            x = self.width - 1 - x;
        }
        if self.madctl & 0x80 != 0 {
            y = self.height - 1 - y;
        }
        (x, y)
    }
}

fn parse_window(data: &[u8], limit: u16) -> Result<(u16, u16)> {
    ensure!(
        data.len() == 4,
        "address-window command requires four bytes"
    );
    let start = u16::from_be_bytes([data[0], data[1]]);
    let end = u16::from_be_bytes([data[2], data[3]]);
    ensure!(
        start <= end && end < limit,
        "address window is outside panel"
    );
    Ok((start, end))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::framebuffer::host_rgb565_to_st7789_wire;

    #[test]
    fn models_t_deck_init_orientation_inversion_backlight_and_wire_pixels() {
        let mut panel = St7789Controller::new(4, 3, SharedSpiBus::default());
        assert!(panel.initialize_t_deck().is_err());
        panel.set_reset(false);
        panel.set_reset(true);
        panel.initialize_t_deck().unwrap();
        panel.set_backlight(192);
        assert!(panel.initialized());
        assert_eq!(panel.madctl(), T_DECK_MADCTL);
        assert!(panel.inverted());
        assert_eq!(panel.backlight(), 192);

        panel.write_command(ST7789_CASET, &[0, 0, 0, 1]).unwrap();
        panel.write_command(ST7789_RASET, &[0, 0, 0, 0]).unwrap();
        panel.write_command(ST7789_RAMWR, &[]).unwrap();
        let host: Vec<_> = [0xf800_u16, 0x07e0]
            .into_iter()
            .flat_map(u16::to_ne_bytes)
            .collect();
        panel
            .write_pixels(&host_rgb565_to_st7789_wire(&host).unwrap())
            .unwrap();

        let pixel = |x: usize, y: usize| {
            let offset = (y * 4 + x) * 2;
            u16::from_ne_bytes([
                panel.framebuffer_host_rgb565()[offset],
                panel.framebuffer_host_rgb565()[offset + 1],
            ])
        };
        assert_eq!(pixel(3, 2), 0xf800);
        assert_eq!(pixel(2, 2), 0x07e0);
    }
}
