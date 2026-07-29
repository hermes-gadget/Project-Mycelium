# Project Mycelium

**Universal T-Deck + Mesh emulator.** Run MeshCore-compatible firmware on your desktop. No hardware required.

A standalone desktop application that provides emulated LilyGo T-Deck hardware. Any MeshCore-based firmware plugs in by compiling a native target that links against Mycelium's HAL. The emulator provides a virtual radio bus — multiple instances can communicate as if over real LoRa, with simulated propagation, range, and collisions.

## Status

🚧 Pre-alpha — design and scaffolding phase.

## Architecture

```
mycelium/
├── core/                  # Rust engine (radio bus, display, input, storage, GPS, board)
├── firmware-sdk/          # C/C++ headers for firmware authors to integrate
├── gui/                   # Web control panel (map, fleet view, inspector, scenarios)
├── cli/                   # Headless CLI for CI
├── examples/              # Example firmware integrations (SigurdOS, minimal)
└── docs/                  # Design docs, API reference, architecture decisions
```
