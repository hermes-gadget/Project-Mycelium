use std::fmt::Write;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Capacity information for the host filesystem containing a virtual SPIFFS.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct SpiffsInfo {
    pub total_bytes: u64,
    pub used_bytes: u64,
    pub free_bytes: u64,
}

/// Emulates an ESP32 SPIFFS partition using a host directory.
#[derive(Debug)]
pub struct VirtualSpiffs {
    instance_id: String,
    base_path: PathBuf,
    mounted: bool,
}

impl VirtualSpiffs {
    pub fn new(instance_id: &str) -> Self {
        let base_path = dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".mycelium")
            .join("instances")
            .join(instance_directory(instance_id))
            .join("spiffs");
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

    /// Mounts the filesystem, creating its directory if needed.
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

    pub fn delete_file(&self, path: &str) -> Result<(), io::Error> {
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

    /// Reports capacity for the host filesystem containing this partition.
    pub fn info(&self) -> SpiffsInfo {
        let total_bytes = fs2::total_space(&self.base_path).unwrap_or(0);
        let free_bytes = fs2::available_space(&self.base_path).unwrap_or(0);
        SpiffsInfo {
            total_bytes,
            used_bytes: total_bytes.saturating_sub(free_bytes),
            free_bytes,
        }
    }

    /// Deletes all partition contents while leaving an empty mounted directory.
    pub fn format(&mut self) -> Result<(), io::Error> {
        if self.base_path.exists() {
            fs::remove_dir_all(&self.base_path)?;
        }
        fs::create_dir_all(&self.base_path)?;
        self.mounted = true;
        Ok(())
    }

    fn resolve(&self, path: &str) -> Result<PathBuf, io::Error> {
        if !self.mounted {
            return Err(io::Error::new(
                io::ErrorKind::NotConnected,
                "SPIFFS is not mounted",
            ));
        }
        safe_join(&self.base_path, path)
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
