//! Host-filesystem-backed storage emulation for firmware instances.

pub mod manager;
pub mod sdcard;
pub mod spiffs;

pub use manager::StorageManager;
pub use sdcard::{
    SdCardInfo, SdFilesystem, SdPartitionTable, VirtualSdCard, FAT32_MAX_FILE_SIZE,
    FAT32_MAX_VOLUME_SIZE, SDHC_MAX_CAPACITY, TDECK_LORA_CS_PIN, TDECK_SDCARD_CS_PIN,
};
pub use spiffs::{
    SpiffsInfo, VirtualSpiffs, DEFAULT_SPIFFS_BLOCK_SIZE, DEFAULT_SPIFFS_PARTITION_SIZE,
    SPIFFS_MAX_FILENAME_CHARS,
};

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
        spiffs.write_file("/settings.bin", b"mycelium").unwrap();
        assert_eq!(spiffs.read_file("settings.bin").unwrap(), b"mycelium");
        assert_eq!(
            spiffs.list_dir("/").unwrap(),
            vec!["settings.bin".to_owned()]
        );

        spiffs.delete_file("/settings.bin").unwrap();
        assert!(spiffs.read_file("/settings.bin").is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn spiffs_rejects_hierarchical_paths_and_does_not_create_directories() {
        let root = test_directory("spiffs-flat");
        let mut spiffs = VirtualSpiffs::at_path("test", root.clone());
        spiffs.mount().unwrap();

        for path in ["dir/file", "/dir/file", "//file", r"dir\file"] {
            assert_eq!(
                spiffs.write_file(path, b"no").unwrap_err().kind(),
                std::io::ErrorKind::InvalidInput
            );
        }
        assert_eq!(spiffs.list_dir("/").unwrap(), Vec::<String>::new());
        assert!(!root.join("dir").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn spiffs_enforces_a_32_character_filename_limit() {
        let root = test_directory("spiffs-name-limit");
        let mut spiffs = VirtualSpiffs::at_path("test", root.clone());
        spiffs.mount().unwrap();

        spiffs.write_file(&"x".repeat(32), b"ok").unwrap();
        assert_eq!(
            spiffs
                .write_file(&"x".repeat(33), b"no")
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::InvalidInput
        );
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
        spiffs.write_file("/two", b"2").unwrap();
        spiffs.format().unwrap();

        assert!(spiffs.is_mounted());
        assert!(spiffs.list_dir("/").unwrap().is_empty());
        assert!(spiffs.read_file("/one").is_err());
        assert_eq!(spiffs.total_write_cycles(), 0);
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

    #[test]
    fn spiffs_reports_bounded_partition_capacity_and_rejects_overflow() {
        let root = test_directory("spiffs-capacity");
        let mut spiffs = VirtualSpiffs::at_path_with_capacity("test", root.clone(), 8, 4);
        spiffs.mount().unwrap();

        assert_eq!(spiffs.info().total_bytes, 8);
        spiffs.write_file("first", b"12345").unwrap();
        assert_eq!(spiffs.info().used_bytes, 5);
        assert_eq!(spiffs.info().free_bytes, 3);
        assert_eq!(
            spiffs.write_file("second", b"1234").unwrap_err().kind(),
            std::io::ErrorKind::StorageFull
        );
        spiffs.write_file("first", b"12345678").unwrap();
        assert_eq!(spiffs.info().free_bytes, 0);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn spiffs_tracks_write_cycles_per_erase_block() {
        let root = test_directory("spiffs-wear");
        let mut spiffs = VirtualSpiffs::at_path_with_capacity("test", root.clone(), 16, 4);
        spiffs.mount().unwrap();

        spiffs.write_file("wear.bin", b"12345").unwrap();
        assert_eq!(spiffs.block_count(), 4);
        assert_eq!(spiffs.total_write_cycles(), 2);
        assert_eq!(
            (0..spiffs.block_count())
                .filter(|&block| spiffs.block_write_cycles(block).unwrap() == 1)
                .count(),
            2
        );
        spiffs.write_file("wear.bin", b"1").unwrap();
        assert_eq!(spiffs.total_write_cycles(), 4);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn sdcard_rejects_non_sdhc_and_oversized_fat32_volumes() {
        let root = test_directory("sdcard-volume-limit");
        let mut sdcard = VirtualSdCard::at_path_with_capacity(
            "test",
            root.clone(),
            super::SDHC_MAX_CAPACITY + 1,
        );

        assert_eq!(
            sdcard.mount().unwrap_err().kind(),
            std::io::ErrorKind::InvalidInput
        );
        assert!(!root.exists());
    }

    #[test]
    fn sdcard_enforces_capacity_without_using_host_free_space() {
        let root = test_directory("sdcard-capacity");
        let mut sdcard = VirtualSdCard::at_path_with_capacity("test", root.clone(), 8);
        sdcard.mount().unwrap();

        sdcard.write_file("first", b"12345").unwrap();
        assert_eq!(sdcard.info().unwrap().total_bytes, 8);
        assert_eq!(sdcard.info().unwrap().free_bytes, 3);
        assert_eq!(
            sdcard.write_file("second", b"1234").unwrap_err().kind(),
            std::io::ErrorKind::StorageFull
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn sdcard_requires_gpio9_high_and_models_gpio39_chip_select() {
        let root = test_directory("sdcard-shared-spi");
        let mut sdcard = VirtualSdCard::at_path_with_capacity("test", root.clone(), 1024);

        assert_eq!(sdcard.lora_chip_select_pin(), super::TDECK_LORA_CS_PIN);
        assert_eq!(sdcard.chip_select_pin(), super::TDECK_SDCARD_CS_PIN);
        assert_eq!(sdcard.partition_table(), super::SdPartitionTable::Mbr);
        assert_eq!(sdcard.filesystem(), super::SdFilesystem::Fat32);
        sdcard.set_lora_cs_high(false);
        assert_eq!(
            sdcard.mount().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
        sdcard.set_lora_cs_high(true);
        sdcard.mount().unwrap();
        sdcard.write_file("file", b"data").unwrap();
        sdcard.set_lora_cs_high(false);
        assert_eq!(
            sdcard.read_file("file").unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
        assert_eq!(
            sdcard.write_file("file", b"new").unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
        fs::remove_dir_all(root).unwrap();
    }
}
