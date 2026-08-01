# Mycelium Web GUI (roadmap)

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

Not yet implemented. The `meshemu serve` CLI command is intentionally a stub
that reports this roadmap status; it does not start an HTTP server. The
headless `meshemu run --headless` mode is available for non-GUI execution, but
it does not host this directory's assets. A future web server can be added as
a new crate or module under `core/` (axum is already a workspace dependency).

Contributions that implement any of the planned views are welcome — see
`../AGENTS.md` for contribution conventions and `../plan.md` for the design
context.
