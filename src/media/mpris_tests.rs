use std::collections::HashMap;
use std::time::Duration;

use serde_json::json;
use tokio::sync::mpsc::unbounded_channel;
use zbus::{OwnedObjectPath, OwnedVariant};

use super::*;
use crate::api::Serving;
use crate::app::state::Playback;
use crate::media::MediaKey;

fn serving(id: &str) -> Serving {
    serde_json::from_value(json!({
        "track": {"id": id, "name": "Ascent", "imageUrl": "https://img/a.webp",
                  "tags": [{"type": "genre", "value": "Electronic"}]},
        "trackVariation": {"id": "v", "url": "a.mp3", "lengthInSeconds": 300}
    }))
    .unwrap()
}

#[test]
fn bus_names_prefer_the_plain_name() {
    assert_eq!(
        bus_names(4242),
        [
            "org.mpris.MediaPlayer2.brainfm".to_owned(),
            "org.mpris.MediaPlayer2.brainfm.instance4242".to_owned()
        ]
    );
}

#[test]
fn track_paths_are_valid_and_unique_per_track() {
    assert_eq!(
        track_path("a1-B_2"),
        "/io/github/dynamotn/brainfm/track/a1_B_2"
    );
    assert_eq!(track_path(""), "/io/github/dynamotn/brainfm/track/_");
    assert_ne!(track_path("x"), track_path("y"));
    assert!(OwnedObjectPath::try_from(track_path("weird id/../é")).is_ok());
}

#[test]
fn position_advances_only_while_playing() {
    let mut s = Shared::default();
    s.set_track(Some(&serving("a")));
    s.status = Playback::Playing;
    s.set_position(Duration::from_secs(10));
    s.position_at -= Duration::from_secs(3);
    assert_eq!(s.position().as_secs(), 13);
    s.status = Playback::Paused;
    assert_eq!(s.position().as_secs(), 10);
}

#[test]
fn position_never_passes_track_length() {
    let mut s = Shared::default();
    s.set_track(Some(&serving("a")));
    s.status = Playback::Playing;
    s.set_position(Duration::from_secs(299));
    s.position_at -= Duration::from_mins(1);
    assert_eq!(s.position(), Duration::from_mins(5));
}

#[test]
fn metadata_carries_trackid_title_and_length() {
    let mut s = Shared::default();
    s.set_track(Some(&serving("t-1")));
    let m = s.metadata();
    let path: OwnedObjectPath = m["mpris:trackid"].clone().try_into().unwrap();
    assert_eq!(path.as_str(), "/io/github/dynamotn/brainfm/track/t_1");
    let title: String = m["xesam:title"].clone().try_into().unwrap();
    assert_eq!(title, "Ascent");
    let length: i64 = m["mpris:length"].clone().try_into().unwrap();
    assert_eq!(length, 300_000_000);
}

#[test]
fn no_track_uses_the_spec_no_track_path() {
    let m = Shared::default().metadata();
    let path: OwnedObjectPath = m["mpris:trackid"].clone().try_into().unwrap();
    assert_eq!(path.as_str(), NO_TRACK);
}

async fn get(conn: &zbus::Connection, dest: &str, iface: &str, prop: &str) -> OwnedVariant {
    let reply = conn
        .call_method(
            Some(dest),
            OBJECT_PATH,
            Some("org.freedesktop.DBus.Properties"),
            "Get",
            &(iface, prop),
        )
        .await
        .unwrap();
    let v: OwnedVariant = reply.body().deserialize().unwrap();
    v
}

/// Needs a session bus: `dbus-run-session -- cargo test --lib mpris -- --ignored`.
#[tokio::test]
#[ignore = "needs a D-Bus session bus"]
async fn serves_mpris_on_the_session_bus() {
    let (tx, mut keys) = unbounded_channel();
    let first = Mpris::start(tx.clone()).await.expect("first instance");
    let second = Mpris::try_start(tx)
        .await
        .expect("second instance gets its own name");
    assert_eq!(first.bus_name(), "org.mpris.MediaPlayer2.brainfm");
    assert!(
        second
            .bus_name()
            .starts_with("org.mpris.MediaPlayer2.brainfm.instance")
    );

    first.set_track(Some(&serving("abc"))).await;
    first.set_playback(Playback::Playing).await;
    first.set_position(Duration::from_secs(42)).await;
    first.set_volume(70).await;

    let client = zbus::Connection::session().await.unwrap();
    let dest = first.bus_name().to_owned();
    let status: String = get(&client, &dest, PLAYER_IFACE, "PlaybackStatus")
        .await
        .try_into()
        .unwrap();
    assert_eq!(status, "Playing");
    let pos: i64 = get(&client, &dest, PLAYER_IFACE, "Position")
        .await
        .try_into()
        .unwrap();
    assert!((42_000_000..43_000_000).contains(&pos), "{pos}");
    let meta: HashMap<String, OwnedVariant> = get(&client, &dest, PLAYER_IFACE, "Metadata")
        .await
        .try_into()
        .unwrap();
    let path: OwnedObjectPath = meta["mpris:trackid"].clone().try_into().unwrap();
    assert_eq!(path.as_str(), "/io/github/dynamotn/brainfm/track/abc");
    for (prop, want) in [
        ("CanGoPrevious", false),
        ("CanSeek", false),
        ("CanGoNext", true),
    ] {
        let v: bool = get(&client, &dest, PLAYER_IFACE, prop)
            .await
            .try_into()
            .unwrap();
        assert_eq!(v, want, "{prop}");
    }

    for (method, key) in [
        ("PlayPause", MediaKey::Toggle),
        ("Next", MediaKey::Next),
        ("Stop", MediaKey::Pause),
    ] {
        client
            .call_method(
                Some(dest.as_str()),
                OBJECT_PATH,
                Some(PLAYER_IFACE),
                method,
                &(),
            )
            .await
            .unwrap();
        assert_eq!(keys.recv().await, Some(key), "{method}");
    }
    client
        .call_method(
            Some(dest.as_str()),
            OBJECT_PATH,
            Some("org.freedesktop.DBus.Properties"),
            "Set",
            &(PLAYER_IFACE, "Volume", zbus::Variant::from(0.25_f64)),
        )
        .await
        .unwrap();
    assert_eq!(keys.recv().await, Some(MediaKey::SetVolume(25)));
    let vol: f64 = get(&client, &dest, PLAYER_IFACE, "Volume")
        .await
        .try_into()
        .unwrap();
    assert!(
        (vol - 0.25).abs() < 1e-9,
        "setter must not report the old volume: {vol}"
    );
}
