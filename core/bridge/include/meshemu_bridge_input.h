#ifndef MESHEMU_BRIDGE_INPUT_H
#define MESHEMU_BRIDGE_INPUT_H

#include <stdbool.h>
#include <stddef.h>
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

void *meshemu_i2c_keyboard_create(void);
void meshemu_i2c_keyboard_inject_key_byte(void *keyboard, uint8_t key_byte);
void meshemu_i2c_keyboard_destroy(void *keyboard);

void *meshemu_wire_shim_create(void);
void meshemu_wire_shim_set_keyboard(void *wire, void *keyboard);
bool meshemu_wire_begin(void *wire);
void meshemu_wire_set_clock(void *wire, uint32_t clock_hz);
void meshemu_wire_begin_transmission(void *wire, uint8_t address);
size_t meshemu_wire_write(void *wire, uint8_t byte);
uint8_t meshemu_wire_end_transmission(void *wire);
uint8_t meshemu_wire_request_from(void *wire, uint8_t address, uint8_t count);
int32_t meshemu_wire_available(void *wire);
int32_t meshemu_wire_read(void *wire);
void meshemu_wire_shim_destroy(void *wire);

#ifdef __cplusplus
}

class MeshemuWireShim final {
public:
    MeshemuWireShim() : handle_(meshemu_wire_shim_create()) {}
    ~MeshemuWireShim() { meshemu_wire_shim_destroy(handle_); }

    MeshemuWireShim(const MeshemuWireShim &) = delete;
    MeshemuWireShim &operator=(const MeshemuWireShim &) = delete;

    void setKeyboard(void *keyboard) {
        meshemu_wire_shim_set_keyboard(handle_, keyboard);
    }
    bool begin() { return meshemu_wire_begin(handle_); }
    void setClock(uint32_t clock_hz) {
        meshemu_wire_set_clock(handle_, clock_hz);
    }
    void beginTransmission(uint8_t address) {
        meshemu_wire_begin_transmission(handle_, address);
    }
    size_t write(uint8_t byte) { return meshemu_wire_write(handle_, byte); }
    uint8_t endTransmission() {
        return meshemu_wire_end_transmission(handle_);
    }
    uint8_t requestFrom(uint8_t address, uint8_t count) {
        return meshemu_wire_request_from(handle_, address, count);
    }
    int available() { return meshemu_wire_available(handle_); }
    int read() { return meshemu_wire_read(handle_); }

private:
    void *handle_;
};
#endif

#endif
