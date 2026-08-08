# Project Mycelium Full Code Audit

Audit date: 2026-08-01  
Scope: all tracked Rust (`53 .rs`) and C API header (`21 .h`) files, plus the C++ bridge adapter, C fixtures/examples, manifests, CI workflow, README claims, and ABI test.  
Method: manual source review, targeted text/symbol reconciliation, and the repository's build/test commands. No security scanner or external scanner was used. No source file was changed.

## 1. Executive Summary

Project Mycelium has a substantial, generally well-tested collection of peripheral models. The strongest parts are RadioBus propagation/collision handling, storage geometry and path validation, GT911 register behavior, RGB565/framebuffer handling, NVS capacity accounting, and defensive null checks at most C boundaries. The workspace builds cleanly: all 205 tests pass, formatting is clean, Clippy passes with warnings denied, and the current C ABI link test passes.

The application is not yet correct as the advertised multi-node T-Deck emulator. Two critical architecture defects dominate the result:

1. The CLI passes a per-frame duration to an API implemented as an absolute clock. After the first frame, RadioBus and GT911 virtual time normally remain around 16 ms, preventing typical LoRa transmissions from completing (`F-01`).
2. Repeated `dlopen` of one firmware path shares the firmware's process globals, while the firmware lifecycle ABI has no instance/context parameter and does not expose the manager-selected ID. The core-created peripherals, display/input routes, partition activation, and firmware-created handles therefore cannot be reliably associated with the same virtual node (`F-02`).

There are 133 exported `meshemu_*` symbols in the built bridge, not the documented 114. Of these, 132 have canonical C declarations and only 129 are called by the ABI link test. The clock header advertises four functions which are implemented in an unbuilt C++ file and are absent from `libmeshemu_bridge.so`. Thus the current passing ABI test is useful but not complete.

Several README completion claims are materially ahead of the code: the web control panel and `serve` command are stubs; GPX movement is internal-only; PSRAM-absent behavior cannot be configured through the runtime or FFI; `meshemu.h` is not an all-subsystems umbrella; and the documented launch/test commands are stale.

Finding totals:

| Severity | Count |
|---|---:|
| Critical | 2 |
| High | 7 |
| Medium | 12 |
| Low | 2 |

Release assessment: peripheral-library quality is promising, but the executable should not be described as a complete multi-instance emulator until `F-01`, `F-02`, lifecycle cleanup, and clock/ABI completeness are fixed and covered by end-to-end tests.

## 2. Architecture Review

### Runtime and instance model

`InstanceManager` owns a firmware wrapper and one Rust-side storage/GPS/board/buzzer/NVS/partition bundle per manager ID (`core/src/instance.rs:82-99`, `core/src/instance.rs:135-148`). The current Tokio runtime is single-threaded (`core/src/main.rs:50`), so iteration over the manager itself is serialized. Global maps are generally protected by `Mutex`, and poisoned locks are deliberately recovered.

The ownership model does not join up at the firmware boundary. `firmware_setup()` and `firmware_loop()` take no context (`core/src/loader.rs:8-10`), and the manager never communicates its generated `nodeN` ID to firmware (`core/src/instance.rs:234-252`). Firmware is expected to invent IDs when creating FFI peripherals. The included minimal C example does exactly that but stores only one static radio and board, overwriting the prior node on the next setup (`examples/minimal-c/firmware.c:31-42`). The C++ example compensates for a shared library handle with a process-global vector and round-robin loop (`examples/minimal-mesh/firmware_entry.cpp:78-102`), but that round robin is not tied to the manager instance whose partition table was just activated.

This also disconnects UI input: visible windows register input routes under the manager ID (`core/display/src/manager.rs:44-59`), while firmware looks up input by the independently chosen FFI ID. There is no API for `firmware_setup()` to discover the manager ID.

### Lifecycle

Loading retains `Library` for the lifetime of copied function pointers, which is correct (`core/src/loader.rs:25-53`). Display capture and destruction are performed while the firmware library is the active symbol scope. However, the lifecycle contract has no shutdown function. `FirmwareInstance::drop` attempts only display destruction (`core/src/loader.rs:121-131`); `Instance::drop` removes only the manager-created buzzer, NVS, and partition registrations (`core/src/instance.rs:213-218`). Firmware-created radio, board, GPS, Wire, keyboard, storage, and input resources are not tracked or reclaimed. Stale radio IDs can make later creation fail, and shared-library display pointers may be destroyed more than once when several wrappers expose the same global display.

### Time model

RadioBus itself uses a sensible absolute, monotonically non-decreasing `now_ms` (`core/bridge/src/ffi.rs:2429-2434`). The executable, however, supplies only the elapsed duration since the immediately preceding frame (`core/src/main.rs:192-195`). Repeated values around 16 never advance an already-16 clock. The README instead describes `meshemu_bus_tick(1)` as advancing time by 1 ms (`README.md:118-120`), so API documentation and implementation disagree as well.

Time is also fragmented across subsystems:

| Subsystem | Time source |
|---|---|
| RadioBus and all GT911 controllers | absolute argument to `meshemu_bus_tick` |
| Manager-owned GPS | fixed 16 ms per CLI frame |
| FFI-created GPS | caller-provided delta |
| Board watchdog | host `Instant` |
| Input activity/auto-off | host `Instant` |
| Deep sleep | directly fast-forwards RadioBus only |

Consequently a pause, scheduler delay, deep sleep, or deterministic replay advances different devices by different amounts. This does not meet the stated atomic monotonic-time design.

### Thread safety and isolation

The major registries use mutexes, but raw opaque handles for board, GPS, and Wire are converted directly to unconstrained Rust references (`core/bridge/src/ffi.rs:74-99`, `core/bridge/src/ffi.rs:156-181`). Concurrent calls on one mutable handle can create aliased `&mut` references and undefined behavior. The API needs an explicit single-thread ownership rule or synchronized handle objects.

Several pieces of state are process-global when they represent one physical board: active partition geometry, last boot phase, last wake cause, keyboard backlight retention, GT911 failure controls/status, and the shared SPI owner. Those globals allow one virtual T-Deck to change another's observations. The global SPI model is additionally wired only into display fidelity; radio and SD never acquire it.

### Architectural positives

- RadioBus serializes mutation and has clear scheduled-transmission state.
- IDs and filesystem roots are encoded to prevent direct `..` traversal.
- NVS writes use a temporary file and rename, providing crash-resistant replacement.
- Display captures own their returned allocation and expose matching free functions.
- The loader keeps libraries alive while function pointers are used.
- Most arithmetic involving sizes, offsets, and timestamps is checked or saturating.

## 3. FFI Audit

The built `target/debug/libmeshemu_bridge.so` contains 133 unique `meshemu_*` dynamic symbols. All return only C-compatible scalars, pointers, or `void`; no Rust enum or struct is returned by value. `meshemu_display_options` is an input `#[repr(C)]`-compatible struct pointer, not a return type.

For the 132 declared exports, the Rust signatures match the canonical bridge headers. Most null inputs return a neutral value and avoid dereference. This cannot make dangling, wrong-type, unterminated-string, or concurrently aliased pointers safe; those remain caller obligations. Owned byte results use `copy_for_caller()` (`core/bridge/src/ffi.rs:139-153`), and `meshemu_storage_data_free` correctly ignores its static empty-read sentinel.

