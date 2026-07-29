//! C ABI and MeshCore C++ adapters for Project Mycelium's virtual radio bus.

mod ffi;

pub use ffi::{
    meshemu_board_create, meshemu_board_destroy, meshemu_board_get_battery, meshemu_board_get_temp,
    meshemu_board_set_battery, meshemu_bus_tick, meshemu_display_capture,
    meshemu_display_capture_free, meshemu_display_create, meshemu_display_create_v,
    meshemu_display_destroy, meshemu_gps_create, meshemu_gps_destroy, meshemu_gps_read,
    meshemu_gps_set_enabled, meshemu_gps_set_position, meshemu_i2c_keyboard_create,
    meshemu_i2c_keyboard_destroy, meshemu_i2c_keyboard_inject_key, meshemu_input_inject_key,
    meshemu_input_inject_touch, meshemu_input_poll_key, meshemu_input_poll_touch,
    meshemu_radio_create, meshemu_radio_destroy, meshemu_radio_get_est_airtime,
    meshemu_radio_get_rssi, meshemu_radio_get_snr, meshemu_radio_is_send_complete,
    meshemu_radio_recv_raw, meshemu_radio_set_position, meshemu_radio_start_send,
    meshemu_sdcard_init, meshemu_sdcard_read, meshemu_sdcard_write, meshemu_spiffs_init,
    meshemu_spiffs_read, meshemu_spiffs_write, meshemu_storage_data_free, meshemu_wire_available,
    meshemu_wire_begin_transmission, meshemu_wire_end_transmission, meshemu_wire_read,
    meshemu_wire_request_from, meshemu_wire_shim_create, meshemu_wire_shim_destroy,
    meshemu_wire_shim_set_keyboard, meshemu_wire_write,
};
pub use mycelium_board::{meshemu_buzzer_beep, meshemu_buzzer_is_playing, meshemu_buzzer_stop};

#[cfg(test)]
mod tests {
    use std::ffi::{c_void, CString};
    use std::sync::Mutex;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    static TEST_BUS: Mutex<()> = Mutex::new(());

    fn create(id: &str, position: (f64, f64)) -> *mut c_void {
        let id = CString::new(id).unwrap();
        unsafe { meshemu_radio_create(id.as_ptr(), 915.0, 125, 7, 5, 14.0, position.0, position.1) }
    }

    fn receive(radio: *mut c_void) -> Vec<u8> {
        let mut buffer = [0_u8; 255];
        let len =
            unsafe { meshemu_radio_recv_raw(radio, buffer.as_mut_ptr(), buffer.len() as i32) };
        buffer[..len.max(0) as usize].to_vec()
    }

    fn send(radio: *mut c_void, packet: &[u8]) -> bool {
        unsafe { meshemu_radio_start_send(radio, packet.as_ptr(), packet.len() as u32) }
    }

    fn destroy(radio: *mut c_void) {
        unsafe { meshemu_radio_destroy(radio) }
    }

    #[test]
    fn two_radios_exchange_a_packet_and_report_link_metrics() {
        let _serial = TEST_BUS.lock().unwrap();
        ffi::reset_bus();
        let sender = create("sender", (51.5074, -0.1278));
        let receiver = create("receiver", (51.5075, -0.1278));
        assert!(!sender.is_null());
        assert!(!receiver.is_null());

        let packet = [1, 2, 3, 4];
        assert!(send(sender, &packet));
        assert_eq!(receive(receiver), packet);
        assert!(unsafe { meshemu_radio_get_rssi(receiver) } < 0.0);
        assert!(unsafe { meshemu_radio_get_snr(receiver) } > 0.0);
        assert_eq!(unsafe { meshemu_radio_get_est_airtime(sender, 16) }, 56);
        assert!(meshemu_radio_is_send_complete(sender));

        destroy(receiver);
        destroy(sender);
    }

    #[test]
    fn moving_a_radio_out_of_range_prevents_reception() {
        let _serial = TEST_BUS.lock().unwrap();
        ffi::reset_bus();
        let sender = create("sender", (0.0, 0.0));
        let receiver = create("receiver", (0.0, 0.0001));

        unsafe { meshemu_radio_set_position(receiver, 0.0, 180.0) };
        meshemu_bus_tick(1_000);
        let packet = [42];
        assert!(send(sender, &packet));
        assert!(receive(receiver).is_empty());

        destroy(receiver);
        destroy(sender);
    }

    #[test]
    fn position_updates_can_bring_a_radio_into_range() {
        let _serial = TEST_BUS.lock().unwrap();
        ffi::reset_bus();
        let sender = create("sender", (0.0, 0.0));
        let receiver = create("receiver", (0.0, 180.0));
        let packet = [7, 8, 9];

        assert!(send(sender, &packet));
        assert!(receive(receiver).is_empty());

        unsafe { meshemu_radio_set_position(receiver, 0.0, 0.0001) };
        meshemu_bus_tick(1_000);
        assert!(send(sender, &packet));
        assert_eq!(receive(receiver), packet);

        destroy(receiver);
        destroy(sender);
    }

