#pragma once

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

// Initialize virtual SPIFFS backed by a host directory.
// Mounts at the path ~/.mycelium/instances/<instance_id>/spiffs/.
bool meshemu_spiffs_init(const char* instance_id);

// Initialize virtual SD card backed by a host directory.
bool meshemu_sdcard_init(const char* instance_id);

// Get the host path for this instance's storage (for debugging).
const char* meshemu_storage_path(const char* instance_id);

#ifdef __cplusplus
}
#endif
