#pragma once

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

// LVGL version the firmware expects.
#define MYCELIUM_LVGL_V9 9
#define MYCELIUM_LVGL_V8 8

// Create an SDL2-backed display with an explicit LVGL ABI.
// Returns an opaque display handle, or NULL when initialization fails.
// Set window_title = NULL for auto-generated title.
void* meshemu_display_create_v(int width, int height, const char* window_title,
                               int lvgl_version);

// Legacy wrapper. Creates an LVGL v9 display.
void* meshemu_display_create(int width, int height, const char* window_title);

// Capture current framebuffer as RGB565 data. Caller frees the buffer.
// size_out receives (width * height * 2) bytes.
uint8_t* meshemu_display_capture(void* display, size_t* size_out);

void meshemu_display_destroy(void* display);

#ifdef __cplusplus
}
#endif
