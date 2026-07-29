# Project Mycelium — Agent Instructions

**You are an AI agent working on Project Mycelium.** This file is your instruction manual. Read it before modifying code, opening PRs, or spawning sub-agents.

---

## What This Project Is

A **T-Deck hardware emulator** in Rust. It lets any MeshCore-compatible firmware run on a desktop — no physical T-Deck needed. The firmware compiles as a shared library that links against Mycelium's C FFI. Multiple instances share a virtual RadioBus with real propagation physics (RSSI, SINR, collisions, airtime).

**114 C FFI functions, 9 subsystems, 189 tests, full parity with SigurdOS and Wadamesh.**

## Quick Start

```bash
cd /home/ben/Project-Mycelium
cargo build --release          # build the emulator (requires SDL2)
cargo test --workspace          # 189 tests, should pass
cargo run --release -- --help   # CLI options
```

## Repo Layout

```
Project-Mycelium/
├── core/
│   ├── bridge/          Rust→C FFI bridge (ffi.rs + flash_ffi.rs)
│   │   ├── include/     10 C header files for the bridge API
│   │   └── src/         lib.rs (tests), ffi.rs (main exports), flash_ffi.rs (NVS)
│   ├── board/           VirtualBoard: battery, GPIO, deep sleep, PSRAM, NVS, WDT
│   ├── radio_bus/       RadioBus: propagation, collisions, RSSI/SINR, airtime
│   ├── display/         ST7789 320×240, LVGL v8/v9 backends, framebuffer
│   ├── input/           Keyboard (I2C@0x55), GT911 touch (@0x5D), trackball, Wire
│   ├── storage/         SPIFFS (flat/32-char), SD card (SDHC, mount ladder)
│   ├── gps/             L76K NMEA — GGA/RMC/GSV/GSA, 1Hz, GPX
│   └── src/             CLI entrypoint (main.rs)
├── firmware-sdk/        C headers for firmware authors
│   ├── meshemu.h        Master header (include this)
│   ├── include/         9 per-subsystem headers
│   └── tests/           ABI link test (C binary calling every FFI function)
├── gui/                 Web control panel (map, fleet, inspector, scenarios)
├── docs/                Parity reports, design docs
├── plan.md              Design plan (architecture, component tree, scenarios)
├── README.md            Human-facing docs
└── AGENTS.md            ← you are here
```

## Test Commands

```bash
cargo test --workspace                               # everything (189 tests)
cargo test -p meshemu_bridge                         # FFI bridge tests
cargo test -p mycelium_board                         # board/virtual hardware tests
cargo test -p radio_bus                              # propagation/collision tests
cargo test -p mycelium_input                         # input subsystem tests
cargo test -p mycelium_display                       # display tests
cargo test -p mycelium_storage                       # storage tests
cargo test -p mycelium_gps                           # GPS tests

# Format and lint
cargo fmt --check
cargo clippy --workspace -- -D warnings

# ABI link test (verifies C compatibility)
cd firmware-sdk/tests && ./check_abi.sh
```

## How to Add a New FFI Function

1. **Add the Rust implementation** in `core/bridge/src/ffi.rs` (or `flash_ffi.rs` for NVS):
   ```rust
   #[no_mangle]
   pub unsafe extern "C" fn meshemu_my_new_fn(param: *const c_char) -> bool {
       // implementation
   }
   ```

2. **Add the C header** in `core/bridge/include/meshemu_bridge_<subsystem>.h`:
   ```c
   bool meshemu_my_new_fn(const char* param);
   ```

3. **Add the firmware-facing header** in `firmware-sdk/include/meshemu_<subsystem>.h`

4. **Call it in the ABI test** at `firmware-sdk/tests/abi_link.c`

5. **Add a test** in `core/bridge/src/lib.rs`

6. **Run all:** `cargo test --workspace && cd firmware-sdk/tests && ./check_abi.sh`

## FFI Surface (114 Functions)

| Subsystem | Header | Functions | Key Patterns |
|-----------|--------|-----------|--------------|
| Board | `meshemu_bridge_board.h` | 18 | create, battery, ADC, PSRAM, deep sleep, WDT, temperature, GPIO quiesce |
| Display | `meshemu_bridge_display.h` | 9 | create/create_v/create_ex, capture, destroy |
| Input | `meshemu_bridge_input.h` | ~20 | inject_touch/key, poll, digital_read, falling_edges, GT911 status/modes |
| Radio | `meshemu_bridge_radio.h` | 12 | create, send, recv_raw (with truncation), RSSI/SNR, airtime, DIO2 config |
| Storage | `meshemu_bridge_storage.h` | 16 | SPIFFS/read/write, SD init/card_type/open/mkdir/read/write/remove/end |
| GPS | `meshemu_bridge_gps.h` | 5 | create, set_position, read, set_enabled, destroy |
| NVS | `meshemu_bridge_nvs.h` | 8 | init, exists, get_bool/string, put_bool/string, remove, destroy |
| Partition | `meshemu_bridge_partition.h` | 5 | set_launcher_mode, find_first, otadata_address, is_under_launcher |
| Wire/I2C | (in ffi.rs) | 10 | begin, write, requestFrom, probe, recovery, stuck-SDA, STOP |
| Bus | (in ffi.rs) | 1 | tick (monotonic time advance) |

