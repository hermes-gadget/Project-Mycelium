# Project Mycelium — Design Plan

> **Universal T-Deck + Mesh emulator.** Run any MeshCore-compatible firmware on your desktop. No hardware required.

---

## 1. Architecture Overview

### High-Level System Diagram

```
┌──────────────────────────────────────────────────────────────┐
│                    Mycelium Desktop App                       │
│                                                              │
│  ┌──────────────────────┐  ┌──────────────────────────────┐  │
│  │   Emulated T-Deck #1 │  │   Emulated T-Deck #2         │  │
│  │   ┌──────────────┐   │  │   ┌──────────────┐           │  │
│  │   │ SDL2 Window  │   │  │   │ SDL2 Window  │  ...      │  │
│  │   │ (320×240)    │   │  │   │ (320×240)    │           │  │
│  │   │ ┌──────────┐ │   │  │   │ ┌──────────┐ │           │  │
│  │   │ │  LVGL v9 │ │   │  │   │ │  LVGL v9 │ │           │  │
│  │   │ │  Firmware│ │   │  │   │ │  Firmware│ │           │  │
│  │   │ │   .so    │ │   │  │   │ │   .so    │ │           │  │
│  │   │ └──────────┘ │   │  │   │ └──────────┘ │           │  │
│  │   └──────────────┘   │  │   └──────────────┘           │  │
│  └──────────┬───────────┘  └──────────┬───────────────────┘  │
│             │                         │                       │
│       VirtualRadio               VirtualRadio                 │
│             │                         │                       │
│             └─────────┬───────────────┘                       │
│                       │                                       │
│              ┌────────▼────────┐                              │
│              │    RadioBus     │                              │
│              │  (propagation,  │                              │
│              │   collision,    │                              │
│              │   RSSI model)   │                              │
│              └────────┬────────┘                              │
│                       │                                       │
│  ┌────────────────────▼───────────────────────────────────┐  │
│  │              Web Control Panel (localhost:9170)          │  │
│  │  ┌─────────┐ ┌─────────┐ ┌───────────┐ ┌────────────┐  │  │
│  │  │  Map    │ │  Fleet  │ │ Inspector │ │ Scenarios  │  │  │
│  │  └─────────┘ └─────────┘ └───────────┘ └────────────┘  │  │
│  └─────────────────────────────────────────────────────────┘  │
└──────────────────────────────────────────────────────────────┘
```

### Component Tree

```
meshemu (Rust binary)
├── Engine
│   ├── InstanceManager      — spawn/kill/pause virtual T-Decks
│   ├── RadioBus             — virtual radio channel with propagation model
│   ├── DisplayManager       — SDL2 window per instance
│   ├── InputManager         — host input → T-Deck peripheral mapping
│   ├── StorageManager       — virtual SPIFFS + SD on host filesystem
│   ├── GpsManager           — NMEA sentence generation per instance
│   └── BoardManager         — virtual MainBoard (battery, power, temp)
├── Firmware Host (C FFI)
│   ├── dlopen() loader      — loads firmware .so per instance
│   └── Host Services API    — C functions the firmware calls back into
├── Web Server (axum/warp)
│   ├── REST API             — instance CRUD, state queries
│   └── WebSocket            — real-time packet trace, position updates
└── GUI (TypeScript/React)
    ├── Map                  — node positions, signal rings, packet animation
    ├── Fleet                — instance list with status, logs
    ├── Inspector            — per-node radio params, packet stats, contacts
    └── Scenario Runner      — YAML scenario editor + executor
```

### Data Flow

```
Firmware calls mesh::Radio::startSendRaw(data, len)
  → VirtualRadio.send(data, len)
    → RadioBus.broadcast(node_id, freq, sf, bw, data, at_time)
      → for each other node in range:
        → calculate RSSI from distance + tx_power
        → check collision window
        → if received cleanly: push to node's VirtualRadio.incoming queue
          → next firmware loop: mesh::Radio::recvRaw() returns the packet
            → Dispatcher.onRecvPacket() — full MeshCore stack processes it
```

### Process Model

**Single process, multiple threads:**

| Thread | Role |
|--------|------|
| Main | SDL2 event loop, window management |
| Per-instance (×N) | Firmware main loop: LVGL tick + mesh::Dispatcher::loop() |
| RadioBus | Dedicated thread handling packet routing, propagation, collision |
| Web server | axum async runtime, serves API + WebSocket |

Each firmware `.so` is loaded once, then each instance thread calls its functions with per-instance state. This avoids the overhead of separate processes while keeping instances isolated.

### Architecture Decision: dlopen() + Per-Instance State

Rather than running separate OS processes per virtual node (expensive, hard to coordinate), Mycelium uses `dlopen()` to load the firmware once, then spawns threads that call `firmware_setup()` → `firmware_loop()` in a loop, with each thread getting its own set of virtual hardware handles. This means:

