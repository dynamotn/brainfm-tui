use std::time::Duration;

use serde_json::json;

use super::*;
use crate::api::Serving;
use crate::media::MediaKey;
use crate::player::{PlayerCommand, PlayerEvent};
use crate::timer::TimerMode;

fn serving(id: &str) -> Serving {
    serde_json::from_value(json!({
        "track": {"id": id, "name": id, "tags": []},
        "trackVariation": {"id": "v", "url": format!("{id}.mp3"), "lengthInSeconds": 300}
    }))
    .unwrap()
}

fn started(m: &mut Machine, id: &str) -> Vec<Cmd> {
    m.on_player(PlayerEvent::Started { id: id.into() })
}

fn fetch_of(cmds: &[Cmd], want: FetchKind) -> Option<u64> {
    cmds.iter().find_map(|c| match c {
        Cmd::Fetch { kind, epoch } if *kind == want => Some(*epoch),
        _ => None,
    })
}

fn loads(cmds: &[Cmd]) -> Vec<String> {
    cmds.iter()
        .filter_map(|c| match c {
            Cmd::Player(PlayerCommand::Load { id, .. }) => Some(id.clone()),
            _ => None,
        })
        .collect()
}

fn playing() -> Machine {
    let (mut m, cmds) = Machine::new(serving("a"), TimerMode::Infinite, 70);
    assert_eq!(loads(&cmds), ["a"]);
    started(&mut m, "a");
    m
}

#[test]
fn skip_during_a_prefetch_ignores_the_stale_prefetch() {
    let mut m = playing();
    let prefetch = fetch_of(
        &m.on_player(PlayerEvent::NearEnd { id: "a".into() }),
        FetchKind::Next,
    )
    .unwrap();
    let skip = fetch_of(&m.on_key(MediaKey::Next), FetchKind::Now).unwrap();
    assert_ne!(prefetch, skip);
    assert_eq!(
        m.on_fetched(FetchKind::Next, prefetch, Ok(serving("stale"))),
        Vec::<Cmd>::new()
    );
    assert_eq!(
        loads(&m.on_fetched(FetchKind::Now, skip, Ok(serving("b")))),
        ["b"]
    );
    started(&mut m, "b");
    assert!(m.next.is_none());
    let again = m.on_player(PlayerEvent::NearEnd { id: "b".into() });
    assert!(
        fetch_of(&again, FetchKind::Next).is_some(),
        "the new track prefetches normally"
    );
}

#[test]
fn repeated_next_presses_fetch_once() {
    let mut m = playing();
    assert!(fetch_of(&m.on_key(MediaKey::Next), FetchKind::Now).is_some());
    assert_eq!(m.on_key(MediaKey::Next), Vec::<Cmd>::new());
}

#[test]
fn skip_while_paused_stays_paused_and_keeps_the_timer_stopped() {
    let (mut m, _) = Machine::new(
        serving("a"),
        TimerMode::Countdown(Duration::from_mins(25)),
        70,
    );
    started(&mut m, "a");
    m.on_key(MediaKey::Pause);
    let e = fetch_of(&m.on_key(MediaKey::Next), FetchKind::Now).unwrap();
    m.on_fetched(FetchKind::Now, e, Ok(serving("b")));
    let cmds = started(&mut m, "b");
    assert!(
        cmds.contains(&Cmd::Player(PlayerCommand::Pause)),
        "{cmds:?}"
    );
    assert!(!m.timer.is_running());
}

#[test]
fn a_user_pause_during_a_break_stops_the_timer_until_resumed() {
    let mode = TimerMode::Intervals {
        focus: Duration::from_secs(10),
        rest: Duration::from_secs(10),
    };
    let (mut m, _) = Machine::new(serving("a"), mode, 70);
    started(&mut m, "a");
    let cmds = m.on_tick(Duration::from_secs(10));
    assert!(
        cmds.contains(&Cmd::Player(PlayerCommand::Pause)),
        "break pauses music"
    );
    assert!(m.timer.is_running(), "breaks keep counting");
    m.on_key(MediaKey::Pause);
    assert!(!m.timer.is_running(), "a user pause stops the clock");
    assert_eq!(m.on_tick(Duration::from_secs(20)), Vec::<Cmd>::new());
    m.on_key(MediaKey::Play);
    assert!(m.timer.is_running());
}

