//! OS media controls: MPRIS on Linux, Now Playing on macOS, nothing elsewhere.
//!
//! Every backend exposes the same `Media` type with async `start`,
//! `set_track`, `set_playback`, `set_position` and `set_volume`, and reports
//! what the user pressed as [`MediaKey`]s on a channel.

/// A command from the desktop's media controls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaKey {
    Play,
    Pause,
    Toggle,
    Next,
    /// 0–100
    SetVolume(u8),
}

#[cfg(target_os = "linux")]
mod mpris;
#[cfg(target_os = "linux")]
pub(crate) use mpris::Mpris as Media;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub(crate) use macos::NowPlaying as Media;

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
mod none;
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub(crate) use none::NoMedia as Media;

use crate::api::Serving;
use crate::app::state::Playback;

/// Remembers what was last pushed to [`Media`], so updates go out only when
/// something changed. `Loading` is never pushed: during a skip the previous
/// track keeps playing until the new one starts.
#[derive(Debug, Default)]
pub(crate) struct Mirror {
    track: Option<String>,
    playback: Option<Playback>,
    volume: Option<u8>,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Changes {
    pub(crate) track: bool,
    pub(crate) playback: bool,
    pub(crate) volume: bool,
}

impl Mirror {
    pub(crate) fn diff(&mut self, track: Option<&str>, playback: Playback, volume: u8) -> Changes {
        let mut c = Changes::default();
        if self.track.as_deref() != track {
            self.track = track.map(str::to_owned);
            c.track = true;
        }
        if playback != Playback::Loading && self.playback != Some(playback) {
            self.playback = Some(playback);
            c.playback = true;
        }
        if self.volume != Some(volume) {
            self.volume = Some(volume);
            c.volume = true;
        }
        c
    }

    /// Pushes whatever changed since the last call.
    pub(crate) async fn sync(
        &mut self,
        media: &Media,
        now: Option<&Serving>,
        playback: Playback,
        volume: u8,
    ) {
        let c = self.diff(now.map(|n| n.track.id.as_str()), playback, volume);
        if c.track {
            media.set_track(now).await;
        }
        if c.playback {
            media.set_playback(playback).await;
        }
        if c.volume {
            media.set_volume(volume).await;
        }
    }
}

#[cfg(test)]
mod mirror_tests {
    use super::*;

    #[test]
    fn first_sync_pushes_everything() {
        let mut m = Mirror::default();
        let c = m.diff(Some("a"), Playback::Playing, 70);
        assert_eq!(
            c,
            Changes {
                track: true,
                playback: true,
                volume: true
            }
        );
    }

    #[test]
    fn unchanged_state_pushes_nothing() {
        let mut m = Mirror::default();
        m.diff(Some("a"), Playback::Playing, 70);
        assert_eq!(m.diff(Some("a"), Playback::Playing, 70), Changes::default());
    }

    #[test]
    fn loading_is_never_pushed() {
        let mut m = Mirror::default();
        m.diff(Some("a"), Playback::Playing, 70);
        assert_eq!(m.diff(Some("a"), Playback::Loading, 70), Changes::default());
        let c = m.diff(Some("b"), Playback::Playing, 70);
        assert_eq!(
            c,
            Changes {
                track: true,
                playback: false,
                volume: false
            }
        );
    }
}
