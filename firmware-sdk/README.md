# Mycelium Firmware SDK

The Mycelium Firmware SDK is the C interface between a MeshCore-based firmware
and Mycelium's virtual T-Deck hardware. A firmware shared library exports three
entry points for the emulator and calls the Host Services API to create and
control its radio, board, display, GPS, storage, NVS, partition table, input,
and buzzer facilities.

The SDK contains declarations only. Mycelium supplies the implementations when
it loads the firmware.

## Canonical headers

The C ABI declarations in `core/bridge/include/meshemu_bridge_*.h` are the
canonical source of truth and must match the Rust `extern "C"` exports. The
headers in `firmware-sdk/include/` are forwarding headers, so firmware uses
SDK-friendly names without maintaining a second copy of each prototype.

When adding or changing a bridge export, update its canonical bridge header and
extend `firmware-sdk/tests/abi_link.c` to call it through the corresponding SDK
header. Run `make test-sdk-headers` to compile the C consumer and link it against
the Rust bridge.

## Integration

Add `firmware-sdk/`, `firmware-sdk/include/`, and `core/bridge/include/` to the
firmware's header search path. Include `meshemu.h` wherever the three firmware
entry points are defined, and include individual Host Services headers wherever
their APIs are used. If the SDK is vendored, vendor the canonical bridge
headers with it and update the third include path accordingly.

### PlatformIO

Add the SDK directories to the environment's `build_flags`:

```ini
[env:mycelium]
platform = native
build_flags =
    -I/path/to/Project-Mycelium/firmware-sdk
    -I/path/to/Project-Mycelium/firmware-sdk/include
    -I/path/to/Project-Mycelium/core/bridge/include
```

When the SDK and bridge headers are vendored inside the PlatformIO project,
replace the absolute paths with paths relative to the project directory.

### CMake

Expose the headers through an interface target and link it to the firmware
target:

```cmake
add_library(meshemu_sdk INTERFACE)
target_include_directories(meshemu_sdk INTERFACE
    "${PROJECT_SOURCE_DIR}/firmware-sdk"
    "${PROJECT_SOURCE_DIR}/firmware-sdk/include"
    "${PROJECT_SOURCE_DIR}/core/bridge/include"
)

target_link_libraries(my_firmware PRIVATE meshemu_sdk)
```

Build the firmware as the shared library format expected by Mycelium. The
emulator resolves the exported entry points and Host Services symbols at load
time.

## API reference

| Header | Purpose |
| --- | --- |
| [`meshemu.h`](meshemu.h) | Required `firmware_setup`, `firmware_loop`, and optional-display contract |
| [`meshemu_radio.h`](include/meshemu_radio.h) | RadioBus-backed virtual radio |
| [`meshemu_board.h`](include/meshemu_board.h) | Virtual MeshCore `MainBoard` and battery state |
| [`meshemu_display.h`](include/meshemu_display.h) | SDL2-backed LVGL display and framebuffer capture |
| [`meshemu_storage.h`](include/meshemu_storage.h) | Host-directory-backed SPIFFS and SD card |
| [`meshemu_nvs.h`](include/meshemu_nvs.h) | Persistent, namespace-aware ESP32 Preferences/NVS |
| [`meshemu_partition.h`](include/meshemu_partition.h) | Standalone/Launcher ESP32 partition layouts |
| [`meshemu_gps.h`](include/meshemu_gps.h) | Virtual GPS position and NMEA sentence stream |
| [`meshemu_input.h`](include/meshemu_input.h) | Virtual T-Deck I2C keyboard |
| [`meshemu_buzzer.h`](include/meshemu_buzzer.h) | Host audio tone playback |

## Minimal firmware

This example creates a display during setup, performs one unit of firmware work
per loop call, and exposes the display handle to Mycelium:

```c
#include "meshemu.h"
#include "meshemu_display.h"

static void* display;

void firmware_setup(void)
{
    display = meshemu_display_create(320, 240, "minimal-firmware");
}

void firmware_loop(void)
{
    /* Run one non-blocking MeshCore/LVGL loop iteration here. */
}

void* firmware_get_display(void)
{
    return display;
}
```

Keep `firmware_loop()` non-blocking so Mycelium can advance every virtual node
and process UI, radio, and input events each frame. A headless firmware should
return `NULL` from `firmware_get_display()`.

## Display memory and fidelity

Display capture returns packed RGB565 in host-native `uint16_t` byte order,
not high-byte-first ST7789 SPI order. The default backend consumes
already-corrected, top-left-origin logical LVGL pixels and uses a 24-row
partial draw buffer.

Use `meshemu_display_create_ex()` to choose a different number of draw-buffer
rows or opt into the ST7789 command/orientation model. See
[`core/display/README.md`](../core/display/README.md) for the exact pixel,
controller, shared-SPI, and UI-only backend contract.
