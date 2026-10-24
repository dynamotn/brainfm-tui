//! Starts `brainfm daemon` in the background and attaches to it.

use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::config::state_dir;
use crate::share::{self, Attach, attach};

/// How long a TUI waits for the daemon it started to answer.
const SPAWN_WAIT: Duration = Duration::from_secs(5);
const POLL: Duration = Duration::from_millis(100);

/// Starts the daemon detached from this terminal (its own process group,
/// no stdio) and returns a view attached to it. If another TUI started one
/// at the same moment, this attaches to whichever won the socket.
pub(super) async fn start_daemon(path: &Path) -> anyhow::Result<Box<share::Client>> {
    let mut child = Command::new(std::env::current_exe()?)
        .arg("daemon")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()?;
    let deadline = Instant::now() + SPAWN_WAIT;
    loop {
        match attach(path).await? {
            Attach::Session(client) => return Ok(client),
            Attach::Busy(reason) => anyhow::bail!("{reason}"),
            // Bound but still starting up (opening the audio device, …).
            Attach::NoHost | Attach::NotResponding => {}
        }
        // A daemon that lost the race exits successfully; keep polling for
        // the winner. A failed one will never answer.
        let failed = matches!(child.try_wait(), Ok(Some(status)) if !status.success());
        if failed || Instant::now() >= deadline {
            anyhow::bail!(
                "brainfm daemon did not start; see {}/brainfm.log",
                state_dir()?.display()
            );
        }
        tokio::time::sleep(POLL).await;
    }
}
