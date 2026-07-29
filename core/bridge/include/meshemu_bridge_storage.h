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
void meshemu_sdcard_set_behavior(bool slow_init, uint32_t wake_delay_ms);

#define MESHEMU_SD_NONE 0
#define MESHEMU_SD_MMC 1
#define MESHEMU_SD_SDSC 2
#define MESHEMU_SD_SDHC 3

#define MESHEMU_SD_OPEN_READ 0
#define MESHEMU_SD_OPEN_WRITE 1
#define MESHEMU_SD_OPEN_APPEND 2

uint32_t meshemu_sdcard_card_type(const char *instance_id);
uint64_t meshemu_sdcard_total_bytes(const char *instance_id);
uint64_t meshemu_sdcard_used_bytes(const char *instance_id);
bool meshemu_sdcard_mkdir(const char *instance_id, const char *path);
bool meshemu_sdcard_exists(const char *instance_id, const char *path);
uint32_t meshemu_sdcard_open(
    const char *instance_id,
    const char *path,
    uint8_t mode
);
int32_t meshemu_sdcard_write_file(
    uint32_t handle,
    const uint8_t *data,
    uint32_t len
);
int32_t meshemu_sdcard_read_file(
    uint32_t handle,
    uint8_t *buf,
    uint32_t max_len
);
bool meshemu_sdcard_close_file(uint32_t handle);
bool meshemu_sdcard_remove(const char *instance_id, const char *path);
void meshemu_sdcard_end(const char *instance_id);

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
