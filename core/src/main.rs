use std::path::PathBuf;
use std::time::Duration;

use anyhow::{ensure, Context, Result};
use clap::{Parser, Subcommand};
use meshemu_bridge::meshemu_bus_tick;
use mycelium_core::display::{
    DisplayConfig, DisplayEvent, DisplayManager, LvglVersion, Rect, T_DECK_HEIGHT, T_DECK_WIDTH,
};
use mycelium_core::instance::{InstanceConfig, InstanceManager};
use mycelium_core::loader::FirmwareInstance;
use tokio::time::MissedTickBehavior;
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

/// Firmware API version required by this build of Mycelium.
const REQUIRED_FIRMWARE_API_VERSION: u32 = 1;
const CONTEXTFUL_FIRMWARE_API_VERSION: u32 = 2;
const FRAME_INTERVAL_MS: u64 = 16;

#[derive(Debug, Parser)]
#[command(name = "meshemu", version, about = "T-Deck + Mesh emulator")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Run the emulator.
    Run {
        /// Native firmware shared object to load.
        #[arg(long, value_name = "PATH")]
        firmware: PathBuf,
        /// Number of virtual T-Deck nodes to start.
        #[arg(long, default_value_t = 1)]
        nodes: usize,
        /// Skip SDL2 display — useful for headless CI / server runs.
        #[arg(long, default_value_t = false)]
        headless: bool,
        /// Permit a known-incompatible firmware API version for deliberate
        /// compatibility testing. The default is fail-closed.
        #[arg(long, default_value_t = false)]
        allow_incompatible_api: bool,
    },
    /// Serve the emulator API (requires the web GUI feature).
    Serve,
    /// Validate a firmware shared library without starting the emulator.
    Test {
        /// Firmware shared object to validate.
        #[arg(long, value_name = "PATH")]
        firmware: PathBuf,
    },
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let cli = Cli::parse();
    info!(command = ?cli.command, "starting mycelium");

    match cli.command {
        Command::Run {
            firmware,
            nodes,
            headless,
            allow_incompatible_api,
        } => run(firmware, nodes, headless, allow_incompatible_api).await?,
        Command::Serve => {
            println!("serve: the web GUI is planned — see gui/README.md for details");
        }
        Command::Test { firmware } => test_firmware(&firmware)?,
    }

    Ok(())
}

/// Validate that a firmware shared library exports the required symbols and,
/// if present, the optional API version symbol.
fn test_firmware(firmware: &PathBuf) -> Result<()> {
    ensure!(
        firmware.is_file(),
        "firmware library does not exist: {}",
        firmware.display()
    );

    let instance = FirmwareInstance::load("test-probe", firmware)?;
    println!("✓ firmware lifecycle — {}", instance.abi_name());

    let version = verify_firmware_api_version(firmware)?;
    if let Some(v) = version {
        println!("✓ firmware_api_version — v{v}");
    } else {
        println!("⚠ firmware_api_version — not exported (optional, assuming v{REQUIRED_FIRMWARE_API_VERSION})");
    }

    if instance.has_display() {
        println!("✓ firmware_get_display — present (firmware has an LVGL UI)");
    } else {
        println!("ℹ firmware_get_display — absent or NULL (headless firmware)");
    }

    println!("\nAll required symbols present. Firmware looks valid.");
    Ok(())
}

/// Check that the firmware exports a compatible API version symbol.
///
/// Returns `Ok(Some(version))` when the symbol is present, `Ok(None)` when
/// the symbol is absent (treated as compatible for backwards compatibility),
/// or an error when the version is present but incompatible.
fn verify_firmware_api_version(firmware: &PathBuf) -> Result<Option<u32>> {
    // SAFETY: we open the library temporarily just to read the version symbol.
    unsafe {
        let lib = libloading::Library::new(firmware)
            .with_context(|| format!("failed to open firmware: {}", firmware.display()))?;
        let version_fn: libloading::Symbol<unsafe extern "C" fn() -> u32> =
            match lib.get(b"meshemu_firmware_api_version\0") {
                Ok(sym) => sym,
                Err(_) => return Ok(None), // symbol optional for backwards compat
            };
        let version = version_fn();
        if version != REQUIRED_FIRMWARE_API_VERSION && version != CONTEXTFUL_FIRMWARE_API_VERSION {
            anyhow::bail!(
                "firmware API version {version} is incompatible with this Mycelium build (supports v{REQUIRED_FIRMWARE_API_VERSION} legacy and v{CONTEXTFUL_FIRMWARE_API_VERSION} contextful ABI)"
            );
        }
        Ok(Some(version))
    }
}

