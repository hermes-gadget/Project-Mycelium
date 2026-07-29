use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::spiffs::{instance_directory, safe_join};

pub const TDECK_LORA_CS_PIN: u8 = 9;
pub const TDECK_SDCARD_CS_PIN: u8 = 39;
pub const SDHC_MAX_CAPACITY: u64 = 32 * 1_024 * 1_024 * 1_024;
pub const FAT32_MAX_VOLUME_SIZE: u64 = 32 * 1_024 * 1_024 * 1_024;
pub const FAT32_MAX_FILE_SIZE: u64 = (4 * 1_024 * 1_024 * 1_024) - 1;
pub const SD_FAST_INIT_HZ: u32 = 4_000_000;
pub const SD_SLOW_INIT_HZ: u32 = 400_000;
/// Wadamesh's cold-card mount ladder: `(settling delay, SPI clock)`.
pub const SD_INIT_LADDER: [(u32, u32); 7] = [
    (40, SD_FAST_INIT_HZ),
    (120, SD_FAST_INIT_HZ),
    (200, SD_FAST_INIT_HZ),
    (300, 1_000_000),
    (450, 1_000_000),
    (650, SD_SLOW_INIT_HZ),
    (900, SD_SLOW_INIT_HZ),
];

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum SdPartitionTable {
    Mbr,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum SdFilesystem {
    Fat32,
}

/// Capacity information for a bounded virtual SD card.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct SdCardInfo {
    pub total_bytes: u64,
    pub used_bytes: u64,
    pub free_bytes: u64,
}

/// Emulates the T-Deck's FAT32 SDHC card on its shared SPI bus.
#[derive(Debug)]
pub struct VirtualSdCard {
    instance_id: String,
    base_path: PathBuf,
    mounted: bool,
    capacity: u64,
    lora_cs_high: bool,
    pub requires_slow_init: bool,
    pub wake_delay_ms: u32,
    last_init_elapsed_ms: u32,
    last_init_frequency_hz: Option<u32>,
}

impl VirtualSdCard {
    pub fn new(instance_id: &str) -> Self {
        let base_path = default_path(instance_id);
        Self::with_path_and_capacity(instance_id, base_path, SDHC_MAX_CAPACITY)
    }

    pub fn with_capacity(instance_id: &str, capacity: u64) -> Self {
        Self::with_path_and_capacity(instance_id, default_path(instance_id), capacity)
    }

    #[cfg(test)]
    pub(crate) fn at_path(instance_id: &str, base_path: PathBuf) -> Self {
        Self::with_path_and_capacity(instance_id, base_path, SDHC_MAX_CAPACITY)
    }

    #[cfg(test)]
    pub(crate) fn at_path_with_capacity(
        instance_id: &str,
        base_path: PathBuf,
        capacity: u64,
    ) -> Self {
        Self::with_path_and_capacity(instance_id, base_path, capacity)
    }

    fn with_path_and_capacity(instance_id: &str, base_path: PathBuf, capacity: u64) -> Self {
        Self {
            instance_id: instance_id.to_owned(),
            base_path,
            mounted: false,
            capacity,
            // The LoRa device is deselected while the bus is idle.
            lora_cs_high: true,
            requires_slow_init: false,
            wake_delay_ms: 0,
            last_init_elapsed_ms: 0,
            last_init_frequency_hz: None,
        }
    }

    pub fn instance_id(&self) -> &str {
        &self.instance_id
    }

    pub const fn chip_select_pin(&self) -> u8 {
        TDECK_SDCARD_CS_PIN
    }

    pub const fn lora_chip_select_pin(&self) -> u8 {
        TDECK_LORA_CS_PIN
    }

    pub const fn partition_table(&self) -> SdPartitionTable {
        SdPartitionTable::Mbr
    }

    pub const fn filesystem(&self) -> SdFilesystem {
        SdFilesystem::Fat32
    }

    pub fn set_lora_cs_high(&mut self, high: bool) {
        self.lora_cs_high = high;
    }

    pub fn lora_cs_high(&self) -> bool {
        self.lora_cs_high
    }

    /// Configures whether this card needs the slow-clock wake-up ladder.
    pub fn set_behavior(&mut self, requires_slow_init: bool, wake_delay_ms: u32) {
        if self.requires_slow_init != requires_slow_init || self.wake_delay_ms != wake_delay_ms {
            self.mounted = false;
        }
        self.requires_slow_init = requires_slow_init;
        self.wake_delay_ms = wake_delay_ms;
    }

    pub fn last_init_elapsed_ms(&self) -> u32 {
        self.last_init_elapsed_ms
    }

    pub fn last_init_frequency_hz(&self) -> Option<u32> {
        self.last_init_frequency_hz
    }

    /// Attempts a fast 4 MHz mount without a retry delay.
    pub fn mount(&mut self) -> Result<bool, io::Error> {
        self.mount_at_frequency(SD_FAST_INIT_HZ, 0)
    }