#[test]
fn play_pause_toggles_during_a_break() {
    let mode = TimerMode::Intervals {
        focus: Duration::from_secs(10),
        rest: Duration::from_secs(10),
    };
    let (mut m, _) = Machine::new(serving("a"), mode, 70);
    started(&mut m, "a");
    m.on_tick(Duration::from_secs(10));
    m.on_key(MediaKey::Toggle);
    assert!(m.pauses.user);
    m.on_key(MediaKey::Toggle);
    assert!(!m.pauses.user);
}

#[test]
fn a_dropped_queued_track_clears_next_without_reloading() {
    let mut m = playing();
    let e = fetch_of(
        &m.on_player(PlayerEvent::NearEnd { id: "a".into() }),
        FetchKind::Next,
    )
    .unwrap();
    m.on_fetched(FetchKind::Next, e, Ok(serving("b")));
    let cmds = m.on_player(PlayerEvent::Error {
        id: "b".into(),
        message: "dropped".into(),
    });
    assert_eq!(cmds, Vec::<Cmd>::new());
    assert!(m.next.is_none());
    assert!(
        fetch_of(
            &m.on_player(PlayerEvent::Ended { id: "a".into() }),
            FetchKind::Now
        )
        .is_some()
    );
}

#[test]
fn positions_of_other_tracks_are_ignored() {
    let mut m = playing();
    m.on_player(PlayerEvent::Position {
        id: "old".into(),
        pos: Duration::from_secs(99),
    });
    assert_eq!(m.pos, Duration::ZERO);
    m.on_player(PlayerEvent::Position {
        id: "a".into(),
        pos: Duration::from_secs(5),
    });
    assert_eq!(m.pos, Duration::from_secs(5));
}

#[test]
fn repeated_load_errors_give_up() {
    let mut m = playing();
    for _ in 0..2 {
        let cmds = m.on_player(PlayerEvent::Error {
            id: "a".into(),
            message: "boom".into(),
        });
        let e = fetch_of(&cmds, FetchKind::Now).unwrap();
        m.on_fetched(FetchKind::Now, e, Ok(serving("a")));
    }
    let cmds = m.on_player(PlayerEvent::Error {
        id: "a".into(),
        message: "boom".into(),
    });
    assert!(
        matches!(cmds.as_slice(), [Cmd::Fail(msg)] if msg.contains("boom")),
        "{cmds:?}"
    );
}

#[test]
fn an_expired_session_during_prefetch_is_fatal() {
    let mut m = playing();
    let e = fetch_of(
        &m.on_player(PlayerEvent::NearEnd { id: "a".into() }),
        FetchKind::Next,
    )
    .unwrap();
    let cmds = m.on_fetched(FetchKind::Next, e, Err(FetchError::Unauthorized));
    assert!(matches!(cmds.as_slice(), [Cmd::Fail(_)]));
}

#[test]
fn other_prefetch_failures_retry_when_the_track_ends() {
    let mut m = playing();
    let e = fetch_of(
        &m.on_player(PlayerEvent::NearEnd { id: "a".into() }),
        FetchKind::Next,
    )
    .unwrap();
    assert_eq!(
        m.on_fetched(FetchKind::Next, e, Err(FetchError::Other("503".into()))),
        Vec::<Cmd>::new()
    );
    assert!(
        fetch_of(
            &m.on_player(PlayerEvent::Ended { id: "a".into() }),
            FetchKind::Now
        )
        .is_some()
    );
}

#[test]
fn countdown_end_stops_with_a_fade() {
    let (mut m, _) = Machine::new(
        serving("a"),
        TimerMode::Countdown(Duration::from_secs(5)),
        70,
    );
    started(&mut m, "a");
    assert_eq!(
        m.on_tick(Duration::from_secs(6)),
        [Cmd::Stop {
            fade: Duration::from_secs(3)
        }]
    );
}

#[test]
fn volume_keys_set_the_player_volume() {
    let mut m = playing();
    assert_eq!(
        m.on_key(MediaKey::SetVolume(30)),
        [Cmd::Player(PlayerCommand::SetVolume(0.3))]
    );
    assert_eq!(m.volume, 30);
}