## Critical Conventions

### NEVER modify
- `AGENTS.md` or `CONTRIBUTING.md` — repo owner only
- Submodule references or upstream repos
- `.github/workflows/ci.yml` to remove checks (only add dependencies or fix compilation)

### Rust conventions
- All public FFI functions are `#[no_mangle] pub unsafe extern "C"`
- All FFI functions return primitive types (bool, i32, u32, f32, f64, pointers) — never Rust enums or structs
- String parameters are `*const c_char` (null-terminated C strings)
- Memory returned to callers via `copy_for_caller()` pattern (malloc + free)

### Test conventions
- Every new FFI function gets a bridge test AND an ABI link test entry
- Tests for failure modes are as important as happy-path tests
- Run `cargo test --workspace` before pushing

### CI conventions
- CI runs: cargo test (workspace), cargo fmt --check, cargo clippy -- -D warnings, ABI link test
- All must pass before merge
- SDL2 dev libraries required (apt install libsdl2-dev on Ubuntu)

## Working with ForgeDeck Agents

When spawning ForgeDeck agents for this repo, follow these rules:

### Spawn pattern
```json
{
  "items": [{
    "cwd": "/home/ben/<worktree-dir>",
    "provider": "codex",
    "model": "gpt-5.6-sol",
    "effort": "medium",  // or "xhigh" for deep investigations
    "yolo": true,
    "category": "project-mycelium",
    "tags": ["fix", "<subsystem>"],
    "prompt": "..."
  }]
}
```

### Worktree pattern
```bash
cd /home/ben/Project-Mycelium
git worktree add --detach /home/ben/<name> origin/master
# agent works here, never touches upstream master
```

### Branch naming: `fix/<subsystem-description>` or `add/<feature>`

### After agent completes
```bash
# Check for open PRs
gh pr list --repo hermes-gadget/Project-Mycelium --state open
# Fix CI if needed, merge if all green
# Clean up
git worktree remove /home/ben/<name> --force
```

### DO NOT
- Swarm more than 6 agents at once (cap)
- Use ForgeDeck as first resort (try to fix issues yourself first)
- Let agents modify submodules or upstream repos
- Merge PRs that fail CI

## Parity Targets

Mycelium targets three real-world implementations for full compatibility:

1. **Physical T-Deck** (SigurdOS remote_test_radio firmware on real hardware via hermes-portable.local /dev/ttyACM0)
2. **SigurdOS HAL** (hermes-gadget/SigurdOS-tdeck at /home/ben/SigurdOS-tdeck)
3. **Wadamesh** (wadamesh-readonly at /home/ben/wadamesh-readonly, commit 4e6a9e0)

Parity reports live in `/home/ben/myc-vs-tdeck-wadamesh-parity-2026-07-29.md`.

## When Things Go Wrong

### CI failures
1. Rebase onto origin/master (other PRs may have merged ahead)
2. For conflicts: use `git merge-file` for 3-way merges on ffi.rs/lib.rs
3. Formatting: `cargo fmt`
4. ABI test segfaults: check for null string pointers, mismatched function signatures

### Agent PRs stuck
1. Check if the branch has commits on origin/ (the agent may have pushed without creating a PR)
2. If branch exists but no PR: `gh pr create --repo hermes-gadget/Project-Mycelium --base master --head <branch>`
3. If CI failing: fix the issue (usually ABI test or formatting), force-push

## Key Design Decisions

- **FFI-first**: Everything is a C function. No Rust enums/structs cross the boundary.
- **Monotonic time**: `meshemu_bus_tick(n)` advances all subsystems atomically.
- **Instance IDs**: String-based. Functions without an instance parameter use operation-global state (radio bus).
- **Storage is flat**: SPIFFS has no subdirectories, 32-char filename max, matching real ESP32.
- **Failure modes are first-class**: Every peripheral has configurable failure states (phantom touches, stuck I2C, slow SD, uncalibrated ADC, cold PSRAM, wrong DIO2).
- **All functions are `pub unsafe extern "C"`**: Safety is the caller's responsibility, matching the firmware integration contract.

## Version History

See the GitHub PR list for full history:
```bash
gh pr list --repo hermes-gadget/Project-Mycelium --state merged --limit 20
```
