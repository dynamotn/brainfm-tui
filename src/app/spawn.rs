//! Starts `brainfm daemon` in the background and attaches to it.

use std::io::{Read, Seek, SeekFrom};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use crate::config::state_dir;
use crate::share::{self, Attach, attach};

/// How long a TUI waits for the daemon it started (or one still starting
/// up) to answer.
const SPAWN_WAIT: Duration = Duration::from_secs(5);
const POLL: Duration = Duration::from_millis(100);

/// Starts the daemon detached from this terminal (its own process group,
/// no stdio, `/` as its directory) and returns a view attached to it. If
/// another TUI started one at the same moment, this attaches to whichever
/// won the socket.
pub(super) async fn start_daemon(path: &Path) -> anyhow::Result<Box<share::Client>> {
    let log = state_dir()?.join("brainfm.log");
    // Only errors this daemon logs are reported, not older ones.
    let log_start = std::fs::metadata(&log).map_or(0, |m| m.len());
    let mut child = Command::new(std::env::current_exe()?)
        .arg("daemon")
        .current_dir("/")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()?;
    if let Polled::Attached(client) = poll(path, Some(&mut child)).await? {
        return Ok(client);
    }
    match read_from(&log, log_start).as_deref().and_then(last_error) {
        Some(reason) => anyhow::bail!(
            "brainfm daemon did not start: {reason} (see {})",
            log.display()
        ),
        None => anyhow::bail!("brainfm daemon did not start; see {}", log.display()),
    }
}

/// Waits up to [`SPAWN_WAIT`] for a host that holds the socket but has not
/// answered yet; starts the daemon if the socket goes away meanwhile.
pub(super) async fn wait_for_host(path: &Path) -> anyhow::Result<Box<share::Client>> {
    match poll(path, None).await? {
        Polled::Attached(client) => Ok(client),
        Polled::NoHost => start_daemon(path).await,
        Polled::GaveUp => anyhow::bail!("{}", share::NOT_RESPONDING),
    }
}

enum Polled {
    Attached(Box<share::Client>),
    /// Nobody holds the socket (only reported when there is no `child`).
    NoHost,
    /// [`SPAWN_WAIT`] ran out, or `child` failed.
    GaveUp,
}

/// Attaches to `path` until a host answers, [`SPAWN_WAIT`] runs out or the
/// `child` daemon fails.
async fn poll(path: &Path, mut child: Option<&mut Child>) -> anyhow::Result<Polled> {
    let deadline = Instant::now() + SPAWN_WAIT;
    loop {
        match attach(path).await? {
            Attach::Session(client) => return Ok(Polled::Attached(client)),
            Attach::Busy(reason) => anyhow::bail!("{reason}"),
            Attach::NoHost if child.is_none() => return Ok(Polled::NoHost),
            // Bound but still starting up (opening the audio device, …).
            Attach::NoHost | Attach::NotResponding => {}
        }
        // A daemon that lost the race exits successfully; keep polling for
        // the winner. A failed one will never answer.
        let failed = child
            .as_deref_mut()
            .is_some_and(|c| matches!(c.try_wait(), Ok(Some(status)) if !status.success()));
        if failed || Instant::now() >= deadline {
            return Ok(Polled::GaveUp);
        }
        tokio::time::sleep(POLL).await;
    }
}

/// The log from byte `start` on (from the beginning if it shrank), or
/// `None` if it cannot be read.
fn read_from(log: &Path, start: u64) -> Option<String> {
    let mut file = std::fs::File::open(log).ok()?;
    let len = file.metadata().ok()?.len();
    file.seek(SeekFrom::Start(if start > len { 0 } else { start }))
        .ok()?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).ok()?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

/// The message of the last `ERROR` line in a `tracing` log, without its
/// timestamp, level and target.
fn last_error(log: &str) -> Option<&str> {
    log.lines().rev().find_map(|l| {
        let (_time, rest) = l.split_once(' ')?;
        let rest = rest.trim_start().strip_prefix("ERROR ")?.trim_start();
        let message = match rest.split_once(": ") {
            Some((target, message)) if !target.contains(' ') => message,
            _ => rest,
        };
        let message = message.trim();
        (!message.is_empty()).then_some(message)
    })
}

#[cfg(test)]
mod tests {
    use super::last_error;

    #[test]
    fn last_error_takes_the_newest_error_message() {
        let log = "\
2026-10-24T09:00:00.000001Z  INFO brainfm::app: starting
2026-10-24T09:00:00.000002Z ERROR brainfm::app::daemon: no audio output device
2026-10-24T09:00:01.000003Z  WARN brainfm::player: retrying
2026-10-24T09:00:02.000004Z ERROR brainfm::app::daemon: not logged in — run `brainfm login`
2026-10-24T09:00:03.000005Z  INFO brainfm::app: bye
";
        assert_eq!(last_error(log), Some("not logged in — run `brainfm login`"));
    }

    #[test]
    fn last_error_is_none_without_an_error_line() {
        assert_eq!(last_error(""), None);
        assert_eq!(
            last_error("2026-10-24T09:00:00Z  WARN brainfm: ERROR in the message\n"),
            None
        );
        assert_eq!(last_error("garbage\n"), None);
    }

    #[test]
    fn last_error_keeps_a_message_without_a_target() {
        assert_eq!(
            last_error("2026-10-24T09:00:00Z ERROR unknown audio host \"x\"\n"),
            Some("unknown audio host \"x\"")
        );
    }
}
