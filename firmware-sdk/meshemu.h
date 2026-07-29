/**
 * @file meshemu.h
 * @brief Firmware entry points called by Mycelium.
 *
 * Every firmware shared library must export this three-function contract:
 *
 *   - firmware_setup() is called exactly once when the virtual node starts.
 *   - firmware_loop() is called once per emulator frame.
 *   - firmware_get_display() returns the firmware's LVGL display, or NULL when
 *     the firmware has no display.
 *
 * Firmware calls the Host Services API to create and control virtual hardware.
 * Those declarations live in include/meshemu_*.h:
 *
 *   include/meshemu_radio.h    - RadioBus-backed radio
 *   include/meshemu_board.h    - virtual MainBoard
 *   include/meshemu_display.h  - SDL2/LVGL display
 *   include/meshemu_storage.h  - SPIFFS and SD card storage
 *   include/meshemu_nvs.h      - persistent Preferences-compatible NVS
 *   include/meshemu_partition.h - ESP32 partition table and Launcher mode
 *   include/meshemu_gps.h      - GPS and NMEA data
 *   include/meshemu_input.h    - T-Deck keyboard
 *   include/meshemu_buzzer.h   - host audio
 *
 * Add firmware-sdk/, firmware-sdk/include/, and core/bridge/include/ to the
 * firmware project's header search path. See firmware-sdk/README.md for
 * integration examples.
 */

#pragma once

#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#endif

// Called once at startup. Mycelium provides all hardware abstractions.
// The firmware stores any needed handles and initializes itself.
void firmware_setup(void);

// Called each frame. The firmware processes one main loop iteration.
void firmware_loop(void);

// Optional: return a display handle for LVGL-based firmwares.
// Mycelium uses this to render the UI into the emulator window.
// Return NULL if the firmware has no display.
void* firmware_get_display(void);

#ifdef __cplusplus
}
#endif
