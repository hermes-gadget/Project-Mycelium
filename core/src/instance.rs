use std::collections::HashMap;
use std::path::Path;

use anyhow::{bail, Result};
use mycelium_board::{
    activate_partition_table, register_buzzer, register_nvs, register_partition_table,
    remove_buzzer, remove_nvs, remove_partition_table, BoardConfig, SharedVirtualBuzzer,
    SharedVirtualNvs, SharedVirtualPartitionTable, VirtualBoard, LAUNCHER_NVS_SIZE,
    STANDALONE_NVS_SIZE,
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
    #[serde(default)]
    pub launcher_mode: bool,
}

impl Default for InstanceBoardConfig {
    fn default() -> Self {
        Self {
            battery_mv: default_battery_mv(),
            launcher_mode: false,
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

/// The one monotonic clock used by the core runtime.
///
/// The value is an absolute simulation timestamp. Callers advance it once per
/// frame and pass the returned value to every absolute-time consumer; the
/// corresponding delta is used only for subsystems whose public API is
/// explicitly delta-based, such as the GPS manager.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SimulationClock {
    now_ms: u64,
}

impl SimulationClock {
    pub fn now_ms(self) -> u64 {
        self.now_ms
    }

    pub fn advance_by(&mut self, delta_ms: u64) -> u64 {
        self.now_ms = self.now_ms.saturating_add(delta_ms);
        self.now_ms
    }

    pub fn advance_to(&mut self, now_ms: u64) -> u64 {
        self.now_ms = self.now_ms.max(now_ms);
        self.now_ms
    }
}

pub struct Instance {
    firmware: FirmwareInstance,
    storage: StorageManager,
    gps: GpsManager,
    board: VirtualBoard,
    buzzer: SharedVirtualBuzzer,
    nvs: SharedVirtualNvs,
    partition_table: SharedVirtualPartitionTable,
    resources: InstanceResourceRegistry,
}

/// Owns the registrations created for one manager instance and centralizes
/// teardown ordering. Firmware is stopped before the host-side registries are
/// removed, so a firmware destroy hook can release its handles while their
/// backing instance identity is still valid.
struct InstanceResourceRegistry {
    id: String,
    host_resources_registered: bool,
    firmware_stopped: bool,
}

impl InstanceResourceRegistry {
    fn new(id: &str) -> Self {
        Self {
            id: id.to_owned(),
            host_resources_registered: true,
            firmware_stopped: false,
        }
    }

    fn teardown(&mut self, firmware: &mut FirmwareInstance) {
        if !self.firmware_stopped {
            firmware.stop();
            self.firmware_stopped = true;
        }
        if self.host_resources_registered {
            remove_buzzer(&self.id);
            remove_nvs(&self.id);
            remove_partition_table(&self.id);
            self.host_resources_registered = false;
        }
    }
}

struct InstancePeripherals {
    storage: StorageManager,
    gps: GpsManager,
    board: VirtualBoard,
    buzzer: SharedVirtualBuzzer,
    nvs: SharedVirtualNvs,
    partition_table: SharedVirtualPartitionTable,
}

fn create_peripherals(id: &str, config: &InstanceConfig) -> Result<InstancePeripherals> {
    let mut storage = StorageManager::new(id);
    storage.init_all()?;
    let nvs = register_nvs(
        id,
        if config.board.launcher_mode {
            LAUNCHER_NVS_SIZE
        } else {
            STANDALONE_NVS_SIZE
        },
    )?;
    let partition_table = register_partition_table(id, config.board.launcher_mode);
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
        nvs,
        partition_table,
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
            nvs: peripherals.nvs,
            partition_table: peripherals.partition_table,
            resources: InstanceResourceRegistry::new(id),
        })
    }

    fn start(&mut self) -> Result<()> {
        activate_partition_table(self.firmware.name());
        self.firmware.start()
    }

    fn tick(&mut self, delta_ms: u64) {
        activate_partition_table(self.firmware.name());
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

    pub fn nvs(&self) -> &SharedVirtualNvs {
        &self.nvs
    }

    pub fn partition_table(&self) -> &SharedVirtualPartitionTable {
        &self.partition_table
    }
}

