//! An MPRIS 2 server on the session bus, so desktop media widgets, media keys
//! and `playerctl` can see and control brainfm.
//!
//! `Previous`, `Seek` and `SetPosition` are not offered (Brain.fm streams a fresh
//! track instead), and the `Can*` properties say so.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use tokio::sync::mpsc::UnboundedSender;
use zbus::fdo::{RequestNameFlags, RequestNameReply};
use zbus::object_server::{InterfaceRef, SignalEmitter};
use zbus::{Connection, interface};
use zbus::{ObjectPath, Variant};

use super::MediaKey;
use crate::api::Serving;
use crate::app::state::Playback;

const BUS_NAME: &str = "org.mpris.MediaPlayer2.brainfm";
pub(super) const OBJECT_PATH: &str = "/org/mpris/MediaPlayer2";
#[cfg(test)]
pub(super) const PLAYER_IFACE: &str = "org.mpris.MediaPlayer2.Player";
pub(super) const NO_TRACK: &str = "/org/mpris/MediaPlayer2/TrackList/NoTrack";
const TRACK_PREFIX: &str = "/io/github/dynamotn/brainfm/track/";

/// The plain name first; a second instance falls back to the spec's
/// `.instance<pid>` suffix instead of losing MPRIS.
pub(super) fn bus_names(pid: u32) -> [String; 2] {
    [BUS_NAME.to_owned(), format!("{BUS_NAME}.instance{pid}")]
}

/// A D-Bus object path unique to the track (only `[A-Za-z0-9_]` allowed).
pub(super) fn track_path(id: &str) -> String {
    let mut safe: String = id
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    if safe.is_empty() {
        safe.push('_');
    }
    format!("{TRACK_PREFIX}{safe}")
}

#[derive(Debug, Clone)]
struct TrackInfo {
    path: String,
    title: String,
    genre: Option<String>,
    art: Option<String>,
    length: Duration,
}

/// What the bus sees; the app pushes updates into it.
#[derive(Debug)]
pub(super) struct Shared {
    track: Option<TrackInfo>,
    pub(super) status: Playback,
    position: Duration,
    pub(super) position_at: Instant,
    volume: u8,
}

impl Default for Shared {
    fn default() -> Self {
        Self {
            track: None,
            status: Playback::Idle,
            position: Duration::ZERO,
            position_at: Instant::now(),
            volume: 70,
        }
    }
}

impl Shared {
    pub(super) fn set_track(&mut self, s: Option<&Serving>) {
        self.track = s.map(|s| TrackInfo {
            path: track_path(&s.track.id),
            title: s.track.name.clone(),
            genre: s.track.genre().map(str::to_owned),
            art: s.track.image_url.clone(),
            length: s.track_variation.length(),
        });
        self.set_position(Duration::ZERO);
    }

    pub(super) fn set_position(&mut self, pos: Duration) {
        self.position = pos;
        self.position_at = Instant::now();
    }

    /// The last reported position, advanced by wall time while playing so
    /// clients polling `Position` see it move between reports.
    pub(super) fn position(&self) -> Duration {
        let mut pos = self.position;
        if self.status == Playback::Playing {
            pos += self.position_at.elapsed();
        }
        match &self.track {
            Some(t) if !t.length.is_zero() => pos.min(t.length),
            _ => pos,
        }
    }

    pub(super) fn metadata(&self) -> HashMap<String, Variant<'static>> {
        let mut m = HashMap::new();
        let path = self.track.as_ref().map_or(NO_TRACK, |t| t.path.as_str());
        let path = ObjectPath::try_from(path.to_owned()).expect("track_path builds valid paths");
        m.insert("mpris:trackid".to_owned(), Variant::from(path));
        if let Some(t) = &self.track {
            m.insert("xesam:title".to_owned(), Variant::from(t.title.clone()));
            m.insert(
                "xesam:artist".to_owned(),
                Variant::from(vec!["Brain.fm".to_owned()]),
            );
            let length = i64::try_from(t.length.as_micros()).unwrap_or(i64::MAX);
            m.insert("mpris:length".to_owned(), Variant::from(length));
            if let Some(g) = &t.genre {
                m.insert("xesam:album".to_owned(), Variant::from(g.clone()));
                m.insert("xesam:genre".to_owned(), Variant::from(vec![g.clone()]));
            }
            if let Some(a) = &t.art {
                m.insert("mpris:artUrl".to_owned(), Variant::from(a.clone()));
            }
        }
        m
    }
}