“Match” below means that the Rust/C types match and the normal null/length boundary was reviewed. It does not waive the cross-cutting raw-handle concurrency issue `F-07`. Finding IDs identify a semantic problem beyond the signature.

### Board

| Function | Rust source | C header | ABI test | Result |
|---|---|---|---:|---|
| `meshemu_board_create` | `core/bridge/src/ffi.rs:895` | `core/bridge/include/meshemu_bridge_board.h:28` | yes | Match |
| `meshemu_board_get_battery` | `core/bridge/src/ffi.rs:919` | `core/bridge/include/meshemu_bridge_board.h:31` | yes | Match |
| `meshemu_board_get_adc` | `core/bridge/src/ffi.rs:931` | `core/bridge/include/meshemu_bridge_board.h:32` | yes | Match |
| `meshemu_board_get_temp` | `core/bridge/src/ffi.rs:941` | `core/bridge/include/meshemu_bridge_board.h:34` | yes | Match |
| `meshemu_board_set_mcu_temperature` | `core/bridge/src/ffi.rs:953` | `core/bridge/include/meshemu_bridge_board.h:35` | yes | Match |
| `meshemu_board_get_mcu_temperature` | `core/bridge/src/ffi.rs:965` | `core/bridge/include/meshemu_bridge_board.h:36` | yes | Match |
| `meshemu_board_set_rtc_noinit` | `core/bridge/src/ffi.rs:978` | `core/bridge/include/meshemu_bridge_board.h:37` | yes | Match |
| `meshemu_board_get_rtc_noinit` | `core/bridge/src/ffi.rs:1005` | `core/bridge/include/meshemu_bridge_board.h:39` | yes | Match |
| `meshemu_board_clear_rtc_noinit` | `core/bridge/src/ffi.rs:1031` | `core/bridge/include/meshemu_bridge_board.h:41` | yes | Match |
| `meshemu_board_psram_found` | `core/bridge/src/ffi.rs:1043` | `core/bridge/include/meshemu_bridge_board.h:42` | yes | Match |
| `meshemu_board_get_psram_free` | `core/bridge/src/ffi.rs:1053` | `core/bridge/include/meshemu_bridge_board.h:43` | yes | Match |
| `meshemu_board_psram_readback_test` | `core/bridge/src/ffi.rs:1065` | `core/bridge/include/meshemu_bridge_board.h:44` | yes | Match |
| `meshemu_board_psram_reserve` | `core/bridge/src/ffi.rs:1075` | `core/bridge/include/meshemu_bridge_board.h:45` | yes | Match |
| `meshemu_board_psram_release` | `core/bridge/src/ffi.rs:1085` | `core/bridge/include/meshemu_bridge_board.h:46` | yes | Match |
| `meshemu_board_set_battery` | `core/bridge/src/ffi.rs:1095` | `core/bridge/include/meshemu_bridge_board.h:30` | yes | Match |
| `meshemu_board_set_adc_calibration` | `core/bridge/src/ffi.rs:1107` | `core/bridge/include/meshemu_bridge_board.h:33` | yes | Match |
| `meshemu_board_digital_write` | `core/bridge/src/ffi.rs:1119` | `core/bridge/include/meshemu_bridge_board.h:47` | yes | Match |
| `meshemu_board_set_periph_power` | `core/bridge/src/ffi.rs:1131` | `core/bridge/include/meshemu_bridge_board.h:48` | yes | Match |
| `meshemu_board_ledc_attach` | `core/bridge/src/ffi.rs:1143` | `core/bridge/include/meshemu_bridge_board.h:49` | yes | Match |
| `meshemu_board_ledc_write` | `core/bridge/src/ffi.rs:1155` | `core/bridge/include/meshemu_bridge_board.h:50` | yes | Match |
| `meshemu_board_set_external_power` | `core/bridge/src/ffi.rs:1171` | `core/bridge/include/meshemu_bridge_board.h:52` | yes | Match |
| `meshemu_board_get_charger_state` | `core/bridge/src/ffi.rs:1183` | `core/bridge/include/meshemu_bridge_board.h:53` | yes | Match |
| `meshemu_board_rtc_gpio_hold` | `core/bridge/src/ffi.rs:1195` | `core/bridge/include/meshemu_bridge_board.h:54` | yes | Match |
| `meshemu_board_set_reset_reason` | `core/bridge/src/ffi.rs:1207` | `core/bridge/include/meshemu_bridge_board.h:55` | yes | Match |
| `meshemu_board_get_reset_reason` | `core/bridge/src/ffi.rs:1217` | `core/bridge/include/meshemu_bridge_board.h:56` | yes | Match |
| `meshemu_board_wdt_init` | `core/bridge/src/ffi.rs:1229` | `core/bridge/include/meshemu_bridge_board.h:57` | yes | Match |
| `meshemu_board_wdt_feed` | `core/bridge/src/ffi.rs:1245` | `core/bridge/include/meshemu_bridge_board.h:59` | yes | Match |
| `meshemu_board_wdt_get_status` | `core/bridge/src/ffi.rs:1255` | `core/bridge/include/meshemu_bridge_board.h:60` | yes | Match |
| `meshemu_board_wdt_disable` | `core/bridge/src/ffi.rs:1267` | `core/bridge/include/meshemu_bridge_board.h:61` | yes | Match |
| `meshemu_board_quiesce_peripherals` | `core/bridge/src/ffi.rs:1279` | `core/bridge/include/meshemu_bridge_board.h:62` | yes | Match |
| `meshemu_board_deep_sleep` | `core/bridge/src/ffi.rs:1301` | `core/bridge/include/meshemu_bridge_board.h:63` | yes | Match; F-08 |
| `meshemu_board_get_sleep_wake_cause` | `core/bridge/src/ffi.rs:1341` | `core/bridge/include/meshemu_bridge_board.h:65` | yes | Match; F-10 |
| `meshemu_board_set_boot_phase` | `core/bridge/src/ffi.rs:1347` | `core/bridge/include/meshemu_bridge_board.h:66` | yes | Match; F-10 |
| `meshemu_board_get_last_boot_phase` | `core/bridge/src/ffi.rs:1353` | `core/bridge/include/meshemu_bridge_board.h:67` | yes | Match; F-10 |
| `meshemu_board_destroy` | `core/bridge/src/ffi.rs:1363` | `core/bridge/include/meshemu_bridge_board.h:68` | yes | Match |

### Buzzer

| Function | Rust source | C header | ABI test | Result |
|---|---|---|---:|---|
| `meshemu_buzzer_beep` | `core/board/src/buzzer.rs:277` | `core/bridge/include/meshemu_bridge_buzzer.h:11` | yes | Match; F-15 |
| `meshemu_buzzer_stop` | `core/board/src/buzzer.rs:299` | `core/bridge/include/meshemu_bridge_buzzer.h:13` | yes | Match |
| `meshemu_buzzer_is_playing` | `core/board/src/buzzer.rs:314` | `core/bridge/include/meshemu_bridge_buzzer.h:14` | yes | Match |

### Display

