#pragma once

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

typedef struct {
    double lat;
    double lon;
    double altitude_m;
} MeshemuPosition;

typedef struct {
    uint16_t battery_mv;
    float mcu_temperature_c;
    const char* manufacturer;
    uint8_t startup_reason;
    bool external_powered;
} MeshemuBoardConfig;

typedef struct {
    double freq_mhz;
    uint16_t bandwidth_khz;
    uint8_t spreading_factor;
    uint8_t coding_rate;
    int8_t tx_power_dbm;
} MeshemuRadioConfig;

typedef struct {
    double latitude;
    double longitude;
    double altitude_m;
    double speed_knots;
    double course_deg;
    uint8_t satellites;
    double hdop;
    bool enabled;
} MeshemuGpsConfig;

typedef enum {
    MYCELIUM_LOG_TRACE,
    MYCELIUM_LOG_DEBUG,
    MYCELIUM_LOG_INFO,
    MYCELIUM_LOG_WARN,
    MYCELIUM_LOG_ERROR,
} MeshemuLogLevel;
