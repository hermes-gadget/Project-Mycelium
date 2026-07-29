cmake_minimum_required(VERSION 3.16)
project(minimal-mesh-firmware CXX)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_CXX_EXTENSIONS OFF)

set(MYCELIUM_ROOT ${CMAKE_CURRENT_SOURCE_DIR}/../..)
set(MESHCORE_ROOT ${MYCELIUM_ROOT}/lib/meshcore)
set(MESHEMU_BRIDGE_LIB
    ${MYCELIUM_ROOT}/target/debug/${CMAKE_SHARED_LIBRARY_PREFIX}meshemu_bridge${CMAKE_SHARED_LIBRARY_SUFFIX}
)

add_custom_target(meshemu-bridge
    COMMAND cargo build --manifest-path ${MYCELIUM_ROOT}/Cargo.toml -p meshemu_bridge
    BYPRODUCTS ${MESHEMU_BRIDGE_LIB}
    WORKING_DIRECTORY ${MYCELIUM_ROOT}
    COMMENT "Building the Rust RadioBus bridge"
)

add_library(firmware SHARED
    firmware_entry.cpp
    ${MESHCORE_ROOT}/src/Dispatcher.cpp
    ${MESHCORE_ROOT}/src/Packet.cpp
    ${MESHCORE_ROOT}/src/helpers/StaticPoolPacketManager.cpp
    ${MYCELIUM_ROOT}/core/bridge/src/meshemu_bridge_clock.cpp
)
add_dependencies(firmware meshemu-bridge)
target_compile_options(firmware PRIVATE -Wall -Wextra)
target_include_directories(firmware PRIVATE
    ${MYCELIUM_ROOT}/core/bridge/include
    ${MESHCORE_ROOT}/src
)
target_include_directories(firmware SYSTEM PRIVATE ${MESHCORE_ROOT}/test/mocks)
target_link_libraries(firmware PRIVATE ${MESHEMU_BRIDGE_LIB})
set_target_properties(firmware PROPERTIES
    BUILD_RPATH ${MYCELIUM_ROOT}/target/debug
    PREFIX ""
)
