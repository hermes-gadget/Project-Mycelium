//! Host-filesystem-backed storage emulation for firmware instances.

pub mod manager;
pub mod sdcard;
pub mod spiffs;

pub use manager::StorageManager;
pub use sdcard::{SdCardInfo, VirtualSdCard};
pub use spiffs::{SpiffsInfo, VirtualSpiffs};

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::Arc;
    use std::thread;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::{VirtualSdCard, VirtualSpiffs};

    fn test_directory(name: &str) -> std::path::PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "mycelium-storage-{name}-{}-{nonce}",
            std::process::id()
        ))
    }

    #[test]
    fn spiffs_create_read_write_delete_round_trip() {
        let root = test_directory("spiffs-round-trip");
        let mut spiffs = VirtualSpiffs::at_path("test", root.clone());

        assert!(spiffs.mount().unwrap());
        spiffs
            .write_file("/config/settings.bin", b"mycelium")
            .unwrap();
        assert_eq!(
            spiffs.read_file("config/settings.bin").unwrap(),
            b"mycelium"
        );
        assert_eq!(
            spiffs.list_dir("/config").unwrap(),
            vec!["settings.bin".to_owned()]
        );

        spiffs.delete_file("/config/settings.bin").unwrap();
        assert!(spiffs.read_file("/config/settings.bin").is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn sdcard_lists_files_in_stable_order() {
        let root = test_directory("sdcard-listing");
        let mut sdcard = VirtualSdCard::at_path("test", root.clone());

        sdcard.mount().unwrap();
        sdcard.write_file("/logs/z-last.txt", b"z").unwrap();
        sdcard.write_file("/logs/a-first.txt", b"a").unwrap();

        assert_eq!(
            sdcard.list_dir("/logs").unwrap(),
            vec!["a-first.txt".to_owned(), "z-last.txt".to_owned()]
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn format_clears_all_spiffs_data_and_keeps_it_mounted() {
        let root = test_directory("spiffs-format");
        let mut spiffs = VirtualSpiffs::at_path("test", root.clone());

        spiffs.mount().unwrap();
        spiffs.write_file("/one", b"1").unwrap();
        spiffs.write_file("/nested/two", b"2").unwrap();
        spiffs.format().unwrap();

        assert!(spiffs.is_mounted());
        assert!(spiffs.list_dir("/").unwrap().is_empty());
        assert!(spiffs.read_file("/one").is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn concurrent_instances_do_not_conflict() {
        let root = test_directory("concurrent");
        let first = root.join("first");
        let second = root.join("second");
        let barrier = Arc::new(std::sync::Barrier::new(2));

        let threads: Vec<_> = [("first", first), ("second", second)]
            .into_iter()
            .map(|(id, path)| {
                let barrier = Arc::clone(&barrier);
                thread::spawn(move || {
                    let mut spiffs = VirtualSpiffs::at_path(id, path);
                    spiffs.mount().unwrap();
                    barrier.wait();
                    spiffs.write_file("/shared-name", id.as_bytes()).unwrap();
                    spiffs.read_file("/shared-name").unwrap()
                })
            })
            .collect();

        let results: Vec<_> = threads
            .into_iter()
            .map(|thread| thread.join().unwrap())
            .collect();
        assert_eq!(results, [b"first".to_vec(), b"second".to_vec()]);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn paths_cannot_escape_an_instance_directory() {
        let root = test_directory("path-traversal");
        let mut spiffs = VirtualSpiffs::at_path("test", root.clone());
        spiffs.mount().unwrap();

        assert_eq!(
            spiffs.write_file("../outside", b"no").unwrap_err().kind(),
            std::io::ErrorKind::InvalidInput
        );
        assert!(!root.parent().unwrap().join("outside").exists());
        fs::remove_dir_all(root).unwrap();
    }
}
