# Mycelium Web GUI (planned)

This directory will hold the **web control panel** for Project Mycelium: a
browser-based view into the virtual radio bus that lets you inspect and drive
emulated T-Deck nodes without touching the CLI.

## Planned features

- **Map view** — live node positions and packet paths over the RadioBus
  propagation model, with RSSI/SINR readouts per link.
- **Fleet** — one panel per running emulator instance: board state (battery,
  temperature, PSRAM), GPS fix, SD/SPIFFS usage, NVS keys.
- **Inspector** — per-node radio state (channel, spreading factor, TX power,
  DIO2 config) and queued RX/TX activity.
- **Scenarios** — scripted interference, node movement, deep-sleep, and
  packet-loss scenarios driven from the browser.

## Status

Not yet implemented. The `meshemu serve` CLI command that would host this
panel is currently a stub, and `gui/` exists in the repository layout so the
planned component has a home. The web server will be added as a new crate or
module under `core/` (axum is already a workspace dependency), serving this
directory's static assets.

Contributions that implement any of the planned views are welcome — see
`../AGENTS.md` for contribution conventions and `../plan.md` for the design
context.
