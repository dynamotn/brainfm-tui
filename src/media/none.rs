//! Platforms without media-control integration.

use std::time::Duration;

use tokio::sync::mpsc::UnboundedSender;

use super::MediaKey;
use crate::api::Serving;
use crate::app::state::Playback;

#[derive(Debug)]
pub(crate) struct NoMedia;

impl NoMedia {
    pub(crate) async fn start(_keys: UnboundedSender<MediaKey>) -> Option<Self> {
        None
    }
    pub(crate) async fn set_track(&self, _s: Option<&Serving>) {}
    pub(crate) async fn set_playback(&self, _status: Playback) {}
    pub(crate) async fn set_position(&self, _pos: Duration) {}
    pub(crate) async fn set_volume(&self, _percent: u8) {}
}
