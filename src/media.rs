//! MPRIS media keys and "now playing" metadata, via souvlaki.

use souvlaki::{MediaControlEvent, MediaControls, MediaMetadata, MediaPlayback, PlatformConfig};
use tokio::sync::mpsc::UnboundedSender;

use crate::api::Serving;
use crate::app::state::Playback;
use crate::app::update::{Action, MediaKey};

pub fn map_media(e: MediaControlEvent) -> Option<MediaKey> {
    match e {
        MediaControlEvent::Play => Some(MediaKey::Play),
        MediaControlEvent::Pause | MediaControlEvent::Stop => Some(MediaKey::Pause),
        MediaControlEvent::Toggle => Some(MediaKey::Toggle),
        MediaControlEvent::Next => Some(MediaKey::Next),
        _ => None,
    }
}

pub struct MediaBridge {
    controls: MediaControls,
}

impl MediaBridge {
    /// Registers on the session bus; `None` (logged) when that is unavailable.
    pub fn new(tx: UnboundedSender<Action>) -> Option<Self> {
        let config = PlatformConfig {
            display_name: "Brain.fm",
            dbus_name: "brainfm",
            hwnd: None,
        };
        let mut controls = match MediaControls::new(config) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("media controls unavailable: {e:?}");
                return None;
            }
        };
        let attached = controls.attach(move |e| {
            if let Some(k) = map_media(e) {
                let _ = tx.send(Action::Media(k));
            }
        });
        if let Err(e) = attached {
            tracing::warn!("could not attach media controls: {e:?}");
            return None;
        }
        Some(Self { controls })
    }

    pub fn set_track(&mut self, s: &Serving) {
        let genre = s.track.genre();
        let meta = MediaMetadata {
            title: Some(&s.track.name),
            artist: Some("Brain.fm"),
            album: genre,
            cover_url: s.track.image_url.as_deref(),
            duration: Some(s.track_variation.length()),
        };
        if let Err(e) = self.controls.set_metadata(meta) {
            tracing::debug!("set_metadata failed: {e:?}");
        }
    }

    pub fn set_playback(&mut self, p: Playback) {
        let playback = match p {
            Playback::Playing => MediaPlayback::Playing { progress: None },
            Playback::Paused | Playback::Loading => MediaPlayback::Paused { progress: None },
            Playback::Idle => MediaPlayback::Stopped,
        };
        if let Err(e) = self.controls.set_playback(playback) {
            tracing::debug!("set_playback failed: {e:?}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn media_events_map_to_actions() {
        assert_eq!(map_media(MediaControlEvent::Play), Some(MediaKey::Play));
        assert_eq!(map_media(MediaControlEvent::Pause), Some(MediaKey::Pause));
        assert_eq!(map_media(MediaControlEvent::Toggle), Some(MediaKey::Toggle));
        assert_eq!(map_media(MediaControlEvent::Next), Some(MediaKey::Next));
        assert_eq!(map_media(MediaControlEvent::Stop), Some(MediaKey::Pause));
        assert_eq!(map_media(MediaControlEvent::Raise), None);
    }
}
