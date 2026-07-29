use std::fs;
use std::io;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::spiffs::{instance_directory, safe_join};

/// Capacity information for the host filesystem containing a virtual SD card.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct SdCardInfo {
    pub total_bytes: u64,
    pub used_bytes: u64,
    pub free_bytes: u64,
}

/// Emulates an SD card using a host directory.
#[derive(Debug)]
pub struct VirtualSdCard {
    instance_id: String,
    base_path: PathBuf,
    mounted: bool,
}

impl VirtualSdCard {
    pub fn new(instance_id: &str) -> Self {
        let base_path = dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".mycelium")
            .join("instances")
            .join(instance_directory(instance_id))
            .join("sdcard");
        Self {
            instance_id: instance_id.to_owned(),
            base_path,
            mounted: false,
        }
    }

    #[cfg(test)]
    pub(crate) fn at_path(instance_id: &str, base_path: PathBuf) -> Self {
        Self {
            instance_id: instance_id.to_owned(),
            base_path,
            mounted: false,
        }
    }

    pub fn instance_id(&self) -> &str {
        &self.instance_id
    }

    /// Mounts the card, creating its directory if needed.
    pub fn mount(&mut self) -> Result<bool, io::Error> {
        fs::create_dir_all(&self.base_path)?;
        self.mounted = true;
        Ok(true)
    }

    pub fn is_mounted(&self) -> bool {
        self.mounted
    }

    pub fn read_file(&self, path: &str) -> Result<Vec<u8>, io::Error> {
        fs::read(self.resolve(path)?)
    }

    pub fn write_file(&self, path: &str, data: &[u8]) -> Result<(), io::Error> {
        let full_path = self.resolve(path)?;
        if let Some(parent) = full_path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(full_path, data)
    }

    pub fn list_dir(&self, path: &str) -> Result<Vec<String>, io::Error> {
        let mut files = Vec::new();
        for entry in fs::read_dir(self.resolve(path)?)? {
            let entry = entry?;
            if let Some(name) = entry.file_name().to_str() {
                files.push(name.to_owned());
            }
        }
        files.sort_unstable();
        Ok(files)
    }

    /// Reports capacity for the host filesystem containing this card.
    pub fn info(&self) -> SdCardInfo {
        let total_bytes = fs2::total_space(&self.base_path).unwrap_or(0);
        let free_bytes = fs2::available_space(&self.base_path).unwrap_or(0);
        SdCardInfo {
            total_bytes,
            used_bytes: total_bytes.saturating_sub(free_bytes),
            free_bytes,
        }
    }

    fn resolve(&self, path: &str) -> Result<PathBuf, io::Error> {
        if !self.mounted {
            return Err(io::Error::new(
                io::ErrorKind::NotConnected,
                "SD card is not mounted",
            ));
        }
        safe_join(&self.base_path, path)
    }
}
