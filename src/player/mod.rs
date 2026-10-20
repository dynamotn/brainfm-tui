//! Audio playback on a dedicated thread, controlled through channels.

mod engine;

use std::sync::mpsc;
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

#[derive(Debug, Clone)]
pub struct PlayerHandle {
    tx: mpsc::Sender<engine::Msg>,
}

impl PlayerHandle {
    pub fn send(&self, cmd: PlayerCommand) {
        // The engine only goes away when the app is exiting.
        let _ = self.tx.send(engine::Msg::Command(cmd));
    }
}

/// True once per track when it is within [`PREFETCH_LEAD`] of its end.
pub fn near_end(length: Duration, pos: Duration, already: bool) -> bool {
    !already && !length.is_zero() && length.saturating_sub(pos) <= PREFETCH_LEAD
}

/// Opens the default audio device and starts the engine thread.
pub fn spawn(
    rt: tokio::runtime::Handle,
) -> anyhow::Result<(PlayerHandle, UnboundedReceiver<PlayerEvent>)> {
    let (tx, rx) = mpsc::channel();
    let events = engine::start(rt, tx.clone(), rx)?;
    Ok((PlayerHandle { tx }, events))
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
