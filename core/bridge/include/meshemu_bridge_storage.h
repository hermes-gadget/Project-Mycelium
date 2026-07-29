#ifndef MESHEMU_BRIDGE_STORAGE_H
#define MESHEMU_BRIDGE_STORAGE_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

bool meshemu_spiffs_init(const char *instance_id);
uint8_t *meshemu_spiffs_read(
    const char *instance_id,
    const char *path,
    size_t *out_len
);
bool meshemu_spiffs_write(
    const char *instance_id,
    const char *path,
    const uint8_t *data,
    size_t len
);

bool meshemu_sdcard_init(const char *instance_id);
uint8_t *meshemu_sdcard_read(
    const char *instance_id,
    const char *path,
    size_t *out_len
);
bool meshemu_sdcard_write(
    const char *instance_id,
    const char *path,
    const uint8_t *data,
    size_t len
);

bool meshemu_storage_destroy(const char *instance_id);
void meshemu_storage_data_free(uint8_t *data);

#ifdef __cplusplus
}
#endif

#endif
