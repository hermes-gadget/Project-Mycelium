use std::ffi::c_void;
use std::path::Path;

use anyhow::{Context, Result};
use libloading::Library;

type FirmwareSetupFn = unsafe extern "C" fn();
type FirmwareLoopFn = unsafe extern "C" fn();
type FirmwareGetDisplayFn = unsafe extern "C" fn() -> *mut c_void;

/// A loaded firmware shared object and its lifecycle entry points.
///
/// The library handle is retained for the lifetime of the copied function
/// pointers, so none of the entry points can outlive the loaded code.
pub struct FirmwareInstance {
    name: String,
    setup: FirmwareSetupFn,
    loop_fn: FirmwareLoopFn,
    get_display: Option<FirmwareGetDisplayFn>,
    running: bool,
    _lib: Library,
}

impl FirmwareInstance {
    pub fn load(name: &str, so_path: &Path) -> Result<Self> {
        // SAFETY: Loading native firmware is the purpose of this boundary.
        // Each resolved symbol is copied as a function pointer while `lib` is
        // retained in the returned instance, keeping the code loaded.
        unsafe {
            let lib = Library::new(so_path).with_context(|| {
                format!("failed to load firmware library {}", so_path.display())
            })?;
            let setup = *lib
                .get::<FirmwareSetupFn>(b"firmware_setup\0")
                .context("firmware is missing required symbol firmware_setup")?;
            let loop_fn = *lib
                .get::<FirmwareLoopFn>(b"firmware_loop\0")
                .context("firmware is missing required symbol firmware_loop")?;
            let get_display = lib
                .get::<FirmwareGetDisplayFn>(b"firmware_get_display\0")
                .ok()
                .map(|symbol| *symbol);

            Ok(Self {
                name: name.to_owned(),
                setup,
                loop_fn,
                get_display,
                running: false,
                _lib: lib,
            })
        }
    }

    /// Calls the firmware's setup entry point exactly once.
    pub fn start(&mut self) {
        if self.running {
            return;
        }

        // SAFETY: The symbol was resolved with this signature during loading
        // and its library remains owned by this instance.
        unsafe { (self.setup)() };
        self.running = true;
    }

    /// Advances the firmware by one frame.
    pub fn tick(&mut self) {
        if !self.running {
            return;
        }

        // SAFETY: The symbol was resolved with this signature and the owning
        // library remains loaded.
        unsafe { (self.loop_fn)() };
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn is_running(&self) -> bool {
        self.running
    }

    pub fn has_display(&self) -> bool {
        self.display().is_some_and(|display| !display.is_null())
    }

    pub fn display(&self) -> Option<*mut c_void> {
        self.get_display.map(|get_display| {
            // SAFETY: The optional symbol was resolved with this signature and
            // its library remains loaded.
            unsafe { get_display() }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loading_a_missing_library_reports_its_path() {
        let path = std::path::PathBuf::from("/definitely/not/a/firmware-library.so");
        let error = FirmwareInstance::load("missing", &path)
            .err()
            .expect("loading should fail");

        assert!(error.to_string().contains(path.to_str().unwrap()));
    }
}