    #[test]
    fn destroy_unregisters_the_node_and_releases_its_identifier() {
        let _serial = TEST_BUS.lock().unwrap();
        ffi::reset_bus();
        let first = create("reusable", (0.0, 0.0));
        assert!(!first.is_null());
        assert!(create("reusable", (0.0, 0.0)).is_null());

        destroy(first);
        let replacement = create("reusable", (0.0, 0.0));
        assert!(!replacement.is_null());
        destroy(replacement);
    }

    #[test]
    fn null_and_invalid_ffi_arguments_fail_safely() {
        let _serial = TEST_BUS.lock().unwrap();
        ffi::reset_bus();

        assert!(unsafe {
            meshemu_radio_create(std::ptr::null(), 915.0, 125, 7, 5, 14.0, 0.0, 0.0)
        }
        .is_null());
        assert!(!unsafe { meshemu_radio_start_send(std::ptr::null_mut(), std::ptr::null(), 1) });
        assert_eq!(
            unsafe { meshemu_radio_recv_raw(std::ptr::null_mut(), std::ptr::null_mut(), 0) },
            0
        );
        destroy(std::ptr::null_mut());
    }

    #[test]
    fn display_ffi_rejects_null_and_invalid_arguments() {
        let title = CString::new("node1").unwrap();
        assert!(unsafe { meshemu_display_create(640, 480, std::ptr::null()) }.is_null());
        assert!(unsafe { meshemu_display_create(0, 240, title.as_ptr()) }.is_null());
        assert!(unsafe { meshemu_display_create_v(320, 200, title.as_ptr(), 8) }.is_null());
        assert!(unsafe { meshemu_display_create_v(320, 240, title.as_ptr(), 7) }.is_null());

        let mut size = usize::MAX;
        assert!(unsafe { meshemu_display_capture(std::ptr::null_mut(), &mut size) }.is_null());
        assert_eq!(size, 0);
        unsafe {
            meshemu_display_capture_free(std::ptr::null_mut(), 0);
            meshemu_display_destroy(std::ptr::null_mut());
        }
    }

    #[test]
    fn keyboard_and_wire_ffi_share_injected_matrix_state() {
        let keyboard = meshemu_i2c_keyboard_create();
        let wire = meshemu_wire_shim_create();
        assert!(!keyboard.is_null());
        assert!(!wire.is_null());

        unsafe {
            meshemu_wire_shim_set_keyboard(wire, keyboard);
            meshemu_i2c_keyboard_inject_key(keyboard, 0, 1, 1);
            meshemu_i2c_keyboard_inject_key(keyboard, 0, 6, 1);
            meshemu_wire_begin_transmission(wire, 0x55);
            assert_eq!(meshemu_wire_write(wire, 0), 1);
            assert_eq!(meshemu_wire_end_transmission(wire), 0);
            assert_eq!(meshemu_wire_request_from(wire, 0x55, 1), 1);
            assert_eq!(meshemu_wire_available(wire), 1);
            assert_eq!(meshemu_wire_read(wire), 0b0100_0010);
            assert_eq!(meshemu_wire_read(wire), -1);

            // The shim owns a shared reference, so destroying the creator's
            // handle does not invalidate an attached Wire instance.
            meshemu_i2c_keyboard_destroy(keyboard);
            meshemu_wire_begin_transmission(wire, 0x55);
            meshemu_wire_write(wire, 4);
            assert_eq!(meshemu_wire_request_from(wire, 0x55, 1), 1);
            assert_eq!(meshemu_wire_read(wire), 1);
            meshemu_wire_shim_destroy(wire);
        }
    }

    #[test]
    fn wire_ffi_rejects_null_handles_and_non_keyboard_addresses() {
        unsafe {
            meshemu_i2c_keyboard_inject_key(std::ptr::null_mut(), 0, 0, 1);
            meshemu_wire_shim_set_keyboard(std::ptr::null_mut(), std::ptr::null_mut());
            assert_eq!(meshemu_wire_write(std::ptr::null_mut(), 0), 0);
            assert_eq!(meshemu_wire_end_transmission(std::ptr::null_mut()), 4);
            assert_eq!(meshemu_wire_request_from(std::ptr::null_mut(), 0x55, 1), 0);
            assert_eq!(meshemu_wire_available(std::ptr::null_mut()), 0);
            assert_eq!(meshemu_wire_read(std::ptr::null_mut()), -1);
            meshemu_i2c_keyboard_destroy(std::ptr::null_mut());
            meshemu_wire_shim_destroy(std::ptr::null_mut());
        }

        let wire = meshemu_wire_shim_create();
        unsafe {
            assert_eq!(meshemu_wire_request_from(wire, 0x42, 1), 0);
            assert_eq!(meshemu_wire_available(wire), 0);
            meshemu_wire_shim_destroy(wire);
        }
    }

    #[test]
    fn storage_ffi_round_trips_spiffs_and_sdcard_files() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let id = CString::new(format!("ffi-storage-{}-{nonce}", std::process::id())).unwrap();
        let path = CString::new("/nested/data.bin").unwrap();
        let spiffs_data = b"spiffs";
        let sdcard_data = b"sdcard";

