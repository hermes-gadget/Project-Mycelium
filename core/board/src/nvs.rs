use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard};

use serde::{Deserialize, Serialize};

/// NVS geometry used by the stock 16 MB T-Deck partition table.
pub const STANDALONE_NVS_SIZE: u32 = 0x5000;
/// NVS geometry used by bmorcelli/Launcher on the T-Deck.
pub const LAUNCHER_NVS_SIZE: u32 = 0x4000;
/// ESP-IDF limits NVS namespace and key names to 15 bytes plus a terminator.
pub const NVS_NAME_MAX_BYTES: usize = 15;

static NVS_INSTANCES: LazyLock<Mutex<HashMap<String, SharedVirtualNvs>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
static TEMP_FILE_SEQUENCE: AtomicU64 = AtomicU64::new(1);

pub type SharedVirtualNvs = Arc<Mutex<VirtualNvs>>;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
enum NvsValue {
    Bool(bool),
    String(String),
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
struct PersistedNvs {
    namespaces: BTreeMap<String, BTreeMap<String, NvsValue>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct OpenNamespace {
    name: String,
    read_only: bool,
}

/// A bounded, namespace-aware ESP32 Preferences/NVS model.
///
/// Values are persisted after every successful mutation. `begin` selects the
/// namespace used by subsequent Preferences-style calls until `end`.
#[derive(Debug)]
pub struct VirtualNvs {
    instance_id: String,
    partition_size: u32,
    backing_path: PathBuf,
    persisted: PersistedNvs,
    open_namespace: Option<OpenNamespace>,
}

impl VirtualNvs {
    /// Opens a standalone-sized NVS image from the host temp directory.
    pub fn new(instance_id: &str) -> Result<Self, io::Error> {
        Self::with_size(instance_id, STANDALONE_NVS_SIZE)
    }

    /// Opens an NVS image with caller-selected partition geometry.
    pub fn with_size(instance_id: &str, partition_size: u32) -> Result<Self, io::Error> {
        Self::at_backing_path(instance_id, partition_size, default_path(instance_id))
    }

