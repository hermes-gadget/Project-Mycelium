#include "meshemu_board.h"
#include "meshemu_buzzer.h"
#include "meshemu_display.h"
#include "meshemu_gps.h"
#include "meshemu_input.h"
#include "meshemu_nvs.h"
#include "meshemu_partition.h"
#include "meshemu_radio.h"
#include "meshemu_storage.h"

#include <stddef.h>
#include <stdint.h>

int main(void)
{
    const char* id = "abi-link";
    uint8_t byte = 0;
    size_t size = 0;

    void* board = meshemu_board_create(id, 3700, 25.0f);
    meshemu_board_set_battery(board, 3600);
    (void)meshemu_board_get_battery(board);
    (void)meshemu_board_get_adc(board, 4);
    meshemu_board_set_adc_calibration(board, true);
    (void)meshemu_board_get_temp(board);
    meshemu_board_set_mcu_temperature(board, 35.0f);
    (void)meshemu_board_get_mcu_temperature(board);
    (void)meshemu_board_set_rtc_noinit(id, 0, &byte, 1);
    (void)meshemu_board_get_rtc_noinit(id, 0, &byte, 1);
    meshemu_board_clear_rtc_noinit(id);
    (void)meshemu_board_psram_found(board);
    (void)meshemu_board_get_psram_free(board);
    (void)meshemu_board_psram_readback_test(board);
    (void)meshemu_board_psram_reserve(board, 1024);
    meshemu_board_psram_release(board, 1024);
    meshemu_board_digital_write(board, 10, true);
    meshemu_board_set_periph_power(board, true);
    meshemu_board_ledc_attach(board, 0, 46);
    (void)meshemu_board_ledc_write(board, 0, 1000, 500);
    meshemu_board_set_external_power(board, true);
    (void)meshemu_board_get_charger_state(board);
    meshemu_board_rtc_gpio_hold(board, 9, true);
    (void)meshemu_board_set_reset_reason(board, RESET_REASON_SW);
    (void)meshemu_board_get_reset_reason(board);
    meshemu_board_wdt_init(board, 30, true);
    (void)meshemu_board_wdt_feed(board);
    (void)meshemu_board_wdt_get_status(board);
    meshemu_board_wdt_disable(board);
    (void)meshemu_board_quiesce_peripherals(board);
    meshemu_board_set_boot_phase(2);
    (void)meshemu_board_get_last_boot_phase();
    (void)meshemu_board_deep_sleep(id, 1, UINT64_C(1) << 45);
    (void)meshemu_board_get_sleep_wake_cause();
    meshemu_board_destroy(board);

    meshemu_buzzer_beep(id, 440, 10);
    meshemu_buzzer_stop(id);
    (void)meshemu_buzzer_is_playing(id);

    void* display = meshemu_display_create(320, 240, id);
    void* versioned_display = meshemu_display_create_v(320, 240, id, 9);
    const meshemu_display_options display_options = {24, true};
    void* fidelity_display =
        meshemu_display_create_ex(320, 240, id, 9, &display_options);
    uint8_t* pixels = meshemu_display_capture(display, &size);
    meshemu_display_capture_free(pixels, size);
    meshemu_display_destroy(fidelity_display);
    meshemu_display_destroy(display);
    meshemu_display_destroy(versioned_display);

    void* gps = meshemu_gps_create(id, 51.5, -0.1);
    meshemu_gps_set_position(gps, 51.5, -0.1, 10.0);
    meshemu_gps_tick(gps, 1000);
    (void)meshemu_gps_read(gps, &byte, 1);
    meshemu_gps_set_enabled(gps, false);
    meshemu_gps_destroy(gps);

    meshemu_input_inject_touch(id, 1, 2, true);
    meshemu_input_inject_key(id, 3, true);
    (void)meshemu_input_poll_touch(id);
    uint16_t touch_x = 0;
    uint16_t touch_y = 0;
    meshemu_input_get_touch_raw(id, &touch_x, &touch_y);
    meshemu_input_get_touch_mapped(id, &touch_x, &touch_y);
    meshemu_input_gt911_set_failure_mode(MESHEMU_GT911_FAILURE_BUS, 0);
    (void)meshemu_input_gt911_get_status();
    (void)meshemu_input_poll_key(id);
    void* keyboard = meshemu_i2c_keyboard_create();
    meshemu_i2c_keyboard_inject_key_byte(keyboard, 'q');
    meshemu_i2c_keyboard_set_cross_reset(keyboard, true);
    void* wire = meshemu_wire_shim_create();
    meshemu_wire_shim_set_keyboard(wire, keyboard);
    (void)meshemu_wire_begin(wire);
    (void)meshemu_wire_probe_address(wire, 0x55);
    uint8_t sda = 0;
    uint8_t scl = 0;
    meshemu_wire_read_idle_levels(wire, &sda, &scl);
    meshemu_wire_begin_transmission(wire, 0x55);
    (void)meshemu_wire_write(wire, byte);
    (void)meshemu_wire_end_transmission(wire);
    (void)meshemu_wire_request_from(wire, 0x55, 1);
    (void)meshemu_wire_available(wire);
    (void)meshemu_wire_read(wire);
    meshemu_wire_shim_destroy(wire);
    meshemu_i2c_keyboard_destroy(keyboard);

    void* radio = meshemu_radio_create(id, 915.0, 125, 7, 5, 14.0, 51.5, -0.1);
    (void)meshemu_radio_start_send(radio, &byte, 1);
    bool truncated = false;
    (void)meshemu_radio_recv_raw(radio, &byte, 1, &truncated);
    (void)meshemu_radio_get_est_airtime(radio, 1);
    (void)meshemu_radio_get_rssi(radio);
    (void)meshemu_radio_get_snr(radio);
    (void)meshemu_radio_is_send_complete(radio);
    meshemu_radio_set_dio2_config(radio, true);
    (void)meshemu_radio_get_dio2_config(radio);
    meshemu_radio_set_position(radio, 51.5, -0.1);
    meshemu_bus_tick(1);
    meshemu_radio_destroy(radio);

    (void)meshemu_spiffs_init(id);
    uint8_t* spiffs_data = meshemu_spiffs_read(id, id, &size);
    (void)meshemu_spiffs_write(id, id, &byte, 1);
    meshemu_storage_data_free(spiffs_data);
    meshemu_sdcard_set_behavior(false, 0);
    (void)meshemu_sdcard_init(id);
    uint8_t* sdcard_data = meshemu_sdcard_read(id, id, &size);
    (void)meshemu_sdcard_write(id, id, &byte, 1);
    meshemu_storage_data_free(sdcard_data);
    (void)meshemu_storage_destroy(id);

    (void)meshemu_nvs_init(id, MESHEMU_NVS_SIZE_STANDALONE);
    (void)meshemu_nvs_exists(id, "touch", "sd_mig_busy");
    (void)meshemu_nvs_get_bool(id, "touch", "sd_mig_busy", false);
    (void)meshemu_nvs_put_bool(id, "touch", "sd_mig_busy", true);
    char string_value[16];
    (void)meshemu_nvs_get_string(id, "touch", "label", "", string_value,
                                 sizeof(string_value));
    (void)meshemu_nvs_put_string(id, "touch", "label", "T-Deck");
    (void)meshemu_nvs_remove(id, "touch", "sd_mig_busy");

    uint32_t address = 0;
    uint32_t partition_size = 0;
    (void)meshemu_partition_set_launcher_mode(id, false);
    (void)meshemu_partition_find_first(
        MESHEMU_PARTITION_TYPE_DATA, MESHEMU_PARTITION_SUBTYPE_DATA_NVS,
        &address, &partition_size);
    (void)meshemu_partition_find_first_for_instance(
        id, MESHEMU_PARTITION_TYPE_DATA, MESHEMU_PARTITION_SUBTYPE_DATA_OTA,
        &address, &partition_size);
    (void)meshemu_get_otadata_address();
    (void)meshemu_is_under_launcher(id);
    (void)meshemu_nvs_destroy(id);

    return 0;
}