| Function | Rust source | C header | ABI test | Result |
|---|---|---|---:|---|
| `meshemu_display_create_v` | `core/bridge/src/ffi.rs:2023` | `core/bridge/include/meshemu_bridge_display.h:23` | yes | Match |
| `meshemu_display_create_ex` | `core/bridge/src/ffi.rs:2046` | `core/bridge/include/meshemu_bridge_display.h:25` | yes | Match |
| `meshemu_display_create` | `core/bridge/src/ffi.rs:2070` | `core/bridge/include/meshemu_bridge_display.h:22` | yes | Match |
| `meshemu_display_capture` | `core/bridge/src/ffi.rs:2088` | `core/bridge/include/meshemu_bridge_display.h:28` | yes | Match |
| `meshemu_display_capture_free` | `core/bridge/src/ffi.rs:2122` | `core/bridge/include/meshemu_bridge_display.h:29` | yes | Match |
| `meshemu_display_destroy` | `core/bridge/src/ffi.rs:2133` | `core/bridge/include/meshemu_bridge_display.h:30` | yes | Match |

### GPS

| Function | Rust source | C header | ABI test | Result |
|---|---|---|---:|---|
| `meshemu_gps_create` | `core/bridge/src/ffi.rs:768` | `core/bridge/include/meshemu_bridge_gps.h:11` | yes | Match |
| `meshemu_gps_set_position` | `core/bridge/src/ffi.rs:793` | `core/bridge/include/meshemu_bridge_gps.h:12` | yes | Match |
| `meshemu_gps_read` | `core/bridge/src/ffi.rs:811` | `core/bridge/include/meshemu_bridge_gps.h:14` | yes | Match |
| `meshemu_gps_tick` | `core/bridge/src/ffi.rs:828` | `core/bridge/include/meshemu_bridge_gps.h:15` | yes | Match; F-06 |
| `meshemu_gps_set_enabled` | `core/bridge/src/ffi.rs:842` | `core/bridge/include/meshemu_bridge_gps.h:16` | yes | Match |
| `meshemu_gps_set_baud_rate` | `core/bridge/src/ffi.rs:857` | `core/bridge/include/meshemu_bridge_gps.h:17` | yes | Match; F-14 |
| `meshemu_gps_set_time` | `core/bridge/src/ffi.rs:870` | `core/bridge/include/meshemu_bridge_gps.h:18` | yes | Match; F-05 |
| `meshemu_gps_destroy` | `core/bridge/src/ffi.rs:883` | `core/bridge/include/meshemu_bridge_gps.h:19` | yes | Match |

### Input and I2C

| Function | Rust source | C header | ABI test | Result |
|---|---|---|---:|---|
| `meshemu_i2c_keyboard_create` | `core/bridge/src/ffi.rs:1371` | `core/bridge/include/meshemu_bridge_input.h:58` | yes | Match |
| `meshemu_i2c_keyboard_inject_key_byte` | `core/bridge/src/ffi.rs:1383` | `core/bridge/include/meshemu_bridge_input.h:59` | yes | Match |
| `meshemu_i2c_keyboard_set_cross_reset` | `core/bridge/src/ffi.rs:1397` | `core/bridge/include/meshemu_bridge_input.h:60` | yes | Match |
| `meshemu_i2c_keyboard_destroy` | `core/bridge/src/ffi.rs:1416` | `core/bridge/include/meshemu_bridge_input.h:61` | yes | Match |
| `meshemu_wire_shim_create` | `core/bridge/src/ffi.rs:1424` | `core/bridge/include/meshemu_bridge_input.h:63` | yes | Match |
| `meshemu_wire_shim_create_for_instance` | `core/bridge/src/ffi.rs:1434` | `core/bridge/include/meshemu_bridge_input.h:64` | no | Match; ABI gap F-13 |
| `meshemu_wire_shim_set_keyboard` | `core/bridge/src/ffi.rs:1456` | `core/bridge/include/meshemu_bridge_input.h:65` | yes | Match |
| `meshemu_wire_begin` | `core/bridge/src/ffi.rs:1472` | `core/bridge/include/meshemu_bridge_input.h:66` | yes | Match |
| `meshemu_wire_set_clock` | `core/bridge/src/ffi.rs:1484` | `core/bridge/include/meshemu_bridge_input.h:67` | no | Match; ABI gap F-13 |
| `meshemu_wire_probe_address` | `core/bridge/src/ffi.rs:1496` | `core/bridge/include/meshemu_bridge_input.h:68` | yes | Match |
| `meshemu_wire_read_idle_levels` | `core/bridge/src/ffi.rs:1509` | `core/bridge/include/meshemu_bridge_input.h:69` | yes | Match |
| `meshemu_wire_clock_out_recovery` | `core/bridge/src/ffi.rs:1540` | `core/bridge/include/meshemu_bridge_input.h:70` | yes | Match |
| `meshemu_wire_emit_stop` | `core/bridge/src/ffi.rs:1552` | `core/bridge/include/meshemu_bridge_input.h:71` | yes | Match |
| `meshemu_wire_set_sda_stuck` | `core/bridge/src/ffi.rs:1564` | `core/bridge/include/meshemu_bridge_input.h:72` | yes | Match |
| `meshemu_wire_begin_transmission` | `core/bridge/src/ffi.rs:1576` | `core/bridge/include/meshemu_bridge_input.h:73` | yes | Match |
| `meshemu_wire_write` | `core/bridge/src/ffi.rs:1588` | `core/bridge/include/meshemu_bridge_input.h:74` | yes | Match; F-15 |
| `meshemu_wire_end_transmission` | `core/bridge/src/ffi.rs:1600` | `core/bridge/include/meshemu_bridge_input.h:75` | yes | Match |
| `meshemu_wire_request_from` | `core/bridge/src/ffi.rs:1612` | `core/bridge/include/meshemu_bridge_input.h:76` | yes | Match |
| `meshemu_wire_available` | `core/bridge/src/ffi.rs:1628` | `core/bridge/include/meshemu_bridge_input.h:77` | yes | Match |
| `meshemu_wire_read` | `core/bridge/src/ffi.rs:1640` | `core/bridge/include/meshemu_bridge_input.h:78` | yes | Match |
| `meshemu_wire_shim_destroy` | `core/bridge/src/ffi.rs:1650` | `core/bridge/include/meshemu_bridge_input.h:79` | yes | Match |
| `meshemu_input_inject_touch` | `core/bridge/src/ffi.rs:1680` | `core/bridge/include/meshemu_bridge_input.h:12` | yes | Match; F-15 |
| `meshemu_input_inject_key` | `core/bridge/src/ffi.rs:1701` | `core/bridge/include/meshemu_bridge_input.h:17` | yes | Match; F-15 |
| `meshemu_input_poll_touch` | `core/bridge/src/ffi.rs:1726` | `core/bridge/include/meshemu_bridge_input.h:31` | yes | Match |
| `meshemu_input_get_touch_raw` | `core/bridge/src/ffi.rs:1750` | `core/bridge/include/meshemu_bridge_input.h:32` | yes | Match |
| `meshemu_input_get_touch_mapped` | `core/bridge/src/ffi.rs:1769` | `core/bridge/include/meshemu_bridge_input.h:36` | yes | Match |
| `meshemu_input_gt911_set_failure_mode` | `core/bridge/src/ffi.rs:1808` | `core/bridge/include/meshemu_bridge_input.h:46` | yes | Match; F-11 |
| `meshemu_input_gt911_get_status` | `core/bridge/src/ffi.rs:1814` | `core/bridge/include/meshemu_bridge_input.h:47` | yes | Match; F-11 |
| `meshemu_input_gt911_save_calibration` | `core/bridge/src/ffi.rs:1827` | `core/bridge/include/meshemu_bridge_input.h:48` | yes | Match |
| `meshemu_input_gt911_load_calibration` | `core/bridge/src/ffi.rs:1860` | `core/bridge/include/meshemu_bridge_input.h:53` | yes | Match |
| `meshemu_input_poll_key` | `core/bridge/src/ffi.rs:1922` | `core/bridge/include/meshemu_bridge_input.h:56` | yes | Match |
| `meshemu_input_digital_read` | `core/bridge/src/ffi.rs:1942` | `core/bridge/include/meshemu_bridge_input.h:80` | no | Match; ABI gap F-13 |
| `meshemu_input_take_falling_edges` | `core/bridge/src/ffi.rs:1959` | `core/bridge/include/meshemu_bridge_input.h:81` | yes | Match |
| `meshemu_input_set_gpio_intr_enabled` | `core/bridge/src/ffi.rs:1979` | `core/bridge/include/meshemu_bridge_input.h:82` | yes | Match |
| `meshemu_input_get_gpio_intr_enabled` | `core/bridge/src/ffi.rs:1999` | `core/bridge/include/meshemu_bridge_input.h:86` | yes | Match |

