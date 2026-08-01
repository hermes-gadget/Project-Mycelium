# Project Mycelium

**Universal T-Deck + Mesh hardware emulator.** Run any MeshCore-compatible firmware on your desktop. No physical T-Deck required.

A Rust desktop application that emulates the LilyGo T-Deck (ESP32-S3, ST7789 320×240, SX1262 LoRa, GT911 touch, I2C keyboard). Firmware compiled as a dynamic library links against Mycelium's C FFI bridge, and multiple instances share a virtual RadioBus with simulated propagation, range, and collisions — just like real LoRa.

The desktop and headless `run` modes are available today. The web control
panel, map/fleet/inspector views, and browser-driven scenarios are planned;
`serve` currently reports that roadmap status.

**[Full FFI Reference →](firmware-sdk/meshemu.h)** | **[Parity Report →](docs/parity-2026-07-29.md)**

## Quick Start

```bash
git clone https://github.com/hermes-gadget/Project-Mycelium.git
cd Project-Mycelium

# Build the desktop emulator (requires SDL2)
cargo build --release

# Validate a firmware shared library
cargo run --release -- test --firmware ./path/to/libfirmware.so

# Run firmware with the desktop display
cargo run --release -- run --firmware ./path/to/libfirmware.so

# Run firmware without opening SDL2 windows (for CI or servers)
cargo run --release -- run --firmware ./path/to/libfirmware.so --headless

# Run all tests (205 tests, 0 failures)
cargo test --workspace

# Check the SDK headers, ABI calls, and built export inventory
make test-sdk-headers
```

## Architecture

```
┌──────────────────────────────────────────────────────────┐
│                   Mycelium Desktop App                    │
│                                                          │
│  ┌──────────────────┐   ┌──────────────────┐            │
│  │ Emulated T-Deck  │   │ Emulated T-Deck  │  ...       │
│  │ ┌──────────────┐ │   │ ┌──────────────┐ │            │
│  │ │ SDL2 Window  │ │   │ │ SDL2 Window  │ │            │
│  │ │  320×240     │ │   │ │  320×240     │ │            │
│  │ │ ┌──────────┐ │ │   │ │ ┌──────────┐ │ │            │
│  │ │ │  LVGL v9 │ │ │   │ │ │  LVGL v9 │ │ │            │
│  │ │ │ Firmware │ │ │   │ │ │ Firmware │ │ │            │
│  │ │ │   .so    │ │ │   │ │ │   .so    │ │ │            │
│  │ │ └──────────┘ │ │   │ │ └──────────┘ │ │            │
│  │ └──────────────┘ │   │ └──────────────┘ │            │
│  └───────┬──────────┘   └───────┬──────────┘            │
│          │                      │                        │
│          └──────────┬───────────┘                        │
│                     │                                    │
│             ┌───────▼────────┐                           │
│             │   RadioBus     │                           │
│             │  propagation,  │                           │
│             │  collision,    │                           │
│             │  RSSI/SINR     │                           │
│             └───────┬────────┘                           │
│                     │                                    │
│  ┌──────────────────▼──────────────────────────────┐    │
│  │     Planned Web Control Panel (not implemented)   │    │
│  │  Map │ Fleet │ Inspector │ Scenarios (roadmap)   │    │
│  └──────────────────────────────────────────────────┘    │
└──────────────────────────────────────────────────────────┘

core/
├── bridge/src/          Rust FFI bridge (133 C exports)
│   ├── ffi.rs           Main FFI — all subsystem exports
│   └── flash_ffi.rs     NVS + partition + launcher FFI
├── board/src/           Virtual T-Deck board, battery, power, PSRAM, NVS
├── radio_bus/src/       RadioBus — propagation, collisions, RSSI/SINR, airtime
├── display/src/          ST7789 320×240, LVGL v8/v9, framebuffer, RGB565
├── input/src/            Keyboard (I2C/0x55), touch (GT911/0x5D), trackball, Wire shim
├── storage/src/          SPIFFS (flat/32-char), SD card (SDHC/32GB), mount ladder
├── gps/src/              L76K NMEA (GGA/RMC/GSV/GSA), 1Hz output, GPX replay
└── src/                  CLI app entrypoint

firmware-sdk/
├── meshemu.h            Master C header (all subsystems)
├── include/              Host Services headers (also included by meshemu.h)
└── tests/                ABI link test (C binary calls every FFI function)

gui/                     Planned web control panel (see gui/README.md)
docs/                    Design docs, parity reports, API reference
```

## Hardware Emulation Coverage

**133 C FFI exports across the bridge.** Core peripheral behavior is tracked against the physical T-Deck, SigurdOS, and Wadamesh in the parity report.

