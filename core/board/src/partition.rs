use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex, MutexGuard};

use crate::nvs::{LAUNCHER_NVS_SIZE, STANDALONE_NVS_SIZE};

pub const ESP_PARTITION_TYPE_APP: u8 = 0x00;
pub const ESP_PARTITION_TYPE_DATA: u8 = 0x01;

pub const ESP_PARTITION_SUBTYPE_DATA_OTA: u8 = 0x00;
pub const ESP_PARTITION_SUBTYPE_DATA_PHY: u8 = 0x01;
pub const ESP_PARTITION_SUBTYPE_DATA_NVS: u8 = 0x02;
pub const ESP_PARTITION_SUBTYPE_DATA_COREDUMP: u8 = 0x03;
pub const ESP_PARTITION_SUBTYPE_DATA_SPIFFS: u8 = 0x82;

pub const ESP_PARTITION_SUBTYPE_APP_OTA_0: u8 = 0x10;
pub const ESP_PARTITION_SUBTYPE_APP_OTA_1: u8 = 0x11;
pub const ESP_PARTITION_SUBTYPE_APP_TEST: u8 = 0x20;

pub const STANDALONE_OTADATA_ADDRESS: u32 = 0xE000;
pub const LAUNCHER_OTADATA_ADDRESS: u32 = 0xD000;

static PARTITION_TABLES: LazyLock<Mutex<HashMap<String, SharedVirtualPartitionTable>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
static ACTIVE_PARTITION_TABLE: LazyLock<Mutex<VirtualPartitionTable>> =
    LazyLock::new(|| Mutex::new(VirtualPartitionTable::standalone()));

pub type SharedVirtualPartitionTable = Arc<Mutex<VirtualPartitionTable>>;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VirtualPartition {
    pub name: String,
    pub partition_type: u8,
    pub subtype: u8,
    pub address: u32,
    pub size: u32,
}

impl VirtualPartition {
    fn new(name: &str, partition_type: u8, subtype: u8, address: u32, size: u32) -> Self {
        Self {
            name: name.to_owned(),
            partition_type,
            subtype,
            address,
            size,
        }
    }
}

/// The flash partition table visible to one virtual T-Deck firmware instance.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VirtualPartitionTable {
    partitions: Vec<VirtualPartition>,
}

impl VirtualPartitionTable {
    /// Arduino `default_16MB.csv`, including the trailing coredump partition.
    pub fn standalone() -> Self {
        Self {
            partitions: vec![
                VirtualPartition::new(
                    "nvs",
                    ESP_PARTITION_TYPE_DATA,
                    ESP_PARTITION_SUBTYPE_DATA_NVS,
                    0x9000,
                    STANDALONE_NVS_SIZE,
                ),
                VirtualPartition::new(
                    "otadata",
                    ESP_PARTITION_TYPE_DATA,
                    ESP_PARTITION_SUBTYPE_DATA_OTA,
                    STANDALONE_OTADATA_ADDRESS,
                    0x2000,
                ),
                VirtualPartition::new(
                    "app0",
                    ESP_PARTITION_TYPE_APP,
                    ESP_PARTITION_SUBTYPE_APP_OTA_0,
                    0x10000,
                    0x640000,
                ),
                VirtualPartition::new(
                    "app1",
                    ESP_PARTITION_TYPE_APP,
                    ESP_PARTITION_SUBTYPE_APP_OTA_1,
                    0x650000,
                    0x640000,
                ),
                VirtualPartition::new(
                    "spiffs",
                    ESP_PARTITION_TYPE_DATA,
                    ESP_PARTITION_SUBTYPE_DATA_SPIFFS,
                    0xC90000,
                    0x360000,
                ),
                VirtualPartition::new(
                    "coredump",
                    ESP_PARTITION_TYPE_DATA,
                    ESP_PARTITION_SUBTYPE_DATA_COREDUMP,
                    0xFF0000,
                    0x10000,
                ),
            ],
        }
    }

