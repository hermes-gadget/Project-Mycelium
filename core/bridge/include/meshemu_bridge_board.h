#ifndef MESHEMU_BRIDGE_BOARD_H
#define MESHEMU_BRIDGE_BOARD_H

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

void* meshemu_board_create(const char* instance_id, uint16_t millivolts,
                           float temperature_c);
void meshemu_board_set_battery(void* board, uint16_t millivolts);
uint16_t meshemu_board_get_battery(void* board);
float meshemu_board_get_temp(void* board);
void meshemu_board_destroy(void* board);

#ifdef __cplusplus
}
#endif

#endif
