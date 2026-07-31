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
        } => run(firmware, nodes, headless).await?,
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
    println!("✓ firmware_setup     — present");
    println!("✓ firmware_loop      — present");

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
        let version_fn: libloading::Symbol<unsafe extern "C" fn() -> u32> = match lib
            .get(b"meshemu_firmware_api_version\0")
        {
            Ok(sym) => sym,
            Err(_) => return Ok(None), // symbol optional for backwards compat
        };
        let version = version_fn();
        if version != REQUIRED_FIRMWARE_API_VERSION {
            anyhow::bail!(
                "firmware API version {version} is incompatible with this Mycelium build (requires v{REQUIRED_FIRMWARE_API_VERSION})"
            );
        }
        Ok(Some(version))
    }
}

async fn run(firmware: PathBuf, nodes: usize, headless: bool) -> Result<()> {
    ensure!(nodes > 0, "--nodes must be at least 1");
    ensure!(
        firmware.is_file(),
        "firmware library does not exist: {}",
        firmware.display()
    );

    // Warn on API version mismatch but don't block (backwards compat).
    if let Err(e) = verify_firmware_api_version(&firmware) {
        warn!(%e, "firmware API version check failed");
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

    let mut ticker = tokio::time::interval(Duration::from_millis(16));
    ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut last_tick = std::time::Instant::now();
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
                let elapsed = last_tick.elapsed().as_millis() as u64;
                last_tick = std::time::Instant::now();
                manager.tick_all_with_delta(16);
                meshemu_bus_tick(elapsed);
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
