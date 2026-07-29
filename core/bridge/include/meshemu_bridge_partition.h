#ifndef MESHEMU_BRIDGE_PARTITION_H
#define MESHEMU_BRIDGE_PARTITION_H

#include <stdbool.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#define MESHEMU_PARTITION_TYPE_APP 0x00U
#define MESHEMU_PARTITION_TYPE_DATA 0x01U

#define MESHEMU_PARTITION_SUBTYPE_DATA_OTA 0x00U
#define MESHEMU_PARTITION_SUBTYPE_DATA_PHY 0x01U
#define MESHEMU_PARTITION_SUBTYPE_DATA_NVS 0x02U
#define MESHEMU_PARTITION_SUBTYPE_DATA_COREDUMP 0x03U
#define MESHEMU_PARTITION_SUBTYPE_DATA_SPIFFS 0x82U

#define MESHEMU_PARTITION_SUBTYPE_APP_OTA_0 0x10U
#define MESHEMU_PARTITION_SUBTYPE_APP_OTA_1 0x11U
#define MESHEMU_PARTITION_SUBTYPE_APP_TEST 0x20U

bool meshemu_partition_set_launcher_mode(const char *instance_id, bool enabled);
bool meshemu_partition_find_first(uint8_t type, uint8_t subtype,
                                  uint32_t *address_out, uint32_t *size_out);
bool meshemu_partition_find_first_for_instance(
    const char *instance_id, uint8_t type, uint8_t subtype,
    uint32_t *address_out, uint32_t *size_out);
uint32_t meshemu_get_otadata_address(void);
bool meshemu_is_under_launcher(const char *instance_id);

#ifdef __cplusplus
}
#endif

#endif
