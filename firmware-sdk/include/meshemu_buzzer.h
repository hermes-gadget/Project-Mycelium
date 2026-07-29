#pragma once

#ifdef __cplusplus
extern "C" {
#endif

// Play a tone on the host's audio output.
void meshemu_buzzer_beep(int frequency_hz, int duration_ms);

// Stop any currently playing tone.
void meshemu_buzzer_stop(void);

#ifdef __cplusplus
}
#endif
