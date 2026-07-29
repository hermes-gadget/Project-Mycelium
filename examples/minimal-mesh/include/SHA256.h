#pragma once

#include <cstddef>
#include <cstdint>
#include <cstring>

// The minimal raw-flood example does not use MeshCore's encrypted packet
// helpers, but Packet.cpp still requires the SHA256 interface at link time.
class SHA256 {
public:
    void update(const void*, std::size_t) {}

    void finalize(std::uint8_t* hash, std::size_t hash_len) {
        std::memset(hash, 0, hash_len);
    }
};