fn lock(shared: &Mutex<Shared>) -> std::sync::MutexGuard<'_, Shared> {
    shared.lock().unwrap_or_else(PoisonError::into_inner)
}

struct Root;

// zbus dispatches D-Bus calls to these methods: they must take `&self` and
// keep every argument of the MPRIS signature.
#[allow(clippy::unused_self)]
#[interface(name = "org.mpris.MediaPlayer2")]
impl Root {
    fn raise(&self) {}
    fn quit(&self) {}
    #[zbus(property)]
    fn can_quit(&self) -> bool {
        false
    }
    #[zbus(property)]
    fn can_raise(&self) -> bool {
        false
    }
    #[zbus(property)]
    fn has_track_list(&self) -> bool {
        false
    }
    #[zbus(property)]
    fn identity(&self) -> &'static str {
        "Brain.fm"
    }
    #[zbus(property)]
    fn supported_uri_schemes(&self) -> Vec<String> {
        Vec::new()
    }
    #[zbus(property)]
    fn supported_mime_types(&self) -> Vec<String> {
        Vec::new()
    }
}

struct Player {
    shared: Arc<Mutex<Shared>>,
    keys: UnboundedSender<MediaKey>,
}

impl Player {
    fn send(&self, key: MediaKey) {
        let _ = self.keys.send(key);
    }
}

// zbus dispatches D-Bus calls to these methods: they must take `&self` and
// keep every argument of the MPRIS signature.
#[allow(clippy::unused_self)]
#[interface(name = "org.mpris.MediaPlayer2.Player")]
impl Player {
    fn next(&self) {
        self.send(MediaKey::Next);
    }
    fn previous(&self) {}
    fn pause(&self) {
        self.send(MediaKey::Pause);
    }
    fn play_pause(&self) {
        self.send(MediaKey::Toggle);
    }
    fn stop(&self) {
        self.send(MediaKey::Pause);
    }
    fn play(&self) {
        self.send(MediaKey::Play);
    }
    // Not supported (CanSeek is false); the calls are accepted and ignored.
    fn seek(&self, offset: i64) {
        let _ = offset;
    }
    fn set_position(&self, track_id: ObjectPath<'_>, position: i64) {
        let _ = (track_id, position);
    }
    fn open_uri(&self, uri: &str) {
        let _ = uri;
    }

    #[zbus(property)]
    fn playback_status(&self) -> &'static str {
        match lock(&self.shared).status {
            Playback::Playing => "Playing",
            Playback::Paused | Playback::Loading => "Paused",
            Playback::Idle => "Stopped",
        }
    }
    #[zbus(property)]
    fn rate(&self) -> f64 {
        1.0
    }
    #[zbus(property)]
    fn minimum_rate(&self) -> f64 {
        1.0
    }
    #[zbus(property)]
    fn maximum_rate(&self) -> f64 {
        1.0
    }
    #[zbus(property)]
    fn metadata(&self) -> HashMap<String, Variant<'static>> {
        lock(&self.shared).metadata()
    }
    #[zbus(property)]
    fn volume(&self) -> f64 {
        f64::from(lock(&self.shared).volume) / 100.0
    }
    #[zbus(property)]
    fn set_volume(&mut self, volume: f64) {
        // Clamped to 0..=100 first, so the cast cannot truncate.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let percent = (volume.clamp(0.0, 1.0) * 100.0).round() as u8;
        self.send(MediaKey::SetVolume(percent));
    }
    /// Per the spec, position changes are not signalled; clients poll.
    #[zbus(property(emits_changed_signal = "false"))]
    fn position(&self) -> i64 {
        i64::try_from(lock(&self.shared).position().as_micros()).unwrap_or(i64::MAX)
    }
    #[zbus(property)]
    fn can_go_next(&self) -> bool {
        true
    }
    #[zbus(property)]
    fn can_go_previous(&self) -> bool {
        false
    }
    #[zbus(property)]
    fn can_play(&self) -> bool {
        true
    }
    #[zbus(property)]
    fn can_pause(&self) -> bool {
        true
    }
    #[zbus(property)]
    fn can_seek(&self) -> bool {
        false
    }
    #[zbus(property)]
    fn can_control(&self) -> bool {
        true
    }
}

