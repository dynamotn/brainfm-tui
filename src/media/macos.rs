//! macOS Now Playing and media keys, via souvlaki.

use std::sync::Mutex;
use std::time::Duration;

use souvlaki::{
    MediaControlEvent, MediaControls, MediaMetadata, MediaPlayback, MediaPosition, PlatformConfig,
};
use tokio::sync::mpsc::UnboundedSender;

use super::MediaKey;
use crate::api::Serving;
use crate::app::state::Playback;

fn map_media(e: &MediaControlEvent) -> Option<MediaKey> {
    match e {
        MediaControlEvent::Play => Some(MediaKey::Play),
        MediaControlEvent::Pause | MediaControlEvent::Stop => Some(MediaKey::Pause),
        MediaControlEvent::Toggle => Some(MediaKey::Toggle),
        MediaControlEvent::Next => Some(MediaKey::Next),
        _ => None,
    }
}

pub(crate) struct NowPlaying {
    controls: Mutex<MediaControls>,
    status: Mutex<(Playback, Duration)>,
}

impl std::fmt::Debug for NowPlaying {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NowPlaying").finish_non_exhaustive()
    }
}

impl NowPlaying {
    pub(crate) async fn start(keys: UnboundedSender<MediaKey>) -> Option<Self> {
        let config = PlatformConfig {
            display_name: "Brain.fm",
            dbus_name: "brainfm",
            hwnd: None,
        };
        let mut controls = MediaControls::new(config)
            .map_err(|e| tracing::warn!("media controls unavailable: {e:?}"))
            .ok()?;
        controls
            .attach(move |e| {
                if let Some(k) = map_media(&e) {
                    let _ = keys.send(k);
                }
            })
            .map_err(|e| tracing::warn!("could not attach media controls: {e:?}"))
            .ok()?;
        Some(Self {
            controls: Mutex::new(controls),
            status: Mutex::new((Playback::Idle, Duration::ZERO)),
        })
    }

    fn push(&self) {
        let (status, pos) = *self
            .status
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let progress = Some(MediaPosition(pos));
        let playback = match status {
            Playback::Playing => MediaPlayback::Playing { progress },
            Playback::Paused | Playback::Loading => MediaPlayback::Paused { progress },
            Playback::Idle => MediaPlayback::Stopped,
        };
        let mut c = self
            .controls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Err(e) = c.set_playback(playback) {
            tracing::debug!("set_playback failed: {e:?}");
        }
    }

    pub(crate) async fn set_track(&self, s: Option<&Serving>) {
        let Some(s) = s else { return };
        let meta = MediaMetadata {
            title: Some(&s.track.name),
            artist: Some("Brain.fm"),
            album: s.track.genre(),
            cover_url: s.track.image_url.as_deref(),
            duration: Some(s.track_variation.length()),
        };
        let mut c = self
            .controls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Err(e) = c.set_metadata(meta) {
            tracing::debug!("set_metadata failed: {e:?}");
        }
    }

    pub(crate) async fn set_playback(&self, status: Playback) {
        self.status
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .0 = status;
        self.push();
    }

    pub(crate) async fn set_position(&self, pos: Duration) {
        self.status
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .1 = pos;
    }

    pub(crate) async fn set_volume(&self, _percent: u8) {}
}