- One `.so` = one firmware = N virtual nodes all running the same code
- Each node's state is isolated through the virtual hardware handles (each gets its own VirtualRadio, VirtualBoard, etc.)
- Multiple `.so` files can be loaded simultaneously if you want different firmware versions in the same simulation

---

## 2. Core Engine Design

### 2.1 RadioBus — Virtual Radio Network

The RadioBus is the heart of Mycelium. It replaces physical LoRa with a software simulation that models real RF behavior.

#### Propagation Model

```
Free-space path loss: L = 20·log₁₀(d) + 20·log₁₀(f) + 32.45
RSSI at receiver: RSSI = TX_power - L + antenna_gain_tx + antenna_gain_rx
Packet received if: RSSI > sensitivity(SF, BW)
Collision if: two packets overlap in time on same frequency
```

| Parameter | Range | Default |
|-----------|-------|---------|
| Frequency | 433/868/915 MHz | 868 MHz |
| Bandwidth | 125/250/500 kHz | 125 kHz |
| Spreading Factor | 7-12 | 9 |
| TX Power | 0-22 dBm | 14 dBm |
| Antenna Gain | configurable | 2.15 dBi |
| Noise Floor | configurable | -120 dBm |

#### Channel Abstraction

The RadioBus models distinct radio *channels* (frequency + BW + SF). Packets on different channels don't collide. This matches real MeshCore behavior where different regions use different channel plans.

```rust
struct RadioChannel {
    freq_mhz: f64,       // center frequency
    bandwidth_khz: u16,  // 125, 250, 500
    spreading_factor: u8, // 7-12
    coding_rate: u8,     // 5-8 (4/5 to 4/8)
}
```

#### Packet Routing

```
VirtualRadio::startSendRaw(bytes, len)
  → RadioBus::enqueue(TxEvent {
        node: sender_id,
        channel: current_channel,
        data: bytes,
        tx_power_dbm: tx_power,
        airtime_ms: airtime,
        timestamp: now,
    })
  → RadioBus::process_tx_event(event)
    → for each node in simulation:
      → if node.channel matches event.channel:
        → d = distance(sender_pos, node_pos)
        → rssi = calc_rssi(d, event.tx_power_dbm, event.channel.freq_mhz)
        → if rssi > sensitivity(event.channel.sf):
          → check collision window at node
          → if no collision: deliver packet to node
```

#### Collision Detection

LoRa is vulnerable to co-channel interference. If two packets overlap in time at a receiver, both are lost (unless one is significantly stronger — the capture effect). Mycelium models:

- **Hard collision**: two packets overlap on same channel → both garbled
- **Capture effect**: if one signal is >6 dB stronger, it may be decoded (SF-dependent)
- **CAD (Channel Activity Detection)**: modeled as a brief pre-TX check window

### 2.2 Display — SDL2 + LVGL Integration

LVGL v9 has built-in SDL2 support via `LV_USE_SDL`. Mycelium leverages this directly.

#### How It Works

1. Mycelium initializes SDL2, creates a window per virtual node
2. The firmware calls `firmware_setup()` which creates its LVGL display via the host services API
3. Mycelium intercepts the display creation: instead of `lv_sdl_window_create()` directly from firmware, Mycelium wraps it to manage window lifecycle
4. The firmware's LVGL flush callback (which normally writes to SPI/ST7789) is replaced with an SDL2 framebuffer flush
5. `sigurdos_display_loop()` / `lv_timer_handler()` runs in the instance thread, Mycelium's SDL event loop runs on the main thread

#### Display Configuration

```c
// Provided by Mycelium host services — firmware calls this instead of direct LVGL init
lv_display_t* meshemu_display_create(int width, int height, const char* title);
```

Under the hood, this calls `lv_sdl_window_create(320, 240)` with a title like "T-Deck — Node3 (SigurdOS)".

#### LVGL Version Strategy

| LVGL Version | Support | Notes |
|-------------|---------|-------|
| v9 | Primary target | SigurdOS uses v9. SDL backend is mature. |
| v8 | Compatibility shim | Wadamesh may use v8. Mycelium provides a v8→v9 mapping layer OR a separate v8 SDL backend. |
| v7 and earlier | Not supported | |

### 2.3 Input — Host → T-Deck Peripheral Mapping

| T-Deck Peripheral | Host Input | Mapping |
|-------------------|-----------|---------|
| GT911 Touch (I2C) | Mouse click/drag in SDL window | Screen coordinates direct-mapped (320×240 → window size) |
| I2C Keyboard (ESP32-C3 × 0x55) | Host keyboard | Scancode → T-Deck key matrix position. Layout-aware. |
| Trackball (5-dir GPIO) | Arrow keys + Enter | Up/Down/Left/Right/Center |
| Power button | Escape key | Triggers sleep/shutdown flow |

