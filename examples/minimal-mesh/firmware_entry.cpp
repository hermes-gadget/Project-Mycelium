#include <cstdio>
#include <cstring>
#include <memory>
#include <string>
#include <vector>

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
                   mesh::PacketManager& packets, const char* node_name)
        : mesh::Dispatcher(radio, clock, packets), node_name_(node_name) {}

protected:
    mesh::DispatcherAction onRecvPacket(mesh::Packet* packet) override {
        std::printf("[firmware:%s] Received packet: %.*s\n", node_name_,
                    static_cast<int>(packet->payload_len),
                    reinterpret_cast<const char*>(packet->payload));
        return ACTION_RELEASE;
    }

private:
    const char* node_name_;
};

struct NodeContext {
    explicit NodeContext(std::size_t node_number)
        : name(std::string(NODE_NAME) + "-" + std::to_string(node_number)),
          radio(name.c_str(), 868.0, 125, 9, 5, 14.0, 54.5, -1.2),
          packets(8),
          dispatcher(radio, clock, packets, name.c_str()) {
        dispatcher.begin();
    }

    void loop() {
        dispatcher.loop();
        MeshemuRadio::tick(clock.getMillis());

        if (!sent_hello) {
            mesh::Packet* packet = packets.allocNew();
            if (packet != nullptr) {
                packet->header =
                    ROUTE_TYPE_TRANSPORT_FLOOD |
                    (PAYLOAD_TYPE_RAW_CUSTOM << PH_TYPE_SHIFT);
                packet->path_len = 0;
                packet->payload_len = std::strlen(HELLO_MESSAGE);
                std::memcpy(packet->payload, HELLO_MESSAGE,
                            packet->payload_len);
                packets.queueOutbound(packet, 0, clock.getMillis());
                std::printf("[firmware:%s] Sent hello packet\n", name.c_str());
                sent_hello = true;
            }
        }
    }

    std::string name;
    MeshemuRadio radio;
    MeshemuClock clock;
    StaticPoolPacketManager packets;
    TestDispatcher dispatcher;
    bool sent_hello = false;
};

// The loader may reuse one dlopen handle for multiple requested instances, so
// keep one firmware context per setup call and advance them round-robin.
std::vector<std::unique_ptr<NodeContext>> nodes;
std::size_t next_node = 0;

}  // namespace

extern "C" {

void firmware_setup(void) {
    std::printf("[firmware] setup() called\n");

    auto node = std::make_unique<NodeContext>(nodes.size() + 1);
    std::printf("[firmware:%s] Radio and dispatcher initialized\n",
                node->name.c_str());
    nodes.push_back(std::move(node));
}

void firmware_loop(void) {
    if (nodes.empty()) {
        return;
    }

    nodes[next_node]->loop();
    next_node = (next_node + 1) % nodes.size();
}

void* firmware_get_display(void) {
    return nullptr;
}

}  // extern "C"
