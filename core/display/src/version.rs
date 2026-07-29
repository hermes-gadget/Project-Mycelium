use std::ffi::c_void;

/// LVGL ABI used by a firmware display.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LvglVersion {
    V8,
    V9,
    Unknown,
}

impl LvglVersion {
    /// Detects a registered Mycelium LVGL v8 display.
    ///
    /// Null means no display. An ambiguous non-null handle defaults to v9,
    /// which is Mycelium's primary LVGL target.
    ///
    pub fn detect(display_handle: *mut c_void) -> Self {
        if display_handle.is_null() {
            return Self::Unknown;
        }
        if crate::lvgl_v8::is_v8_display(display_handle) {
            return Self::V8;
        }

        Self::V9
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn null_is_unknown_and_ambiguous_handles_default_to_v9() {
        let mut ambiguous = 0_u8;

        assert_eq!(
            LvglVersion::detect(std::ptr::null_mut()),
            LvglVersion::Unknown
        );
        assert_eq!(
            LvglVersion::detect((&mut ambiguous as *mut u8).cast()),
            LvglVersion::V9
        );
    }
}
