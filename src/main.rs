use std::process::ExitCode;

use brainfm::api::ApiError;
use brainfm::{app, cli};
use clap::Parser;

#[tokio::main]
async fn main() -> ExitCode {
    let cli = cli::Cli::parse();
    let result = match cli.command {
        Some(cmd) => cli::run(cmd).await,
        None => app::run().await,
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) if matches!(e.downcast_ref::<ApiError>(), Some(ApiError::Unauthorized)) => {
            eprintln!("Session expired — run `brainfm login`");
            ExitCode::from(2)
        }
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}
