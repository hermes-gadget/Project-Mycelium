use clap::{Parser, Subcommand};
use tracing::info;
use tracing_subscriber::EnvFilter;

#[derive(Debug, Parser)]
#[command(name = "mycelium", version, about = "T-Deck + Mesh emulator")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Run the emulator.
    Run,
    /// Serve the emulator API.
    Serve,
    /// Run emulator diagnostics.
    Test,
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let cli = Cli::parse();
    info!(command = ?cli.command, "starting mycelium");

    match cli.command {
        Command::Run => println!("run: not yet implemented"),
        Command::Serve => println!("serve: not yet implemented"),
        Command::Test => println!("test: not yet implemented"),
    }
}