#### Keyboard Mapping Details

The T-Deck keyboard uses an ESP32-C3 co-processor on I2C address 0x55. Mycelium doesn't emulate the co-processor — it intercepts at the LVGL input device level. When the firmware creates a keyboard input device, Mycelium replaces it with an SDL keyboard driver that maps host keys to the T-Deck key matrix.

For firmware that reads raw I2C keyboard data (bypassing LVGL), Mycelium provides a virtual I2C bus that returns pre-mapped key events.

### 2.4 Storage — Virtual SPIFFS & SD Card

| Storage | ESP32 Implementation | Mycelium Emulation |
|---------|---------------------|-------------------|
| SPIFFS | ESP32 flash partition | `~/.mycelium/instances/<id>/spiffs/` directory |
| SD Card | SPI to FAT32 | `~/.mycelium/instances/<id>/sdcard/` directory |

Mycelium provides `fopen()/fwrite()/fread()` compatible wrappers. For firmware using Arduino's `SPIFFS` or `SD` libraries directly, Mycelium provides shim implementations that redirect to the host filesystem.

### 2.5 GPS — Virtual NMEA Generator

```rust
struct VirtualGps {
    latitude: f64,       // decimal degrees
    longitude: f64,
    altitude: f64,       // meters
    speed_knots: f64,
    course_deg: f64,
    satellites: u8,      // fake satellite count
    hdop: f64,           // horizontal dilution of precision
    enabled: bool,
    movement_model: MovementModel,
}

enum MovementModel {
    Static,                          // fixed position
    Linear { speed_ms: f64, heading: f64 },  // constant velocity
    Waypoint { points: Vec<(f64,f64)>, speed_ms: f64 }, // route following
    GpxReplay { path: PathBuf, speed_multiplier: f64 },  // GPX file replay
}
```

NMEA sentences generated: `$GPGGA`, `$GPRMC`, `$GPGSA`, `$GPGSV`.

### 2.6 Board — Virtual MainBoard

Implements `mesh::MainBoard` with configurable values:

```c
typedef struct {
    uint16_t battery_mv;      // default 3900
    float mcu_temperature;    // default 35.0
    const char* manufacturer; // "Mycelium Virtual T-Deck"
    uint8_t startup_reason;   // BD_STARTUP_NORMAL
    bool external_powered;    // default false
} MeshemuBoardConfig;
```

### 2.7 Instance Lifecycle

```
spawn("Node1", firmware.so, config)  → InstanceHandle
  → dlopen(firmware.so)
  → create SDL window
  → init VirtualRadio, VirtualBoard, VirtualGps, VirtualStorage
  → spawn instance thread
  → thread calls firmware_setup(), then loops firmware_loop()

pause(handle)   → suspend instance thread, freeze radio
resume(handle)  → resume thread
kill(handle)    → stop thread, destroy window, free resources
snapshot(handle) → dump full state for save/restore
```

---

## 3. Firmware SDK

### 3.1 The 3-Function API

```c
// meshemu.h — Stable public ABI

#ifdef __cplusplus
extern "C" {
#endif

// Called once at startup. The firmware initializes all subsystems.
// Mycelium provides hardware handles via global host service functions.
void firmware_setup(void);

// Called each main loop iteration (~100 Hz target).
// The firmware processes one tick: LVGL, mesh, GPS, etc.
void firmware_loop(void);

// Optional: returns LVGL display handle. NULL for headless firmwares.
// Used by Mycelium to detect LVGL version and configure SDL rendering.
void* firmware_get_display(void);

#ifdef __cplusplus
}
#endif
```

### 3.2 Host Services (What Mycelium Provides to Firmware)

Rather than expanding the 3-function API, Mycelium provides a set of C functions the firmware calls to access virtual hardware. These are linked at compile time.

```c
// Radio
void meshemu_radio_init(mesh::Radio** out_radio, const char* instance_id);
// Returns a VirtualRadio that talks to the shared RadioBus

// Board
void meshemu_board_init(mesh::MainBoard** out_board, const MeshemuBoardConfig* config);

// Clock
void meshemu_clock_init(mesh::MillisecondClock** out_clock);

// Packet Manager
void meshemu_packet_manager_init(mesh::PacketManager** out_pm, int pool_size);

// Display (for LVGL firmwares)
lv_display_t* meshemu_display_create(int width, int height, const char* window_title);

// Storage
void meshemu_spiffs_init(const char* instance_id);
void meshemu_sdcard_init(const char* instance_id);

// GPS
void meshemu_gps_init(const MeshemuGpsConfig* config);
void meshemu_gps_update_position(double lat, double lon);  // runtime position changes

// Buzzer
void meshemu_buzzer_beep(int duration_ms, int frequency_hz);

// Logging (bridged to Rust tracing)
void meshemu_log(const char* level, const char* message);
```

### 3.3 Build System Integration

