#pragma once

#include <Mesh.h>
#include <meshemu_types.h>

#include <atomic>
#include <cstdint>
#include <string>

class MeshemuBoard final : public mesh::MainBoard {
public:
    explicit MeshemuBoard(const MeshemuBoardConfig* config);

    uint16_t getBattMilliVolts() override;
    float getMCUTemperature() override;
    const char* getManufacturerName() const override;
    void reboot() override;
    void powerOff() override;
    uint8_t getStartupReason() const override;
    bool isExternalPowered() override;
    uint16_t getBootVoltage() override;

    void setBatteryMilliVolts(uint16_t millivolts);
    bool rebootRequested() const;
    bool powerOffRequested() const;

private:
    std::atomic<uint16_t> battery_mv_;
    float mcu_temperature_c_;
    std::string manufacturer_;
    uint8_t startup_reason_;
    bool external_powered_;
    uint16_t boot_voltage_mv_;
    std::atomic<bool> reboot_requested_;
    std::atomic<bool> power_off_requested_;
};

extern "C" {
void* meshemu_board_create(const char* instance_id,
                           const MeshemuBoardConfig* config);
void meshemu_board_set_battery(void* board, uint16_t millivolts);
uint16_t meshemu_board_get_battery(void* board);
void meshemu_board_destroy(void* board);
}
