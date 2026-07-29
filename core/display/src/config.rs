use crate::LvglVersion;

pub const T_DECK_WIDTH: u32 = 320;
pub const T_DECK_HEIGHT: u32 = 240;
pub const DEFAULT_DRAW_BUFFER_ROWS: u32 = 24;

/// Firmware-facing display backend options.
#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DisplayBackendOptions {
    /// Number of full-width RGB565 rows allocated for LVGL partial rendering.
    pub draw_buffer_rows: u32,
    /// Enable the optional ST7789 command/SPI fidelity model.
    pub st7789_fidelity: bool,
}

impl Default for DisplayBackendOptions {
    fn default() -> Self {
        Self {
            draw_buffer_rows: DEFAULT_DRAW_BUFFER_ROWS,
            st7789_fidelity: false,
        }
    }
}

impl DisplayBackendOptions {
    pub(crate) fn validated(self, height: u32) -> Option<Self> {
        (self.draw_buffer_rows > 0 && self.draw_buffer_rows <= height).then_some(self)
    }
}

/// Configuration shared by the LVGL v8 and v9 display backends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisplayConfig {
    pub width: u32,
    pub height: u32,
    pub scale: u32,
    pub lvgl_version: LvglVersion,
    pub window_title: String,
    pub show_fps: bool,
}

impl Default for DisplayConfig {
    fn default() -> Self {
        Self {
            width: T_DECK_WIDTH,
            height: T_DECK_HEIGHT,
            scale: 2,
            lvgl_version: LvglVersion::V9,
            window_title: "T-Deck".to_string(),
            show_fps: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn t_deck_defaults_target_lvgl_v9() {
        let config = DisplayConfig::default();
        assert_eq!((config.width, config.height, config.scale), (320, 240, 2));
        assert_eq!(config.lvgl_version, LvglVersion::V9);
        assert_eq!(config.window_title, "T-Deck");
        assert!(!config.show_fps);
    }

    #[test]
    fn t_deck_geometry_is_fixed() {
        assert_eq!((T_DECK_WIDTH, T_DECK_HEIGHT), (320, 240));
    }

    #[test]
    fn backend_defaults_use_a_partial_t_deck_buffer() {
        let options = DisplayBackendOptions::default();
        assert_eq!(options.draw_buffer_rows, 24);
        assert!(options.draw_buffer_rows < T_DECK_HEIGHT);
        assert!(!options.st7789_fidelity);
    }
}
