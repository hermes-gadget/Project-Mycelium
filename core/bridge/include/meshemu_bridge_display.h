#ifndef MESHEMU_BRIDGE_DISPLAY_H
#define MESHEMU_BRIDGE_DISPLAY_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

void *meshemu_display_create(const char *instance_id, int width, int height);
uint8_t *meshemu_display_capture(void *display, size_t *size_out);
void meshemu_display_capture_free(uint8_t *data, size_t size);

#ifdef __cplusplus
}
#endif

#endif