Firmware authors add a PlatformIO `native_emu` environment:

```ini
# platformio.ini addition
[native_emu]
platform = native
build_flags =
    -DMYCELIUM_EMULATION
    -I/path/to/mycelium/firmware-sdk/include
lib_deps =
    mycelium-sdk
```

Or for CMake-based firmwares:

```cmake
find_package(mycelium-sdk REQUIRED)
add_library(my_firmware SHARED firmware_entry.cpp ...)
target_link_libraries(my_firmware PRIVATE mycelium-sdk::host-services meshcore)
```

### 3.4 The Adapter Pattern (Key for Wadamesh Compatibility)

Each supported firmware gets a small adapter `.cpp` file shipped WITH Mycelium:

```
mycelium/
└── adapters/
    ├── sigurdos/
    │   └── adapter.cpp       # ~200 lines — wires SigurdOS HAL to Mycelium host services
    ├── wadamesh/
    │   └── adapter.cpp       # ~200 lines — wires Wadamesh HAL to Mycelium host services
    └── minimal/
        └── adapter.cpp       # ~100 lines — reference implementation
```

**The adapter does NOT modify the firmware. It sits between Mycelium and the firmware, translating calls.**

What an adapter handles:
1. **Pin definitions**: Mycelium doesn't have real pins. The adapter provides stub pin values or redirects pin-dependent init to Mycelium host services.
2. **Radio init**: Intercepts `RadioLibWrapper` or direct SX1262 init, replaces with `meshemu_radio_init()`.
3. **Display init**: Intercepts LovyanGFX/ST7789 init, replaces with `meshemu_display_create()`.
4. **Board init**: Replaces `TDeckBoard::begin()` or equivalent with `meshemu_board_init()`.
5. **Platform-specific includes**: Provides shim headers for `Arduino.h`, `SPI.h`, `Wire.h`, `esp_*.h` that redirect to Mycelium equivalents.
6. **Preprocessor guards**: The adapter compiles the firmware with `-DMYCELIUM_EMULATION` so the firmware can `#ifdef` out ESP32-specific code paths. If the firmware doesn't have these guards, the adapter's shim headers paper over the differences.

For Wadamesh specifically, the adapter needs to handle:
- Wadamesh's HAL layer (likely different file structure than SigurdOS)
- Potentially different LVGL version (v8 vs v9)
- Different pin naming conventions
- Different init sequences

All of this is handled in the adapter — zero changes to Wadamesh source.

---

## 4. Compatibility Layer

### 4.1 Design Philosophy

Mycelium works with any firmware that targets MeshCore's public interfaces. The compatibility strategy has three tiers:

| Tier | Effort | What it covers |
|------|--------|---------------|
| **Tier 1: Drop-in** | Zero firmware changes | Firmware uses MeshCore abstractions (Radio, MainBoard, etc.) through virtual dispatch. Mycelium provides implementations. |
| **Tier 2: Shim headers** | Firmware compiles with `-DMYCELIUM_EMULATION` | If firmware directly includes ESP32 headers, Mycelium provides shim headers that redirect. Firmware source unchanged. |
| **Tier 3: Adapter .cpp** | Small adapter file in Mycelium repo | For firmwares with tight coupling to specific hardware. The adapter lives in Mycelium's `adapters/` directory. Firmware source unchanged. |

### 4.2 How It Works For Different Firmware Architectures

**SigurdOS (Tier 1-2):**
SigurdOS already has `native_test` environment with mocks. The adapter simply replaces those mocks with Mycelium's live implementations. Most of the work is done — SigurdOS was built with testing in mind.

**Wadamesh (Tier 2-3):**
Wadamesh is the litmus test. The adapter provides:
1. Shim `Arduino.h` — redirects `millis()`, `delay()`, `pinMode()`, `digitalWrite()` to Mycelium equivalents
2. Shim `SPI.h` / `Wire.h` — I2C keyboard reads return fake scan codes, SPI display writes go to SDL framebuffer
3. Virtual SX1262 — intercepts RadioLib calls, redirects to VirtualRadio
4. Stub `esp_*` headers — provide ESP32-specific types/functions that don't exist on x86

**Any other MeshCore firmware:**
If it uses `mesh::Radio`, `mesh::MainBoard`, `mesh::MillisecondClock`, `mesh::PacketManager`, and `mesh::Dispatcher` through their virtual interfaces, it works with zero adapter code.

### 4.3 Handling Firmware-Specific Quirks

| Quirk | How Mycelium Handles It |
|-------|------------------------|
| Different LVGL version | Dual LVGL backends: v9 (primary) + v8 shim. Detected via `firmware_get_display()`. |
| Different pin definitions | Mycelium ignores pins. All GPIO reads/writes go through virtual board. |
| ESP32-specific peripherals (RTC, eFuse, etc.) | Shim headers provide no-op or sensible defaults. |
| Different Arduino API versions | Mycelium provides a superset of commonly-used Arduino functions. |
| FreeRTOS dependencies | Single-threaded in Mycelium; `xTaskCreate` → direct function call. |

