//! C ABI and MeshCore C++ adapters for Project Mycelium's virtual radio bus.

mod ffi;

pub use ffi::{
    meshemu_board_create, meshemu_board_deep_sleep, meshemu_board_destroy,
    meshemu_board_digital_write, meshemu_board_get_adc, meshemu_board_get_battery,
    meshemu_board_get_charger_state, meshemu_board_get_last_boot_phase,
    meshemu_board_get_psram_free, meshemu_board_get_sleep_wake_cause, meshemu_board_get_temp,
    meshemu_board_ledc_attach, meshemu_board_ledc_write, meshemu_board_psram_found,
    meshemu_board_psram_readback_test, meshemu_board_psram_release, meshemu_board_psram_reserve,
    meshemu_board_rtc_gpio_hold, meshemu_board_set_adc_calibration, meshemu_board_set_battery,
    meshemu_board_set_boot_phase, meshemu_board_set_external_power, meshemu_board_set_periph_power,
    meshemu_bus_tick, meshemu_display_capture, meshemu_display_capture_free,
    meshemu_display_create, meshemu_display_create_ex, meshemu_display_create_v,
    meshemu_display_destroy, meshemu_gps_create, meshemu_gps_destroy, meshemu_gps_read,
    meshemu_gps_set_enabled, meshemu_gps_set_position, meshemu_gps_tick,
    meshemu_i2c_keyboard_create, meshemu_i2c_keyboard_destroy,
    meshemu_i2c_keyboard_inject_key_byte, meshemu_i2c_keyboard_set_cross_reset,
    meshemu_input_digital_read, meshemu_input_get_touch_mapped, meshemu_input_get_touch_raw,
    meshemu_input_gt911_get_status, meshemu_input_gt911_set_failure_mode, meshemu_input_inject_key,
    meshemu_input_inject_touch, meshemu_input_poll_key, meshemu_input_poll_touch,
    meshemu_input_take_falling_edges, meshemu_radio_create, meshemu_radio_destroy,
    meshemu_radio_get_dio2_config, meshemu_radio_get_est_airtime, meshemu_radio_get_rssi,
    meshemu_radio_get_snr, meshemu_radio_is_send_complete, meshemu_radio_recv_raw,
    meshemu_radio_set_dio2_config, meshemu_radio_set_position, meshemu_radio_start_send,
    meshemu_sdcard_init, meshemu_sdcard_read, meshemu_sdcard_set_behavior, meshemu_sdcard_write,
    meshemu_spiffs_init, meshemu_spiffs_read, meshemu_spiffs_write, meshemu_storage_data_free,
    meshemu_storage_destroy, meshemu_wire_available, meshemu_wire_begin,
    meshemu_wire_begin_transmission, meshemu_wire_end_transmission, meshemu_wire_probe_address,
    meshemu_wire_read, meshemu_wire_read_idle_levels, meshemu_wire_request_from,
    meshemu_wire_set_clock, meshemu_wire_shim_create, meshemu_wire_shim_create_for_instance,
    meshemu_wire_shim_destroy, meshemu_wire_shim_set_keyboard, meshemu_wire_write,
};
pub use mycelium_board::{meshemu_buzzer_beep, meshemu_buzzer_is_playing, meshemu_buzzer_stop};

#[cfg(test)]
mod tests {
    use std::ffi::{c_void, CString};
    use std::sync::Mutex;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    static TEST_BUS: Mutex<()> = Mutex::new(());
    static TEST_STORAGE: Mutex<()> = Mutex::new(());

    fn create(id: &str, position: (f64, f64)) -> *mut c_void {
        let id = CString::new(id).unwrap();
        unsafe { meshemu_radio_create(id.as_ptr(), 915.0, 125, 7, 5, 14.0, position.0, position.1) }
    }

