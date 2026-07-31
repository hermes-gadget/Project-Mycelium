# Minimal C Firmware

The smallest firmware written in plain C that exercises Project Mycelium's
three-function contract and host-services C API:

```text
firmware.so -> meshemu_radio_create / meshemu_board_create -> RadioBus
```

Each `firmware_setup()` call mints a fresh instance id (`minimal-c-0`,
`minimal-c-1`, ...) and creates a virtual radio and board for that node. After
ten loop iterations the firmware beacons `hello from C` once; every 100
iterations it prints the emulated battery voltage and reset reason.

## Build

```sh
make
```

`make` builds the Rust bridge (`libmeshemu_bridge.so`) if needed, then
compiles `firmware.c` into `firmware.so` with the SDK headers on the include
path.

## Run

From this directory:

```sh
meshemu run --firmware ./firmware.so --nodes 2
```

Both virtual nodes come up with a radio on 868 MHz and a board reporting
3900 mV. Node 0's beacon is delivered over the shared RadioBus and both nodes
print periodic board diagnostics.

## Firmware API contract

Every firmware shared library exports three C-linkage functions:

- `firmware_setup()` — called once per virtual node.
- `firmware_loop()` — called once per emulator frame; must stay non-blocking.
- `firmware_get_display()` — returns the firmware's LVGL display handle, or
  `NULL` for headless firmware such as this one.

See `../../firmware-sdk/meshemu.h` for the contract and
`../../firmware-sdk/include/` for the host-services headers. The `minimal-mesh`
example (`../minimal-mesh`) shows the full C++/MeshCore pipeline.
