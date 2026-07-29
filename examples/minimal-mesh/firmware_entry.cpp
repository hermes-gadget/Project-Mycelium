#include <cstdio>
#include <cstring>

#include <Dispatcher.h>
#include <helpers/StaticPoolPacketManager.h>

#include "meshemu_bridge_clock.h"
#include "meshemu_bridge_radio.h"

namespace {

constexpr const char* NODE_NAME = "test-node";
constexpr const char* HELLO_MESSAGE = "hello mesh";

class TestDispatcher final : public mesh::Dispatcher {
public:
    TestDispatcher(mesh::Radio& radio, mesh::MillisecondClock& clock,
                   mesh::PacketManager& packets)
        : mesh::Dispatcher(radio, clock, packets) {}

protected:
    mesh::DispatcherAction onRecvPacket(mesh::Packet* packet) override {
        std::printf("[firmware] Received packet: %.*s\n",
                    static_cast<int>(packet->payload_len),
                    reinterpret_cast<const char*>(packet->payload));
        return ACTION_RELEASE;
    }
};

MeshemuRadio* radio = nullptr;
MeshemuClock* clock = nullptr;
StaticPoolPacketManager* packets = nullptr;
TestDispatcher* dispatcher = nullptr;
bool sent_hello = false;

}  // namespace

extern "C" {

void firmware_setup(void) {
    std::printf("[firmware] setup() called\n");

    radio = new MeshemuRadio(NODE_NAME, 868.0, 125, 9, 5, 14.0, 54.5, -1.2);
    clock = new MeshemuClock();
    packets = new StaticPoolPacketManager(8);
    dispatcher = new TestDispatcher(*radio, *clock, *packets);
    dispatcher->begin();

    std::printf("[firmware] Radio and dispatcher initialized\n");
}

void firmware_loop(void) {
    if (dispatcher == nullptr) {
        return;
    }

    dispatcher->loop();
    meshemu_bus_tick(clock->getMillis());

    if (!sent_hello) {
        mesh::Packet* packet = packets->allocNew();
        if (packet != nullptr) {
            packet->header =
                ROUTE_TYPE_TRANSPORT_FLOOD |
                (PAYLOAD_TYPE_RAW_CUSTOM << PH_TYPE_SHIFT);
            packet->path_len = 0;
            packet->payload_len = std::strlen(HELLO_MESSAGE);
            std::memcpy(packet->payload, HELLO_MESSAGE, packet->payload_len);
            packets->queueOutbound(packet, 0, clock->getMillis());
            std::printf("[firmware] Sent hello packet\n");
            sent_hello = true;
        }
    }
}

void* firmware_get_display(void) {
    return nullptr;
}

}  // extern "C"
