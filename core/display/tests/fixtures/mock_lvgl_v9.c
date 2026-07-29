#include <stddef.h>
#include <stdint.h>

typedef struct {
    int32_t x1;
    int32_t y1;
    int32_t x2;
    int32_t y2;
} lv_area_t;

typedef void (*flush_cb_t)(void *, const lv_area_t *, uint8_t *);

static int display;
static int delete_calls;
static int flush_ready_calls;
static uint32_t color_format;
static uint32_t buffer_size;
static uint32_t render_mode;
static flush_cb_t flush_cb;

void lv_init(void) {}

void *lv_display_create(int width, int height) {
    if (width != 320 || height != 240) {
        return 0;
    }
    return &display;
}

void lv_display_delete(void *ignored) {
    (void)ignored;
    delete_calls++;
}

void lv_display_set_color_format(void *ignored, uint32_t format) {
    (void)ignored;
    color_format = format;
}

void lv_display_set_buffers(void *ignored, void *buffer1, void *buffer2,
                            uint32_t size, uint32_t mode) {
    (void)ignored;
    (void)buffer1;
    (void)buffer2;
    buffer_size = size;
    render_mode = mode;
}

void lv_display_set_flush_cb(void *ignored, flush_cb_t callback) {
    (void)ignored;
    flush_cb = callback;
}

void lv_display_flush_ready(void *ignored) {
    (void)ignored;
    flush_ready_calls++;
}

void mock_flush_area(int32_t x1, int32_t y1, int32_t x2, int32_t y2,
                     uint16_t *pixels) {
    lv_area_t area = {x1, y1, x2, y2};
    flush_cb(&display, &area, (uint8_t *)pixels);
}

int mock_delete_calls(void) {
    return delete_calls;
}

int mock_flush_ready_calls(void) {
    return flush_ready_calls;
}

uint32_t mock_color_format(void) {
    return color_format;
}

uint32_t mock_buffer_size(void) {
    return buffer_size;
}

uint32_t mock_render_mode(void) {
    return render_mode;
}
