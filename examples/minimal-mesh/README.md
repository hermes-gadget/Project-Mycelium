# Minimal Mesh Firmware

This headless example is the smallest firmware that exercises Mycelium's
end-to-end mesh path:

```text
firmware.so -> MeshemuRadio -> RadioBus -> MeshemuRadio -> firmware.so
```

Each virtual node creates a MeshCore dispatcher and queues one
`hello mesh` flood packet. Other nodes print the payload when their dispatcher
receives it.

## Build

From this directory:

```sh
git submodule update --init ../../lib/meshcore
make
```

The submodule command is only needed once per checkout. The build produces
`firmware.so`. The equivalent CMake build is:

```sh
cmake -S . -B build
cmake --build build
```

## Run

Run two virtual nodes from this directory:

```sh
meshemu run --firmware ./firmware.so --nodes 2
```

Both nodes initialize a virtual LoRa radio on the same channel, send a flood
packet, and print received `hello mesh` packets. The firmware is headless, so
the emulator does not create display windows.

## Firmware API contract

Every firmware shared library exposes three C-linkage functions:

- `firmware_setup()` is called once to initialize the firmware and virtual
  hardware.
- `firmware_loop()` is called repeatedly and must remain non-blocking.
- `firmware_get_display()` returns the firmware's display handle, or `nullptr`
  for headless firmware such as this example.

The emulator loads these symbols from the `.so`; calls made by `MeshemuRadio`
then cross the C++/Rust bridge into the shared `RadioBus`.
