use std::collections::hash_map::DefaultHasher;
use std::fmt::Write;
use std::fs;
use std::hash::{Hash, Hasher};
use std::io;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Size of the SPIFFS partition in the T-Deck 16 MB partition table.
pub const DEFAULT_SPIFFS_PARTITION_SIZE: u64 = 0x36_0000;
/// ESP32 SPIFFS erase block size.
pub const DEFAULT_SPIFFS_BLOCK_SIZE: u64 = 4_096;
/// SPIFFS limits the complete object name, excluding an optional leading slash.
pub const SPIFFS_MAX_FILENAME_CHARS: usize = 32;

/// Capacity information for a virtual SPIFFS partition.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct SpiffsInfo {
    pub total_bytes: u64,
    pub used_bytes: u64,
    pub free_bytes: u64,
}

/// Emulates the flat, bounded SPIFFS partition on an ESP32.
#[derive(Debug)]
pub struct VirtualSpiffs {
    instance_id: String,
    base_path: PathBuf,
    mounted: bool,
    partition_size: u64,
    block_size: u64,
    write_cycles: Vec<u64>,
}

impl VirtualSpiffs {
    pub fn new(instance_id: &str) -> Self {
        let base_path = dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".mycelium")
            .join("instances")
            .join(instance_directory(instance_id))
            .join("spiffs");
        Self::with_path_and_capacity(
            instance_id,
            base_path,
            DEFAULT_SPIFFS_PARTITION_SIZE,
            DEFAULT_SPIFFS_BLOCK_SIZE,
        )
    }

    /// Creates a SPIFFS emulator with a caller-selected partition and erase block size.
    pub fn with_capacity(instance_id: &str, partition_size: u64, block_size: u64) -> Self {
        let base_path = dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".mycelium")
            .join("instances")
            .join(instance_directory(instance_id))
            .join("spiffs");
        Self::with_path_and_capacity(instance_id, base_path, partition_size, block_size)
    }

    #[cfg(test)]
    pub(crate) fn at_path(instance_id: &str, base_path: PathBuf) -> Self {
        Self::with_path_and_capacity(
            instance_id,
            base_path,
            DEFAULT_SPIFFS_PARTITION_SIZE,
            DEFAULT_SPIFFS_BLOCK_SIZE,
        )
    }

    #[cfg(test)]
    pub(crate) fn at_path_with_capacity(
        instance_id: &str,
        base_path: PathBuf,
        partition_size: u64,
        block_size: u64,
    ) -> Self {
        Self::with_path_and_capacity(instance_id, base_path, partition_size, block_size)
    }

    fn with_path_and_capacity(
        instance_id: &str,
        base_path: PathBuf,
        partition_size: u64,
        block_size: u64,
    ) -> Self {
        let block_count = if block_size == 0 {
            0
        } else {
            partition_size.div_ceil(block_size)
        };
        Self {
            instance_id: instance_id.to_owned(),
            base_path,
            mounted: false,
            partition_size,
            block_size,
            write_cycles: vec![0; usize::try_from(block_count).unwrap_or(0)],
        }
    }

    pub fn instance_id(&self) -> &str {
        &self.instance_id
    }

    /// Mounts the filesystem, creating its one host backing directory if needed.
    pub fn mount(&mut self) -> Result<bool, io::Error> {
        if self.partition_size == 0 || self.block_size == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "SPIFFS partition and block sizes must be non-zero",
            ));
        }
        fs::create_dir_all(&self.base_path)?;
        self.mounted = true;
        Ok(true)
    }

    pub fn unmount(&mut self) {
        self.mounted = false;
    }

    pub fn is_mounted(&self) -> bool {
        self.mounted
    }

    pub fn read_file(&self, path: &str) -> Result<Vec<u8>, io::Error> {
        fs::read(self.resolve_file(path)?)
    }

    pub fn write_file(&mut self, path: &str, data: &[u8]) -> Result<(), io::Error> {
        let (filename, full_path) = self.resolve_filename(path)?;
        let old_size = full_path
            .metadata()
            .map(|metadata| metadata.len())
            .unwrap_or(0);
        let used_without_file = self.used_bytes()?.saturating_sub(old_size);
        let new_size = u64::try_from(data.len())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "SPIFFS file is too large"))?;
        if used_without_file.saturating_add(new_size) > self.partition_size {
            return Err(io::Error::new(
                io::ErrorKind::StorageFull,
                "write exceeds SPIFFS partition capacity",
            ));
        }

        // SPIFFS is flat: no parent directory is created here.
        fs::write(full_path, data)?;
        self.record_write(filename, old_size.max(new_size));
        Ok(())
    }

    pub fn delete_file(&mut self, path: &str) -> Result<(), io::Error> {
        let (filename, full_path) = self.resolve_filename(path)?;
        let old_size = full_path.metadata()?.len();
        fs::remove_file(full_path)?;
        self.record_write(filename, old_size);
        Ok(())
    }

    pub fn list_dir(&self, path: &str) -> Result<Vec<String>, io::Error> {
        self.ensure_mounted()?;
        if !matches!(path, "" | "/") {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "SPIFFS is flat and only its root can be listed",
            ));
        }
        let mut files = Vec::new();
        for entry in fs::read_dir(&self.base_path)? {
            let entry = entry?;
            if entry.file_type()?.is_file() {
                if let Some(name) = entry.file_name().to_str() {
                    files.push(name.to_owned());
                }
            }
        }
        files.sort_unstable();
        Ok(files)
    }

    pub fn info(&self) -> SpiffsInfo {
        let used_bytes = self.used_bytes().unwrap_or(0).min(self.partition_size);
        SpiffsInfo {
            total_bytes: self.partition_size,
            used_bytes,
            free_bytes: self.partition_size.saturating_sub(used_bytes),
        }
    }

    /// Returns the tracked erase/write count for one physical partition block.
    pub fn block_write_cycles(&self, block: usize) -> Option<u64> {
        self.write_cycles.get(block).copied()
    }

    pub fn block_count(&self) -> usize {
        self.write_cycles.len()
    }

    pub fn total_write_cycles(&self) -> u64 {
        self.write_cycles.iter().sum()
    }

    /// Deletes all partition contents and resets wear counters.
    pub fn format(&mut self) -> Result<(), io::Error> {
        if self.base_path.exists() {
            fs::remove_dir_all(&self.base_path)?;
        }
        fs::create_dir_all(&self.base_path)?;
        self.write_cycles.fill(0);
        self.mounted = true;
        Ok(())
    }

    fn ensure_mounted(&self) -> Result<(), io::Error> {
        if self.mounted {
            Ok(())
        } else {
            Err(io::Error::new(
                io::ErrorKind::NotConnected,
                "SPIFFS is not mounted",
            ))
        }
    }

    fn resolve_file(&self, path: &str) -> Result<PathBuf, io::Error> {
        self.resolve_filename(path).map(|(_, path)| path)
    }

    fn resolve_filename<'a>(&self, path: &'a str) -> Result<(&'a str, PathBuf), io::Error> {
        self.ensure_mounted()?;
        let filename = path.strip_prefix('/').unwrap_or(path);
        if filename.is_empty() || filename.contains(['/', '\\']) || matches!(filename, "." | "..") {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "SPIFFS paths must contain one flat filename",
            ));
        }
        if filename.chars().count() > SPIFFS_MAX_FILENAME_CHARS {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "SPIFFS filenames are limited to 32 characters",
            ));
        }
        Ok((filename, self.base_path.join(filename)))
    }

    fn used_bytes(&self) -> Result<u64, io::Error> {
        self.ensure_mounted()?;
        fs::read_dir(&self.base_path)?.try_fold(0_u64, |used, entry| {
            let entry = entry?;
            let metadata = entry.metadata()?;
            Ok(used.saturating_add(if metadata.is_file() {
                metadata.len()
            } else {
                0
            }))
        })
    }

    fn record_write(&mut self, filename: &str, changed_bytes: u64) {
        if self.write_cycles.is_empty() {
            return;
        }
        let mut hasher = DefaultHasher::new();
        filename.hash(&mut hasher);
        let start = usize::try_from(hasher.finish()).unwrap_or(0) % self.write_cycles.len();
        let touched = changed_bytes.max(1).div_ceil(self.block_size);
        for offset in 0..usize::try_from(touched).unwrap_or(self.write_cycles.len()) {
            let block = (start + offset) % self.write_cycles.len();
            self.write_cycles[block] = self.write_cycles[block].saturating_add(1);
        }
    }
}

pub(crate) fn safe_join(base_path: &Path, path: &str) -> Result<PathBuf, io::Error> {
    let relative = path.trim_start_matches(['/', '\\']);
    let path = Path::new(relative);
    if path
        .components()
        .any(|component| !matches!(component, Component::Normal(_) | Component::CurDir))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "storage path must stay inside the instance directory",
        ));
    }
    Ok(base_path.join(path))
}

pub(crate) fn instance_directory(instance_id: &str) -> String {
    if !instance_id.is_empty()
        && instance_id != "."
        && instance_id != ".."
        && instance_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return instance_id.to_owned();
    }

    let mut encoded = String::new();
    for byte in instance_id.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_') {
            encoded.push(char::from(byte));
        } else {
            write!(&mut encoded, "%{byte:02X}").expect("writing to a String cannot fail");
        }
    }
    if encoded.is_empty() {
        "%00".to_owned()
    } else {
        encoded
    }
}