    fn receive(radio: *mut c_void) -> Vec<u8> {
        let mut buffer = [0_u8; 255];
        let mut truncated = false;
        let len = unsafe {
            meshemu_radio_recv_raw(
                radio,
                buffer.as_mut_ptr(),
                buffer.len() as i32,
                &mut truncated,
            )
        };
        assert!(!truncated);
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
        assert!(receive(receiver).is_empty());
        assert!(!unsafe { meshemu_radio_is_send_complete(sender) });
        let airtime = unsafe { meshemu_radio_get_est_airtime(sender, packet.len() as i32) };
        meshemu_bus_tick(u64::from(airtime));
        assert_eq!(receive(receiver), packet);
        assert!(unsafe { meshemu_radio_get_rssi(receiver) } < 0.0);
        assert!(unsafe { meshemu_radio_get_snr(receiver) } > 0.0);
        assert_eq!(unsafe { meshemu_radio_get_est_airtime(sender, 16) }, 52);
        assert!(unsafe { meshemu_radio_is_send_complete(sender) });

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
        let airtime = unsafe { meshemu_radio_get_est_airtime(sender, packet.len() as i32) };
        meshemu_bus_tick(u64::from(airtime));
        assert!(receive(receiver).is_empty());

        unsafe { meshemu_radio_set_position(receiver, 0.0, 0.0001) };
        meshemu_bus_tick(1_000);
        assert!(send(sender, &packet));
        meshemu_bus_tick(1_000 + u64::from(airtime));
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
            unsafe {
                meshemu_radio_recv_raw(
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    0,
                    std::ptr::null_mut(),
                )
            },
            0
        );
        destroy(std::ptr::null_mut());
    }

    #[test]
    fn invalid_sx1262_configs_are_rejected_without_panicking() {
        let _serial = TEST_BUS.lock().unwrap();
        ffi::reset_bus();

        let invalid = [
            (149.9, 125, 7, 5, 14.0),
            (960.1, 125, 7, 5, 14.0),
            (915.0, 62, 7, 5, 14.0),
            (915.0, 125, 6, 5, 14.0),
            (915.0, 125, 64, 5, 14.0),
            (915.0, 125, 7, 4, 14.0),
            (915.0, 125, 7, 9, 14.0),
            (915.0, 125, 7, 5, 22.1),
        ];
        for (index, (freq, bandwidth, sf, coding_rate, power)) in invalid.into_iter().enumerate() {
            let id = CString::new(format!("invalid-{index}")).unwrap();
            let radio = unsafe {
                meshemu_radio_create(
                    id.as_ptr(),
                    freq,
                    bandwidth,
                    sf,
                    coding_rate,
                    power,
                    0.0,
                    0.0,
                )
            };
            assert!(radio.is_null());
        }
    }

    #[test]
    fn overlapping_send_is_rejected_until_airtime_elapses() {
        let _serial = TEST_BUS.lock().unwrap();
        ffi::reset_bus();
        let sender = create("sender", (0.0, 0.0));
        let packet = [1, 2, 3, 4];
        let airtime = unsafe { meshemu_radio_get_est_airtime(sender, packet.len() as i32) };

        assert!(send(sender, &packet));
        assert!(!send(sender, &packet));
        meshemu_bus_tick(u64::from(airtime - 1));
        assert!(!unsafe { meshemu_radio_is_send_complete(sender) });
        assert!(!send(sender, &packet));
        meshemu_bus_tick(u64::from(airtime));
        assert!(unsafe { meshemu_radio_is_send_complete(sender) });
        assert!(send(sender, &packet));

        destroy(sender);
    }

    #[test]
    fn undersized_receive_buffer_leaves_packet_queued_for_retry() {
        let _serial = TEST_BUS.lock().unwrap();
        ffi::reset_bus();
        let sender = create("sender", (0.0, 0.0));
        let receiver = create("receiver", (0.0, 0.0001));
        let packet = [1, 2, 3, 4];

        assert!(send(sender, &packet));
        let airtime = unsafe { meshemu_radio_get_est_airtime(sender, packet.len() as i32) };
        meshemu_bus_tick(u64::from(airtime));

        let mut small = [0_u8; 2];
        let mut truncated = false;
        assert_eq!(
            unsafe {
                meshemu_radio_recv_raw(
                    receiver,
                    small.as_mut_ptr(),
                    small.len() as i32,
                    &mut truncated,
                )
            },
            -4
        );
        assert!(truncated);
        assert_eq!(receive(receiver), packet);

        destroy(receiver);
        destroy(sender);
    }

    #[test]
    fn radio_initialization_models_sx1262_board_controls() {
        let _serial = TEST_BUS.lock().unwrap();
        ffi::reset_bus();
        let radio = create("sx1262-state", (0.0, 0.0));

        let state = unsafe { ffi::radio_state(radio) }.unwrap();
        assert!(!state.dio2_rf_switch_enabled);
        assert!(!unsafe { meshemu_radio_get_dio2_config(radio) });
        assert_eq!(state.dio3_tcxo_voltage_v, Some(1.8));

        unsafe { meshemu_radio_set_dio2_config(radio, true) };
        assert!(unsafe { meshemu_radio_get_dio2_config(radio) });

        destroy(radio);
    }

    #[test]
    fn dio2_ffi_switch_restores_sixteen_db_tx_path() {
        let _serial = TEST_BUS.lock().unwrap();
        ffi::reset_bus();
        let sender = create("dio2-sender", (0.0, 0.0));
        let receiver = create("dio2-receiver", (0.0, 0.0001));
        unsafe { meshemu_radio_set_dio2_config(receiver, true) };
        let packet = [42];
        let airtime = unsafe { meshemu_radio_get_est_airtime(sender, packet.len() as i32) };

        assert!(send(sender, &packet));
        meshemu_bus_tick(u64::from(airtime));
        assert_eq!(receive(receiver), packet);
        let lossy_rssi = unsafe { meshemu_radio_get_rssi(receiver) };

        unsafe { meshemu_radio_set_dio2_config(sender, true) };
        assert!(send(sender, &packet));
        meshemu_bus_tick(2 * u64::from(airtime));
        assert_eq!(receive(receiver), packet);
        let normal_rssi = unsafe { meshemu_radio_get_rssi(receiver) };
        assert!((normal_rssi - lossy_rssi - 16.0).abs() < 0.01);

        destroy(receiver);
        destroy(sender);
    }

    #[test]
    fn bus_time_does_not_move_backward() {
        let _serial = TEST_BUS.lock().unwrap();
        ffi::reset_bus();
        let sender = create("sender", (0.0, 0.0));
        let packet = [1, 2, 3, 4];

        meshemu_bus_tick(1_000);
        assert!(send(sender, &packet));
        let airtime = unsafe { meshemu_radio_get_est_airtime(sender, packet.len() as i32) };
        meshemu_bus_tick(10);
        assert!(!unsafe { meshemu_radio_is_send_complete(sender) });
        assert!(!send(sender, &packet));
        meshemu_bus_tick(1_000 + u64::from(airtime));
        assert!(unsafe { meshemu_radio_is_send_complete(sender) });

        destroy(sender);
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
    fn keyboard_and_wire_ffi_share_injected_key_bytes() {
        let keyboard = meshemu_i2c_keyboard_create();
        let wire = meshemu_wire_shim_create();
        assert!(!keyboard.is_null());
        assert!(!wire.is_null());

        unsafe {
            meshemu_wire_shim_set_keyboard(wire, keyboard);
            assert!(meshemu_wire_begin(wire));
            meshemu_wire_set_clock(wire, 100_000);
            meshemu_i2c_keyboard_inject_key_byte(keyboard, b'w');
            meshemu_i2c_keyboard_inject_key_byte(keyboard, b'U');
            meshemu_wire_begin_transmission(wire, 0x55);
            assert_eq!(meshemu_wire_write(wire, 0x04), 1);
            assert_eq!(meshemu_wire_end_transmission(wire), 0);
            assert_eq!(meshemu_wire_request_from(wire, 0x55, 1), 1);
            assert_eq!(meshemu_wire_available(wire), 1);
            assert_eq!(meshemu_wire_read(wire), i32::from(b'w'));
            assert_eq!(meshemu_wire_read(wire), -1);

            // The shim owns a shared reference, so destroying the creator's
            // handle does not invalidate an attached Wire instance.
            meshemu_i2c_keyboard_destroy(keyboard);
            assert_eq!(meshemu_wire_request_from(wire, 0x55, 1), 1);
            assert_eq!(meshemu_wire_read(wire), i32::from(b'U'));
            meshemu_wire_shim_destroy(wire);
        }
    }

    #[test]
    fn instance_wire_observes_input_manager_keyboard_and_touch_state() {
        let id = CString::new("ffi-shared-input").unwrap();
        let manager = mycelium_input::register_input_manager("ffi-shared-input", 1.0);
        let wire = unsafe { meshemu_wire_shim_create_for_instance(id.as_ptr()) };
        assert!(!wire.is_null());

        {
            let mut manager = manager.lock().unwrap();
            manager.inject_key(sdl2::keyboard::Keycode::Q, true);
            manager.inject_touch(100, 40, true);
        }

        unsafe {
            assert!(meshemu_wire_begin(wire));
            meshemu_wire_set_clock(wire, 100_000);
            assert!(meshemu_wire_probe_address(
                wire,
                mycelium_input::KEYBOARD_I2C_ADDRESS
            ));
            assert!(meshemu_wire_probe_address(
                wire,
                mycelium_input::GT911_I2C_ADDRESS
            ));
            assert!(!meshemu_wire_probe_address(wire, 0x14));
            let mut sda = 0;
            let mut scl = 0;
            meshemu_wire_read_idle_levels(wire, &mut sda, &mut scl);
            assert_eq!((sda, scl), (1, 1));
            meshemu_wire_begin_transmission(wire, mycelium_input::KEYBOARD_I2C_ADDRESS);
            meshemu_wire_write(wire, mycelium_input::KEYBOARD_KEY_MODE_COMMAND);
            assert_eq!(meshemu_wire_end_transmission(wire), 0);
            assert_eq!(
                meshemu_wire_request_from(wire, mycelium_input::KEYBOARD_I2C_ADDRESS, 1),
                1
            );
            assert_eq!(meshemu_wire_read(wire), i32::from(b'q'));

            meshemu_wire_begin_transmission(wire, mycelium_input::GT911_I2C_ADDRESS);
            for byte in mycelium_input::GT911_STATUS_REGISTER.to_be_bytes() {
                meshemu_wire_write(wire, byte);
            }
            assert_eq!(meshemu_wire_end_transmission(wire), 0);
            assert_eq!(
                meshemu_wire_request_from(wire, mycelium_input::GT911_I2C_ADDRESS, 9),
                9
            );
            assert_eq!(meshemu_wire_read(wire), 0x81);
            assert!(!meshemu_input_digital_read(
                id.as_ptr(),
                mycelium_input::GT911_INT_GPIO
            ));
            meshemu_wire_shim_destroy(wire);
        }
        mycelium_input::remove_input_manager("ffi-shared-input");
    }

    #[test]
    fn gpio10_power_loss_makes_instance_wire_nack() {
        let id = CString::new("ffi-wire-power-node").unwrap();
        let board = unsafe { meshemu_board_create(id.as_ptr(), 3_900, 35.0) };
        let wire = unsafe { meshemu_wire_shim_create_for_instance(id.as_ptr()) };

        unsafe {
            assert!(meshemu_wire_begin(wire));
            meshemu_wire_set_clock(wire, 100_000);
            meshemu_wire_begin_transmission(wire, mycelium_input::KEYBOARD_I2C_ADDRESS);
            assert_eq!(meshemu_wire_write(wire, 0x04), 1);
            assert_eq!(meshemu_wire_end_transmission(wire), 0);

            meshemu_board_set_periph_power(board, false);
            meshemu_wire_begin_transmission(wire, mycelium_input::KEYBOARD_I2C_ADDRESS);
            assert_eq!(meshemu_wire_write(wire, 0x04), 0);
            assert_eq!(meshemu_wire_end_transmission(wire), 0x02);
            assert_eq!(
                meshemu_wire_request_from(wire, mycelium_input::KEYBOARD_I2C_ADDRESS, 1),
                0
            );

            meshemu_board_set_periph_power(board, true);
            meshemu_wire_begin_transmission(wire, mycelium_input::KEYBOARD_I2C_ADDRESS);
            assert_eq!(meshemu_wire_write(wire, 0x04), 1);
            assert_eq!(meshemu_wire_end_transmission(wire), 0);

            meshemu_wire_shim_destroy(wire);
            meshemu_board_destroy(board);
        }
        mycelium_input::remove_input_manager("ffi-wire-power-node");
    }

    #[test]
    fn wire_ffi_rejects_null_handles_and_non_keyboard_addresses() {
        unsafe {
            meshemu_i2c_keyboard_inject_key_byte(std::ptr::null_mut(), 0);
            meshemu_i2c_keyboard_set_cross_reset(std::ptr::null_mut(), false);
            meshemu_wire_shim_set_keyboard(std::ptr::null_mut(), std::ptr::null_mut());
            assert!(!meshemu_wire_begin(std::ptr::null_mut()));
            meshemu_wire_set_clock(std::ptr::null_mut(), 100_000);
            assert!(!meshemu_wire_probe_address(std::ptr::null_mut(), 0x55));
            let mut sda = u8::MAX;
            let mut scl = u8::MAX;
            meshemu_wire_read_idle_levels(std::ptr::null_mut(), &mut sda, &mut scl);
            assert_eq!((sda, scl), (0, 0));
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
        let _serial = TEST_STORAGE.lock().unwrap();
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let id = CString::new(format!("ffi-storage-{}-{nonce}", std::process::id())).unwrap();
        let path = CString::new("data.bin").unwrap();
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
        let _serial = TEST_STORAGE.lock().unwrap();
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
        let mut raw = (0, 0);
        let mut mapped = (0, 0);
        unsafe {
            meshemu_input_get_touch_raw(id.as_ptr(), &mut raw.0, &mut raw.1);
            meshemu_input_get_touch_mapped(id.as_ptr(), &mut mapped.0, &mut mapped.1);
        }
        assert_eq!(raw, (123, 45));
        assert_eq!(mapped, (45, 196));
        assert_eq!(
            unsafe { meshemu_input_poll_touch(id.as_ptr()) },
            45 | (196 << 16) | (u64::from(mycelium_input::DEFAULT_GT911_CONTACT_SIZE) << 32)
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

        unsafe { meshemu_gps_tick(gps, 1_000) };
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
            assert_eq!(meshemu_board_get_adc(board, 4), 2_389);
            assert_eq!(meshemu_board_get_adc(board, 5), 0);
            assert_eq!(meshemu_board_get_temp(board), 37.5);
            meshemu_board_set_battery(board, 3_700);
            assert_eq!(meshemu_board_get_battery(board), 3_700);
            assert_eq!(meshemu_board_get_adc(board, 4), 2_296);
            meshemu_board_set_battery(board, 4_200);
            meshemu_board_set_adc_calibration(board, false);
            let uncalibrated_mv = f64::from(meshemu_board_get_adc(board, 4))
                * mycelium_board::BATTERY_MV_PER_ADC_COUNT;
            assert!((uncalibrated_mv - 3_780.0).abs() < 5.0);
            meshemu_board_set_adc_calibration(board, true);
            let calibrated_mv = f64::from(meshemu_board_get_adc(board, 4))
                * mycelium_board::BATTERY_MV_PER_ADC_COUNT;
            assert!((calibrated_mv - 4_200.0).abs() < 5.0);
            meshemu_board_set_battery(board, u16::MAX);
            assert_eq!(meshemu_board_get_adc(board, 4), 4_095);
            meshemu_board_destroy(board);
        }
    }

    #[test]
    fn board_ffi_models_psram_capacity_pressure_and_readback() {
        let id = CString::new("ffi-psram-node").unwrap();
        let board = unsafe { meshemu_board_create(id.as_ptr(), 3_900, 35.0) };

        unsafe {
            assert!(meshemu_board_psram_found(board));
            assert_eq!(
                meshemu_board_get_psram_free(board),
                mycelium_board::DEFAULT_PSRAM_SIZE_BYTES
            );
            assert!(meshemu_board_psram_readback_test(board));
            assert_eq!(
                meshemu_board_get_psram_free(board),
                mycelium_board::DEFAULT_PSRAM_SIZE_BYTES
            );

            assert!(meshemu_board_psram_reserve(board, 1_000_000));
            assert_eq!(
                meshemu_board_get_psram_free(board),
                mycelium_board::DEFAULT_PSRAM_SIZE_BYTES - 1_000_000
            );
            assert!(!meshemu_board_psram_reserve(
                board,
                mycelium_board::DEFAULT_PSRAM_SIZE_BYTES
            ));
            meshemu_board_psram_release(board, 400_000);
            assert_eq!(
                meshemu_board_get_psram_free(board),
                mycelium_board::DEFAULT_PSRAM_SIZE_BYTES - 600_000
            );
            meshemu_board_destroy(board);

            let mut missing = Box::new(mycelium_board::VirtualBoard::new(
                "ffi-no-psram",
                mycelium_board::BoardConfig::default(),
            ));
            missing.psram_size_bytes = 0;
            let missing = Box::into_raw(missing).cast::<c_void>();
            assert!(!meshemu_board_psram_found(missing));
            assert_eq!(meshemu_board_get_psram_free(missing), 0);
            assert!(!meshemu_board_psram_readback_test(missing));
            meshemu_board_destroy(missing);

            assert!(!meshemu_board_psram_found(std::ptr::null_mut()));
            assert_eq!(meshemu_board_get_psram_free(std::ptr::null_mut()), 0);
            assert!(!meshemu_board_psram_readback_test(std::ptr::null_mut()));
            assert!(!meshemu_board_psram_reserve(std::ptr::null_mut(), 1));
            meshemu_board_psram_release(std::ptr::null_mut(), 1);
        }
    }

    #[test]
    fn deep_sleep_advances_time_drops_rx_and_restores_the_radio() {
        let _serial = TEST_BUS.lock().unwrap();
        ffi::reset_bus();
        let sender = create("sleep-sender", (0.0, 0.0));
        let receiver = create("sleep-receiver", (0.0, 0.0001));
        let receiver_id = CString::new("sleep-receiver").unwrap();
        let packet = [1, 2, 3, 4];
        let airtime = unsafe { meshemu_radio_get_est_airtime(sender, packet.len() as i32) };

        meshemu_bus_tick(1_000);
        assert!(send(sender, &packet));
        let wake_at = unsafe { meshemu_board_deep_sleep(receiver_id.as_ptr(), 2, 1_u64 << 45) };

        assert_eq!(wake_at, 3_000);
        assert_eq!(
            meshemu_board_get_sleep_wake_cause(),
            mycelium_board::SLEEP_WAKE_CAUSE_TIMER_EXT1
        );
        assert_eq!(
            ffi::sleep_request("sleep-receiver"),
            Some((1_000, 3_000, 2, 1_u64 << 45, false))
        );
        assert!(receive(receiver).is_empty());

        assert!(send(sender, &packet));
        meshemu_bus_tick(wake_at + u64::from(airtime));
        assert_eq!(receive(receiver), packet);

        destroy(receiver);
        destroy(sender);
    }

    #[test]
    fn deep_sleep_reports_timer_ext1_and_unknown_wake_sources() {
        let _serial = TEST_BUS.lock().unwrap();
        ffi::reset_bus();
        let id = CString::new("wake-causes").unwrap();

        meshemu_bus_tick(10);
        assert_eq!(
            unsafe { meshemu_board_deep_sleep(id.as_ptr(), 2, 0) },
            2_010
        );
        assert_eq!(
            meshemu_board_get_sleep_wake_cause(),
            mycelium_board::SLEEP_WAKE_CAUSE_TIMER
        );
        assert_eq!(
            unsafe { meshemu_board_deep_sleep(id.as_ptr(), 0, 1_u64 << 3) },
            2_010
        );
        assert_eq!(
            meshemu_board_get_sleep_wake_cause(),
            mycelium_board::SLEEP_WAKE_CAUSE_EXT1
        );
        assert_eq!(
            unsafe { meshemu_board_deep_sleep(id.as_ptr(), 0, 0) },
            2_010
        );
        assert_eq!(
            meshemu_board_get_sleep_wake_cause(),
            mycelium_board::SLEEP_WAKE_CAUSE_UNKNOWN
        );
        assert_eq!(
            unsafe { meshemu_board_deep_sleep(std::ptr::null(), 10, 0) },
            2_010
        );
    }

    #[test]
    fn boot_phase_persists_across_destroy_recreate_and_deep_sleep() {
        let _serial = TEST_BUS.lock().unwrap();
        ffi::reset_bus();
        let id = CString::new("ffi-boot-phase").unwrap();
        let board = unsafe { meshemu_board_create(id.as_ptr(), 3_900, 35.0) };

        meshemu_board_set_boot_phase(7);
        unsafe {
            meshemu_board_rtc_gpio_hold(board, 9, true);
            meshemu_board_rtc_gpio_hold(board, 45, false);
            meshemu_board_destroy(board);
            assert_eq!(meshemu_board_deep_sleep(id.as_ptr(), 1, 0), 1_000);
        }
        let restarted = unsafe { meshemu_board_create(id.as_ptr(), 3_900, 35.0) };

        assert_eq!(meshemu_board_get_last_boot_phase(), 7);
        unsafe { meshemu_board_destroy(restarted) };
    }

    #[test]
    fn board_power_gpio_gates_gps_sd_and_gpio46_pwm_buzzer() {
        let _serial = TEST_STORAGE.lock().unwrap();
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let instance = format!("ffi-power-node-{}-{nonce}", std::process::id());
        let id = CString::new(instance.clone()).unwrap();
        let path = CString::new("/power-test.bin").unwrap();
        let data = b"powered";
        let board = unsafe { meshemu_board_create(id.as_ptr(), 3_900, 35.0) };
        let gps = unsafe { meshemu_gps_create(id.as_ptr(), 51.5, -0.1) };
        let buzzer = mycelium_board::register_buzzer(&instance);
        let mut gps_buffer = [0_u8; 256];

        unsafe {
            meshemu_board_ledc_attach(board, 3, mycelium_board::BUZZER_GPIO);
            meshemu_gps_tick(gps, 1_000);
            assert!(meshemu_board_ledc_write(board, 3, 500, 125));
            {
                let buzzer = buzzer
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                assert!(buzzer.is_playing());
                assert_eq!(buzzer.frequency_hz(), 2_000);
                assert_eq!(buzzer.duty_cycle(), 0.25);
            }
            assert_eq!(meshemu_input_take_falling_edges(id.as_ptr(), 46), 0);

            meshemu_board_digital_write(board, mycelium_board::PERIPH_PWR_EN_GPIO, false);
            assert!(!buzzer
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .is_playing());
            assert_eq!(
                meshemu_gps_read(
                    gps,
                    gps_buffer.as_mut_ptr(),
                    gps_buffer.len().try_into().unwrap()
                ),
                0
            );
            assert!(!meshemu_sdcard_init(id.as_ptr()));
            assert!(!meshemu_sdcard_write(
                id.as_ptr(),
                path.as_ptr(),
                data.as_ptr(),
                data.len()
            ));

            meshemu_board_digital_write(board, mycelium_board::PERIPH_PWR_EN_GPIO, true);
            meshemu_gps_tick(gps, 1_000);
            assert!(
                meshemu_gps_read(
                    gps,
                    gps_buffer.as_mut_ptr(),
                    gps_buffer.len().try_into().unwrap()
                ) > 0
            );
            assert!(meshemu_sdcard_init(id.as_ptr()));
            assert!(meshemu_sdcard_write(
                id.as_ptr(),
                path.as_ptr(),
                data.as_ptr(),
                data.len()
            ));
            assert!(meshemu_board_ledc_write(board, 3, 1_000, 500));
            assert!(buzzer
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .is_playing());

            meshemu_gps_destroy(gps);
            meshemu_board_destroy(board);
        }
        mycelium_board::remove_buzzer(&instance);
    }

    #[test]
    fn sdcard_ffi_runs_slow_card_retry_ladder() {
        let _serial = TEST_STORAGE.lock().unwrap();
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let id = CString::new(format!("ffi-slow-sd-{}-{nonce}", std::process::id())).unwrap();

        meshemu_sdcard_set_behavior(true, 3_000);
        assert!(!unsafe { meshemu_sdcard_init(id.as_ptr()) });
        meshemu_sdcard_set_behavior(true, 2_000);
        assert!(unsafe { meshemu_sdcard_init(id.as_ptr()) });

        assert!(unsafe { meshemu_storage_destroy(id.as_ptr()) });
        meshemu_sdcard_set_behavior(false, 0);
    }

    #[test]
    fn board_ffi_models_external_power_and_tp4054_state() {
        let id = CString::new("ffi-charger-node").unwrap();
        let board = unsafe { meshemu_board_create(id.as_ptr(), 3_900, 35.0) };

        unsafe {
            assert_eq!(
                meshemu_board_get_charger_state(board),
                mycelium_board::Tp4054State::Charged as u8
            );
            meshemu_board_set_external_power(board, true);
            assert_eq!(
                meshemu_board_get_charger_state(board),
                mycelium_board::Tp4054State::Charging as u8
            );
            meshemu_board_set_battery(board, 0);
            assert_eq!(
                meshemu_board_get_charger_state(board),
                mycelium_board::Tp4054State::NoBattery as u8
            );
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
            assert_eq!(meshemu_board_get_adc(std::ptr::null_mut(), 4), 0);
            assert_eq!(meshemu_board_get_temp(std::ptr::null_mut()), 0.0);
            assert_eq!(
                meshemu_board_get_charger_state(std::ptr::null_mut()),
                mycelium_board::Tp4054State::NoBattery as u8
            );
            assert!(!meshemu_board_ledc_write(
                std::ptr::null_mut(),
                0,
                1_000,
                500
            ));
            meshemu_gps_destroy(std::ptr::null_mut());
            meshemu_board_destroy(std::ptr::null_mut());
        }
    }
}