---

## 5. Web Control Panel

### 5.1 Tech Stack

| Layer | Choice | Rationale |
|-------|--------|-----------|
| Frontend | TypeScript + React | Mature ecosystem, good map libraries |
| Map rendering | Leaflet.js or deck.gl | Open-source, handles custom tile layers |
| Charts | uPlot or Chart.js | Lightweight, real-time updates |
| Build | Vite | Fast HMR, TypeScript-native |
| Backend comms | WebSocket (primary) + REST (CRUD) | Real-time packet tracing needs WS |

### 5.2 Views

**Map View**
- Node markers at configured lat/lon positions
- Animated signal propagation rings on TX
- Color-coded node status (idle, transmitting, receiving, error)
- Click node → inspector panel
- Drag nodes to reposition (updates GPS emulation)
- Range rings showing each node's effective coverage
- Packet trace overlay: animated line from sender → receiver on each successful delivery

**Fleet View**
- Table of all running instances
- Status: running/paused/stopped
- Uptime, packet count, last activity
- Start/stop/pause/kill buttons
- CPU/memory usage per instance
- Log stream per instance

**Inspector View**
- Per-node detail panel:
  - Radio: frequency, SF, BW, TX power, RSSI history graph
  - Contacts: known nodes table
  - Channels: subscribed channels
  - Packet stats: sent/received/dropped/collided
  - Storage: SPIFFS/SD usage
  - GPS: current position, satellite count, fix status
  - LVGL screen: live framebuffer capture (screenshot button)

**Scenario Runner**
- YAML editor with syntax highlighting
- Scenario library (pre-built scenarios)
- Run button with live progress
- Assertion results table (pass/fail)
- Timeline view showing actions over simulation time

### 5.3 WebSocket Protocol

```
// Client → Server
{ "type": "spawn", "name": "Node1", "firmware": "sigurdos", "config": {...} }
{ "type": "pause", "instance_id": "..." }
{ "type": "resume", "instance_id": "..." }
{ "type": "kill", "instance_id": "..." }
{ "type": "move", "instance_id": "...", "lat": 54.5, "lon": -1.2 }
{ "type": "inject_message", "instance_id": "...", "channel": "public", "text": "hello" }

// Server → Client
{ "type": "packet_trace", "from": "Node1", "to": "Node2", "rssi": -72, "snr": 7.5, "airtime_ms": 45 }
{ "type": "instance_state", "instance_id": "...", "state": "running", "stats": {...} }
{ "type": "scenario_progress", "step": 3, "total": 10, "status": "running" }
{ "type": "assertion_result", "step": "flood_3", "passed": true, "detail": "Node5 received flood in 234ms" }
```

---

## 6. CLI Interface

### 6.1 Commands

```bash
# Run a simulation
meshemu run \
  --firmware ./sigurdos.so \
  --nodes 5 \
  --scenario flood_test.yaml \
  --duration 300 \
  --output results.json

# Headless mode (no GUI, no SDL)
meshemu run \
  --headless \
  --firmware ./sigurdos.so \
  --nodes 20 \
  --scenario stress_test.yaml

# List available firmwares
meshemu list-firmwares

# Run a single scenario and exit
meshemu test --scenario regression.yaml

# Version
meshemu version

# Start the web control panel standalone
meshemu serve --port 9170
```

### 6.2 Configuration File

```yaml
# meshemu.yaml — placed in /home/ben/Project-Mycelium/ or ~/.mycelium/
radio:
  default_frequency_mhz: 868.0
  default_bandwidth_khz: 125
  default_sf: 9
  default_tx_power_dbm: 14
  noise_floor_dbm: -120
  propagation_model: freespace  # or: simple_range, itu_r

display:
  window_scale: 2  # 2x pixel scaling for larger window
  show_fps: false

firmwares:
  sigurdos:
    path: /home/ben/SigurdOS-tdeck/.pio/build/native_emu/firmware.so
    lvgl_version: v9
  wadamesh:
    path: /home/ben/wadamesh/.pio/build/native_emu/firmware.so
    lvgl_version: v8
    adapter: adapters/wadamesh/adapter.cpp

instances:
  max_nodes: 100
  default_config:
    battery_mv: 3900
    storage_path: ~/.mycelium/instances/

web:
  port: 9170
  host: 127.0.0.1

cli:
  color: true
  log_level: info
```

### 6.3 CI Integration

```bash
# Exit code reflects scenario assertion results
meshemu test --scenario ci/regression.yaml --format tap > results.tap
echo $?  # 0 = all passed, 1 = assertions failed, 2 = runtime error

# JSON output for programmatic consumption
meshemu test --scenario ci/stress.yaml --format json > results.json
```