impl Drop for Instance {
    fn drop(&mut self) {
        self.resources.teardown(&mut self.firmware);
    }
}

pub struct InstanceManager {
    instances: HashMap<String, Instance>,
    next_id: u64,
    clock: SimulationClock,
}

impl InstanceManager {
    pub fn new() -> Self {
        Self {
            instances: HashMap::new(),
            next_id: 1,
            clock: SimulationClock::default(),
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
        if !instance.firmware.is_contextful() && !self.instances.is_empty() {
            bail!(
                "multiple firmware instances require the contextful v2 ABI; {} exports the legacy v1 ABI",
                firmware_path.display()
            );
        }
        instance.start()?;
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
        let _ = self.tick_all_with_delta(1);
    }

    /// Advances the central simulation clock and all instances by one delta.
    ///
    /// The returned value is the new absolute simulation timestamp and should
    /// be supplied to `meshemu_bus_tick` for the same frame.
    pub fn tick_all_with_delta(&mut self, delta_ms: u64) -> u64 {
        let now_ms = self.clock.advance_by(delta_ms);
        self.tick_instances(delta_ms);
        now_ms
    }

    /// Advances all instances to an absolute timestamp from the central
    /// runtime clock. Backward timestamps are ignored, matching the bus clock.
    pub fn tick_all_at(&mut self, now_ms: u64) {
        let previous = self.clock.now_ms();
        let now_ms = self.clock.advance_to(now_ms);
        self.tick_instances(now_ms.saturating_sub(previous));
    }

    fn tick_instances(&mut self, delta_ms: u64) {
        for instance in self.instances.values_mut() {
            instance.tick(delta_ms);
        }
    }

    pub fn simulation_now_ms(&self) -> u64 {
        self.clock.now_ms()
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
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

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
        remove_nvs(&id);
        remove_partition_table(&id);
    }

    #[test]
    fn crash_breadcrumb_survives_peripheral_restart_and_skips_migration() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let id = format!("instance-nvs-crash-{}-{nonce}", std::process::id());
        let config = InstanceConfig::default();
        let first = create_peripherals(&id, &config).unwrap();
        let backing_path = {
            let mut nvs = first.nvs.lock().unwrap();
            assert!(nvs.begin("touch", false));
            assert_eq!(nvs.put_bool("sd_mig_busy", true), 1);
            nvs.backing_path().to_owned()
        };

        // Simulate an abrupt instance loss: discard live registries but leave
        // the durable NVS image exactly as a reset would.
        remove_nvs(&id);
        remove_partition_table(&id);
        remove_buzzer(&id);
        drop(first);

        let restarted = create_peripherals(&id, &config).unwrap();
        let mut nvs = restarted.nvs.lock().unwrap();
        assert!(nvs.begin("touch", true));
        let migration_should_run = !nvs.get_bool("sd_mig_busy", false);
        assert!(
            !migration_should_run,
            "Wadamesh must skip migration after a crash leaves sd_mig_busy set"
        );
        drop(nvs);

        remove_nvs(&id);
        remove_partition_table(&id);
        remove_buzzer(&id);
        drop(restarted);
        fs::remove_file(backing_path).unwrap();
    }

    #[test]
    fn launcher_instance_uses_coherent_partition_and_nvs_geometry() {
        let id = format!(
            "launcher-peripherals-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let config = InstanceConfig {
            board: InstanceBoardConfig {
                launcher_mode: true,
                ..InstanceBoardConfig::default()
            },
            ..InstanceConfig::default()
        };
        let peripherals = create_peripherals(&id, &config).unwrap();

        assert_eq!(
            peripherals.nvs.lock().unwrap().partition_size(),
            LAUNCHER_NVS_SIZE
        );
        assert!(peripherals
            .partition_table
            .lock()
            .unwrap()
            .is_under_launcher());

        remove_nvs(&id);
        remove_partition_table(&id);
        remove_buzzer(&id);
    }
}