    /// Simulates one `SD.begin()` attempt after the supplied settling time.
    pub fn mount_at_frequency(
        &mut self,
        clock_hz: u32,
        elapsed_wake_ms: u32,
    ) -> Result<bool, io::Error> {
        self.ensure_bus_available()?;
        self.validate_capacity()?;
        if clock_hz == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "SD initialization clock must be nonzero",
            ));
        }
        self.last_init_elapsed_ms = elapsed_wake_ms;
        self.last_init_frequency_hz = Some(clock_hz);
        if self.requires_slow_init
            && (clock_hz > SD_SLOW_INIT_HZ || elapsed_wake_ms < self.wake_delay_ms)
        {
            self.mounted = false;
            return Ok(false);
        }
        fs::create_dir_all(&self.base_path)?;
        self.mounted = true;
        Ok(true)
    }

    /// Runs the same 4 MHz → 1 MHz → 400 kHz retry ladder as Wadamesh.
    pub fn mount_with_retry_ladder(&mut self) -> Result<bool, io::Error> {
        let mut elapsed_wake_ms = 0_u32;
        for (settle_ms, clock_hz) in SD_INIT_LADDER {
            elapsed_wake_ms = elapsed_wake_ms.saturating_add(settle_ms);
            if self.mount_at_frequency(clock_hz, elapsed_wake_ms)? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub fn unmount(&mut self) {
        self.mounted = false;
    }

    pub fn is_mounted(&self) -> bool {
        self.mounted
    }

    pub fn read_file(&self, path: &str) -> Result<Vec<u8>, io::Error> {
        fs::read(self.resolve(path)?)
    }

    pub fn write_file(&self, path: &str, data: &[u8]) -> Result<(), io::Error> {
        let new_size = u64::try_from(data.len())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "SD file is too large"))?;
        validate_file_size(new_size)?;
        let full_path = self.resolve(path)?;
        let old_size = full_path
            .metadata()
            .map(|metadata| metadata.len())
            .unwrap_or(0);
        let used_without_file = directory_size(&self.base_path)?.saturating_sub(old_size);
        if used_without_file.saturating_add(new_size) > self.capacity {
            return Err(io::Error::new(
                io::ErrorKind::StorageFull,
                "write exceeds SD card capacity",
            ));
        }
        if let Some(parent) = full_path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(full_path, data)
    }

    pub fn create_dir(&self, path: &str) -> Result<(), io::Error> {
        fs::create_dir_all(self.resolve(path)?)
    }

    pub fn exists(&self, path: &str) -> Result<bool, io::Error> {
        Ok(self.resolve(path)?.exists())
    }

    pub fn remove_file(&self, path: &str) -> Result<(), io::Error> {
        fs::remove_file(self.resolve(path)?)
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

    pub fn info(&self) -> Result<SdCardInfo, io::Error> {
        self.ensure_ready()?;
        let used_bytes = directory_size(&self.base_path)?.min(self.capacity);
        Ok(SdCardInfo {
            total_bytes: self.capacity,
            used_bytes,
            free_bytes: self.capacity.saturating_sub(used_bytes),
        })
    }

    fn resolve(&self, path: &str) -> Result<PathBuf, io::Error> {
        self.ensure_ready()?;
        safe_join(&self.base_path, path)
    }

    fn ensure_ready(&self) -> Result<(), io::Error> {
        if !self.mounted {
            return Err(io::Error::new(
                io::ErrorKind::NotConnected,
                "SD card is not mounted",
            ));
        }
        self.ensure_bus_available()
    }

    fn ensure_bus_available(&self) -> Result<(), io::Error> {
        if self.lora_cs_high {
            Ok(())
        } else {
            Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "GPIO9 LoRa CS must be HIGH before SD access",
            ))
        }
    }

    fn validate_capacity(&self) -> Result<(), io::Error> {
        if self.capacity == 0
            || self.capacity > SDHC_MAX_CAPACITY
            || self.capacity > FAT32_MAX_VOLUME_SIZE
        {
            Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "SDHC FAT32 volumes must be between 1 byte and 32 GiB",
            ))
        } else {
            Ok(())
        }
    }
}

fn default_path(instance_id: &str) -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".mycelium")
        .join("instances")
        .join(instance_directory(instance_id))
        .join("sdcard")
}

fn validate_file_size(size: u64) -> Result<(), io::Error> {
    if size > FAT32_MAX_FILE_SIZE {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "FAT32 files cannot exceed 4 GiB minus one byte",
        ))
    } else {
        Ok(())
    }
}

fn directory_size(path: &Path) -> Result<u64, io::Error> {
    fs::read_dir(path)?.try_fold(0_u64, |used, entry| {
        let entry = entry?;
        let metadata = entry.metadata()?;
        if metadata.is_dir() {
            Ok(used.saturating_add(directory_size(&entry.path())?))
        } else if metadata.is_file() {
            Ok(used.saturating_add(metadata.len()))
        } else {
            Ok(used)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::{validate_file_size, FAT32_MAX_FILE_SIZE};

    #[test]
    fn fat32_rejects_files_larger_than_four_gibibytes_minus_one() {
        assert!(validate_file_size(FAT32_MAX_FILE_SIZE).is_ok());
        assert_eq!(
            validate_file_size(FAT32_MAX_FILE_SIZE + 1)
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::InvalidInput
        );
    }
}
