# Display fidelity contract

Mycelium's default display backend is a **UI-only backend**. LVGL renders
already-corrected logical pixels in a 320×240, top-left-origin framebuffer.
Those pixels are ready for the host window: Mycelium does not apply panel
mounting rotation, MADCTL mirroring, display inversion, reset delays, or
backlight brightness to the default capture.

Both the presentation and capture ABIs use tightly packed RGB565 in
host-native `uint16_t` byte order. On a little-endian host, red `0xf800` is
stored as `00 f8`. ST7789 SPI transfers are high-byte-first (`f8 00`).
`host_rgb565_to_st7789_wire()` and `st7789_wire_to_host_rgb565()` provide the
explicit conversion.

## Partial rendering

LVGL v8 and v9 register Mycelium-owned partial flush callbacks. Each callback
copies only its inclusive flush area into a persistent logical framebuffer.
The visible SDL window also retains one streaming texture and updates only the
changed rectangle. Capture reads the persistent driver-owned framebuffer; it
never reads SDL's renderer after `SDL_RenderPresent()`.

The default LVGL draw buffer is 24 rows (15,360 bytes at 320 pixels and
RGB565), rather than a full 153,600-byte screen. Firmware can select another
value from 1 through 240 with `meshemu_display_create_ex()` and
`meshemu_display_options.draw_buffer_rows`.

## Optional ST7789 fidelity

Set `meshemu_display_options.st7789_fidelity` when creating a display to route
flushes through the optional controller model. It models:

- reset state and the T-Deck initialization values MADCTL `0x55` and INVON
  `0x21`;
- the documented T-Deck X/Y orientation correction;
- CASET, RASET, and RAMWR address-window writes;
- host RGB565 to high-byte-first SPI conversion;
- GPIO42/PWM backlight duty state; and
- exclusive display transactions on a shared SPI arbitration object.

The controller and shared-bus types are public Rust APIs for lower-level
firmware adapters and tests.

## Deliberate hardware gaps

The display transaction path participates in a chip-select ownership model,
but the current SX1262 and SD abstractions operate above raw SPI and do not yet
submit their transfers to the same arbiter. Electrical timing, reset delays,
PWM timing, DMA alignment, SPI clock limits, and bus contention with those
devices are therefore not emulated. Tests that depend on those properties
still require the real T-Deck or a dedicated hardware-in-the-loop backend.
