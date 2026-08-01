#if __has_include(<Mesh.h>)
#include "meshemu_bridge_clock.h"
#else
// The Rust bridge is built in repositories where the optional MeshCore
// submodule is not checked out. Keep the C ABI implementation buildable there;
// firmware builds that include MeshCore still use the real base class from the
// public header above.
#include <atomic>
#include <chrono>
#include <cstdint>
namespace mesh {
class MillisecondClock {
public:
    virtual unsigned long getMillis() = 0;
};
}  // namespace mesh

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
#endif

#include <cstdint>
#include <limits>
#include <new>

#if defined(__GNUC__)
#define MESHEMU_CLOCK_EXPORT __attribute__((visibility("default")))
#else
#define MESHEMU_CLOCK_EXPORT
#endif

MeshemuClock::MeshemuClock()
    : started_at_(std::chrono::steady_clock::now()), offset_ms_(0) {}

uint64_t MeshemuClock::getMillis64() const {
    const auto elapsed = std::chrono::duration_cast<std::chrono::milliseconds>(
        std::chrono::steady_clock::now() - started_at_);
    const auto elapsed_ms = static_cast<uint64_t>(elapsed.count());
    const auto offset_ms = offset_ms_.load();
    if (offset_ms >= 0) {
        const auto positive_offset = static_cast<uint64_t>(offset_ms);
        if (elapsed_ms > std::numeric_limits<uint64_t>::max() - positive_offset) {
            return std::numeric_limits<uint64_t>::max();
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

extern "C" MESHEMU_CLOCK_EXPORT void* meshemu_clock_create() {
    return new (std::nothrow) MeshemuClock();
}

extern "C" MESHEMU_CLOCK_EXPORT uint64_t meshemu_clock_millis(void* clock) {
    const auto* instance = static_cast<const MeshemuClock*>(clock);
    return instance == nullptr ? 0 : instance->getMillis64();
}

extern "C" MESHEMU_CLOCK_EXPORT void meshemu_clock_set_offset(void* clock, int64_t offset_ms) {
    auto* instance = static_cast<MeshemuClock*>(clock);
    if (instance != nullptr) {
        instance->setOffset(offset_ms);
    }
}

extern "C" MESHEMU_CLOCK_EXPORT void meshemu_clock_destroy(void* clock) {
    delete static_cast<MeshemuClock*>(clock);
}
