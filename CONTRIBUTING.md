# Contributing to Project Mycelium

## Getting Started

1. **Clone**: `git clone https://github.com/hermes-gadget/Project-Mycelium.git`
2. **Build**: `cargo build --release`
3. **Test**: `cargo test --workspace` (189 tests must pass)
4. **Format**: `cargo fmt --check && cargo clippy --workspace -- -D warnings`

## Pull Request Checklist

- [ ] `cargo test --workspace` passes (all 189 tests)
- [ ] `cargo fmt --check` passes
- [ ] `cargo clippy --workspace -- -D warnings` passes
- [ ] `firmware-sdk/tests/check_abi.sh` passes (ABI link test)
- [ ] New FFI functions have: Rust impl, C header (bridge), C header (firmware), ABI test entry, bridge test
- [ ] No breaking changes to existing FFI signatures
- [ ] Branch is rebased on `origin/master`

## Code Conventions

- FFI functions: `#[no_mangle] pub unsafe extern "C" fn meshemu_*`
- String params: `*const c_char` (null-terminated C strings)
- Memory ownership: caller frees via `meshemu_*_free()` or `meshemu_storage_data_free()`
- Tests: every new function gets a test. Failure modes are tested too.

## Subsystem Owners

| Subsystem | Primary files | Test package |
|-----------|---------------|-------------|
| Board/Battery/PSRAM/NVS | `core/board/` | `mycelium_board` |
| Display | `core/display/` | `mycelium_display` |
| Input | `core/input/` | `mycelium_input` |
| Radio | `core/radio_bus/` | `radio_bus` |
| Storage | `core/storage/` | `mycelium_storage` |
| GPS | `core/gps/` | `mycelium_gps` |
| Bridge/FFI | `core/bridge/` | `meshemu_bridge` |

## For Autonomous Agents

See [`AGENTS.md`](AGENTS.md) for comprehensive agent instructions including FFI surface reference, test patterns, ForgeDeck spawning conventions, and parity targets.
