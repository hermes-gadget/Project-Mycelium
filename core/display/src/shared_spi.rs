use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};

use anyhow::{bail, Result};

/// Devices which share the T-Deck SPI host.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SpiDevice {
    Display,
    Sx1262,
    SdCard,
}

/// Small arbitration model for the T-Deck's shared SPI host.
///
/// It deliberately models chip-select ownership rather than timing. Callers
/// obtain the bus for a `transaction()` and must release it before another
/// device can take ownership — matching the real hardware requirement that
/// only one device drive the shared SCLK/MISO/MOSI lines at a time.
#[derive(Clone, Default)]
pub struct SharedSpiBus {
    owner: Arc<Mutex<Option<SpiDevice>>>,
}

static INSTANCE_SPI_BUSES: LazyLock<Mutex<HashMap<String, SharedSpiBus>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

thread_local! {
    /// The instance context used by legacy display constructors which do not
    /// yet carry the ID into the SPI helper directly.
    static CURRENT_SPI_INSTANCE: RefCell<Option<String>> = const { RefCell::new(None) };
    static LEGACY_SPI_BUS: SharedSpiBus = SharedSpiBus::default();
}

/// Selects the SPI arbiter used by compatibility display constructors on the
/// current thread. Instance-aware callers should prefer
/// [`spi_bus_for_instance`] and pass the returned bus explicitly.
pub fn set_current_instance(instance_id: &str) {
    CURRENT_SPI_INSTANCE.with(|current| {
        *current.borrow_mut() = Some(instance_id.to_owned());
    });
}

/// Returns the independent SPI arbiter for one virtual board instance.
pub fn spi_bus_for_instance(instance_id: &str) -> SharedSpiBus {
    let mut buses = INSTANCE_SPI_BUSES
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    buses.entry(instance_id.to_owned()).or_default().clone()
}

/// Removes an instance's registry entry after all of its device handles have
/// been destroyed. Existing clones remain valid until their final owner drops.
pub fn remove_instance(instance_id: &str) {
    INSTANCE_SPI_BUSES
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .remove(instance_id);
}

/// Returns the bus selected by the current instance context.
pub fn global_spi_bus() -> SharedSpiBus {
    let instance_id = CURRENT_SPI_INSTANCE.with(|current| current.borrow().clone());
    if let Some(instance_id) = instance_id {
        return spi_bus_for_instance(&instance_id);
    }
    LEGACY_SPI_BUS.with(Clone::clone)
}

impl SharedSpiBus {
    pub fn transaction<T>(&self, device: SpiDevice, transfer: impl FnOnce() -> T) -> Result<T> {
        {
            let mut owner = self.owner.lock().unwrap_or_else(|lock| lock.into_inner());
            if let Some(active) = *owner {
                bail!("shared SPI bus is owned by {active:?}");
            }
            *owner = Some(device);
        }
        let _guard = TransactionGuard {
            owner: Arc::clone(&self.owner),
        };
        Ok(transfer())
    }

    pub fn owner(&self) -> Option<SpiDevice> {
        *self.owner.lock().unwrap_or_else(|lock| lock.into_inner())
    }
}

struct TransactionGuard {
    owner: Arc<Mutex<Option<SpiDevice>>>,
}

impl Drop for TransactionGuard {
    fn drop(&mut self) {
        *self.owner.lock().unwrap_or_else(|lock| lock.into_inner()) = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_overlapping_devices_and_releases_after_transaction() {
        let bus = SharedSpiBus::default();
        bus.transaction(SpiDevice::Display, || {
            assert_eq!(bus.owner(), Some(SpiDevice::Display));
            assert!(bus.transaction(SpiDevice::SdCard, || ()).is_err());
        })
        .unwrap();
        assert_eq!(bus.owner(), None);
        bus.transaction(SpiDevice::Sx1262, || ()).unwrap();
    }

    #[test]
    fn instance_buses_do_not_contend_across_virtual_boards() {
        let first = spi_bus_for_instance("spi-isolation-first");
        let second = spi_bus_for_instance("spi-isolation-second");

        first
            .transaction(SpiDevice::Display, || {
                assert_eq!(first.owner(), Some(SpiDevice::Display));
                assert!(second.transaction(SpiDevice::SdCard, || ()).is_ok());
            })
            .unwrap();
        assert_eq!(second.owner(), None);

        remove_instance("spi-isolation-first");
        remove_instance("spi-isolation-second");
    }

    #[test]
    fn compatibility_bus_follows_the_current_instance_context() {
        set_current_instance("spi-context-first");
        let first = global_spi_bus();
        set_current_instance("spi-context-second");
        let second = global_spi_bus();

        first
            .transaction(SpiDevice::Display, || {
                assert_eq!(first.owner(), Some(SpiDevice::Display));
                assert_eq!(second.owner(), None);
            })
            .unwrap();

        remove_instance("spi-context-first");
        remove_instance("spi-context-second");
    }
}
