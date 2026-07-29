cmake_minimum_required(VERSION 3.16)
project(minimal-mesh-firmware CXX)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_CXX_EXTENSIONS OFF)

add_library(firmware SHARED firmware_entry.cpp)
target_compile_options(firmware PRIVATE -Wall -Wextra)
target_include_directories(firmware PRIVATE
    ${CMAKE_CURRENT_SOURCE_DIR}/../../core/bridge/include
    ${CMAKE_CURRENT_SOURCE_DIR}/../../lib/meshcore/src
)
