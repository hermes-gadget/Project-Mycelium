use std::ffi::c_void;

pub(crate) const DISPLAY_MAGIC: u64 = 0x4d59_4345_4c49_554d;
const LVGL_V8_ABI: u32 = 8;
const LVGL_V9_ABI: u32 = 9;

/// LVGL ABI used by a firmware display.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LvglVersion {
    V8,
    V9,
    Unknown,
}

#[repr(C)]
pub(crate) struct DisplayHeader {
    magic: u64,
    abi_version: u32,
}

impl DisplayHeader {
    pub(crate) fn new(version: LvglVersion) -> Self {
        let abi_version = match version {
            LvglVersion::V8 => LVGL_V8_ABI,
            LvglVersion::V9 | LvglVersion::Unknown => LVGL_V9_ABI,
        };
        Self {
            magic: DISPLAY_MAGIC,
            abi_version,
        }
    }

    pub(crate) fn is_mycelium_handle(&self) -> bool {
        self.magic == DISPLAY_MAGIC
    }
}

impl LvglVersion {
    /// Detects the ABI marker at the start of a Mycelium display handle.
    ///
    /// Null means no display. An ambiguous non-null handle defaults to v9,
    /// which is Mycelium's primary LVGL target.
    ///
    /// The caller must only pass null or a pointer readable for at least the
    /// size of Mycelium's internal display header.
    pub fn detect(display_handle: *mut c_void) -> Self {
        if display_handle.is_null() {
            return Self::Unknown;
        }

        let header = unsafe { &*display_handle.cast::<DisplayHeader>() };
        if header.magic == DISPLAY_MAGIC {
            match header.abi_version {
                LVGL_V8_ABI => return Self::V8,
                LVGL_V9_ABI => return Self::V9,
                _ => {}
            }
        }
        Self::V9
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[repr(C)]
    struct MockDisplay {
        header: DisplayHeader,
        version_specific_field: usize,
    }

    #[test]
    fn detects_mock_v8_and_v9_handles() {
        let mut v8 = MockDisplay {
            header: DisplayHeader::new(LvglVersion::V8),
            version_specific_field: 8,
        };
        let mut v9 = MockDisplay {
            header: DisplayHeader::new(LvglVersion::V9),
            version_specific_field: 9,
        };

        assert_eq!(
            LvglVersion::detect((&mut v8 as *mut MockDisplay).cast()),
            LvglVersion::V8
        );
        assert_eq!(
            LvglVersion::detect((&mut v9 as *mut MockDisplay).cast()),
            LvglVersion::V9
        );
    }

    #[test]
    fn null_is_unknown_and_ambiguous_handles_default_to_v9() {
        let mut ambiguous = MockDisplay {
            header: DisplayHeader {
                magic: 0,
                abi_version: 0,
            },
            version_specific_field: 0,
        };

        assert_eq!(
            LvglVersion::detect(std::ptr::null_mut()),
            LvglVersion::Unknown
        );
        assert_eq!(
            LvglVersion::detect((&mut ambiguous as *mut MockDisplay).cast()),
            LvglVersion::V9
        );
    }
}
