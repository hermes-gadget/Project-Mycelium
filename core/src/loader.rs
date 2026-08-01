use std::ffi::{c_void, CString};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::Path;
use std::ptr::NonNull;

use anyhow::{anyhow, bail, Context, Result};
use libloading::Library;
use mycelium_display::{capture_managed_rgb565, lvgl_v9, with_firmware_library, LvglVersion};

type FirmwareSetupFn = unsafe extern "C" fn();
type FirmwareLoopFn = unsafe extern "C" fn();
type FirmwareGetDisplayFn = unsafe extern "C" fn() -> *mut c_void;
type FirmwareCreateFn = unsafe extern "C" fn(*const std::ffi::c_char) -> *mut c_void;
type FirmwareContextLoopFn = unsafe extern "C" fn(*mut c_void);
type FirmwareContextGetDisplayFn = unsafe extern "C" fn(*mut c_void) -> *mut c_void;
type FirmwareDestroyFn = unsafe extern "C" fn(*mut c_void);

enum FirmwareAbi {
    Legacy {
        setup: FirmwareSetupFn,
        loop_fn: FirmwareLoopFn,
        get_display: Option<FirmwareGetDisplayFn>,
    },
    Contextful {
        create: FirmwareCreateFn,
        loop_fn: FirmwareContextLoopFn,
        get_display: Option<FirmwareContextGetDisplayFn>,
        destroy: FirmwareDestroyFn,
    },
}

impl FirmwareAbi {
    fn is_contextful(&self) -> bool {
        matches!(self, Self::Contextful { .. })
    }
}

/// Resolve the first available symbol from a compatibility-ordered list.
///
/// The copied function pointer is only used while the corresponding `Library`
/// is retained by `FirmwareInstance`.
unsafe fn optional_symbol<T: Copy>(library: &Library, names: &[&[u8]]) -> Option<T> {
    names.iter().find_map(|name| {
        // SAFETY: The caller supplies the symbol's exact function-pointer type
        // and the library outlives every copied pointer returned here.
        unsafe { library.get::<T>(name).ok().map(|symbol| *symbol) }
    })
}

unsafe fn required_symbol<T: Copy>(
    library: &Library,
    names: &[&[u8]],
    description: &str,
) -> Result<T> {
    // SAFETY: See `optional_symbol`; the returned pointer is retained with the
    // owning library.
    unsafe { optional_symbol(library, names) }
        .with_context(|| format!("firmware is missing required symbol {description}"))
}

/// A loaded firmware shared object and its lifecycle entry points.
///
/// The library handle is retained for the lifetime of the copied function
/// pointers, so none of the entry points can outlive the loaded code.
pub struct FirmwareInstance {
    name: String,
    abi: FirmwareAbi,
    context: Option<NonNull<c_void>>,
    running: bool,
    stopped: bool,
    lib: Library,
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

            // The v2 contract is deliberately detected by its create symbol.
            // A contextful firmware may use either the explicit `_v2` names or
            // the unversioned names reserved for the contextful contract. The
            // old three-function ABI remains available for one-node firmware.
            let contextful_create = optional_symbol::<FirmwareCreateFn>(
                &lib,
                &[b"firmware_create_v2\0", b"firmware_create\0"],
            );
            let abi = if let Some(create) = contextful_create {
                let loop_fn = required_symbol::<FirmwareContextLoopFn>(
                    &lib,
                    &[b"firmware_loop_v2\0", b"firmware_loop\0"],
                    "contextful firmware_loop",
                )?;
                let destroy = required_symbol::<FirmwareDestroyFn>(
                    &lib,
                    &[b"firmware_destroy_v2\0", b"firmware_destroy\0"],
                    "contextful firmware_destroy",
                )?;
                let get_display = optional_symbol::<FirmwareContextGetDisplayFn>(
                    &lib,
                    &[b"firmware_get_display_v2\0", b"firmware_get_display\0"],
                );
                FirmwareAbi::Contextful {
                    create,
                    loop_fn,
                    get_display,
                    destroy,
                }
            } else {
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
                FirmwareAbi::Legacy {
                    setup,
                    loop_fn,
                    get_display,
                }
            };

