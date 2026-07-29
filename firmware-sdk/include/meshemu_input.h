#pragma once

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

// Create a virtual I2C bus that emulates the T-Deck keyboard controller at address 0x55.
void* meshemu_i2c_keyboard_create(void);

// Inject the exact key byte returned by the T-Deck ESP32-C3.
void meshemu_i2c_keyboard_inject_key_byte(void* kb, uint8_t key_byte);

void meshemu_i2c_keyboard_destroy(void* kb);

// Arduino Wire-compatible operations backed by the virtual keyboard bus.
void* meshemu_wire_shim_create(void);
void meshemu_wire_shim_set_keyboard(void* wire, void* kb);
bool meshemu_wire_begin(void* wire);
void meshemu_wire_set_clock(void* wire, uint32_t clock_hz);
void meshemu_wire_begin_transmission(void* wire, uint8_t address);
size_t meshemu_wire_write(void* wire, uint8_t byte);
uint8_t meshemu_wire_end_transmission(void* wire);
uint8_t meshemu_wire_request_from(void* wire, uint8_t address, uint8_t count);
int32_t meshemu_wire_available(void* wire);
int32_t meshemu_wire_read(void* wire);
void meshemu_wire_shim_destroy(void* wire);

#ifdef __cplusplus
}
#endif
