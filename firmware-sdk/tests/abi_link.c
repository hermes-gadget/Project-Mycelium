#include "meshemu_board.h"
#include "meshemu_buzzer.h"
#include "meshemu_display.h"
#include "meshemu_gps.h"
#include "meshemu_input.h"
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
    (void)meshemu_board_get_temp(board);
    meshemu_board_digital_write(board, 10, true);
    meshemu_board_ledc_attach(board, 0, 46);
    (void)meshemu_board_ledc_write(board, 0, 1000, 500);
    meshemu_board_set_external_power(board, true);
    (void)meshemu_board_get_charger_state(board);
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
    (void)meshemu_input_poll_key(id);
    void* keyboard = meshemu_i2c_keyboard_create();
    meshemu_i2c_keyboard_inject_key_byte(keyboard, 'q');
    void* wire = meshemu_wire_shim_create();
    meshemu_wire_shim_set_keyboard(wire, keyboard);
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
    meshemu_radio_set_position(radio, 51.5, -0.1);
    meshemu_bus_tick(1);
    meshemu_radio_destroy(radio);

    (void)meshemu_spiffs_init(id);
    uint8_t* spiffs_data = meshemu_spiffs_read(id, id, &size);
    (void)meshemu_spiffs_write(id, id, &byte, 1);
    meshemu_storage_data_free(spiffs_data);
    (void)meshemu_sdcard_init(id);
    uint8_t* sdcard_data = meshemu_sdcard_read(id, id, &size);
    (void)meshemu_sdcard_write(id, id, &byte, 1);
    meshemu_storage_data_free(sdcard_data);
    (void)meshemu_storage_destroy(id);

    return 0;
}
