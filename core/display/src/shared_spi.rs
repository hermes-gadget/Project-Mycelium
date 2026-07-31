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

/// A single global bus shared by all virtual T-Deck instances.
///
/// The real T-Deck has exactly one SPI host; this global instance models
/// that constraint so that simultaneous SD + LoRa + display access is
/// detectable and rejectable.
static GLOBAL_SPI_BUS: LazyLock<SharedSpiBus> = LazyLock::new(SharedSpiBus::default);

/// Returns a reference to the global SPI bus shared by all instances.
pub fn global_spi_bus() -> &'static SharedSpiBus {
    &GLOBAL_SPI_BUS
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
}