### Radio and bus

| Function | Rust source | C header | ABI test | Result |
|---|---|---|---:|---|
| `meshemu_radio_create` | `core/bridge/src/ffi.rs:2146` | `core/bridge/include/meshemu_bridge_radio.h:11` | yes | Match |
| `meshemu_radio_start_send` | `core/bridge/src/ffi.rs:2217` | `core/bridge/include/meshemu_bridge_radio.h:14` | yes | Match |
| `meshemu_radio_recv_raw` | `core/bridge/src/ffi.rs:2268` | `core/bridge/include/meshemu_bridge_radio.h:18` | yes | Match |
| `meshemu_radio_get_est_airtime` | `core/bridge/src/ffi.rs:2311` | `core/bridge/include/meshemu_bridge_radio.h:20` | yes | Match |
| `meshemu_radio_get_rssi` | `core/bridge/src/ffi.rs:2333` | `core/bridge/include/meshemu_bridge_radio.h:21` | yes | Match |
| `meshemu_radio_get_snr` | `core/bridge/src/ffi.rs:2344` | `core/bridge/include/meshemu_bridge_radio.h:22` | yes | Match |
| `meshemu_radio_is_send_complete` | `core/bridge/src/ffi.rs:2355` | `core/bridge/include/meshemu_bridge_radio.h:23` | yes | Match |
| `meshemu_radio_set_position` | `core/bridge/src/ffi.rs:2367` | `core/bridge/include/meshemu_bridge_radio.h:29` | yes | Match |
| `meshemu_radio_set_dio2_config` | `core/bridge/src/ffi.rs:2386` | `core/bridge/include/meshemu_bridge_radio.h:27` | yes | Match |
| `meshemu_radio_get_dio2_config` | `core/bridge/src/ffi.rs:2402` | `core/bridge/include/meshemu_bridge_radio.h:28` | yes | Match |
| `meshemu_radio_destroy` | `core/bridge/src/ffi.rs:2416` | `core/bridge/include/meshemu_bridge_radio.h:30` | yes | Match |
| `meshemu_bus_tick` | `core/bridge/src/ffi.rs:2429` | `core/bridge/include/meshemu_bridge_radio.h:31` | yes | Match; F-01/F-08 |

### Storage

| Function | Rust source | C header | ABI test | Result |
|---|---|---|---:|---|
| `meshemu_spiffs_init` | `core/bridge/src/ffi.rs:227` | `core/bridge/include/meshemu_bridge_storage.h:12` | yes | Match |
| `meshemu_spiffs_read` | `core/bridge/src/ffi.rs:253` | `core/bridge/include/meshemu_bridge_storage.h:13` | yes | Match |
| `meshemu_spiffs_write` | `core/bridge/src/ffi.rs:282` | `core/bridge/include/meshemu_bridge_storage.h:18` | yes | Match |
| `meshemu_sdcard_init` | `core/bridge/src/ffi.rs:315` | `core/bridge/include/meshemu_bridge_storage.h:25` | yes | Match |
| `meshemu_sdcard_set_behavior` | `core/bridge/src/ffi.rs:343` | `core/bridge/include/meshemu_bridge_storage.h:26` | yes | Match |
| `meshemu_sdcard_card_type` | `core/bridge/src/ffi.rs:357` | `core/bridge/include/meshemu_bridge_storage.h:37` | yes | Match |
| `meshemu_sdcard_total_bytes` | `core/bridge/src/ffi.rs:380` | `core/bridge/include/meshemu_bridge_storage.h:38` | yes | Match |
| `meshemu_sdcard_used_bytes` | `core/bridge/src/ffi.rs:392` | `core/bridge/include/meshemu_bridge_storage.h:39` | yes | Match |
| `meshemu_sdcard_mkdir` | `core/bridge/src/ffi.rs:414` | `core/bridge/include/meshemu_bridge_storage.h:40` | yes | Match |
| `meshemu_sdcard_exists` | `core/bridge/src/ffi.rs:435` | `core/bridge/include/meshemu_bridge_storage.h:41` | yes | Match |
| `meshemu_sdcard_open` | `core/bridge/src/ffi.rs:459` | `core/bridge/include/meshemu_bridge_storage.h:42` | yes | Match |
| `meshemu_sdcard_write_file` | `core/bridge/src/ffi.rs:534` | `core/bridge/include/meshemu_bridge_storage.h:47` | yes | Match |
| `meshemu_sdcard_read_file` | `core/bridge/src/ffi.rs:587` | `core/bridge/include/meshemu_bridge_storage.h:52` | yes | Match |
| `meshemu_sdcard_close_file` | `core/bridge/src/ffi.rs:617` | `core/bridge/include/meshemu_bridge_storage.h:57` | yes | Match |
| `meshemu_sdcard_remove` | `core/bridge/src/ffi.rs:627` | `core/bridge/include/meshemu_bridge_storage.h:58` | yes | Match |
| `meshemu_sdcard_end` | `core/bridge/src/ffi.rs:648` | `core/bridge/include/meshemu_bridge_storage.h:59` | yes | Match |
| `meshemu_sdcard_read` | `core/bridge/src/ffi.rs:667` | `core/bridge/include/meshemu_bridge_storage.h:61` | yes | Match |
| `meshemu_sdcard_write` | `core/bridge/src/ffi.rs:698` | `core/bridge/include/meshemu_bridge_storage.h:66` | yes | Match |
| `meshemu_storage_destroy` | `core/bridge/src/ffi.rs:731` | `core/bridge/include/meshemu_bridge_storage.h:73` | yes | Match |
| `meshemu_storage_data_free` | `core/bridge/src/ffi.rs:755` | `core/bridge/include/meshemu_bridge_storage.h:74` | yes | Match |

