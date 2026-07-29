use std::collections::HashMap;
use std::path::Path;

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};

use crate::loader::FirmwareInstance;

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct InstanceConfig {
    /// An optional caller-selected ID. When absent, `nodeN` is generated.
    #[serde(default)]
    pub instance_id: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct InstanceInfo {
    pub id: String,
    pub running: bool,
    pub has_display: bool,
}

pub struct InstanceManager {
    instances: HashMap<String, FirmwareInstance>,
    next_id: u64,
}

impl InstanceManager {
    pub fn new() -> Self {
        Self {
            instances: HashMap::new(),
            next_id: 1,
        }
    }

    pub fn spawn(&mut self, firmware_path: &Path, config: InstanceConfig) -> Result<String> {
        let id = match config.instance_id {
            Some(id) => {
                if id.is_empty() {
                    bail!("instance ID cannot be empty");
                }
                id
            }
            None => self.next_available_id(),
        };

        if self.instances.contains_key(&id) {
            bail!("instance {id} already exists");
        }

        let mut instance = FirmwareInstance::load(&id, firmware_path)?;
        instance.start();
        self.instances.insert(id.clone(), instance);
        Ok(id)
    }

    pub fn kill(&mut self, id: &str) -> Result<()> {
        if self.instances.remove(id).is_none() {
            bail!("instance {id} does not exist");
        }
        Ok(())
    }

    pub fn tick_all(&mut self) {
        for instance in self.instances.values_mut() {
            instance.tick();
        }
    }

    pub fn list(&self) -> Vec<InstanceInfo> {
        let mut instances: Vec<_> = self
            .instances
            .iter()
            .map(|(id, instance)| InstanceInfo {
                id: id.clone(),
                running: instance.is_running(),
                has_display: instance.has_display(),
            })
            .collect();
        instances.sort_unstable_by(|left, right| left.id.cmp(&right.id));
        instances
    }

    pub fn get(&self, id: &str) -> Option<&FirmwareInstance> {
        self.instances.get(id)
    }

    fn next_available_id(&mut self) -> String {
        loop {
            let id = format!("node{}", self.next_id);
            self.next_id += 1;
            if !self.instances.contains_key(&id) {
                return id;
            }
        }
    }
}

impl Default for InstanceManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_manager_is_empty() {
        let manager = InstanceManager::new();
        assert!(manager.list().is_empty());
        assert!(manager.get("node1").is_none());
    }

    #[test]
    fn killing_an_unknown_instance_is_an_error() {
        let mut manager = InstanceManager::new();
        assert!(manager.kill("node1").is_err());
    }

    #[test]
    fn generated_ids_skip_existing_ids() {
        let mut manager = InstanceManager::new();
        manager.next_id = 2;
        assert_eq!(manager.next_available_id(), "node2");
        assert_eq!(manager.next_available_id(), "node3");
    }
}
