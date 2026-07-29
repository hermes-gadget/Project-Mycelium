#ifndef MESHEMU_BRIDGE_INPUT_H
#define MESHEMU_BRIDGE_INPUT_H

#include <stdbool.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

void meshemu_input_inject_touch(
    const char *instance_id,
    uint16_t x,
    uint16_t y,
    bool pressed);
void meshemu_input_inject_key(
    const char *instance_id,
    uint32_t keycode,
    bool pressed);

/* Touch packing: x[0..15], y[16..31], pressure[32..39]. */
uint64_t meshemu_input_poll_touch(const char *instance_id);

/* Keyboard packing: row[0..7], col[8..15], pressed[16]. */
uint64_t meshemu_input_poll_key(const char *instance_id);

#ifdef __cplusplus
}
#endif

#endif
