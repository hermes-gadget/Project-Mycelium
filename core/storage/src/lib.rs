//! Persistent storage emulation.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

#[derive(Clone, Debug)]
pub struct StorageManager {
    root: PathBuf,
}

impl StorageManager {
    pub fn new(instance_id: &str) -> Self {
        let base = std::env::var_os("MYCELIUM_STORAGE_ROOT")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME")
                    .map(PathBuf::from)
                    .map(|home| home.join(".mycelium").join("instances"))
            })
            .unwrap_or_else(|| std::env::temp_dir().join("mycelium").join("instances"));
        Self {
            root: base.join(instance_id),
        }
    }

    pub fn init_all(&self) -> Result<()> {
        for name in ["spiffs", "sdcard"] {
            let path = self.root.join(name);
            std::fs::create_dir_all(&path)
                .with_context(|| format!("failed to initialize storage at {}", path.display()))?;
        }
        Ok(())
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn spiffs_path(&self) -> PathBuf {
        self.root.join("spiffs")
    }

    pub fn sdcard_path(&self) -> PathBuf {
        self.root.join("sdcard")
    }
}
