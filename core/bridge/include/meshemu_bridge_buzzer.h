#ifndef MESHEMU_BRIDGE_BUZZER_H
#define MESHEMU_BRIDGE_BUZZER_H

#include <stdbool.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

void meshemu_buzzer_beep(const char* instance_id, uint32_t frequency_hz,
                         uint32_t duration_ms);
void meshemu_buzzer_stop(const char* instance_id);
bool meshemu_buzzer_is_playing(const char* instance_id);

#ifdef __cplusplus
}
#endif

#endif