    /// Launcher's resident `custom_16Mb.csv` layout.
    ///
    /// Installed applications and their SPIFFS partitions are placed
    /// dynamically after this resident region. The invariant entries used for
    /// runtime detection are represented exactly here.
    pub fn launcher() -> Self {
        Self {
            partitions: vec![
                VirtualPartition::new(
                    "nvs",
                    ESP_PARTITION_TYPE_DATA,
                    ESP_PARTITION_SUBTYPE_DATA_NVS,
                    0x9000,
                    LAUNCHER_NVS_SIZE,
                ),
                VirtualPartition::new(
                    "otadata",
                    ESP_PARTITION_TYPE_DATA,
                    ESP_PARTITION_SUBTYPE_DATA_OTA,
                    LAUNCHER_OTADATA_ADDRESS,
                    0x2000,
                ),
                VirtualPartition::new(
                    "phy_init",
                    ESP_PARTITION_TYPE_DATA,
                    ESP_PARTITION_SUBTYPE_DATA_PHY,
                    0xF000,
                    0x1000,
                ),
                VirtualPartition::new(
                    "app0",
                    ESP_PARTITION_TYPE_APP,
                    ESP_PARTITION_SUBTYPE_APP_TEST,
                    0x10000,
                    0x180000,
                ),
                VirtualPartition::new(
                    "coredump",
                    ESP_PARTITION_TYPE_DATA,
                    ESP_PARTITION_SUBTYPE_DATA_COREDUMP,
                    0x190000,
                    0x10000,
                ),
            ],
        }
    }

    pub fn entries(&self) -> &[VirtualPartition] {
        &self.partitions
    }

    pub fn find_first(&self, partition_type: u8, subtype: u8) -> Option<&VirtualPartition> {
        self.partitions.iter().find(|partition| {
            partition.partition_type == partition_type && partition.subtype == subtype
        })
    }

    pub fn otadata_address(&self) -> Option<u32> {
        self.find_first(ESP_PARTITION_TYPE_DATA, ESP_PARTITION_SUBTYPE_DATA_OTA)
            .map(|partition| partition.address)
    }

    /// Matches SigurdOS/Wadamesh's dual-signal Launcher check.
    pub fn is_under_launcher(&self) -> bool {
        self.find_first(ESP_PARTITION_TYPE_APP, ESP_PARTITION_SUBTYPE_APP_TEST)
            .is_some()
            && self.otadata_address() == Some(LAUNCHER_OTADATA_ADDRESS)
    }

    pub fn set_launcher_mode(&mut self, enabled: bool) {
        *self = if enabled {
            Self::launcher()
        } else {
            Self::standalone()
        };
    }
}

impl Default for VirtualPartitionTable {
    fn default() -> Self {
        Self::standalone()
    }
}

pub fn register_partition_table(
    instance_id: &str,
    launcher_mode: bool,
) -> SharedVirtualPartitionTable {
    let mut tables = lock(&PARTITION_TABLES);
    if let Some(table) = tables.get(instance_id) {
        lock(table).set_launcher_mode(launcher_mode);
        activate_table(table);
        return Arc::clone(table);
    }
    let table = Arc::new(Mutex::new(if launcher_mode {
        VirtualPartitionTable::launcher()
    } else {
        VirtualPartitionTable::standalone()
    }));
    activate_table(&table);
    tables.insert(instance_id.to_owned(), Arc::clone(&table));
    table
}

pub fn get_partition_table(instance_id: &str) -> Option<SharedVirtualPartitionTable> {
    lock(&PARTITION_TABLES).get(instance_id).cloned()
}

/// Selects the table seen by ESP-IDF-compatible APIs without an instance ID.
///
/// The core calls this immediately before entering each firmware instance.
pub fn activate_partition_table(instance_id: &str) -> bool {
    let Some(table) = get_partition_table(instance_id) else {
        return false;
    };
    activate_table(&table);
    true
}

pub fn active_partition_table() -> VirtualPartitionTable {
    lock(&ACTIVE_PARTITION_TABLE).clone()
}