async fn run(
    firmware: PathBuf,
    nodes: usize,
    headless: bool,
    allow_incompatible_api: bool,
) -> Result<()> {
    ensure!(nodes > 0, "--nodes must be at least 1");
    ensure!(
        firmware.is_file(),
        "firmware library does not exist: {}",
        firmware.display()
    );

    // Fail closed on an explicitly incompatible ABI. Deliberate compatibility
    // testing can opt into the old behavior with an explicit flag.
    if let Err(e) = verify_firmware_api_version(&firmware) {
        if !allow_incompatible_api {
            return Err(e.context(
                "refusing to run incompatible firmware; pass --allow-incompatible-api only for deliberate testing",
            ));
        }
        warn!(%e, "running firmware with an explicitly allowed incompatible API version");
    }

    let mut manager = InstanceManager::new();
    let mut display_manager = None;
    for _ in 0..nodes {
        let id = manager.spawn(&firmware, InstanceConfig::default())?;
        info!(instance_id = %id, firmware = %firmware.display(), "started firmware instance");
        if headless {
            continue;
        }
        let has_display = manager
            .get(&id)
            .and_then(|instance| instance.display())
            .is_some_and(|display| !display.is_null());
        if has_display {
            let displays = match display_manager.as_mut() {
                Some(displays) => displays,
                None => display_manager.insert(DisplayManager::new()?),
            };
            let lvgl_version = manager
                .get(&id)
                .map(|instance| instance.display_version())
                .unwrap_or(LvglVersion::Unknown);
            displays.create_window(
                &id,
                DisplayConfig {
                    lvgl_version,
                    ..DisplayConfig::default()
                },
            )?;
            info!(instance_id = %id, "created firmware display window");
        }
    }

    if headless {
        info!("running headless ({} node(s))", nodes);
    }

    let mut ticker = tokio::time::interval(Duration::from_millis(FRAME_INTERVAL_MS));
    ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let shutdown = tokio::signal::ctrl_c();
    tokio::pin!(shutdown);

    loop {
        tokio::select! {
            signal = &mut shutdown => {
                signal?;
                info!("received SIGINT, shutting down");
                break;
            }
            _ = ticker.tick() => {
                // `meshemu_bus_tick` consumes an absolute monotonic timestamp,
                // not a per-frame delta. Advance the one simulation clock once
                // and feed the same timestamp to every time-based subsystem.
                let sim_now_ms = manager.tick_all_with_delta(FRAME_INTERVAL_MS);
                meshemu_bus_tick(sim_now_ms);
                if let Some(displays) = display_manager.as_mut() {
                    for instance_id in displays.list_windows() {
                        let Some(pixels) = manager
                            .get(&instance_id)
                            .and_then(|instance| instance.capture_display_rgb565())
                        else {
                            continue;
                        };
                        if let Err(error) = displays.present_framebuffer(
                            &instance_id,
                            &pixels,
                            Rect {
                                x: 0,
                                y: 0,
                                width: T_DECK_WIDTH,
                                height: T_DECK_HEIGHT,
                            },
                        ) {
                            warn!(%instance_id, %error, "failed to present firmware framebuffer");
                        }
                    }
                    for event in displays.handle_events() {
                        match event {
                            DisplayEvent::Close { instance_id } => {
                                displays.destroy_window(&instance_id);
                                info!(%instance_id, "closed firmware display window");
                            }
                            DisplayEvent::Resized { instance_id, width, height } => {
                                info!(%instance_id, width, height, "resized firmware display window");
                            }
                        }
                    }
                }
            },
        }
    }

    for instance in manager.list() {
        if let Err(error) = manager.kill(&instance.id) {
            warn!(instance_id = %instance.id, %error, "failed to stop firmware instance");
        }
    }
    info!("all firmware instances stopped");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use mycelium_core::instance::SimulationClock;

    #[test]
    fn executable_frame_clock_accumulates_across_multiple_frames() {
        let mut clock = SimulationClock::default();

        assert_eq!(clock.advance_by(FRAME_INTERVAL_MS), 16);
        assert_eq!(clock.advance_by(FRAME_INTERVAL_MS), 32);
        assert_eq!(clock.advance_by(FRAME_INTERVAL_MS), 48);
        assert_eq!(clock.now_ms(), 48);
    }

    #[test]
    fn executable_frame_loop_feeds_cumulative_time_to_radio_bus() {
        use std::ffi::CString;

        let id = CString::new(format!("frame-clock-test-{}", std::process::id())).unwrap();
        // SAFETY: all arguments are valid for the duration of this call and
        // the returned handle is destroyed exactly once below.
        let radio = unsafe {
            meshemu_bridge::meshemu_radio_create(
                id.as_ptr(),
                915.0,
                125,
                7,
                5,
                14.0,
                51.5074,
                -0.1278,
            )
        };
        assert!(!radio.is_null());

        let packet = [0x42_u8; 16];
        // SAFETY: `radio` is the live handle created above and `packet` is a
        // valid immutable buffer for this call.
        assert!(unsafe {
            meshemu_bridge::meshemu_radio_start_send(radio, packet.as_ptr(), packet.len() as u32)
        });

        let mut clock = SimulationClock::default();
        for _ in 0..3 {
            meshemu_bus_tick(clock.advance_by(FRAME_INTERVAL_MS));
        }
        // The sixteen-byte LoRa frame takes longer than 48 ms. If the old
        // per-frame delta bug regresses, all three calls would also leave the
        // bus at 16 ms and this assertion would stay true forever.
        assert!(!unsafe { meshemu_bridge::meshemu_radio_is_send_complete(radio) });

        meshemu_bus_tick(clock.advance_by(FRAME_INTERVAL_MS));
        // SAFETY: `radio` is still live.
        assert!(unsafe { meshemu_bridge::meshemu_radio_is_send_complete(radio) });
        // SAFETY: `radio` was returned by the matching create function and is
        // not used after this call.
        unsafe { meshemu_bridge::meshemu_radio_destroy(radio) };
    }
}
