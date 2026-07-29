#ifndef MESHEMU_BRIDGE_RADIO_H
#define MESHEMU_BRIDGE_RADIO_H

#include <stdbool.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

void* meshemu_radio_create(const char* id, double freq_mhz, uint16_t bandwidth_khz,
                           uint8_t spreading_factor, uint8_t coding_rate,
                           double tx_power_dbm, double lat, double lon);
bool meshemu_radio_start_send(void* radio, const uint8_t* data, uint32_t len);
// Returns the packet length, zero if empty, or the negative required length
// without removing the packet when max_len is too small.
int32_t meshemu_radio_recv_raw(void* radio, uint8_t* buffer, int32_t max_len);
uint32_t meshemu_radio_get_est_airtime(void* radio, int32_t len);
float meshemu_radio_get_rssi(void* radio);
float meshemu_radio_get_snr(void* radio);
bool meshemu_radio_is_send_complete(void* radio);
void meshemu_radio_set_position(void* radio, double lat, double lon);
void meshemu_radio_destroy(void* radio);
void meshemu_bus_tick(uint64_t now_ms);

#ifdef __cplusplus
}
#endif

#endif