pub fn remove_partition_table(instance_id: &str) -> Option<SharedVirtualPartitionTable> {
    lock(&PARTITION_TABLES).remove(instance_id)
}

fn activate_table(table: &SharedVirtualPartitionTable) {
    *lock(&ACTIVE_PARTITION_TABLE) = lock(table).clone();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standalone_layout_matches_default_16mb_csv() {
        let table = VirtualPartitionTable::standalone();
        assert_eq!(table.entries().len(), 6);
        assert_eq!(
            table.find_first(ESP_PARTITION_TYPE_DATA, ESP_PARTITION_SUBTYPE_DATA_NVS),
            Some(&VirtualPartition::new(
                "nvs",
                ESP_PARTITION_TYPE_DATA,
                ESP_PARTITION_SUBTYPE_DATA_NVS,
                0x9000,
                0x5000,
            ))
        );
        assert_eq!(table.otadata_address(), Some(0xE000));
        assert_eq!(
            table.find_first(ESP_PARTITION_TYPE_APP, ESP_PARTITION_SUBTYPE_APP_OTA_0),
            Some(&VirtualPartition::new(
                "app0",
                ESP_PARTITION_TYPE_APP,
                ESP_PARTITION_SUBTYPE_APP_OTA_0,
                0x10000,
                0x640000,
            ))
        );
        assert_eq!(
            table.find_first(ESP_PARTITION_TYPE_APP, ESP_PARTITION_SUBTYPE_APP_OTA_1),
            Some(&VirtualPartition::new(
                "app1",
                ESP_PARTITION_TYPE_APP,
                ESP_PARTITION_SUBTYPE_APP_OTA_1,
                0x650000,
                0x640000,
            ))
        );
        assert_eq!(
            table.find_first(ESP_PARTITION_TYPE_DATA, ESP_PARTITION_SUBTYPE_DATA_SPIFFS),
            Some(&VirtualPartition::new(
                "spiffs",
                ESP_PARTITION_TYPE_DATA,
                ESP_PARTITION_SUBTYPE_DATA_SPIFFS,
                0xC90000,
                0x360000,
            ))
        );
        assert!(!table.is_under_launcher());
    }

    #[test]
    fn launcher_layout_has_both_detection_signals_and_smaller_nvs() {
        let table = VirtualPartitionTable::launcher();
        assert_eq!(table.otadata_address(), Some(LAUNCHER_OTADATA_ADDRESS));
        assert_eq!(
            table
                .find_first(ESP_PARTITION_TYPE_DATA, ESP_PARTITION_SUBTYPE_DATA_NVS)
                .map(|partition| partition.size),
            Some(LAUNCHER_NVS_SIZE)
        );
        assert_eq!(
            table.find_first(ESP_PARTITION_TYPE_APP, ESP_PARTITION_SUBTYPE_APP_TEST),
            Some(&VirtualPartition::new(
                "app0",
                ESP_PARTITION_TYPE_APP,
                ESP_PARTITION_SUBTYPE_APP_TEST,
                0x10000,
                0x180000,
            ))
        );
        assert!(table.is_under_launcher());
    }

    #[test]
    fn toggling_layout_changes_all_geometry_not_just_the_detection_flag() {
        let mut table = VirtualPartitionTable::standalone();
        table.set_launcher_mode(true);
        assert_eq!(table, VirtualPartitionTable::launcher());
        assert!(table.is_under_launcher());

        table.set_launcher_mode(false);
        assert_eq!(table, VirtualPartitionTable::standalone());
        assert!(!table.is_under_launcher());
    }

    #[test]
    fn active_table_tracks_the_instance_about_to_run() {
        let first = "partition-active-standalone";
        let second = "partition-active-launcher";
        register_partition_table(first, false);
        register_partition_table(second, true);

        assert!(activate_partition_table(first));
        assert_eq!(
            active_partition_table().otadata_address(),
            Some(STANDALONE_OTADATA_ADDRESS)
        );
        assert!(activate_partition_table(second));
        assert!(active_partition_table().is_under_launcher());

        remove_partition_table(first);
        remove_partition_table(second);
    }
}