### NVS and partition

| Function | Rust source | C header | ABI test | Result |
|---|---|---|---:|---|
| `meshemu_nvs_init` | `core/bridge/src/flash_ffi.rs:74` | `core/bridge/include/meshemu_bridge_nvs.h:15` | yes | Match |
| `meshemu_nvs_exists` | `core/bridge/src/flash_ffi.rs:91` | `core/bridge/include/meshemu_bridge_nvs.h:16` | yes | Match |
| `meshemu_nvs_get_bool` | `core/bridge/src/flash_ffi.rs:116` | `core/bridge/include/meshemu_bridge_nvs.h:18` | yes | Match |
| `meshemu_nvs_put_bool` | `core/bridge/src/flash_ffi.rs:146` | `core/bridge/include/meshemu_bridge_nvs.h:20` | yes | Match |
| `meshemu_nvs_get_string` | `core/bridge/src/flash_ffi.rs:177` | `core/bridge/include/meshemu_bridge_nvs.h:28` | yes | Match |
| `meshemu_nvs_put_string` | `core/bridge/src/flash_ffi.rs:232` | `core/bridge/include/meshemu_bridge_nvs.h:32` | yes | Match |
| `meshemu_nvs_remove` | `core/bridge/src/flash_ffi.rs:262` | `core/bridge/include/meshemu_bridge_nvs.h:35` | yes | Match |
| `meshemu_nvs_destroy` | `core/bridge/src/flash_ffi.rs:287` | `core/bridge/include/meshemu_bridge_nvs.h:39` | yes | Match |
| `meshemu_partition_set_launcher_mode` | `core/bridge/src/flash_ffi.rs:304` | `core/bridge/include/meshemu_bridge_partition.h:24` | yes | Match |
| `meshemu_partition_find_first` | `core/bridge/src/flash_ffi.rs:330` | `core/bridge/include/meshemu_bridge_partition.h:25` | yes | Match; F-09 |
| `meshemu_partition_find_first_for_instance` | `core/bridge/src/flash_ffi.rs:353` | `core/bridge/include/meshemu_bridge_partition.h:27` | yes | Match |
| `meshemu_get_otadata_address` | `core/bridge/src/flash_ffi.rs:373` | `core/bridge/include/meshemu_bridge_partition.h:30` | yes | Match; F-09 |
| `meshemu_is_under_launcher` | `core/bridge/src/flash_ffi.rs:383` | `core/bridge/include/meshemu_bridge_partition.h:31` | yes | Match |

### Other export

| Function | Rust source | C header | ABI test | Result |
|---|---|---|---:|---|
| `meshemu_spi_bus_owner` | `core/bridge/src/ffi.rs:1905` | none | no | Undeclared; ABI gap F-12/F-13 |

### Declared but absent from the built bridge

| Function | Declaration | Implementation source | Built symbol |
|---|---|---|---|
| `meshemu_clock_create` | `core/bridge/include/meshemu_bridge_clock.h:23` | `core/bridge/src/meshemu_bridge_clock.cpp:33` | absent |
| `meshemu_clock_millis` | `core/bridge/include/meshemu_bridge_clock.h:24` | `core/bridge/src/meshemu_bridge_clock.cpp:37` | absent |
| `meshemu_clock_set_offset` | `core/bridge/include/meshemu_bridge_clock.h:25` | `core/bridge/src/meshemu_bridge_clock.cpp:42` | absent |
| `meshemu_clock_destroy` | `core/bridge/include/meshemu_bridge_clock.h:26` | `core/bridge/src/meshemu_bridge_clock.cpp:49` | absent |

`core/bridge/Cargo.toml:6-18` has no build script or C++ build dependency, and `nm -D` confirms the four symbols are absent. These functions are also omitted from the SDK forwarding headers and ABI test.

## 4. Subsystem Deep-Dives

### 4.1 Core runtime and loader

Implemented: Clap `run`, `serve`, and `test` commands; firmware symbol loading; API-version probing; instance list/spawn/kill; frame loop; display presentation; SIGINT shutdown.

Correctness: retaining `Library` is sound, and missing required symbols produce contextual errors. Multi-node isolation, manager-to-firmware identity, teardown, clock progression, and strict version enforcement are not correct (`F-01` through `F-03`, `F-22`). `serve` is a print-only stub (`core/src/main.rs:65-67`).

### 4.2 Board and buzzer

Implemented: battery and ADC curve behavior, charger state, peripheral rail, RTC_NOINIT storage, reset reasons, RTC GPIO holds, PSRAM accounting/readback, LEDC-to-buzzer behavior, deep sleep, watchdog, quiesce sequencing, and host audio fallback.

Gaps: watchdog time is host time; sleep advances only radio time; wake cause and boot phase are global; PSRAM absence is constructible only by Rust tests; and timed buzzer calls can create unbounded sleeping threads. The board constructor ignores any configurable PSRAM size and always installs 8 MiB (`core/board/src/lib.rs:184-205`).

### 4.3 Display

Implemented: T-Deck geometry validation, LVGL v8/v9 symbol adapters, partial RGB565 buffers, flush capture, version-aware destruction, ST7789 command/orientation fidelity, SDL windows, resize/close routing, screenshot encoding, and native/wire byte-order conversion.

The v8 ABI layout is necessarily configuration-sensitive and hardcodes a 16-bit canonical driver layout. Both LVGL registries are thread-local (`core/display/src/lvgl_v8.rs:224-226`, `core/display/src/lvgl_v9.rs:100-102`); creation, capture, and destruction must stay on the creating thread. The current runtime does so, but a future web server must preserve that affinity. `--headless` skips the visible manager window only after firmware setup; LVGL v8 setup still tries to initialize a hidden SDL window (`core/display/src/lvgl_v8.rs:147-163`).

### 4.4 Keyboard and I2C

Implemented: C3 address `0x55`, key-mode/brightness commands, retained backlight, a Wire-like transaction state machine, instance-backed GT911 attachment, probe/idle levels, stuck-SDA recovery, clock-out and STOP, and key byte injection.

Gaps: retained backlight is one process-global byte rather than per emulated C3, Wire writes have no hardware-sized buffer cap, and the accepted clock value is stored but does not influence timing. The ABI test misses instance Wire creation and `set_clock`.

### 4.5 Touch and trackball

Implemented: coordinate scaling/rotation, raw/mapped values, GT911 registers and frame semantics, contact sizing, interrupt level, I2C/frame/phantom watchdogs, sticky status, calibration persistence, five trackball GPIOs, falling-edge accounting, and SDL routing.

Gaps: failure configuration and sticky status are global across every controller (`F-11`), event queues are unbounded, and input activity uses host time rather than simulation time. The manager-created input route is keyed by a manager ID that firmware cannot discover (`F-02`).

### 4.6 RadioBus

Implemented: channel compatibility, SX1262 configuration validation, Semtech airtime, free-space/two-ray/log-distance propagation, DIO2 antenna-path penalties, receiver sensitivity, noise/interference power summing, capture threshold, SINR, overlap collisions, busy-until behavior, positions, receive disable during sleep, RSSI/SNR, truncation-with-retry, and deterministic tests.

