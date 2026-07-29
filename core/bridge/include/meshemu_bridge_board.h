#ifndef MESHEMU_BRIDGE_BOARD_H
#define MESHEMU_BRIDGE_BOARD_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#define MESHEMU_TP4054_CHARGING 0
#define MESHEMU_TP4054_CHARGED 1
#define MESHEMU_TP4054_NO_BATTERY 2
#define MESHEMU_SLEEP_WAKE_UNKNOWN 0
#define MESHEMU_SLEEP_WAKE_TIMER 1
#define MESHEMU_SLEEP_WAKE_EXT1 2
#define MESHEMU_SLEEP_WAKE_TIMER_EXT1 3
#define MESHEMU_DEFAULT_PSRAM_SIZE_BYTES 8388608U
#define RESET_REASON_UNKNOWN 0
#define RESET_REASON_DEEPSLEEP 5
#define RESET_REASON_TASK_WDT 9
#define RESET_REASON_SW 12
#define MESHEMU_WDT_DISABLED 0
#define MESHEMU_WDT_ENABLED 1
#define MESHEMU_WDT_TIMED_OUT 2

void* meshemu_board_create(const char* instance_id, uint16_t millivolts,
                           float temperature_c);
void meshemu_board_set_battery(void* board, uint16_t millivolts);
uint16_t meshemu_board_get_battery(void* board);
uint16_t meshemu_board_get_adc(void* board, uint8_t gpio);
void meshemu_board_set_adc_calibration(void* board, bool calibrated);
float meshemu_board_get_temp(void* board);
void meshemu_board_set_mcu_temperature(void* board, float celsius);
float meshemu_board_get_mcu_temperature(void* board);
bool meshemu_board_set_rtc_noinit(const char* instance_id, size_t offset,
                                  const uint8_t* data, size_t len);
bool meshemu_board_get_rtc_noinit(const char* instance_id, size_t offset,
                                  uint8_t* data, size_t len);
void meshemu_board_clear_rtc_noinit(const char* instance_id);
bool meshemu_board_psram_found(void* board);
uint32_t meshemu_board_get_psram_free(void* board);
bool meshemu_board_psram_readback_test(void* board);
bool meshemu_board_psram_reserve(void* board, uint32_t bytes);
void meshemu_board_psram_release(void* board, uint32_t bytes);
void meshemu_board_digital_write(void* board, uint8_t gpio, bool high);
void meshemu_board_set_periph_power(void* board, bool enabled);
void meshemu_board_ledc_attach(void* board, uint8_t channel, uint8_t gpio);
bool meshemu_board_ledc_write(void* board, uint8_t channel, uint32_t period_us,
                              uint32_t high_time_us);
void meshemu_board_set_external_power(void* board, bool powered);
uint8_t meshemu_board_get_charger_state(void* board);
void meshemu_board_rtc_gpio_hold(void* board, uint8_t gpio, bool level);
bool meshemu_board_set_reset_reason(void* board, uint8_t reason);
uint8_t meshemu_board_get_reset_reason(void* board);
void meshemu_board_wdt_init(void* board, uint32_t timeout_sec,
                            bool panic_on_timeout);
bool meshemu_board_wdt_feed(void* board);
uint8_t meshemu_board_wdt_get_status(void* board);
void meshemu_board_wdt_disable(void* board);
bool meshemu_board_quiesce_peripherals(void* board);
uint64_t meshemu_board_deep_sleep(const char* instance_id, uint32_t sleep_secs,
                                 uint64_t wake_pin_mask);
uint8_t meshemu_board_get_sleep_wake_cause(void);
void meshemu_board_set_boot_phase(uint8_t phase);
uint8_t meshemu_board_get_last_boot_phase(void);
void meshemu_board_destroy(void* board);

#ifdef __cplusplus
}
#endif

#endif