---

## 7. Scenario System

### 7.1 YAML Schema

```yaml
# scenario: basic_flood.yaml
name: "Basic Flood Test"
description: "5 nodes, verify flood reaches all recipients within 2 seconds"
duration_seconds: 60  # max runtime

nodes:
  - id: node1
    firmware: sigurdos
    position: { lat: 54.500, lon: -1.200 }
    config:
      node_name: "Alpha"
      frequency_mhz: 868.0
      sf: 9
      tx_power_dbm: 14
  - id: node2
    firmware: sigurdos
    position: { lat: 54.501, lon: -1.199 }
    config:
      node_name: "Bravo"
  - id: node3
    firmware: sigurdos
    position: { lat: 54.502, lon: -1.201 }
    config:
      node_name: "Charlie"
  # ... more nodes

steps:
  - wait: 5s  # let nodes boot and discover each other

  - action: send_message
    from: node1
    to: broadcast
    channel: public
    text: "Hello mesh!"

  - wait: 2s

  - assert:
      type: message_received
      by: [node2, node3, node4, node5]
      text_contains: "Hello mesh!"
      within: 2s  # from send_message

  - assert:
      type: packet_count
      node: node1
      sent: { min: 1 }
      
  - assert:
      type: packet_count
      node: node2
      received: { min: 1 }

  - action: move
    node: node5
    to: { lat: 54.600, lon: -1.300 }
    speed: walking  # predefined speed models: walking, driving, instant

  - wait: 5s

  - assert:
      type: contact_count
      node: node5
      min: 0   # moved out of range
```

### 7.2 Assertion Types

| Assertion | What it checks |
|-----------|---------------|
| `message_received` | Node received a message matching criteria |
| `packet_count` | Packet stats (sent/received/dropped/collided) |
| `contact_count` | Known contacts in node's contact store |
| `rssi_range` | Signal strength of received packet |
| `latency_range` | Time from send to receive |
| `route_contains` | Packet routing path includes specific nodes |
| `channel_subscription` | Node is subscribed to expected channels |
| `state_equals` | Arbitrary state check via inspector API |
| `log_contains` | Node log contains expected string |
| `no_crash` | Node is still running after scenario |

### 7.3 Scenario Execution Model

```
1. Parse YAML → Scenario AST
2. Validate node positions, actions, assertions
3. Spawn all nodes (parallel startup)
4. Execute steps sequentially:
   - wait: pause execution for duration
   - action: send command to node(s)
   - assert: collect data, evaluate condition
5. On assertion failure: log failure, optionally abort or continue
6. Tear down all nodes
7. Output results in requested format
```

---

## 8. Phased Development Roadmap

### Phase 1: RadioBus + VirtualRadio (Week 1-2)
**Goal:** Multiple virtual MeshCore nodes communicating through simulated radio.

| Deliverable | Detail |
|-------------|--------|
| VirtualRadio | Implements mesh::Radio, connects to RadioBus |
| RadioBus (in-process) | Shared-memory channel, basic propagation |
| mesh::Dispatcher integration | Full production MeshCore stack running natively |
| Minimal test firmware | No display, just radio — sends/receives packets |
| CLI: `meshemu run --nodes 2` | Two instances, can exchange mesh packets |
| Node position config | Manual lat/lon in config, used for range calc |

**Testable:** `meshemu run --nodes 3` → node1 sends flood → node2/node3 receive. Move node3 out of range → it stops receiving.

**Dependencies:** None. Pure Rust + C FFI for firmware loading.

### Phase 2: Display Emulation (Week 3-4)
**Goal:** LVGL UI visible in SDL2 windows.

| Deliverable | Detail |
|-------------|--------|
| SDL2 display backend | Window per instance, LVGL v9 integration |
| Firmware display handoff | `firmware_get_display()` returns LVGL handle |
| SigurdOS adapter (partial) | Enough to boot SigurdOS UI on desktop |
| Framebuffer capture | Screenshot per instance for web panel |
| Multi-window management | Tiling, minimize, focus handling |

**Testable:** `meshemu run --firmware sigurdos.so --nodes 1` → T-Deck UI appears in a window. Navigate screens with mouse.

**Dependencies:** Phase 1 (radio init is part of firmware boot).

### Phase 3: Input Emulation (Week 5)
**Goal:** Fully interactive emulated T-Deck — type, tap, navigate.

| Deliverable | Detail |
|-------------|--------|
| Mouse → Touch mapping | Click/drag → GT911-style LVGL touch events |
| Host keyboard → T-Deck keyboard | Scancode mapping, layout-aware |
| Arrow keys → Trackball | Up/Down/Left/Right/Center events |
| I2C keyboard shim | For firmware reading raw I2C, provide virtual bus |
| Auto-off / wake behavior | Matches hardware power-saving |

