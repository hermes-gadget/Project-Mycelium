#pragma once

#include <stdbool.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

// Create a virtual I2C bus that emulates the T-Deck keyboard controller at address 0x55.
void* meshemu_i2c_keyboard_create(void);

// Inject a key event into the virtual keyboard (called by Mycelium's input system).
void meshemu_i2c_keyboard_inject_key(void* kb, uint8_t row, uint8_t col, bool pressed);

void meshemu_i2c_keyboard_destroy(void* kb);

#ifdef __cplusplus
}
#endif
