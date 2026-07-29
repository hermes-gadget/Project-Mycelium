#include "meshemu_bridge_clock.h"

#include <new>

MeshemuClock::MeshemuClock()
    : started_at_(std::chrono::steady_clock::now()), offset_ms_(0) {}

uint64_t MeshemuClock::getMillis64() const {
    const auto elapsed = std::chrono::duration_cast<std::chrono::milliseconds>(
        std::chrono::steady_clock::now() - started_at_);
    const auto elapsed_ms = static_cast<uint64_t>(elapsed.count());
    const auto offset_ms = offset_ms_.load();
    if (offset_ms >= 0) {
        const auto positive_offset = static_cast<uint64_t>(offset_ms);
        if (elapsed_ms > UINT64_MAX - positive_offset) {
            return UINT64_MAX;
        }
        return elapsed_ms + positive_offset;
    }
    const auto negative_offset =
        static_cast<uint64_t>(-(offset_ms + 1)) + 1;
    return elapsed_ms > negative_offset ? elapsed_ms - negative_offset : 0;
}

unsigned long MeshemuClock::getMillis() {
    return static_cast<unsigned long>(getMillis64());
}

void MeshemuClock::setOffset(int64_t offset_ms) {
    offset_ms_.store(offset_ms);
}

extern "C" void* meshemu_clock_create() {
    return new (std::nothrow) MeshemuClock();
}

extern "C" uint64_t meshemu_clock_millis(void* clock) {
    const auto* instance = static_cast<const MeshemuClock*>(clock);
    return instance == nullptr ? 0 : instance->getMillis64();
}

extern "C" void meshemu_clock_set_offset(void* clock, int64_t offset_ms) {
    auto* instance = static_cast<MeshemuClock*>(clock);
    if (instance != nullptr) {
        instance->setOffset(offset_ms);
    }
}

extern "C" void meshemu_clock_destroy(void* clock) {
    delete static_cast<MeshemuClock*>(clock);
}