/// A running MPRIS server; dropping it releases the bus name.
pub(crate) struct Mpris {
    _conn: Connection,
    player: InterfaceRef<Player>,
    shared: Arc<Mutex<Shared>>,
    bus_name: String,
}

impl std::fmt::Debug for Mpris {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Mpris")
            .field("bus_name", &self.bus_name)
            .finish_non_exhaustive()
    }
}

impl Mpris {
    /// Registers on the session bus; `None` (logged) when there is no bus.
    pub(crate) async fn start(keys: UnboundedSender<MediaKey>) -> Option<Self> {
        match Self::try_start(keys).await {
            Ok(m) => {
                tracing::info!("MPRIS registered as {}", m.bus_name);
                Some(m)
            }
            Err(e) => {
                tracing::warn!("MPRIS unavailable: {e}");
                None
            }
        }
    }

    async fn try_start(keys: UnboundedSender<MediaKey>) -> zbus::Result<Self> {
        let shared = Arc::new(Mutex::new(Shared::default()));
        let conn = zbus::connection::Builder::session()
            .serve_at(OBJECT_PATH, Root)
            .serve_at(
                OBJECT_PATH,
                Player {
                    shared: shared.clone(),
                    keys,
                },
            )
            .build()
            .await?;
        let mut bus_name = None;
        for name in bus_names(std::process::id()) {
            // Never take a name from a running instance, and never wait for it.
            match conn
                .request_name_with_flags(name.as_str(), RequestNameFlags::DO_NOT_QUEUE)
                .await
            {
                Ok(RequestNameReply::PrimaryOwner) => {
                    bus_name = Some(name);
                    break;
                }
                Ok(_) | Err(zbus::Error::NameTaken) => {}
                Err(e) => return Err(e),
            }
        }
        let bus_name = bus_name.ok_or(zbus::Error::NameTaken)?;
        let player = conn
            .object_server()
            .interface::<_, Player>(OBJECT_PATH)
            .await?;
        Ok(Self {
            _conn: conn,
            player,
            shared,
            bus_name,
        })
    }

    #[cfg(test)]
    pub(crate) fn bus_name(&self) -> &str {
        &self.bus_name
    }

    fn emitter(&self) -> &SignalEmitter<'static> {
        self.player.signal_emitter()
    }

    pub(crate) async fn set_track(&self, s: Option<&Serving>) {
        lock(&self.shared).set_track(s);
        let iface = self.player.get().await;
        if let Err(e) = iface.metadata_changed(self.emitter()).await {
            tracing::debug!("MPRIS metadata signal: {e}");
        }
    }

    pub(crate) async fn set_playback(&self, status: Playback) {
        {
            let mut s = lock(&self.shared);
            // Freeze the interpolated position at the moment playback stops.
            let pos = s.position();
            s.status = status;
            s.set_position(pos);
        }
        let iface = self.player.get().await;
        if let Err(e) = iface.playback_status_changed(self.emitter()).await {
            tracing::debug!("MPRIS status signal: {e}");
        }
    }

    // Async like every other backend method, so callers need no cfg.
    #[allow(clippy::unused_async)]
    pub(crate) async fn set_position(&self, pos: Duration) {
        lock(&self.shared).set_position(pos);
    }

    pub(crate) async fn set_volume(&self, percent: u8) {
        lock(&self.shared).volume = percent.min(100);
        let iface = self.player.get().await;
        if let Err(e) = iface.volume_changed(self.emitter()).await {
            tracing::debug!("MPRIS volume signal: {e}");
        }
    }
}

#[cfg(test)]
#[path = "mpris_tests.rs"]
mod tests;