The underlying bus is one of the strongest components. The executable clock bug prevents it from operating correctly in normal `meshemu run`, however. Propagation configuration and channel activity detection are internal Rust capabilities rather than public runtime/FFI controls. Radio and SD operations do not participate in the advertised shared SPI arbiter.

### 4.7 Storage

Implemented: per-instance SPIFFS and SD roots, flat SPIFFS naming and 32-character limit, partition/capacity accounting, wear counters, SDHC/FAT32 limits, mount frequency ladder, open/read/write/append/close handles, mkdir/exists/remove/end, and power gating.

The lexical traversal checks are good for ordinary paths. Host symlinks inside an instance root are followed by `fs::read`, `fs::write`, and recursive size accounting, so the root is not a containment boundary (`F-21`). Stream writes reread and rewrite the full file on every call (`core/bridge/src/ffi.rs:550-576`), which is correct for current semantics but has poor behavior for large files.

### 4.8 GPS

Implemented: GGA/RMC/GSA/GSV checksummed output, 1 Hz batching, UART byte allowance, static/linear/waypoint/GPX/timestamped movement models, telemetry, power gating, position updates, and enable/disable reset behavior.

The FFI-exposed fixed timestamp is ignored during streamed sentence generation (`F-05`), extreme delta input can exhaust CPU/memory (`F-06`), and baud validation does not match its L76K documentation (`F-14`). GPX and other movement models are not exposed through the FFI, config, or CLI (`F-16`).

### 4.9 NVS and partition

Implemented: namespace/key length limits, bool/string types, read-only behavior, bounded serialized size, atomic replacement, durable migration breadcrumbs, standalone/Launcher geometry, partition lookup, and instance ID encoding.

The per-instance tables are sound, but the compatibility APIs without an ID depend on one process-global active table (`F-09`). NVS is stored under the host temporary directory and does not enforce private file permissions (`F-20`), unlike SPIFFS/SD roots under `~/.mycelium`. The README's “password store” wording therefore needs an explicit security warning or stronger storage permissions.

## 5. Error Handling and Edge Cases

Strengths:

- Null C strings, null output pointers, invalid UTF-8 in most APIs, invalid positions, invalid SX1262 parameters, negative/oversized radio lengths, and invalid handles represented as zero are rejected with neutral return values.
- Receive truncation preserves the queued packet and returns the negative required size.
- Checked additions protect RTC offsets and file positions.
- SPIFFS and SD enforce capacity before writes.
- NVS mutation rolls back if the serialized state no longer fits or persistence fails.
- Poisoned mutexes are recovered consistently.

Weaknesses:

- A non-null opaque pointer cannot be checked for type, liveness, or exclusive access. Error logs call such pointers “dangling” even though dereferencing one to decide that is itself undefined behavior.
- No FFI-wide panic containment exists. A Rust panic reached from an `extern "C"` export or LVGL callback terminates the process.
- GPS delta, Wire transaction length, input event queues, and buzzer timer creation are not bounded.
- Instance cleanup does not include firmware-owned resources.
- `run` logs an incompatible firmware ABI and continues (`core/src/main.rs:137-140`).
- Radio IDs use lossy UTF-8 conversion (`core/bridge/src/ffi.rs:2181-2186`) while other subsystem IDs reject invalid UTF-8, creating inconsistent identity behavior.

## 6. Test Coverage

The workspace currently runs 205 tests, not the README's 189:

| Target | Tests |
|---|---:|
| `meshemu_bridge` | 38 |
| root binary | 0 |
| `mycelium_board` | 28 |
| `mycelium_core` unit | 7 |
| firmware lifecycle integration | 2 |
| `mycelium_display` unit | 23 |
| LVGL v9 integration | 1 |
| `mycelium_gps` | 16 |
| `mycelium_input` | 48 |
| `mycelium_storage` | 16 |
| `radio_bus` | 26 |
| **Total** | **205** |

Coverage quality is strong at subsystem unit level, particularly for radio collisions, propagation, GT911 watchdogs, storage constraints, NVS restart behavior, display partial flushes, and board failure states.

Material missing tests:

- No executable-loop test proves that virtual time advances over multiple frames; this would catch `F-01`.
- No two-node same-library test verifies independent firmware globals, IDs, peripherals, displays, input routes, partition activation, and teardown.
- The lifecycle fixture tests only setup/loop counters and one manager-owned NVS restart; it creates no firmware-owned FFI resource.
- `meshemu_gps_set_time` is called but the resulting NMEA timestamp is never asserted (`core/bridge/src/lib.rs:1242-1247`).
- No extreme GPS delta/resource-bound test exists.
- The baud test checks only 9600/38400 and two outside-range values, so it does not detect acceptance of 4800, 19200, or 115200 (`core/bridge/src/lib.rs:736-749`).
- No same-handle concurrent FFI test or documented single-thread rule exists.
- No test checks per-instance boot phase, wake cause, C3 retained brightness, or GT911 failure injection.
- No test asserts that the dynamic symbol set equals the canonical header set and ABI-call set.

The ABI link test calls 129 of 133 actual exports. It omits:

- `meshemu_wire_shim_create_for_instance`
- `meshemu_wire_set_clock`
- `meshemu_input_digital_read`
- `meshemu_spi_bus_owner` (also has no declaration)

It also cannot catch the four missing clock implementations because it never includes or calls the clock header. The test validates compilation/linkage and runs a smoke path, but most calls have no behavioral assertions; it does not substantiate the README claim that every FFI function and failure mode is covered.

## 7. Code Quality

Positive observations:

- Crate boundaries are understandable and responsibilities are mostly cohesive.
- Public APIs and most unsafe functions have useful documentation.
- `SAFETY` comments are present around SDL ownership, dynamic symbol calls, framebuffer slices, and loader calls.
- Checked and saturating arithmetic is used widely.
- Forwarding SDK headers avoid maintaining two independent C prototype copies.
- Tests are colocated with behavior and cover many failure cases.

Issues:

- The public API and documentation inventory is not generated from a single source, causing the 114/133 count and ABI drift.
- Time concepts mix absolute timestamps, deltas, and host `Instant` without type separation.
- Context-free firmware entrypoints force global state and example-specific workarounds.
- Mutable raw handles manufacture arbitrary-lifetime `&mut` references, so safety depends on undocumented exclusivity.
- Process-global state is used for several per-board properties.
- Shared SPI fidelity is incomplete: the type lists Display/SX1262/SD, but production callers exist only for Display.
- GPS documentation contradicts its accepted baud range.
- The code contains capabilities with no integration path (movement models, web dependency, scenario design, PSRAM absence).

## 8. Build and CI Results

All required commands passed in the audited worktree on 2026-08-01:

| Command | Result |
|---|---|
| `cargo test --workspace` | PASS — 205 tests, 0 failed, 0 ignored |
| `cargo fmt --check` | PASS |
| `cargo clippy --workspace -- -D warnings` | PASS |
| `cargo clippy` | PASS |
| `cd firmware-sdk/tests && ./check_abi.sh` | PASS |
| `cargo run --quiet -- --help` | PASS; confirms required `run` subcommand |

