#ifndef MESHEMU_BRIDGE_DISPLAY_H
#define MESHEMU_BRIDGE_DISPLAY_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

enum {
    MYCELIUM_LVGL_V8 = 8,
    MYCELIUM_LVGL_V9 = 9,
};

/* T-Deck logical geometry is fixed at 320x240. LVGL v9 must use
 * LV_COLOR_DEPTH=16 so the SDL driver allocates RGB565 buffers. */
void *meshemu_display_create_v(int width, int height, const char *window_title,
                               int lvgl_version);
void *meshemu_display_create(int width, int height, const char *window_title);
uint8_t *meshemu_display_capture(void *display, size_t *size_out);
void meshemu_display_capture_free(uint8_t *data, size_t size);
void meshemu_display_destroy(void *display);

#ifdef __cplusplus
}
#endif

#endif
