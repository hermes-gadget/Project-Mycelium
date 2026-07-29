use std::io;

use crate::{VirtualSdCard, VirtualSpiffs};

/// Owns all virtual storage attached to one emulator instance.
#[derive(Debug)]
pub struct StorageManager {
    pub spiffs: VirtualSpiffs,
    pub sdcard: VirtualSdCard,
}

impl StorageManager {
    pub fn new(instance_id: &str) -> Self {
        Self {
            spiffs: VirtualSpiffs::new(instance_id),
            sdcard: VirtualSdCard::new(instance_id),
        }
    }

    pub fn init_all(&mut self) -> Result<(), io::Error> {
        self.spiffs.mount()?;
        self.sdcard.mount()?;
        Ok(())
    }

    pub fn unmount_all(&mut self) {
        self.spiffs.unmount();
        self.sdcard.unmount();
    }
}