        unsafe {
            assert!(meshemu_spiffs_init(id.as_ptr()));
            assert!(meshemu_spiffs_write(
                id.as_ptr(),
                path.as_ptr(),
                spiffs_data.as_ptr(),
                spiffs_data.len()
            ));
            let mut len = usize::MAX;
            let data = meshemu_spiffs_read(id.as_ptr(), path.as_ptr(), &mut len);
            assert!(!data.is_null());
            assert_eq!(std::slice::from_raw_parts(data, len), spiffs_data);
            meshemu_storage_data_free(data);

            assert!(meshemu_sdcard_init(id.as_ptr()));
            assert!(meshemu_sdcard_write(
                id.as_ptr(),
                path.as_ptr(),
                sdcard_data.as_ptr(),
                sdcard_data.len()
            ));
            let data = meshemu_sdcard_read(id.as_ptr(), path.as_ptr(), &mut len);
            assert!(!data.is_null());
            assert_eq!(std::slice::from_raw_parts(data, len), sdcard_data);
            meshemu_storage_data_free(data);
        }
    }

    #[test]
    fn storage_ffi_rejects_null_arguments() {
        let mut len = usize::MAX;
        unsafe {
            assert!(!meshemu_spiffs_init(std::ptr::null()));
            assert!(meshemu_spiffs_read(std::ptr::null(), std::ptr::null(), &mut len).is_null());
            assert_eq!(len, 0);
            assert!(!meshemu_spiffs_write(
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                1
            ));
            meshemu_storage_data_free(std::ptr::null_mut());
        }
    }

    #[test]
    fn input_ffi_injects_and_consumes_packed_events() {
        let id = CString::new("ffi-input-node").unwrap();
        unsafe {
            meshemu_input_inject_touch(id.as_ptr(), 123, 45, true);
        }
        assert_eq!(
            unsafe { meshemu_input_poll_touch(id.as_ptr()) },
            123 | (45 << 16) | (255 << 32)
        );
        assert_eq!(unsafe { meshemu_input_poll_touch(id.as_ptr()) }, 0);

        unsafe {
            meshemu_input_inject_key(
                id.as_ptr(),
                i32::from(sdl2::keyboard::Keycode::Q) as u32,
                true,
            );
        }
        assert_eq!(unsafe { meshemu_input_poll_key(id.as_ptr()) }, 1 << 16);
        assert_eq!(unsafe { meshemu_input_poll_key(id.as_ptr()) }, 0);
        mycelium_input::remove_input_manager("ffi-input-node");
    }

    #[test]
    fn gps_ffi_streams_nmea_and_can_be_disabled() {
        let id = CString::new("ffi-gps-node").unwrap();
        let gps = unsafe { meshemu_gps_create(id.as_ptr(), 51.5074, -0.1278) };
        assert!(!gps.is_null());
        let mut buffer = [0_u8; 256];

        let len = unsafe { meshemu_gps_read(gps, buffer.as_mut_ptr(), buffer.len() as i32) };
        let sentence = std::str::from_utf8(&buffer[..len as usize]).unwrap();
        assert!(sentence.starts_with("$GPGGA,"));
        assert!(sentence.contains(",5130.4440,N,00007.6680,W,"));

        unsafe {
            meshemu_gps_set_position(gps, -33.8688, 151.2093, 58.0);
            meshemu_gps_set_enabled(gps, false);
            assert_eq!(
                meshemu_gps_read(gps, buffer.as_mut_ptr(), buffer.len() as i32),
                0
            );
            meshemu_gps_destroy(gps);
        }
    }

    #[test]
    fn board_ffi_exposes_battery_and_temperature() {
        let id = CString::new("ffi-board-node").unwrap();
        let board = unsafe { meshemu_board_create(id.as_ptr(), 3_850, 37.5) };
        assert!(!board.is_null());

        unsafe {
            assert_eq!(meshemu_board_get_battery(board), 3_850);
            assert_eq!(meshemu_board_get_temp(board), 37.5);
            meshemu_board_set_battery(board, 3_700);
            assert_eq!(meshemu_board_get_battery(board), 3_700);
            meshemu_board_destroy(board);
        }
    }

    #[test]
    fn gps_and_board_ffi_reject_invalid_arguments() {
        let id = CString::new("ffi-invalid-node").unwrap();
        unsafe {
            assert!(meshemu_gps_create(std::ptr::null(), 0.0, 0.0).is_null());
            assert!(meshemu_gps_create(id.as_ptr(), 91.0, 0.0).is_null());
            assert_eq!(
                meshemu_gps_read(std::ptr::null_mut(), std::ptr::null_mut(), 0),
                0
            );
            assert!(meshemu_board_create(std::ptr::null(), 3_900, 35.0).is_null());
            assert!(meshemu_board_create(id.as_ptr(), 3_900, f32::NAN).is_null());
            assert_eq!(meshemu_board_get_battery(std::ptr::null_mut()), 0);
            assert_eq!(meshemu_board_get_temp(std::ptr::null_mut()), 0.0);
            meshemu_gps_destroy(std::ptr::null_mut());
            meshemu_board_destroy(std::ptr::null_mut());
        }
    }
}