**Testable:** Type a message in SigurdOS chat, send via mesh, receive on another virtual node. Navigate settings, change theme.

**Dependencies:** Phase 2.

### Phase 4: Peripheral Emulation (Week 6-7)

| Deliverable | Detail |
|-------------|--------|
| SPIFFS emulation | Host directory per instance, file read/write |
| SD card emulation | Host directory, directory listing, file ops |
| GPS NMEA generator | Static/linear/waypoint/GPX models |
| Battery emulation | Configurable voltage, discharge simulation |
| Buzzer emulation | Audio beep via host sound |
| Board emulation | mesh::MainBoard full implementation |
| WiFi stub | If firmware has WiFi-dependent features, provide minimal stub |

**Testable:** Full SigurdOS boot cycle — storage mount, GPS fix, battery reading, settings persistence across restarts.

**Dependencies:** Phase 2-3.

### Phase 5: Web Control Panel (Week 8-9)

| Deliverable | Detail |
|-------------|--------|
| Backend (axum) | REST API + WebSocket server |
| Map view | Leaflet.js, node markers, signal rings |
| Fleet view | Instance table, start/stop, live logs |
| Inspector | Per-node detail panel |
| Scenario runner UI | YAML editor, run/stop, results view |
| WebSocket protocol | Real-time packet trace, state updates |

**Testable:** `meshemu serve` → open browser → see live map with virtual nodes. Click node → inspector shows radio params. Drag node → position updates.

**Dependencies:** Phase 1-4 (needs running instances to display).

### Phase 6: Scenario System + CLI Polish (Week 10-11)

| Deliverable | Detail |
|-------------|--------|
| YAML scenario parser | Full schema support |
| Scenario executor | Sequential step execution with assertions |
| Assertion engine | All assertion types from Section 7.2 |
| TAP output format | For CI integration |
| JSON output format | For programmatic consumption |
| CLI polish | Error messages, progress bars, colors |
| Headless mode | `--headless` flag, no SDL dependency |
| CI example | GitHub Actions workflow using Mycelium |

**Testable:** `meshemu test --scenario flood_test.yaml` → all assertions pass → exit code 0. Change scenario to expect wrong RSSI → assertion fails → exit code 1.

**Dependencies:** Phase 1-4 (scenarios control instances).

### Phase 7: Firmware SDK Polish + Wadamesh Support (Week 12-13)

| Deliverable | Detail |
|-------------|--------|
| SigurdOS adapter (complete) | Full adapter, documented, tested |
| Wadamesh adapter | Proves Mycelium is firmware-agnostic |
| Minimal mesh example | Reference firmware for new integrations |
| SDK documentation | API reference, integration guide, examples |
| LVGL v8 compatibility | Shim layer for v8-based firmwares |
| Build system templates | PlatformIO env + CMakeLists.txt |
| `mycelium-sdk` package | Distributable C SDK |

**Testable:** Wadamesh firmware running in Mycelium with zero source changes. `meshemu run --firmware wadamesh.so` → Wadamesh UI appears, mesh works.

**Dependencies:** All prior phases.

### Effort Summary

| Phase | Weeks | Core Deliverable |
|-------|-------|-----------------|
| 1 | 1-2 | RadioBus + VirtualRadio |
| 2 | 3-4 | SDL2 display |
| 3 | 5 | Input emulation |
| 4 | 6-7 | Peripherals (storage, GPS, battery) |
| 5 | 8-9 | Web control panel |
| 6 | 10-11 | Scenario system + CLI |
| 7 | 12-13 | SDK polish, wadamesh, docs |
| **Total** | **~13 weeks** | **v1.0 release** |

---

## 9. Technical Decisions & Rationale

| Decision | Choice | Why |
|----------|--------|-----|
| Language | Rust for engine, C FFI for firmware interface | Safety, performance, strong FFI story. Rust's ownership model prevents data races in multi-threaded simulation. |
| Display backend | SDL2 via LVGL's built-in driver | Zero work to get LVGL rendering on desktop. LVGL v9 has `LV_USE_SDL` with display+mouse+keyboard in one call. |
| Firmware loading | dlopen() shared library (.so) | No recompilation of emulator when firmware changes. Standard on Linux. For macOS: dlopen(). For Windows: LoadLibrary(). |
| Radio simulation | In-process shared bus (<20 nodes), optional UDP multicast (>20 nodes) | Shared memory is zero-copy and fast for typical mesh sizes. UDP scales to hundreds of virtual nodes across machines. |
| Web panel | Axum (Rust) + React (TypeScript) + WebSocket | Axum is async-native Rust. React is ubiquitous. WebSocket gives real-time packet tracing without polling. |
| LVGL strategy | v9 primary, v8 shim | SigurdOS uses v9 (future-proof). Wadamesh may use v8. Shim layer catches v8 → v9 API differences. |
| Single process, threads | Yes | Easier than IPC. Threads share the RadioBus efficiently. dlopen handles symbol isolation. |
| Scenario format | YAML | Human-readable, diffable, CI-friendly. |
| License | MIT or Apache 2.0 | Maximize adoption. Wadamesh is GPL but Mycelium is a separate process that links at runtime via dlopen, so license compatibility is not an issue. |

