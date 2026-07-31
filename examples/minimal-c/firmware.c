/*
 * minimal-c — the smallest C firmware for Project Mycelium.
 *
 * Demonstrates the three-function firmware contract every firmware shared
 * library must export (firmware_setup / firmware_loop / firmware_get_display)
 * together with the plain-C host services API: a virtual radio, a virtual
 * board, and a one-shot "hello from C" beacon.
 *
 * Build with `make` (see README.md), then run with:
 *
 *   meshemu run --firmware ./firmware.so --nodes 2
 *
 * The loader may reuse one dlopen handle for multiple requested nodes, so
 * each firmware_setup() call mints a fresh instance id and its own handles.
 */

#include <stdio.h>
#include <stdint.h>
#include <string.h>

#include "meshemu.h"
#include "meshemu_board.h"
#include "meshemu_radio.h"

#define BAND_MHZ 868.0
#define BATTERY_MV 3900
#define TEMP_C 26.5f

static const char HELLO[] = "hello from C";

static void* radio;
static void* board;
static unsigned int setup_calls;
static unsigned int loop_count;

void firmware_setup(void) {
    char id[32];
    snprintf(id, sizeof id, "minimal-c-%u", setup_calls++);

    radio = meshemu_radio_create(id, BAND_MHZ, 125, 9, 5, 14.0, 54.5, -1.2);
    board = meshemu_board_create(id, BATTERY_MV, TEMP_C);
    if (radio == NULL || board == NULL) {
        fprintf(stderr, "[minimal-c] setup failed for %s\n", id);
        return;
    }
    /* T-Deck firmware must enable the DIO2 RF switch to avoid antenna loss. */
    meshemu_radio_set_dio2_config(radio, true);
    printf("[minimal-c:%s] setup complete\n", id);
}

void firmware_loop(void) {
    if (radio == NULL || board == NULL) {
        return;
    }
    loop_count++;
    if (loop_count == 10) {
        meshemu_radio_start_send(radio, (const uint8_t*)HELLO,
                                 (uint32_t)(sizeof HELLO - 1));
    }
    if (loop_count % 100 == 0) {
        printf("[minimal-c] loop %u, battery %u mV, reset reason %u\n",
               loop_count, meshemu_board_get_battery(board),
               meshemu_board_get_reset_reason(board));
    }
}

void* firmware_get_display(void) {
    /* Headless firmware: no LVGL display to hand to the emulator. */
    return NULL;
}