CI runs formatting, Clippy with denied warnings, workspace build, ABI link, and workspace tests (`.github/workflows/ci.yml:28-41`). That is a good baseline. Recommended CI additions are a dynamic-symbol/header/ABI inventory check, executable clock integration test, and multi-node same-library lifecycle test.

The documented commands are stale: the README omits the `run` subcommand (`README.md:18-20`) and names a nonexistent `make test-abi` target (`README.md:151-152`); the root Makefile provides `test-sdk-headers` instead (`Makefile:9-10`).

## 9. Security — Manual Review Only

No automated security scanner was run. Manual tracked-source searches found no hardcoded private keys, AWS-style access keys, API keys, access tokens, client secrets, or literal password assignments.

Security-relevant observations:

- Firmware `.so` files are native code loaded with `libloading` (`core/src/loader.rs:25-33`). They execute constructors and then run in the emulator process with the user's full privileges. This is an intentional architecture boundary, not a sandbox; documentation should explicitly say never to load untrusted firmware libraries.
- Raw handles are memory-unsafe if forged, reused after destruction, destroyed twice, or called concurrently (`F-07`). Null checking cannot mitigate these cases.
- NVS JSON may contain the README-advertised password store, but persistence uses ordinary `create_dir_all`/`fs::write` without forcing `0700` directories or `0600` files (`core/board/src/nvs.rs:300-315`). Under a common `022` umask, the file may be readable by other local users (`F-20`).
- Storage paths reject lexical traversal, but host symlinks under the backing root are followed (`F-21`). This becomes security-sensitive if future web/scenario APIs expose storage operations to a less-trusted client.
- There is no process isolation, privilege drop, resource quota, or crash containment for firmware. GPS and other unbounded inputs make denial of service easier even for otherwise valid callers.
- Incompatible firmware API versions are allowed to execute after a warning, which can turn a known ABI mismatch into memory corruption or process termination (`F-22`).

## 10. Findings Summary