---

## 10. Open Questions & Risks

### 10.1 LVGL Timer Callbacks Without ESP32 Timer
**Risk:** SigurdOS uses `lv_timer_handler()` which needs a millisecond-precise tick source. On ESP32, this comes from the FreeRTOS tick.
**Mitigation:** Mycelium provides `meshemu_get_millis()` which returns host `std::chrono::steady_clock` time. The instance thread runs a tight loop calling `firmware_loop()` which calls `lv_timer_handler()` with ~1ms granularity. Verified by the native_test environment which already does this.

### 10.2 Thread Safety of RadioBus
**Risk:** Multiple instance threads calling `VirtualRadio::startSendRaw()` simultaneously while RadioBus processes packets.
**Mitigation:** RadioBus uses a lock-free SPSC queue per virtual radio. The bus thread drains all queues, computes propagation, and pushes to receiver queues. Instance threads never directly access shared state.

### 10.3 Determinism for CI
**Risk:** CI tests need reproducible results. Thread scheduling and timing are non-deterministic.
**Mitigation:** RadioBus supports a "deterministic mode" where time advances in discrete steps. The scenario runner controls the clock. All random decisions (collision probability, noise) use a seeded PRNG. This enables byte-for-byte reproducible test runs.

### 10.4 Memory Scaling With Node Count
**Risk:** Each virtual node runs a full MeshCore stack. At 100 nodes, memory could be substantial.
**Mitigation:** Profile early. MeshCore's per-node state is mostly the Dispatcher (~few KB), packet pool (~tens of KB), and LVGL display buffer (320×240×2 = 150KB). At 100 nodes without displays (headless), roughly 5-10 MB per node = 500MB-1GB total. With displays, much more. Headless mode for large-scale testing.

### 10.5 Cross-Platform Support

| Platform | Status |
|----------|--------|
| Linux | Primary target and best supported |
| macOS | SDL2 + LVGL work. dlopen() is dylib. Minor path differences. |
| Windows | LoadLibrary() instead of dlopen(). Build with MSVC or mingw. Lower priority. |

### 10.6 Wadamesh Compatibility Risk
**Risk:** Wadamesh may use ESP32-specific features that are hard to shim.
**Mitigation:** Research Wadamesh source during Phase 1-2 to identify potential blockers. The adapter approach means any incompatibility is solved once in the adapter, not in Wadamesh. Known potential issues: direct ESP-IDF API calls (WiFi, BLE, deep sleep) — these get stubbed or mapped to host equivalents.

### 10.7 LVGL v8 vs v9 API Differences
**Risk:** Wadamesh may use LVGL v8, which has different APIs than v9.
**Mitigation:** Mycelium's dual-backend approach. The v8 shim provides the v8 LVGL API backed by Mycelium's SDL2 infrastructure. Most LVGL v8 apps compile against v9 with minor changes; the shim handles these.

---

## Appendix A: Wadamesh Compatibility Strategy

Wadamesh (https://github.com/ALLFATHER-BV/wadamesh) is the benchmark for Mycelium's firmware-agnostic claim. It is a separate open-source T-Deck firmware that also uses MeshCore and LVGL.

**Strategy:**
1. Mycelium ships a `adapters/wadamesh/adapter.cpp` (~200 lines)
2. This adapter provides:
   - Shim headers replacing ESP32-specific includes
   - Virtual radio init replacing Wadamesh's SX1262 setup
   - Virtual display replacing Wadamesh's ST7789 init
   - Virtual board replacing Wadamesh's power management
3. Wadamesh source code: ZERO changes
4. Build command: `pio run -e native_emu` (or equivalent CMake) — added by the user per the SDK docs, not modifying Wadamesh's repo
5. The adapter.cpp is distributed with Mycelium, not with Wadamesh

**Success criterion:** `meshemu run --firmware wadamesh.so` boots Wadamesh UI, connects to mesh, sends/receives messages — with no modifications to the Wadamesh repository.

---

## Appendix B: SigurdOS Compatibility

SigurdOS is the primary target and easiest integration. It already has:
- `native_test` environment with mocks for Arduino, LVGL, RadioLib, MeshCore
- Clean HAL abstraction layer (`src/hal/`)
- MeshCore integration via `mesh::Dispatcher` with `mesh::Radio` virtual interface

The SigurdOS adapter primarily maps the existing mock infrastructure to Mycelium's live implementations, replacing file-based test mocks with real interactive hardware emulation.

---

*Plan version: 1.0 — 2026-07-29*
