use std::collections::HashMap;
use std::path::Path;

use anyhow::{bail, Result};
use mycelium_board::{
    register_buzzer, remove_buzzer, BoardConfig, SharedVirtualBuzzer, VirtualBoard,
};
use mycelium_gps::GpsManager;
use mycelium_storage::StorageManager;
use serde::{Deserialize, Serialize};

use crate::loader::FirmwareInstance;
use mycelium_display::LvglVersion;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct GpsConfig {
    #[serde(default = "default_latitude")]
    pub latitude: f64,
    #[serde(default = "default_longitude")]
    pub longitude: f64,
}

impl Default for GpsConfig {
    fn default() -> Self {
        Self {
            latitude: default_latitude(),
            longitude: default_longitude(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct InstanceBoardConfig {
    #[serde(default = "default_battery_mv")]
    pub battery_mv: u16,
}

impl Default for InstanceBoardConfig {
    fn default() -> Self {
        Self {
            battery_mv: default_battery_mv(),
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct InstanceConfig {
    /// An optional caller-selected ID. When absent, `nodeN` is generated.
    #[serde(default)]
    pub instance_id: Option<String>,
    #[serde(default)]
    pub gps: GpsConfig,
    #[serde(default)]
    pub board: InstanceBoardConfig,
}

fn default_latitude() -> f64 {
    51.5074
}

fn default_longitude() -> f64 {
    -0.1278
}

fn default_battery_mv() -> u16 {
    3_700
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct InstanceInfo {
    pub id: String,
    pub running: bool,
    pub has_display: bool,
}

pub struct Instance {
    firmware: FirmwareInstance,
    storage: StorageManager,
    gps: GpsManager,
    board: VirtualBoard,
    buzzer: SharedVirtualBuzzer,
}

struct InstancePeripherals {
    storage: StorageManager,
    gps: GpsManager,
    board: VirtualBoard,
    buzzer: SharedVirtualBuzzer,
}

fn create_peripherals(id: &str, config: &InstanceConfig) -> Result<InstancePeripherals> {
    let mut storage = StorageManager::new(id);
    storage.init_all()?;
    let gps = GpsManager::new(config.gps.latitude, config.gps.longitude);
    let board = VirtualBoard::new(
        id,
        BoardConfig {
            battery_mv: config.board.battery_mv,
            mcu_temperature: 35.0,
            manufacturer: "Mycelium Virtual T-Deck".to_owned(),
            ..BoardConfig::default()
        },
    );
    let buzzer = register_buzzer(id);

    Ok(InstancePeripherals {
        storage,
        gps,
        board,
        buzzer,
    })
}

impl Instance {
    fn create(id: &str, firmware_path: &Path, config: &InstanceConfig) -> Result<Self> {
        let firmware = FirmwareInstance::load(id, firmware_path)?;
        let peripherals = create_peripherals(id, config)?;

        Ok(Self {
            firmware,
            storage: peripherals.storage,
            gps: peripherals.gps,
            board: peripherals.board,
            buzzer: peripherals.buzzer,
        })
    }

    fn start(&mut self) {
        self.firmware.start();
    }

    fn tick(&mut self, delta_ms: u64) {
        self.firmware.tick();
        if self.board.periph_pwr_enabled {
            self.gps.tick(delta_ms);
        }
    }

    pub fn name(&self) -> &str {
        self.firmware.name()
    }

    pub fn is_running(&self) -> bool {
        self.firmware.is_running()
    }

    pub fn has_display(&self) -> bool {
        self.firmware.has_display()
    }

    pub fn display(&self) -> Option<*mut std::ffi::c_void> {
        self.firmware.display()
    }

    pub fn display_version(&self) -> LvglVersion {
        self.firmware.display_version()
    }

    pub fn capture_display_rgb565(&self) -> Option<Vec<u8>> {
        self.firmware.capture_display_rgb565()
    }

    pub fn storage(&self) -> &StorageManager {
        &self.storage
    }

    pub fn gps(&self) -> &GpsManager {
        &self.gps
    }

    pub fn board(&self) -> &VirtualBoard {
        &self.board
    }

    pub fn buzzer(&self) -> &SharedVirtualBuzzer {
        &self.buzzer
    }
}

impl Drop for Instance {
    fn drop(&mut self) {
        remove_buzzer(self.firmware.name());
    }
}

pub struct InstanceManager {
    instances: HashMap<String, Instance>,
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
        let id = match config.instance_id.as_deref() {
            Some(id) => {
                if id.is_empty() {
                    bail!("instance ID cannot be empty");
                }
                id.to_owned()
            }
            None => self.next_available_id(),
        };

        if self.instances.contains_key(&id) {
            bail!("instance {id} already exists");
        }

        let mut instance = Instance::create(&id, firmware_path, &config)?;
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

    /// Advances all instances by the legacy one-millisecond emulator step.
    pub fn tick_all(&mut self) {
        self.tick_all_with_delta(1);
    }

    /// Advances all instances and their time-based peripherals.
    pub fn tick_all_with_delta(&mut self, delta_ms: u64) {
        for instance in self.instances.values_mut() {
            instance.tick(delta_ms);
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

    pub fn get(&self, id: &str) -> Option<&Instance> {
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

    #[test]
    fn peripheral_creation_does_not_panic() {
        let config = InstanceConfig::default();
        let id = format!("peripheral-test-{}", std::process::id());
        let peripherals = create_peripherals(&id, &config).unwrap();

        assert!(peripherals.storage.spiffs.is_mounted());
        assert!(peripherals.storage.sdcard.is_mounted());
        assert_eq!(
            (
                peripherals.gps.state().latitude,
                peripherals.gps.state().longitude
            ),
            (51.5074, -0.1278)
        );
        assert_eq!(peripherals.board.get_battery_mv(), 3_700);
        assert!(!peripherals.buzzer.lock().unwrap().is_playing());
        remove_buzzer(&id);
    }
}
