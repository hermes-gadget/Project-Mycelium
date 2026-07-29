#pragma once

#include <stdbool.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

// Play a tone on the host's audio output.
void meshemu_buzzer_beep(const char* instance_id, uint32_t frequency_hz,
                         uint32_t duration_ms);

// Stop any currently playing tone.
void meshemu_buzzer_stop(const char* instance_id);

// Return true while the instance's current tone is active.
bool meshemu_buzzer_is_playing(const char* instance_id);

#ifdef __cplusplus
}
#endif
