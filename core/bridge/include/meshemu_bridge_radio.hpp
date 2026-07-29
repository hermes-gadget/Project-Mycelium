#pragma once

#include "meshemu_bridge_radio.h"

#include <Mesh.h>

#include <vector>

class MeshemuRadio final : public mesh::Radio {
public:
    MeshemuRadio(const char* id, double freq_mhz, uint16_t bandwidth_khz,
                 uint8_t spreading_factor, uint8_t coding_rate,
                 double tx_power_dbm, double lat, double lon)
        : handle_(meshemu_radio_create(id, freq_mhz, bandwidth_khz,
                                       spreading_factor, coding_rate,
                                       tx_power_dbm, lat, lon)) {}

    ~MeshemuRadio() { meshemu_radio_destroy(handle_); }

    MeshemuRadio(const MeshemuRadio&) = delete;
    MeshemuRadio& operator=(const MeshemuRadio&) = delete;
    MeshemuRadio(MeshemuRadio&&) = delete;
    MeshemuRadio& operator=(MeshemuRadio&&) = delete;

    bool valid() const { return handle_ != nullptr; }

    int recvRaw(uint8_t* bytes, int size) override {
        return meshemu_radio_recv_raw(handle_, bytes, size);
    }

    uint32_t getEstAirtimeFor(int len_bytes) override {
        return meshemu_radio_get_est_airtime(handle_, len_bytes);
    }

    float packetScore(float, int) override { return 1.0f; }

    bool startSendRaw(const uint8_t* bytes, int len) override {
        if (bytes == nullptr || len < 0) {
            return false;
        }
        last_send_buffer_.assign(bytes, bytes + len);
        return meshemu_radio_start_send(
            handle_, last_send_buffer_.data(),
            static_cast<uint32_t>(last_send_buffer_.size()));
    }

    bool isSendComplete() override {
        return meshemu_radio_is_send_complete(handle_);
    }

    void onSendFinished() override { last_send_buffer_.clear(); }
    bool isInRecvMode() const override { return handle_ != nullptr; }
    float getLastRSSI() const override {
        return meshemu_radio_get_rssi(handle_);
    }
    float getLastSNR() const override {
        return meshemu_radio_get_snr(handle_);
    }

    void setPosition(double lat, double lon) {
        meshemu_radio_set_position(handle_, lat, lon);
    }

    static void tick(uint64_t now_ms) { meshemu_bus_tick(now_ms); }

private:
    void* handle_;
    std::vector<uint8_t> last_send_buffer_;
};
