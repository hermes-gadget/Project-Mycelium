#pragma once

#include "meshemu_types.h"

#ifdef __cplusplus
extern "C" {
#endif

void* meshemu_gps_create(const char* instance_id, const MeshemuGpsConfig* config);
void meshemu_gps_set_position(void* gps, double lat, double lon, double altitude);

// Get the next NMEA sentence. Returns length or 0 if no data.
// Call in a loop until it returns 0 to drain the sentence buffer.
int meshemu_gps_read(void* gps, char* buffer, int max_len);

void meshemu_gps_set_enabled(void* gps, bool enabled);
void meshemu_gps_destroy(void* gps);

#ifdef __cplusplus
}
#endif
