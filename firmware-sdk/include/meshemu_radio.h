#pragma once

#include "meshemu_types.h"

#ifdef __cplusplus
extern "C" {
#endif

// Initialize a virtual radio connected to the shared RadioBus.
// Returns a handle that implements mesh::Radio (via MeshCore's virtual interface).
// The instance_id ties this radio to a specific virtual node in the RadioBus.
void* meshemu_radio_create(const char* instance_id, const MeshemuRadioConfig* config);

// Update radio position (used by RadioBus for propagation).
void meshemu_radio_set_position(void* radio, double lat, double lon);

// Get packet statistics.
void meshemu_radio_get_stats(void* radio, uint32_t* sent, uint32_t* received,
                             uint32_t* errors, uint32_t* collisions);

// Destroy radio and disconnect from RadioBus.
void meshemu_radio_destroy(void* radio);

#ifdef __cplusplus
}
#endif
