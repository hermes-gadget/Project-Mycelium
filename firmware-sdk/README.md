# Mycelium Firmware SDK

The Mycelium Firmware SDK is the C interface between a MeshCore-based firmware
and Mycelium's virtual T-Deck hardware. A firmware shared library exports three
entry points for the emulator and calls the Host Services API to create and
control its radio, board, display, GPS, storage, input, clock, packet manager,
buzzer, and logging facilities.

The SDK contains declarations only. Mycelium supplies the implementations when
it loads the firmware.

## Integration

Add both `firmware-sdk/` and `firmware-sdk/include/` to the firmware's header
search path. Include `meshemu.h` wherever the three firmware entry points are
defined, and include individual Host Services headers wherever their APIs are
used.

### PlatformIO

Add the SDK directories to the environment's `build_flags`:

```ini
[env:mycelium]
platform = native
build_flags =
    -I/path/to/Project-Mycelium/firmware-sdk
    -I/path/to/Project-Mycelium/firmware-sdk/include
```

When the SDK is vendored inside the PlatformIO project, replace the absolute
paths with paths relative to the project directory, such as
`-Ifirmware-sdk` and `-Ifirmware-sdk/include`.

### CMake

Expose the headers through an interface target and link it to the firmware
target:

```cmake
add_library(meshemu_sdk INTERFACE)
target_include_directories(meshemu_sdk INTERFACE
    "${PROJECT_SOURCE_DIR}/firmware-sdk"
    "${PROJECT_SOURCE_DIR}/firmware-sdk/include"
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
| [`meshemu_types.h`](include/meshemu_types.h) | Shared board, radio, GPS, position, and logging types |
| [`meshemu_radio.h`](include/meshemu_radio.h) | RadioBus-backed virtual radio and packet statistics |
| [`meshemu_board.h`](include/meshemu_board.h) | Virtual MeshCore `MainBoard` and battery state |
| [`meshemu_clock.h`](include/meshemu_clock.h) | Virtual MeshCore millisecond clock |
| [`meshemu_packets.h`](include/meshemu_packets.h) | Virtual packet manager |
| [`meshemu_display.h`](include/meshemu_display.h) | SDL2-backed LVGL display and framebuffer capture |
| [`meshemu_storage.h`](include/meshemu_storage.h) | Host-directory-backed SPIFFS and SD card |
| [`meshemu_gps.h`](include/meshemu_gps.h) | Virtual GPS position and NMEA sentence stream |
| [`meshemu_input.h`](include/meshemu_input.h) | Virtual T-Deck I2C keyboard |
| [`meshemu_buzzer.h`](include/meshemu_buzzer.h) | Host audio tone playback |
| [`meshemu_log.h`](include/meshemu_log.h) | Firmware-to-Mycelium tracing bridge |

## Minimal firmware

This example creates a display during setup, performs one unit of firmware work
per loop call, and exposes the display handle to Mycelium:

```c
#include "meshemu.h"
#include "meshemu_display.h"
#include "meshemu_log.h"

static void* display;

void firmware_setup(void)
{
    display = meshemu_display_create(320, 240, "Minimal Mycelium Firmware");
    MYCELIUM_INFO("firmware", "setup complete");
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
