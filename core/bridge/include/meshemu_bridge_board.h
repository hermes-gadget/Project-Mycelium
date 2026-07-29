#ifndef MESHEMU_BRIDGE_BOARD_H
#define MESHEMU_BRIDGE_BOARD_H

#include <stdbool.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#define MESHEMU_TP4054_CHARGING 0
#define MESHEMU_TP4054_CHARGED 1
#define MESHEMU_TP4054_NO_BATTERY 2

void* meshemu_board_create(const char* instance_id, uint16_t millivolts,
                           float temperature_c);
void meshemu_board_set_battery(void* board, uint16_t millivolts);
uint16_t meshemu_board_get_battery(void* board);
uint16_t meshemu_board_get_adc(void* board, uint8_t gpio);
float meshemu_board_get_temp(void* board);
void meshemu_board_digital_write(void* board, uint8_t gpio, bool high);
void meshemu_board_ledc_attach(void* board, uint8_t channel, uint8_t gpio);
bool meshemu_board_ledc_write(void* board, uint8_t channel, uint32_t period_us,
                              uint32_t high_time_us);
void meshemu_board_set_external_power(void* board, bool powered);
uint8_t meshemu_board_get_charger_state(void* board);
void meshemu_board_destroy(void* board);

#ifdef __cplusplus
}
#endif

#endif
