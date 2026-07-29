use std::sync::{Arc, Mutex};

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
/// It deliberately models chip-select ownership rather than timing. Radio and
/// SD backends do not yet route bytes through this type; they can share an
/// instance when those hardware shims gain transaction-level SPI fidelity.
#[derive(Clone, Default)]
pub struct SharedSpiBus {
    owner: Arc<Mutex<Option<SpiDevice>>>,
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
