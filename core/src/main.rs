use std::path::PathBuf;
use std::time::Duration;

use anyhow::{ensure, Result};
use clap::{Parser, Subcommand};
use mycelium_core::instance::{InstanceConfig, InstanceManager};
use tokio::time::MissedTickBehavior;
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

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
    },
    /// Serve the emulator API.
    Serve,
    /// Run emulator diagnostics.
    Test,
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let cli = Cli::parse();
    info!(command = ?cli.command, "starting mycelium");

    match cli.command {
        Command::Run { firmware, nodes } => run(firmware, nodes).await?,
        Command::Serve => println!("serve: not yet implemented"),
        Command::Test => println!("test: not yet implemented"),
    }

    Ok(())
}

async fn run(firmware: PathBuf, nodes: usize) -> Result<()> {
    ensure!(nodes > 0, "--nodes must be at least 1");
    ensure!(
        firmware.is_file(),
        "firmware library does not exist: {}",
        firmware.display()
    );

    let mut manager = InstanceManager::new();
    for _ in 0..nodes {
        let id = manager.spawn(&firmware, InstanceConfig::default())?;
        info!(instance_id = %id, firmware = %firmware.display(), "started firmware instance");
    }

    let mut ticker = tokio::time::interval(Duration::from_millis(1));
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
            _ = ticker.tick() => manager.tick_all(),
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
