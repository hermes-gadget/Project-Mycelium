#ifndef MESHEMU_BRIDGE_GPS_H
#define MESHEMU_BRIDGE_GPS_H

#include <stdbool.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

void* meshemu_gps_create(const char* instance_id, double lat, double lon);
void meshemu_gps_set_position(void* gps, double lat, double lon,
                              double altitude);
int32_t meshemu_gps_read(void* gps, uint8_t* buffer, int32_t max_len);
void meshemu_gps_tick(void* gps, uint64_t delta_ms);
void meshemu_gps_set_enabled(void* gps, bool enabled);
void meshemu_gps_destroy(void* gps);

#ifdef __cplusplus
}
#endif

#endif