| Subsystem | Physical Hardware | Mycelium FFI | Failure Modes | Key Capabilities |
|-----------|-------------------|--------------|---------------|------------------|
| **Display** | ST7789 320×240 SPI | 9 functions | v8/v9 backends, partial updates, RGB565 byte order | MADCTL, ST7789 emulation, shared SPI |
| **Keyboard** | ESP32-C3 @0x55 I2C | 8 functions | Cross-reset backlight, key-mode CMD 0x04 | Backlight CMD 0x01/0x02, Wire shim |
| **Touch** | GT911 @0x5D I2C | 10 functions | 3 phantom watchdogs, bus failure, frame stall, frozen-point | Raw/mapped coords, product-ID regs |
| **Trackball** | 5× GPIO (0,1,2,3,15) | 5 functions | gpio_intr_disable warm-handoff, sticky interrupt bits | Digital read, falling edges, event queue |
| **Radio** | SX1262 LoRa SPI | 12 functions | DIO2 RF switch (-16dB TX / -3dB RX), truncation | Semtech airtime, SINR, propagation, collisions |
| **Storage** | SPIFFS + SD (CS=39) | 16 functions | Flat namespace (32-char), SDHC ≤32GB, mount ladder | SD.open/read/write/mkdir/remove/cardType/totalBytes |
| **GPS** | L76K Serial1 (43/44) | 5 functions | 1Hz output gate, GSV fragments, GPX telemetry | NMEA GGA/RMC/GSV/GSA, baud cycling |
| **Board** | ESP32-S3 | 18 functions | ADC nonlinearity, GPIO10 quiesce, PSRAM absent halt | Battery ÷2 divider, deep sleep, RTC hold, WDT, MCU temp |
| **NVS/Partition** | 16-20KB flash partition | 13 functions | Launcher (otadata@0xD000), NVS geometry switch | Persistent JSON, chat scopes, password store |
| **I2C Bus** | Shared 18/8 @400kHz | 8 functions | SDA-stuck recovery, clock-out, STOP emit | Probe address, idle levels |
| **Boot Resilience** | ESP32 RTC_NOINIT | 5 functions | RTC RAM persistence, reset reasons, WDT timeout→reset | RTC_NOINIT_ATTR, esp_task_wdt, esp_reset_reason |

**Every failure mode Wadamesh tests in the field is emulated.** GT911 phantom touches, backlight surviving ESP.restart(), SD mount frequency stepping, DIO2 16dB penalty, Launcher NVS geometry, battery ADC nonlinearity, and storage migration crash recovery are all directly testable.

## For Firmware Authors

### Linking Against Mycelium

```c
#include "meshemu.h"  // single header for all subsystems

int main(void) {
    // Create virtual hardware
    void* board = meshemu_board_create("my-instance", 3700, 25.0f);
    void* display = meshemu_display_create(320, 240, "my-instance");
    void* radio = meshemu_radio_create("my-node", 869.618, 125, 8, 5, 14.0, 51.5, -0.1);
    void* gps = meshemu_gps_create("my-instance", 51.5, -0.1);

    // Main loop
    uint64_t now_ms = 0;
    while (1) {
        meshemu_bus_tick(++now_ms);  // provide a cumulative monotonic timestamp

        // Poll keyboard
        meshemu_input_inject_key("my-instance", 'a', true);

        // Send a radio packet
        uint8_t packet[] = "Hello mesh!";
        meshemu_radio_start_send(radio, packet, sizeof(packet));

        // Capture display
        size_t len = 0;
        uint8_t* pixels = meshemu_display_capture(display, &len);
        // ... render pixels ...
        meshemu_display_capture_free(pixels, len);
    }
}
```

See `firmware-sdk/meshemu.h` for the complete FFI reference and `firmware-sdk/include/` for per-subsystem headers.

## Built-In Test Patterns

```bash
# Run all 205 tests
cargo test --workspace

# Test specific subsystems
cargo test -p mycelium_board
cargo test -p meshemu_bridge
cargo test -p radio_bus

# ABI/header/export inventory and link test
make test-sdk-headers
```

## Project Status

- **FFI surface:** 133 C exports; canonical headers and ABI calls are inventory-checked
- **CLI:** `run`, `test`, and `run --headless` are available
- **Web control panel:** planned; `serve` is currently a stub
- **Test coverage:** 205 tests, 0 failures
- **Parity targets:** Physical T-Deck ✅ | SigurdOS ✅ | Wadamesh ✅
- **Failure-mode resilience:** GT911 phantoms ✅ | C3 cross-reset ✅ | DIO2 RF loss ✅ | SD mount ladder ✅ | NVS migration ✅
- **PRs merged:** 18 (audit fixes + peripheral + medium + gap closure)
- **Dependencies:** Rust (cargo + sdl2), C toolchain (for firmware SDK)

## Contributing

See [`AGENTS.md`](AGENTS.md) for autonomous agent instructions and [`CONTRIBUTING.md`](CONTRIBUTING.md) for human contributor guidelines.
