use crate::LvglVersion;

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
            width: 320,
            height: 240,
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
}
