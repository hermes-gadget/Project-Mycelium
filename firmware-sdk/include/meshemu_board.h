#pragma once

#include "meshemu_types.h"

#ifdef __cplusplus
extern "C" {
#endif

// Create a virtual board implementing mesh::MainBoard.
// Caller receives an opaque pointer they can cast to mesh::MainBoard*.
void* meshemu_board_create(const char* instance_id, const MeshemuBoardConfig* config);

// Update battery voltage (simulates discharge or charging).
void meshemu_board_set_battery(void* board, uint16_t millivolts);

// Get current battery reading.
uint16_t meshemu_board_get_battery(void* board);

void meshemu_board_destroy(void* board);

#ifdef __cplusplus
}
#endif
