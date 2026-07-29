# meshemu.h — Mycelium Public API

#ifdef __cplusplus
extern "C" {
#endif

// Called once at startup. Mycelium provides all hardware abstractions.
// The firmware stores any needed handles and initializes itself.
void firmware_setup(void);

// Called each frame. The firmware processes one main loop iteration.
void firmware_loop(void);

// Optional: return a display handle for LVGL-based firmwares.
// Mycelium uses this to render the UI into the emulator window.
// Return NULL if the firmware has no display.
void* firmware_get_display(void);

#ifdef __cplusplus
}
#endif