    fn at_backing_path(
        instance_id: &str,
        partition_size: u32,
        backing_path: PathBuf,
    ) -> Result<Self, io::Error> {
        if instance_id.is_empty() || partition_size == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "NVS instance ID and partition size must be non-empty",
            ));
        }
        let persisted = match fs::read(&backing_path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|error| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("invalid virtual NVS JSON: {error}"),
                )
            })?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => PersistedNvs::default(),
            Err(error) => return Err(error),
        };

        Ok(Self {
            instance_id: instance_id.to_owned(),
            partition_size,
            backing_path,
            persisted,
            open_namespace: None,
        })
    }

    #[cfg(test)]
    fn at_path(
        instance_id: &str,
        partition_size: u32,
        backing_path: PathBuf,
    ) -> Result<Self, io::Error> {
        Self::at_backing_path(instance_id, partition_size, backing_path)
    }

    pub fn instance_id(&self) -> &str {
        &self.instance_id
    }

    pub fn partition_size(&self) -> u32 {
        self.partition_size
    }

    pub fn backing_path(&self) -> &Path {
        &self.backing_path
    }

    /// Changes the visible partition geometry without erasing persisted values.
    ///
    /// Existing values remain readable after a standalone/Launcher geometry
    /// switch. New writes still have to fit in the newly selected geometry.
    pub fn set_partition_size(&mut self, partition_size: u32) -> bool {
        if partition_size == 0 {
            return false;
        }
        self.partition_size = partition_size;
        true
    }

    /// Opens one namespace. A read-only open fails when it does not yet exist.
    pub fn begin(&mut self, namespace: &str, read_only: bool) -> bool {
        if !valid_name(namespace) {
            self.open_namespace = None;
            return false;
        }
        if read_only && !self.persisted.namespaces.contains_key(namespace) {
            self.open_namespace = None;
            return false;
        }
        if !read_only && !self.persisted.namespaces.contains_key(namespace) {
            let previous = self.persisted.clone();
            self.persisted
                .namespaces
                .insert(namespace.to_owned(), BTreeMap::new());
            if !self.fits_partition() || self.persist().is_err() {
                self.persisted = previous;
                self.open_namespace = None;
                return false;
            }
        }
        self.open_namespace = Some(OpenNamespace {
            name: namespace.to_owned(),
            read_only,
        });
        true
    }

    pub fn end(&mut self) {
        self.open_namespace = None;
    }

    pub fn exists(&self, key: &str) -> bool {
        valid_name(key)
            && self
                .open_values()
                .is_some_and(|values| values.contains_key(key))
    }

    pub fn get_bool(&self, key: &str, default: bool) -> bool {
        match self.open_value(key) {
            Some(NvsValue::Bool(value)) => *value,
            _ => default,
        }
    }

    pub fn get_string(&self, key: &str, default: &str) -> String {
        match self.open_value(key) {
            Some(NvsValue::String(value)) => value.clone(),
            _ => default.to_owned(),
        }
    }

    /// Stores a bool and returns its Arduino Preferences payload size.
    pub fn put_bool(&mut self, key: &str, value: bool) -> usize {
        if self.try_put_bool(key, value) {
            1
        } else {
            0
        }
    }

    pub fn try_put_bool(&mut self, key: &str, value: bool) -> bool {
        self.put(key, NvsValue::Bool(value))
    }

    /// Stores a UTF-8 string and returns its byte length on success.
    pub fn put_string(&mut self, key: &str, value: &str) -> usize {
        if self.try_put_string(key, value) {
            value.len()
        } else {
            0
        }
    }

    pub fn try_put_string(&mut self, key: &str, value: &str) -> bool {
        self.put(key, NvsValue::String(value.to_owned()))
    }

    pub fn remove(&mut self, key: &str) -> bool {
        if !valid_name(key) || !self.can_write() {
            return false;
        }
        let namespace = self
            .open_namespace
            .as_ref()
            .expect("can_write requires an open namespace")
            .name
            .clone();
        let previous = self.persisted.clone();
        let removed = self
            .persisted
            .namespaces
            .get_mut(&namespace)
            .and_then(|values| values.remove(key))
            .is_some();
        if !removed {
            return false;
        }
        if self.persist().is_err() {
            self.persisted = previous;
            return false;
        }
        true
    }

    fn put(&mut self, key: &str, value: NvsValue) -> bool {
        if !valid_name(key) || !self.can_write() {
            return false;
        }
        let namespace = self
            .open_namespace
            .as_ref()
            .expect("can_write requires an open namespace")
            .name
            .clone();
        let previous = self.persisted.clone();
        self.persisted
            .namespaces
            .get_mut(&namespace)
            .expect("writable begin creates the namespace")
            .insert(key.to_owned(), value);
        if !self.fits_partition() || self.persist().is_err() {
            self.persisted = previous;
            return false;
        }
        true
    }

    fn open_values(&self) -> Option<&BTreeMap<String, NvsValue>> {
        let namespace = &self.open_namespace.as_ref()?.name;
        self.persisted.namespaces.get(namespace)
    }

    fn open_value(&self, key: &str) -> Option<&NvsValue> {
        valid_name(key)
            .then(|| self.open_values()?.get(key))
            .flatten()
    }

    fn can_write(&self) -> bool {
        self.open_namespace
            .as_ref()
            .is_some_and(|namespace| !namespace.read_only)
    }

    fn encoded(&self) -> Result<Vec<u8>, serde_json::Error> {
        serde_json::to_vec_pretty(&self.persisted)
    }

    fn fits_partition(&self) -> bool {
        self.encoded()
            .is_ok_and(|bytes| bytes.len() <= self.partition_size as usize)
    }

    fn persist(&self) -> Result<(), io::Error> {
        let bytes = self
            .encoded()
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        if bytes.len() > self.partition_size as usize {
            return Err(io::Error::new(
                io::ErrorKind::StorageFull,
                "NVS data exceeds the configured partition size",
            ));
        }
        let Some(parent) = self.backing_path.parent() else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "NVS backing file must have a parent directory",
            ));
        };
        fs::create_dir_all(parent)?;
        let sequence = TEMP_FILE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let temp_path = parent.join(format!(".nvs-{}-{sequence}.tmp", std::process::id()));
        fs::write(&temp_path, bytes)?;
        if let Err(error) = fs::rename(&temp_path, &self.backing_path) {
            let _ = fs::remove_file(&temp_path);
            return Err(error);
        }
        Ok(())
    }
}

/// Opens or reconfigures the live NVS registry entry for one instance.
pub fn register_nvs(instance_id: &str, partition_size: u32) -> Result<SharedVirtualNvs, io::Error> {
    let mut instances = lock(&NVS_INSTANCES);
    if let Some(nvs) = instances.get(instance_id) {
        if !lock(nvs).set_partition_size(partition_size) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "NVS partition size must be non-zero",
            ));
        }
        return Ok(Arc::clone(nvs));
    }
    let nvs = Arc::new(Mutex::new(VirtualNvs::with_size(
        instance_id,
        partition_size,
    )?));
    instances.insert(instance_id.to_owned(), Arc::clone(&nvs));
    Ok(nvs)
}

pub fn get_nvs(instance_id: &str) -> Option<SharedVirtualNvs> {
    lock(&NVS_INSTANCES).get(instance_id).cloned()
}

/// Removes only the live handle. The JSON image intentionally survives restart.
pub fn remove_nvs(instance_id: &str) -> Option<SharedVirtualNvs> {
    lock(&NVS_INSTANCES).remove(instance_id)
}

fn valid_name(value: &str) -> bool {
    !value.is_empty() && value.len() <= NVS_NAME_MAX_BYTES && !value.as_bytes().contains(&0)
}

fn default_path(instance_id: &str) -> PathBuf {
    std::env::temp_dir()
        .join("mycelium")
        .join("instances")
        .join(instance_directory(instance_id))
        .join("nvs.json")
}

