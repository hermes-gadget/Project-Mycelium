#pragma once

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

// Create an SDL2-backed LVGL display. Maps to lv_sdl_window_create() internally.
// Returns lv_display_t* that the firmware uses like a real display.
// Set window_title = NULL for auto-generated title.
void* meshemu_display_create(int width, int height, const char* window_title);

// Capture current framebuffer as RGB565 data. Caller frees the buffer.
// size_out receives (width * height * 2) bytes.
uint8_t* meshemu_display_capture(void* display, size_t* size_out);

void meshemu_display_destroy(void* display);

#ifdef __cplusplus
}
#endif
