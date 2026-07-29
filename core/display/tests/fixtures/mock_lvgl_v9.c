#include <stdbool.h>
#include <stdint.h>
#include <string.h>

#ifndef MOCK_COLOR_FORMAT
#define MOCK_COLOR_FORMAT 0x12
#endif

static int display;
static int window;
static int renderer;
static int delete_calls;
static int hide_calls;

void lv_init(void) {}

void *lv_sdl_window_create(int width, int height) {
    if (width != 320 || height != 240) {
        return 0;
    }
    return &display;
}

uint32_t lv_display_get_color_format(void *ignored) {
    (void)ignored;
    return MOCK_COLOR_FORMAT;
}

void lv_display_delete(void *ignored) {
    (void)ignored;
    delete_calls++;
}

void lv_sdl_window_set_title(void *ignored, const char *title) {
    (void)ignored;
    (void)title;
}

void lv_sdl_window_set_resizeable(void *ignored, bool value) {
    (void)ignored;
    (void)value;
}

void *lv_sdl_window_get_window(void *ignored) {
    (void)ignored;
    return &window;
}

void SDL_HideWindow(void *ignored) {
    (void)ignored;
    hide_calls++;
}

void *lv_sdl_window_get_renderer(void *ignored) {
    (void)ignored;
    return &renderer;
}

int lv_display_get_horizontal_resolution(void *ignored) {
    (void)ignored;
    return 320;
}

int lv_display_get_vertical_resolution(void *ignored) {
    (void)ignored;
    return 240;
}

int SDL_RenderReadPixels(void *ignored_renderer, const void *ignored_rect,
                         uint32_t ignored_format, void *pixels, int pitch) {
    (void)ignored_renderer;
    (void)ignored_rect;
    (void)ignored_format;
    memset(pixels, 0x5a, (size_t)pitch * 240);
    return 0;
}

int mock_delete_calls(void) {
    return delete_calls;
}

int mock_hide_calls(void) {
    return hide_calls;
}
