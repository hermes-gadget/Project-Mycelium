# Project Mycelium

**Universal T-Deck + Mesh hardware emulator.** Run any MeshCore-compatible firmware on your desktop. No physical T-Deck required.

A Rust desktop application that emulates the LilyGo T-Deck (ESP32-S3, ST7789 320×240, SX1262 LoRa, GT911 touch, I2C keyboard). Firmware compiled as a dynamic library links against Mycelium's C FFI bridge, and multiple instances share a virtual RadioBus with simulated propagation, range, and collisions — just like real LoRa.

**[Full FFI Reference →](firmware-sdk/meshemu.h)** | **[Parity Report →](docs/parity-2026-07-29.md)**

## Quick Start

```bash
git clone https://github.com/hermes-gadget/Project-Mycelium.git
cd Project-Mycelium

# Build the desktop emulator (requires SDL2)
cargo build --release

# Run with a test firmware
cargo run --release -- --firmware ./target/release/libmycelium_example.so

# Run all tests (189 tests, 0 failures)
cargo test --workspace

# Open the web control panel
open http://localhost:9170
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
│  │          Web Control Panel (:9170)                │    │
│  │  Map │ Fleet │ Inspector │ Scenarios             │    │
│  └──────────────────────────────────────────────────┘    │
└──────────────────────────────────────────────────────────┘

core/
├── bridge/src/          Rust FFI bridge (114 C functions)
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
├── include/              Individual subsystem headers
└── tests/                ABI link test (C binary calls every FFI function)

gui/                     Web control panel (map, fleet, inspector, scenarios)
cli/                     Headless CLI for CI/CD
docs/                    Design docs, parity reports, API reference
```

## Hardware Emulation Coverage

**114 C FFI functions across 9 subsystems.** Full parity with physical T-Deck + SigurdOS + Wadamesh.

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
    while (1) {
        meshemu_bus_tick(1);  // advance time by 1ms

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
# Run all 189 tests (covers every FFI function with failure-modes)
cargo test --workspace

# Test specific subsystems
cargo test -p mycelium_board
cargo test -p meshemu_bridge
cargo test -p radio_bus

# ABI link test — compiles a C binary that calls every FFI function
cd firmware-sdk && make test-abi
```

## Project Status

- **FFI surface:** 114 C functions, complete
- **Subsystems covered:** 9/9 (display, keyboard, touch, trackball, radio, storage, GPS, board, NVS)
- **Test coverage:** 189 tests, 0 failures
- **Parity targets:** Physical T-Deck ✅ | SigurdOS ✅ | Wadamesh ✅
- **Failure-mode resilience:** GT911 phantoms ✅ | C3 cross-reset ✅ | DIO2 RF loss ✅ | SD mount ladder ✅ | NVS migration ✅
- **PRs merged:** 18 (audit fixes + peripheral + medium + gap closure)
- **Dependencies:** Rust (cargo + sdl2), C toolchain (for firmware SDK)

## Contributing

See [`AGENTS.md`](AGENTS.md) for autonomous agent instructions and [`CONTRIBUTING.md`](CONTRIBUTING.md) for human contributor guidelines.
