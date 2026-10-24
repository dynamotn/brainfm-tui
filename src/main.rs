use std::process::ExitCode;

use brainfm::api::is_unauthorized;
use brainfm::{app, cli};
use clap::Parser;

async fn run() -> u8 {
    let cli = cli::Cli::parse();
    let during_login = matches!(cli.command, Some(cli::Command::Login { .. }));
    let result = match cli.command {
        Some(cmd) => cli::run(cmd).await,
        None => app::run().await,
    };
    match result {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("{}", cli::describe_error(&e, during_login));
            if is_unauthorized(&e) { 2 } else { 1 }
        }
    }
}

fn block_on_run() -> u8 {
    match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime.block_on(run()),
        Err(e) => {
            eprintln!("cannot start the async runtime: {e}");
            1
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn main() -> ExitCode {
    ExitCode::from(block_on_run())
}

// macOS delivers Now Playing and media keys through the main thread's event
// loop, so the main thread runs one and tokio lives on another thread.
#[cfg(target_os = "macos")]
fn main() -> ExitCode {
    use objc2_core_foundation::CFRunLoop;

    std::thread::spawn(|| {
        // A panic would unwind only this thread and leave the main thread
        // looping forever, so turn it into the usual 101 exit.
        let code = std::panic::catch_unwind(block_on_run).map_or(101, i32::from);
        std::process::exit(code)
    });

    loop {
        // Returns at once while the loop has no sources; avoid spinning.
        CFRunLoop::run();
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}