| ID | Severity | Subsystem | File:line | Description | Recommended fix |
|---|---|---|---|---|---|
| F-01 | Critical | Runtime / Radio / Input | `core/src/main.rs:192-195`; `core/bridge/src/ffi.rs:2429-2434` | CLI supplies a frame delta to an absolute monotonic clock. Repeated ~16 values stop RadioBus and GT911 time after the first frame; ordinary transmissions never complete. | Define one clock contract. Prefer a cumulative `sim_now_ms`, pass it everywhere, rename the API to make absolute semantics explicit, and add a multi-frame executable test. |
| F-02 | Critical | Runtime / Instances | `core/src/loader.rs:8-23`; `core/src/instance.rs:234-252`; `examples/minimal-c/firmware.c:31-42`; `examples/minimal-mesh/firmware_entry.cpp:78-102` | Same-path library loads share firmware globals, lifecycle calls have no context, and firmware cannot discover the manager ID. Nodes, peripherals, displays, inputs, and loops are not reliably isolated or associated. | Introduce a contextful v2 firmware ABI (`create(id) -> ctx`, `loop(ctx)`, `display(ctx)`, `destroy(ctx)`), or load truly isolated namespaces/copies. Pass the canonical manager ID explicitly. |
| F-03 | High | Runtime / Lifecycle | `core/src/loader.rs:121-131`; `core/src/instance.rs:213-218` | Killing an instance does not reclaim firmware-created radio/board/GPS/Wire/keyboard/storage/input resources; no shutdown hook exists. Stale registrations and double/shared display destruction are possible. | Add `firmware_destroy(ctx)` and an instance resource registry; make teardown idempotent and test kill/restart with every handle type. |
| F-04 | High | Clock FFI / Build | `core/bridge/include/meshemu_bridge_clock.h:23-26`; `core/bridge/src/meshemu_bridge_clock.cpp:33-50`; `core/bridge/Cargo.toml:6-18` | Four advertised clock functions are absent from the built bridge because the C++ file is not compiled or linked. The minimal Mesh example depends on this header. | Compile/link the C++ adapter with a build script and test it, or replace it with Rust exports/remove the unsupported API. |
| F-05 | High | GPS | `core/gps/src/manager.rs:115-120`; `core/gps/src/manager.rs:300-305`; `core/bridge/src/ffi.rs:861-874` | `meshemu_gps_set_time` stores a fixed time, but streamed GGA/RMC generation always uses `Utc::now()`. The API is behaviorally ineffective. | Generate epochs through `GpsState`'s selected time or expose a `current_time()` accessor; assert exact NMEA time/date after `set_time`. |
| F-06 | High | GPS / Resource handling | `core/gps/src/manager.rs:261-273`; `core/bridge/src/ffi.rs:828-833` | Arbitrary `u64` delta causes one full sentence batch per elapsed second. A very large FFI tick performs an effectively unbounded loop and grows the queue until CPU/memory exhaustion. | Cap catch-up epochs and queue size, coalesce skipped epochs, or emit only the latest epoch; test `u64::MAX`. |
| F-07 | High | FFI / Memory safety | `core/bridge/src/ffi.rs:74-99`; `core/bridge/src/ffi.rs:156-181` | Opaque pointers are converted to arbitrary-lifetime shared/mutable references. Concurrent calls on one mutable GPS/board/Wire handle can create aliased `&mut` and undefined behavior; stale/wrong-type handles also dereference unchecked memory. | Store handles in typed registries behind mutexes and use integer/generation IDs, or document and enforce strict thread ownership with synchronized wrapper objects. |
| F-08 | High | Architecture / Time | `core/src/main.rs:192-195`; `core/board/src/lib.rs:345-386`; `core/input/src/manager.rs:206-213`; `core/bridge/src/ffi.rs:1321-1330` | Even after F-01, GPS uses a fixed frame delta, watchdog/input use host time, and deep sleep advances only RadioBus. Simulation time is not atomic or deterministic across subsystems. | Add a central `SimulationClock`; tick radio, GPS, GT911, watchdog, input, and sleep from the same cumulative value/delta in one operation. |
| F-09 | High | Partition / Instances | `core/board/src/partition.rs:22-25`; `core/board/src/partition.rs:221-241`; `core/src/instance.rs:151-158` | ID-less partition APIs read one process-global active table. It is unsafe under concurrency and can select the wrong node when a shared firmware library round-robins its own contexts independently of manager iteration. | Make active selection thread-/context-local, prefer the instance-explicit API, and bind partition lookup to the v2 firmware context. |
| F-10 | Medium | Board / Keyboard | `core/bridge/src/ffi.rs:20-35`; `core/board/src/lib.rs:50`; `core/input/src/i2c_keyboard.rs:8-35` | Last wake cause, boot phase, and retained C3 backlight are process-global although each virtual T-Deck has its own hardware state. | Key each value by canonical instance ID and include the ID/context in compatibility APIs. |
| F-11 | Medium | Touch | `core/input/src/gt911.rs:37-44`; `core/input/src/gt911.rs:463-492`; `core/bridge/src/ffi.rs:1806-1815` | GT911 failure injection applies to all current/future controllers and status ORs every node, preventing per-node scenarios and attribution. | Add instance-specific set/get APIs; optionally retain an explicitly named global broadcast control. |
| F-12 | Medium | SPI / Completeness | `core/display/src/shared_spi.rs:24-33`; `core/display/src/lvgl_v8.rs:406-415`; `core/display/src/lvgl_v9.rs:260-273` | One SPI owner is shared across all emulated boards, while production acquisition is wired only to display—not SD or SX1262. This both couples nodes and fails to model on-board contention. | Create one arbiter per instance and route display, SD, and radio transactions through it. |
| F-13 | Medium | FFI / ABI / Docs | `core/bridge/src/ffi.rs:1434`; `core/bridge/src/ffi.rs:1484`; `core/bridge/src/ffi.rs:1905`; `core/bridge/src/ffi.rs:1942`; `firmware-sdk/tests/abi_link.c:80-115`; `README.md:63-86` | Built bridge has 133 exports, not 114. One export has no header; four exports are absent from the ABI test. Passing ABI therefore does not prove advertised completeness. | Generate symbol/header/ABI inventories in CI, declare or remove `meshemu_spi_bus_owner`, add all four calls, and update counts/docs. |
| F-14 | Medium | GPS | `core/gps/src/manager.rs:12-19`; `core/gps/src/manager.rs:98-112`; `core/bridge/src/lib.rs:736-749` | Code/comments/test name say L76K accepts 9600 and 38400, but implementation accepts every rate from 4800 through 115200. Tests miss in-range unsupported values. | Accept exactly supported rates or correct the contract and emulate arbitrary UART rates intentionally; add table-driven tests. |
| F-15 | Medium | Resource handling | `core/input/src/wire_shim.rs:128-138`; `core/input/src/manager.rs:37-40`; `core/board/src/buzzer.rs:199-212` | Wire TX and input event queues are unbounded; every timed beep spawns a sleeper thread. Repeated FFI input can exhaust memory/threads. | Model finite hardware FIFO sizes with defined overflow semantics and use one timer/audio worker rather than a thread per beep. |
| F-16 | Medium | GPS / Completeness | `core/gps/src/manager.rs:41-63`; `core/gps/src/manager.rs:127-167`; `core/bridge/include/meshemu_bridge_gps.h:11-19`; `README.md:71-96` | Linear/waypoint/GPX replay exists only as a Rust API. No CLI/config/FFI path can activate the README-advertised GPX feature. | Add validated configuration/FFI endpoints and GPX parsing/wiring, or mark the capability internal/planned. |
| F-17 | Medium | Board / Completeness | `core/bridge/src/ffi.rs:895-912`; `core/board/src/lib.rs:184-205`; `README.md:97-102` | README claims configurable PSRAM-absent failure behavior, but runtime and FFI constructors always create 8 MiB. Only Rust tests can mutate the public field to zero. | Add board configuration/FFI for PSRAM size/presence and an end-to-end failure-mode test. |
| F-18 | Medium | GUI / CLI / Completeness | `README.md:24-25`; `README.md:56-80`; `core/src/main.rs:40-41`; `core/src/main.rs:65-67`; `gui/README.md:18-23` | README presents a working web panel, map/fleet/inspector/scenarios, and headless CLI, but `serve` is a stub and GUI contains only a planning README. | Implement the server/UI or label all of these as planned and remove working quick-start claims. |
| F-19 | Medium | SDK / Documentation | `README.md:18-20`; `README.md:74-77`; `README.md:108-109`; `README.md:151-152`; `firmware-sdk/meshemu.h:44-54`; `Makefile:9-10` | Quick start omits `run`; `meshemu.h` is not an all-subsystem master header; `make test-abi` does not exist. The published C sample cannot compile with its stated single include. | Correct commands and wording, or turn `meshemu.h` into a true umbrella; test README snippets in CI. |
| F-20 | Medium | NVS / Security | `core/board/src/nvs.rs:300-315`; `core/board/src/nvs.rs:352-357`; `README.md:98` | Persistent JSON, including advertised password data, is written under the temp directory without enforced private permissions. Common umasks may produce locally readable files, and temp cleanup may erase persistence. | Store under a private per-user data directory; create directories/files with `0700`/`0600`, use `create_new` for temp files, and document plaintext-at-rest behavior. |
| F-21 | Low | Storage / Security | `core/storage/src/spiffs.rs:278-290`; `core/storage/src/sdcard.rs:195-218`; `core/storage/src/sdcard.rs:316-327` | Path containment is lexical; symlinks inside a backing root can redirect reads/writes or recursive accounting outside the instance directory. | Reject symlink components or use descriptor-relative/no-follow filesystem operations where storage becomes a trust boundary. |
| F-22 | Medium | Loader / Security | `core/src/main.rs:104-125`; `core/src/main.rs:137-140` | `run` continues after detecting an explicitly incompatible firmware API version, exposing the process to ABI mismatch crashes/corruption. | Fail closed by default; add an explicit `--allow-incompatible-api` escape hatch for deliberate testing. |
| F-23 | Low | FFI / Reliability | `core/bridge/src/ffi.rs:74-181`; `core/display/src/lvgl_v8.rs:228-252`; `core/display/src/lvgl_v9.rs:104-122` | Exports/callbacks have no panic boundary. A panic from allocation, `RefCell` reentrancy, or an internal invariant aborts the whole emulator at a C ABI boundary. | Keep callbacks non-panicking, replace `expect`/borrow panics in boundary paths, and use explicit `catch_unwind` wrappers where recovery is sound. |

## 11. Recommendations

Priority 0 — restore core correctness:

1. Fix `F-01` and add a real executable-loop test that sends a packet, advances several frames, receives it, and observes GT911 watchdog time.
2. Design a contextful firmware ABI that passes the canonical instance ID and supports deterministic teardown. Treat this as the prerequisite for credible `--nodes > 1` support.
3. Put all emulated time behind one `SimulationClock`, with explicit absolute/delta types and one atomic tick path.

Priority 1 — make the FFI contract trustworthy:

1. Resolve or remove the clock adapter.
2. Generate and compare the dynamic symbol list, canonical header declarations, SDK forwarding coverage, and ABI-test calls in CI.
3. Replace raw mutable pointers with synchronized registry handles or formally enforce single-thread ownership.
4. Add resource limits and panic-safe boundary behavior.

Priority 2 — restore instance isolation:

1. Key boot phase, wake cause, C3 retention, GT911 controls, and SPI buses per canonical node.
2. Bind partition lookup, display, input, GPS, storage, and firmware-created resources to the same instance context.
3. Test two nodes from the same firmware path through setup, loop, UI input, radio exchange, deep sleep, kill, and restart.

Priority 3 — align claims and completeness:

1. Wire GPX/movement and PSRAM failure configuration into public runtime controls, or mark them internal/planned.
2. Either implement `serve`/GUI/scenarios or make the README explicitly roadmap-oriented.
3. Correct the CLI, Make, header, function-count, subsystem-count, and test-count documentation.
4. Document that firmware libraries are fully trusted native code and secure NVS persistence permissions.

The existing subsystem tests and clean build provide a good base for these changes. The next milestone should be defined by an end-to-end invariant: two independently identified firmware contexts advance from one simulation clock, interact through RadioBus, receive their own input/peripheral state, and leave no live resources after teardown.