            Ok(Self {
                name: name.to_owned(),
                abi,
                context: None,
                running: false,
                stopped: false,
                lib,
            })
        }
    }

    fn invoke<T>(&self, operation: &str, call: impl FnOnce() -> T) -> Result<T> {
        catch_unwind(AssertUnwindSafe(|| with_firmware_library(&self.lib, call)))
            .map_err(|_| anyhow!("firmware {operation} panicked"))
    }

    /// Calls the firmware's setup/create entry point exactly once.
    pub fn start(&mut self) -> Result<()> {
        if self.running {
            return Ok(());
        }
        if self.stopped {
            bail!("firmware instance {} has already been destroyed", self.name);
        }

        let context = match &self.abi {
            FirmwareAbi::Legacy { setup, .. } => {
                // SAFETY: The symbol was resolved with this signature during
                // loading and its library remains owned by this instance.
                self.invoke("firmware_setup", || unsafe { setup() })?;
                None
            }
            FirmwareAbi::Contextful { create, .. } => {
                let instance_id = CString::new(self.name.as_bytes())
                    .context("firmware instance ID contains an interior NUL")?;
                // SAFETY: The symbol was resolved with this signature during
                // loading; `instance_id` remains alive for the call.
                let context = self.invoke("firmware_create", || unsafe {
                    create(instance_id.as_ptr())
                })?;
                let Some(context) = NonNull::new(context) else {
                    bail!("firmware_create returned NULL for instance {}", self.name);
                };
                Some(context)
            }
        };
        self.context = context;
        self.running = true;
        Ok(())
    }

    /// Advances the firmware by one frame.
    pub fn tick(&mut self) {
        if !self.running {
            return;
        }

        let result = match (&self.abi, self.context) {
            (FirmwareAbi::Legacy { loop_fn, .. }, _) => {
                // SAFETY: The symbol was resolved with this signature and the
                // owning library remains loaded.
                self.invoke("firmware_loop", || unsafe { loop_fn() })
            }
            (FirmwareAbi::Contextful { loop_fn, .. }, Some(context)) => {
                // SAFETY: The context came from the matching firmware_create
                // call and remains owned by this instance until stop/drop.
                self.invoke("contextful firmware_loop", || unsafe {
                    loop_fn(context.as_ptr())
                })
            }
            (FirmwareAbi::Contextful { .. }, None) => {
                Err(anyhow!("contextful firmware is running without a context"))
            }
        };
        if let Err(error) = result {
            eprintln!(
                "firmware {} stopped after a boundary panic: {error}",
                self.name
            );
            self.running = false;
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn is_running(&self) -> bool {
        self.running
    }

    pub fn is_contextful(&self) -> bool {
        self.abi.is_contextful()
    }

    pub fn abi_name(&self) -> &'static str {
        if self.is_contextful() {
            "contextful v2"
        } else {
            "legacy v1"
        }
    }

    pub fn has_display(&self) -> bool {
        self.display().is_some_and(|display| !display.is_null())
    }

    pub fn display(&self) -> Option<*mut c_void> {
        match (&self.abi, self.context) {
            (FirmwareAbi::Legacy { get_display, .. }, _) => get_display.and_then(|get_display| {
                // SAFETY: The optional symbol was resolved with this signature
                // and its library remains loaded.
                self.invoke("firmware_get_display", || unsafe { get_display() })
                    .ok()
            }),
            (FirmwareAbi::Contextful { get_display, .. }, Some(context)) => {
                get_display.and_then(|get_display| {
                    // SAFETY: The context came from the matching create call.
                    self.invoke("contextful firmware_get_display", || unsafe {
                        get_display(context.as_ptr())
                    })
                    .ok()
                })
            }
            (FirmwareAbi::Contextful { .. }, None) => None,
        }
    }

    pub fn display_version(&self) -> LvglVersion {
        self.display()
            .map_or(LvglVersion::Unknown, LvglVersion::detect)
    }

    pub fn capture_display_rgb565(&self) -> Option<Vec<u8>> {
        let display = self.display()?;
        if display.is_null() {
            return None;
        }
        self.invoke("display capture", || {
            // SAFETY: `display` comes from this firmware and its library stays
            // loaded throughout capture.
            unsafe {
                capture_managed_rgb565(display).or_else(|| lvgl_v9::capture_lvgl_rgb565(display))
            }
        })
        .ok()
        .flatten()
    }

    /// Calls the contextful firmware shutdown hook at most once.
    pub fn stop(&mut self) {
        if self.stopped {
            return;
        }
        let display = self.display();
        let context = self.context.take();
        self.running = false;
        self.stopped = true;

        if let Some((destroy, context)) = match (&self.abi, context) {
            (FirmwareAbi::Contextful { destroy, .. }, Some(context)) => Some((*destroy, context)),
            _ => None,
        } {
            // Clear the context before invoking foreign code so a re-entrant
            // drop cannot destroy the same firmware state twice.
            if let Err(error) =
                self.invoke("firmware_destroy", || unsafe { destroy(context.as_ptr()) })
            {
                eprintln!("firmware {} destroy hook failed: {error}", self.name);
            }
        }

        if let Some(display) = display.filter(|display| !display.is_null()) {
            if let Err(error) = self.invoke("display destroy", || unsafe {
                mycelium_display::destroy_managed_display(display);
            }) {
                eprintln!("firmware {} display destroy failed: {error}", self.name);
            }
        }
    }
}

impl Drop for FirmwareInstance {
    fn drop(&mut self) {
        self.stop();
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
