#ifndef MESHEMU_BRIDGE_DISPLAY_H
#define MESHEMU_BRIDGE_DISPLAY_H

#include <stddef.h>
#include <stdbool.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/**
 * Mycelium capture and presentation buffers contain packed RGB565 pixels in
 * host-native uint16_t byte order. They are not ST7789 SPI wire bytes; the
 * optional fidelity layer converts those transfers to high-byte-first order.
 */
typedef struct meshemu_display_options {
    uint32_t draw_buffer_rows;
    bool st7789_fidelity;
} meshemu_display_options;

void *meshemu_display_create(int width, int height, const char *window_title);
void *meshemu_display_create_v(int width, int height, const char *window_title,
                               int lvgl_version);
void *meshemu_display_create_ex(int width, int height, const char *window_title,
                                int lvgl_version,
                                const meshemu_display_options *options);
uint8_t *meshemu_display_capture(void *display, size_t *size_out);
void meshemu_display_capture_free(uint8_t *data, size_t size);
void meshemu_display_destroy(void *display);

#ifdef __cplusplus
}
#endif

#endif
