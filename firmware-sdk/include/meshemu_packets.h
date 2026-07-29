#pragma once

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

void* meshemu_packets_create(int pool_size);
void meshemu_packets_destroy(void* pm);

#ifdef __cplusplus
}
#endif
