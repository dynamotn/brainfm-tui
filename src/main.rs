use std::process::ExitCode;

use brainfm::api::is_unauthorized;
use brainfm::{app, cli};
use clap::Parser;

#[tokio::main]
async fn main() -> ExitCode {
    let cli = cli::Cli::parse();
    let during_login = matches!(cli.command, Some(cli::Command::Login { .. }));
    let result = match cli.command {
        Some(cmd) => cli::run(cmd).await,
        None => app::run().await,
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{}", cli::describe_error(&e, during_login));
            if is_unauthorized(&e) {
                ExitCode::from(2)
            } else {
                ExitCode::FAILURE
            }
        }
    }
}
