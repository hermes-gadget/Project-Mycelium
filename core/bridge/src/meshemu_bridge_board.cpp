#include "meshemu_bridge_board.h"

#include <cmath>
#include <new>

namespace {
constexpr uint16_t kDefaultBatteryMillivolts = 3700;
constexpr const char* kDefaultManufacturer = "Project Mycelium";
}  // namespace

MeshemuBoard::MeshemuBoard(const MeshemuBoardConfig* config)
    : battery_mv_(config == nullptr ? kDefaultBatteryMillivolts
                                    : config->battery_mv),
      mcu_temperature_c_(config == nullptr ? NAN
                                           : config->mcu_temperature_c),
      manufacturer_(config == nullptr || config->manufacturer == nullptr
                        ? kDefaultManufacturer
                        : config->manufacturer),
      startup_reason_(config == nullptr ? 0 : config->startup_reason),
      external_powered_(config != nullptr && config->external_powered),
      boot_voltage_mv_(battery_mv_.load()),
      reboot_requested_(false),
      power_off_requested_(false) {}

uint16_t MeshemuBoard::getBattMilliVolts() {
    return battery_mv_.load();
}

float MeshemuBoard::getMCUTemperature() {
    return mcu_temperature_c_;
}

const char* MeshemuBoard::getManufacturerName() const {
    return manufacturer_.c_str();
}

void MeshemuBoard::reboot() {
    reboot_requested_.store(true);
}

void MeshemuBoard::powerOff() {
    power_off_requested_.store(true);
}

uint8_t MeshemuBoard::getStartupReason() const {
    return startup_reason_;
}

bool MeshemuBoard::isExternalPowered() {
    return external_powered_;
}

uint16_t MeshemuBoard::getBootVoltage() {
    return boot_voltage_mv_;
}

void MeshemuBoard::setBatteryMilliVolts(uint16_t millivolts) {
    battery_mv_.store(millivolts);
}

bool MeshemuBoard::rebootRequested() const {
    return reboot_requested_.load();
}

bool MeshemuBoard::powerOffRequested() const {
    return power_off_requested_.load();
}

extern "C" void* meshemu_board_create(
    const char* instance_id, const MeshemuBoardConfig* config) {
    (void)instance_id;
    try {
        return new MeshemuBoard(config);
    } catch (...) {
        return nullptr;
    }
}

extern "C" void meshemu_board_set_battery(void* board, uint16_t millivolts) {
    auto* instance = static_cast<MeshemuBoard*>(board);
    if (instance != nullptr) {
        instance->setBatteryMilliVolts(millivolts);
    }
}

extern "C" uint16_t meshemu_board_get_battery(void* board) {
    auto* instance = static_cast<MeshemuBoard*>(board);
    return instance == nullptr ? 0 : instance->getBattMilliVolts();
}

extern "C" void meshemu_board_destroy(void* board) {
    delete static_cast<MeshemuBoard*>(board);
}
