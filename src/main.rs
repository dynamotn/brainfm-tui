use brainfm::{app, cli};
use clap::Parser;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = cli::Cli::parse();
    match cli.command {
        Some(cmd) => cli::run(cmd).await,
        None => app::run().await,
    }
}
