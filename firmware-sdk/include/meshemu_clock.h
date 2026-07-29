#pragma once

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

void* meshemu_clock_create(void);
uint64_t meshemu_clock_millis(void* clock);
void meshemu_clock_set_offset(void* clock, int64_t offset_ms);
void meshemu_clock_destroy(void* clock);

#ifdef __cplusplus
}
#endif
