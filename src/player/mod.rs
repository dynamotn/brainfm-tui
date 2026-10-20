//! Audio playback on a dedicated thread, controlled through channels.

mod engine;

use std::sync::{Arc, mpsc};
use std::time::Duration;

use tokio::sync::mpsc::UnboundedReceiver;

/// How long before a track ends the next one is requested.
pub const PREFETCH_LEAD: Duration = Duration::from_secs(20);

#[derive(Debug, Clone, PartialEq)]
pub enum PlayerCommand {
    /// Replace whatever is playing and fade in.
    Load {
        id: String,
        url: String,
        length: Duration,
    },
    /// Append after the current track for a gapless follow-on.
    Queue {
        id: String,
        url: String,
        length: Duration,
    },
    Play,
    Pause,
    Stop {
        fade: Duration,
    },
    /// Drop any Load or Queue still being opened; leave playback alone.
    CancelPending,
    /// 0.0–1.0
    SetVolume(f32),
    /// A short two-tone chime mixed over the music.
    Chime,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PlayerEvent {
    Started { id: String },
    Position { id: String, pos: Duration },
    NearEnd { id: String },
    Ended { id: String },
    Error { id: String, message: String },
}

/// Stops the engine thread when the last [`PlayerHandle`] clone is dropped.
#[derive(Debug)]
struct ShutdownOnDrop(mpsc::Sender<engine::Msg>);

impl Drop for ShutdownOnDrop {
    fn drop(&mut self) {
        let _ = self.0.send(engine::Msg::Shutdown);
    }
}

#[derive(Debug, Clone)]
pub struct PlayerHandle {
    tx: mpsc::Sender<engine::Msg>,
    _shutdown: Arc<ShutdownOnDrop>,
}

impl PlayerHandle {
    pub fn send(&self, cmd: PlayerCommand) {
        // The engine only goes away when the app is exiting.
        let _ = self.tx.send(engine::Msg::Command(cmd));
    }
}

/// Replaces the query string of every URL with `?…`: signed audio URLs carry
/// short-lived credentials there and must not reach logs or the screen.
pub fn redact_urls(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = ["https://", "http://"]
        .iter()
        .filter_map(|p| rest.find(p))
        .min()
    {
        out.push_str(&rest[..start]);
        let url_len = rest[start..]
            .find(|c: char| c.is_whitespace() || matches!(c, ')' | '"' | '\'' | '>'))
            .unwrap_or(rest.len() - start);
        let url = &rest[start..start + url_len];
        match url.find('?') {
            Some(q) => {
                out.push_str(&url[..=q]);
                out.push('…');
            }
            None => out.push_str(url),
        }
        rest = &rest[start + url_len..];
    }
    out.push_str(rest);
    out
}

/// True once per track when it is within [`PREFETCH_LEAD`] of its end.
pub fn near_end(length: Duration, pos: Duration, already: bool) -> bool {
    !already && !length.is_zero() && length.saturating_sub(pos) <= PREFETCH_LEAD
}

type Spawned = anyhow::Result<(PlayerHandle, UnboundedReceiver<PlayerEvent>)>;

/// Opens the default audio device and starts the engine thread. Audio is only
/// fetched over HTTPS.
pub fn spawn(rt: tokio::runtime::Handle) -> Spawned {
    spawn_with(rt, true)
}

/// Like [`spawn`] but allows plain-HTTP audio, for tests against local servers.
#[doc(hidden)]
pub fn spawn_insecure(rt: tokio::runtime::Handle) -> Spawned {
    spawn_with(rt, false)
}

fn spawn_with(rt: tokio::runtime::Handle, https_only: bool) -> Spawned {
    let (tx, rx) = mpsc::channel();
    let events = engine::start(rt, tx.clone(), rx, https_only)?;
    let handle = PlayerHandle {
        _shutdown: Arc::new(ShutdownOnDrop(tx.clone())),
        tx,
    };
    Ok((handle, events))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn near_end_fires_once_at_20s() {
        let len = Duration::from_secs(100);
        assert!(!near_end(len, Duration::from_secs(79), false));
        assert!(near_end(len, Duration::from_secs(80), false));
        assert!(!near_end(len, Duration::from_secs(80), true));
    }

    #[test]
    fn near_end_ignores_unknown_length() {
        assert!(!near_end(Duration::ZERO, Duration::from_secs(5), false));
    }
}

#[cfg(test)]
mod redact_tests {
    use super::*;

    #[test]
    fn redacts_signed_query_strings() {
        let msg = "HTTP 403 for url (https://cdn.brain.fm/a/b.mp3?Expires=1&Signature=abc&Key-Pair-Id=K) and http://x.y/z?t=1.";
        let out = redact_urls(msg);
        assert!(!out.contains("Signature"), "{out}");
        assert!(!out.contains("t=1"), "{out}");
        assert!(out.contains("https://cdn.brain.fm/a/b.mp3?…"), "{out}");
        assert!(out.contains("http://x.y/z?…"), "{out}");
    }

    #[test]
    fn leaves_plain_text_alone() {
        assert_eq!(redact_urls("timed out"), "timed out");
    }
}
