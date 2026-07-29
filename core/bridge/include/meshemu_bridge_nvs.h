#ifndef MESHEMU_BRIDGE_NVS_H
#define MESHEMU_BRIDGE_NVS_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#define MESHEMU_NVS_SIZE_STANDALONE 0x5000U
#define MESHEMU_NVS_SIZE_LAUNCHER 0x4000U

bool meshemu_nvs_init(const char *instance_id, uint32_t size_bytes);
bool meshemu_nvs_exists(const char *instance_id, const char *namespace_name,
                        const char *key);
bool meshemu_nvs_get_bool(const char *instance_id, const char *namespace_name,
                          const char *key, bool default_value);
bool meshemu_nvs_put_bool(const char *instance_id, const char *namespace_name,
                          const char *key, bool value);

/*
 * Copies a NUL-terminated string and returns its full byte length, excluding
 * the terminator. A short buffer receives a truncated, NUL-terminated value.
 * Pass NULL/0 to query the required length.
 */
size_t meshemu_nvs_get_string(const char *instance_id,
                              const char *namespace_name, const char *key,
                              const char *default_value, char *buffer,
                              size_t buffer_len);
bool meshemu_nvs_put_string(const char *instance_id,
                            const char *namespace_name, const char *key,
                            const char *value);
bool meshemu_nvs_remove(const char *instance_id, const char *namespace_name,
                        const char *key);

/* Drops the live handle; the JSON-backed contents intentionally survive. */
bool meshemu_nvs_destroy(const char *instance_id);

#ifdef __cplusplus
}
#endif

#endif
