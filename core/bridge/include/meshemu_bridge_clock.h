#pragma once

#include <Mesh.h>

#include <atomic>
#include <chrono>
#include <cstdint>

class MeshemuClock final : public mesh::MillisecondClock {
public:
    MeshemuClock();

    unsigned long getMillis() override;
    uint64_t getMillis64() const;
    void setOffset(int64_t offset_ms);

private:
    std::chrono::steady_clock::time_point started_at_;
    std::atomic<int64_t> offset_ms_;
};

extern "C" {
void* meshemu_clock_create();
uint64_t meshemu_clock_millis(void* clock);
void meshemu_clock_set_offset(void* clock, int64_t offset_ms);
void meshemu_clock_destroy(void* clock);
}