fn instance_directory(instance_id: &str) -> String {
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

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    fn test_path(name: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "mycelium-nvs-{name}-{}-{nonce}.json",
            std::process::id()
        ))
    }

    #[test]
    fn preferences_namespaces_types_remove_and_end_match_arduino_semantics() {
        let path = test_path("preferences");
        let mut nvs = VirtualNvs::at_path("prefs", STANDALONE_NVS_SIZE, path.clone()).unwrap();

        assert!(!nvs.begin("missing", true));
        assert!(nvs.begin("touch", false));
        assert_eq!(nvs.put_bool("enabled", true), 1);
        assert_eq!(nvs.put_string("label", "T-Deck"), 6);
        assert!(nvs.exists("enabled"));
        assert!(nvs.get_bool("enabled", false));
        assert_eq!(nvs.get_string("label", "fallback"), "T-Deck");
        assert_eq!(nvs.get_string("enabled", "wrong-type"), "wrong-type");
        nvs.end();
        assert!(!nvs.exists("enabled"));
        assert!(!nvs.get_bool("enabled", false));
        assert_eq!(nvs.put_bool("closed", true), 0);

        assert!(nvs.begin("other", false));
        assert!(!nvs.exists("enabled"));
        assert_eq!(nvs.put_bool("enabled", false), 1);
        nvs.end();

        assert!(nvs.begin("touch", true));
        assert!(nvs.get_bool("enabled", false));
        assert_eq!(nvs.put_bool("blocked", true), 0);
        assert!(!nvs.remove("enabled"));
        nvs.end();

        assert!(nvs.begin("touch", false));
        assert!(nvs.remove("enabled"));
        assert!(!nvs.exists("enabled"));
        assert!(!nvs.remove("enabled"));
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn oversized_write_is_rejected_without_losing_the_previous_value() {
        let path = test_path("capacity");
        let mut nvs = VirtualNvs::at_path("small", 160, path.clone()).unwrap();
        assert!(nvs.begin("touch", false));
        assert_eq!(nvs.put_string("value", "fits"), 4);
        assert_eq!(nvs.put_string("value", &"x".repeat(512)), 0);
        assert_eq!(nvs.get_string("value", ""), "fits");

        drop(nvs);
        let mut reopened = VirtualNvs::at_path("small", 160, path.clone()).unwrap();
        assert!(reopened.begin("touch", true));
        assert_eq!(reopened.get_string("value", ""), "fits");
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn migration_breadcrumb_survives_crash_and_skips_the_next_attempt() {
        let path = test_path("crash");
        {
            let mut first =
                VirtualNvs::at_path("crash-node", STANDALONE_NVS_SIZE, path.clone()).unwrap();
            assert!(first.begin("touch", false));
            assert_eq!(first.put_bool("sd_mig_busy", true), 1);
            // Simulate reset/crash: no cleanup or breadcrumb removal.
        }

        let mut restarted =
            VirtualNvs::at_path("crash-node", LAUNCHER_NVS_SIZE, path.clone()).unwrap();
        assert!(restarted.begin("touch", true));
        let migration_should_run = !restarted.get_bool("sd_mig_busy", false);
        assert!(
            !migration_should_run,
            "a stale busy breadcrumb must skip migration after restart"
        );
        assert_eq!(restarted.partition_size(), LAUNCHER_NVS_SIZE);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn namespace_and_key_limits_are_enforced() {
        let path = test_path("names");
        let mut nvs = VirtualNvs::at_path("names", STANDALONE_NVS_SIZE, path.clone()).unwrap();
        assert!(!nvs.begin("", false));
        assert!(!nvs.begin(&"n".repeat(NVS_NAME_MAX_BYTES + 1), false));
        assert!(nvs.begin(&"n".repeat(NVS_NAME_MAX_BYTES), false));
        assert_eq!(nvs.put_bool("", true), 0);
        assert_eq!(nvs.put_bool(&"k".repeat(NVS_NAME_MAX_BYTES + 1), true), 0);
        assert_eq!(nvs.put_bool(&"k".repeat(NVS_NAME_MAX_BYTES), true), 1);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn default_constructor_uses_standalone_geometry() {
        let id = format!(
            "nvs-default-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let nvs = VirtualNvs::new(&id).unwrap();
        let path = nvs.backing_path().to_owned();
        assert_eq!(nvs.partition_size(), STANDALONE_NVS_SIZE);
        drop(nvs);
        if path.exists() {
            fs::remove_file(path).unwrap();
        }
    }

    #[test]
    fn malformed_json_is_reported_instead_of_silently_erasing_nvs() {
        let path = test_path("corrupt");
        fs::write(&path, b"{not json").unwrap();
        let error = VirtualNvs::at_path("corrupt", STANDALONE_NVS_SIZE, path.clone()).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn unsafe_instance_ids_cannot_escape_the_temp_root() {
        assert_eq!(instance_directory("../../node"), "%2E%2E%2F%2E%2E%2Fnode");
        assert_eq!(instance_directory("normal-node_1"), "normal-node_1");
    }
}
