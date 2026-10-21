use std::time::Duration;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde_json::json;

use super::state::*;
use super::update::*;
use crate::api::{Activity, EffectLevel, MentalState, Serving};
use crate::config::Config;
use crate::player::{PlayerCommand, PlayerEvent};
use crate::timer::{Phase, Timer, TimerMode};

fn key(c: char) -> Action {
    Action::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE))
}

fn code(k: KeyCode) -> Action {
    Action::Key(KeyEvent::new(k, KeyModifiers::NONE))
}

fn serving(id: &str) -> Serving {
    serde_json::from_value(json!({
        "track": {"id": id, "name": format!("Track {id}"), "tags": []},
        "trackVariation": {"id": format!("v{id}"), "tokenedUrl": format!("https://cdn/{id}.mp3"),
                           "lengthInSeconds": 300}
    }))
    .unwrap()
}

fn ready() -> AppState {
    let mut s = AppState::new(Config::default(), true, Some("u1".into()));
    s.states = ["Focus", "Relax", "Sleep", "Meditate"]
        .iter()
        .map(|n| MentalState {
            id: n.to_lowercase(),
            display_value: n.to_string(),
            default_activity: Some(Activity {
                id: format!("{}-act", n.to_lowercase()),
                display_value: "Default".into(),
            }),
        })
        .collect();
    s.activities = vec![Activity {
        id: "focus-act".into(),
        display_value: "Deep Work".into(),
    }];
    s.genres = vec!["Piano".into(), "Electronic".into()];
    s
}

fn playing() -> AppState {
    let mut s = ready();
    {
        let a = reply(&s, Intent::Now, vec![serving("a")]);
        update(&mut s, a)
    };
    update(
        &mut s,
        Action::Player(PlayerEvent::Started { id: "a".into() }),
    );
    s
}

fn has_session(fx: &[Effect], intent: Intent) -> bool {
    fx.iter()
        .any(|e| matches!(e, Effect::StartSession { intent: i, .. } if *i == intent))
}

#[test]
fn space_from_idle_starts_session() {
    let mut s = ready();
    let fx = update(&mut s, key(' '));
    assert!(has_session(&fx, Intent::Now));
    assert_eq!(s.playback, Playback::Loading);
    let Some(Effect::StartSession { req, .. }) =
        fx.iter().find(|e| matches!(e, Effect::StartSession { .. }))
    else {
        panic!()
    };
    assert_eq!(req.dynamic_activity_id, "focus-act");
}

#[test]
fn servings_now_loads_player() {
    let mut s = ready();
    let fx = {
        let a = reply(&s, Intent::Now, vec![serving("a")]);
        update(&mut s, a)
    };
    assert!(fx.contains(&Effect::Player(PlayerCommand::Load {
        id: "a".into(),
        url: "https://cdn/a.mp3".into(),
        length: Duration::from_mins(5),
    })));
    assert_eq!(s.pending.as_ref().unwrap().track.id, "a");
    assert!(
        s.now.is_none(),
        "becomes now only when the engine starts it"
    );
}

#[test]
fn started_sets_playing_and_starts_timer() {
    let s = playing();
    assert_eq!(s.playback, Playback::Playing);
    assert!(s.timer.is_running());
}

#[test]
fn near_end_requests_next_then_queues() {
    let mut s = playing();
    let fx = update(
        &mut s,
        Action::Player(PlayerEvent::NearEnd { id: "a".into() }),
    );
    assert!(has_session(&fx, Intent::Next));
    let fx = {
        let a = reply(&s, Intent::Next, vec![serving("b")]);
        update(&mut s, a)
    };
    assert!(
        matches!(fx.as_slice(), [Effect::Player(PlayerCommand::Queue { id, .. })] if id == "b")
    );
}

#[test]
fn ended_promotes_next_to_now() {
    let mut s = playing();
    {
        let a = reply(&s, Intent::Next, vec![serving("b")]);
        update(&mut s, a)
    };
    let fx = update(
        &mut s,
        Action::Player(PlayerEvent::Ended { id: "a".into() }),
    );
    assert!(!has_session(&fx, Intent::Now));
    update(
        &mut s,
        Action::Player(PlayerEvent::Started { id: "b".into() }),
    );
    assert_eq!(s.now.as_ref().unwrap().track.id, "b");
    assert!(s.next.is_none());
}

#[test]
fn ended_without_next_fetches_now() {
    let mut s = playing();
    let fx = update(
        &mut s,
        Action::Player(PlayerEvent::Ended { id: "a".into() }),
    );
    assert!(has_session(&fx, Intent::Now));
}

#[test]
fn player_error_skips_to_next() {
    let mut s = playing();
    let fx = update(
        &mut s,
        Action::Player(PlayerEvent::Error {
            id: "a".into(),
            message: "boom".into(),
        }),
    );
    assert!(has_session(&fx, Intent::Now));
    assert!(s.toast.as_ref().unwrap().text.contains("boom"));
    assert_eq!(s.playback, Playback::Loading);
}

#[test]
fn repeated_player_errors_stop() {
    let mut s = playing();
    for _ in 0..2 {
        update(
            &mut s,
            Action::Player(PlayerEvent::Error {
                id: "a".into(),
                message: "x".into(),
            }),
        );
    }
    let fx = update(
        &mut s,
        Action::Player(PlayerEvent::Error {
            id: "a".into(),
            message: "x".into(),
        }),
    );
    assert!(!has_session(&fx, Intent::Now));
    assert_eq!(s.playback, Playback::Idle);
}

#[test]
fn empty_session_shows_toast() {
    let mut s = ready();
    update(&mut s, key(' '));
    let fx = {
        let a = reply(&s, Intent::Now, vec![]);
        update(&mut s, a)
    };
    assert_eq!(fx, Vec::<Effect>::new());
    assert_eq!(
        s.toast.as_ref().unwrap().text,
        "No tracks match these filters"
    );
    assert_eq!(s.playback, Playback::Idle);
}

#[test]
fn unauthorized_opens_login() {
    let mut s = playing();
    let fx = update(
        &mut s,
        Action::Api(ApiReply::Failed {
            what: "session",
            error: ApiErrorKind::Unauthorized,
        }),
    );
    assert!(matches!(s.overlay, Some(Overlay::Login { .. })));
    assert!(fx.contains(&Effect::Player(PlayerCommand::Pause)));
    assert_eq!(s.playback, Playback::Paused);
}

#[test]
fn login_overlay_collects_and_submits() {
    let mut s = ready();
    s.overlay = Some(Overlay::login());
    for c in "a@b.c".chars() {
        update(&mut s, key(c));
    }
    update(&mut s, code(KeyCode::Tab));
    for c in "pw q".chars() {
        update(&mut s, key(c));
    }
    let fx = update(&mut s, code(KeyCode::Enter));
    assert_eq!(
        fx,
        vec![Effect::Login {
            email: "a@b.c".into(),
            password: "pw q".into()
        }]
    );
    assert!(!s.quit);
}

#[test]
fn logged_in_closes_overlay_and_loads() {
    let mut s = ready();
    s.overlay = Some(Overlay::login());
    let fx = update(
        &mut s,
        Action::Api(ApiReply::LoggedIn {
            user_id: "u2".into(),
        }),
    );
    assert!(s.overlay.is_none());
    assert_eq!(s.user_id.as_deref(), Some("u2"));
    assert!(fx.contains(&Effect::FetchStates) && fx.contains(&Effect::FetchLibrary));
}

#[test]
fn countdown_finished_fades_out() {
    let mut s = playing();
    s.timer = Timer::new(TimerMode::Countdown(Duration::from_secs(1)));
    s.timer.start();
    let fx = update(&mut s, Action::Tick(Duration::from_secs(2)));
    assert!(fx.contains(&Effect::Player(PlayerCommand::Stop {
        fade: Duration::from_secs(3)
    })));
    assert_eq!(s.playback, Playback::Idle);
}

#[test]
fn interval_phase_change_chimes_and_pauses_on_rest() {
    let mut s = playing();
    s.timer = Timer::new(TimerMode::Intervals {
        focus: Duration::from_secs(1),
        rest: Duration::from_secs(1),
    });
    s.timer.start();
    let fx = update(&mut s, Action::Tick(Duration::from_secs(1)));
    assert_eq!(
        fx,
        vec![
            Effect::Player(PlayerCommand::Chime),
            Effect::Player(PlayerCommand::Pause)
        ]
    );
    assert_eq!(s.timer.phase(), Phase::Rest);
    let fx = update(&mut s, Action::Tick(Duration::from_secs(1)));
    assert_eq!(
        fx,
        vec![
            Effect::Player(PlayerCommand::Chime),
            Effect::Player(PlayerCommand::Play)
        ]
    );
    assert_eq!(s.playback, Playback::Playing);
}

#[test]
fn tab_cycles_state_and_refetches_filters() {
    let mut s = ready();
    let fx = update(&mut s, code(KeyCode::Tab));
    assert_eq!(s.state_idx, 1);
    assert!(fx.contains(&Effect::FetchFilters("relax".into())));
    assert!(fx.contains(&Effect::SaveConfig));
    assert_eq!(s.config.last_state.as_deref(), Some("relax"));
    let fx = update(&mut s, key('4'));
    assert_eq!(s.state_idx, 3);
    assert!(fx.contains(&Effect::FetchFilters("meditate".into())));
}

#[test]
fn changing_state_while_playing_restarts_session() {
    let mut s = playing();
    let fx = update(&mut s, code(KeyCode::Tab));
    assert!(has_session(&fx, Intent::Now));
}

#[test]
fn favorite_toggles_optimistically() {
    let mut s = playing();
    let fx = update(&mut s, key('f'));
    assert!(s.favorite_ids.contains("a"));
    assert_eq!(
        fx,
        vec![Effect::SetFavorite {
            track_id: "a".into(),
            on: true
        }]
    );
    let fx = update(&mut s, key('f'));
    assert!(!s.favorite_ids.contains("a"));
    assert_eq!(
        fx,
        vec![Effect::SetFavorite {
            track_id: "a".into(),
            on: false
        }]
    );
}

#[test]
fn dislike_skips() {
    let mut s = playing();
    let fx = update(&mut s, key('d'));
    assert!(fx.contains(&Effect::Dislike {
        track_id: "a".into()
    }));
    assert!(has_session(&fx, Intent::Now));
}

#[test]
fn space_toggles_pause() {
    let mut s = playing();
    let fx = update(&mut s, key(' '));
    assert_eq!(fx, vec![Effect::Player(PlayerCommand::Pause)]);
    assert_eq!(s.playback, Playback::Paused);
    assert!(!s.timer.is_running());
    let fx = update(&mut s, key(' '));
    assert_eq!(fx, vec![Effect::Player(PlayerCommand::Play)]);
    assert!(s.timer.is_running());
}

#[test]
fn volume_clamps_0_100() {
    let mut s = ready();
    s.config.volume = 95;
    let fx = update(&mut s, key('+'));
    assert_eq!(s.config.volume, 100);
    assert!(fx.contains(&Effect::Player(PlayerCommand::SetVolume(1.0))));
    s.config.volume = 5;
    update(&mut s, key('-'));
    assert_eq!(s.config.volume, 0);
}

#[test]
fn malformed_config_never_saves() {
    let mut s = ready();
    s.config_writable = false;
    let fx = update(&mut s, code(KeyCode::Tab));
    assert!(!fx.contains(&Effect::SaveConfig));
}

#[test]
fn genres_overlay_applies_choice() {
    let mut s = ready();
    update(&mut s, key('g'));
    assert!(matches!(s.overlay, Some(Overlay::Genres { .. })));
    update(&mut s, key('j'));
    update(&mut s, key(' '));
    let fx = update(&mut s, code(KeyCode::Enter));
    assert!(s.overlay.is_none());
    assert_eq!(s.current_prefs().genres, vec!["Electronic"]);
    assert!(fx.contains(&Effect::SaveConfig));
    assert_eq!(s.session_request().unwrap().genre_names, vec!["Electronic"]);
}

#[test]
fn effect_overlay_applies_choice() {
    let mut s = ready();
    update(&mut s, key('e'));
    update(&mut s, key('j'));
    update(&mut s, key('j'));
    update(&mut s, key('j'));
    update(&mut s, key(' '));
    update(&mut s, code(KeyCode::Enter));
    assert_eq!(s.current_prefs().effects, vec![EffectLevel::High]);
}

#[test]
fn timer_overlay_sets_mode() {
    let mut s = ready();
    update(&mut s, key('t'));
    update(&mut s, key('j'));
    update(&mut s, code(KeyCode::Enter));
    assert_eq!(
        *s.timer.mode(),
        TimerMode::Countdown(Duration::from_mins(30))
    );
}

#[test]
fn quit_stops_player() {
    let mut s = playing();
    let fx = update(&mut s, key('q'));
    assert!(s.quit);
    assert!(fx.contains(&Effect::Player(PlayerCommand::Stop {
        fade: Duration::from_millis(500)
    })));
}

#[test]
fn states_reply_selects_last_state() {
    let mut s = AppState::new(
        Config {
            last_state: Some("sleep".into()),
            ..Config::default()
        },
        true,
        Some("u1".into()),
    );
    let states = ready().states;
    let fx = update(&mut s, Action::Api(ApiReply::States(states)));
    assert_eq!(s.state_idx, 2);
    assert_eq!(fx, vec![Effect::FetchFilters("sleep".into())]);
}

#[test]
fn panel_enter_plays_selected_favorite() {
    let mut s = ready();
    update(
        &mut s,
        Action::Api(ApiReply::Library {
            favorites: vec![serving("f1")],
            recents: vec![serving("r1")],
        }),
    );
    assert!(s.favorite_ids.contains("f1"));
    s.panel.visible = true;
    update(&mut s, key('j'));
    let fx = update(&mut s, code(KeyCode::Enter));
    assert!(
        matches!(fx.as_slice(), [Effect::Player(PlayerCommand::Load { id, .. })] if id == "r1")
    );
}

#[test]
fn toast_expires() {
    let mut s = ready();
    s.toast = Some(Toast::new("hi"));
    update(&mut s, Action::Tick(Duration::from_secs(10)));
    assert!(s.toast.is_none());
}

#[test]
fn api_errors_map_to_reducer_kinds() {
    use crate::api::ApiError;
    assert_eq!(
        super::error_kind(&ApiError::Unauthorized),
        ApiErrorKind::Unauthorized
    );
    assert!(
        matches!(super::error_kind(&ApiError::RateLimited), ApiErrorKind::Other(m) if m.contains("rate limited"))
    );
}

pub(crate) fn reply(s: &AppState, intent: Intent, items: Vec<Serving>) -> Action {
    Action::Api(ApiReply::Servings {
        intent,
        generation: s.session_gen,
        items,
    })
}

fn gen_of(fx: &[Effect]) -> u64 {
    fx.iter()
        .find_map(|e| match e {
            Effect::StartSession { generation, .. } => Some(*generation),
            _ => None,
        })
        .expect("a StartSession effect")
}

#[test]
fn failed_skip_keeps_playing() {
    let mut s = playing();
    update(&mut s, key('n'));
    assert_eq!(s.playback, Playback::Loading);
    {
        let a = reply(&s, Intent::Now, vec![]);
        update(&mut s, a)
    };
    assert_eq!(s.playback, Playback::Playing);
    assert_eq!(
        update(&mut s, key(' ')),
        vec![Effect::Player(PlayerCommand::Pause)]
    );
}

#[test]
fn session_error_during_skip_keeps_playing() {
    let mut s = playing();
    update(&mut s, key('n'));
    update(
        &mut s,
        Action::Api(ApiReply::Failed {
            what: "session",
            error: ApiErrorKind::Other("500".into()),
        }),
    );
    assert_eq!(s.playback, Playback::Playing);
}

#[test]
fn unauthorized_during_skip_pauses_engine() {
    let mut s = playing();
    update(&mut s, key('n'));
    let fx = update(
        &mut s,
        Action::Api(ApiReply::Failed {
            what: "session",
            error: ApiErrorKind::Unauthorized,
        }),
    );
    assert!(fx.contains(&Effect::Player(PlayerCommand::Pause)));
    assert_eq!(s.playback, Playback::Paused);
}

#[test]
fn serving_without_url_does_not_hang() {
    let mut s = ready();
    update(&mut s, key(' '));
    let bad: Serving = serde_json::from_value(json!({
        "track": {"id": "x", "name": "X", "tags": []},
        "trackVariation": {"id": "vx", "lengthInSeconds": 10}
    }))
    .unwrap();
    let fx = {
        let a = reply(&s, Intent::Now, vec![bad]);
        update(&mut s, a)
    };
    assert_eq!(fx, Vec::<Effect>::new());
    assert_eq!(s.playback, Playback::Idle);
}

#[test]
fn stale_servings_reply_ignored() {
    let mut s = playing();
    let first = gen_of(&update(&mut s, key('n')));
    update(&mut s, code(KeyCode::Tab));
    let fx = update(
        &mut s,
        Action::Api(ApiReply::Servings {
            intent: Intent::Now,
            generation: first,
            items: vec![serving("old")],
        }),
    );
    assert_eq!(fx, Vec::<Effect>::new());
    assert_eq!(s.now.as_ref().unwrap().track.id, "a");
}

#[test]
fn next_reply_after_session_end_ignored() {
    let mut s = playing();
    let g = gen_of(&update(
        &mut s,
        Action::Player(PlayerEvent::NearEnd { id: "a".into() }),
    ));
    s.timer = Timer::new(TimerMode::Countdown(Duration::from_secs(1)));
    s.timer.start();
    update(&mut s, Action::Tick(Duration::from_secs(2)));
    let fx = update(
        &mut s,
        Action::Api(ApiReply::Servings {
            intent: Intent::Next,
            generation: g,
            items: vec![serving("b")],
        }),
    );
    assert_eq!(fx, Vec::<Effect>::new());
}

#[test]
fn events_for_unknown_tracks_ignored() {
    let mut s = playing();
    let fx = update(
        &mut s,
        Action::Player(PlayerEvent::Error {
            id: "zzz".into(),
            message: "late".into(),
        }),
    );
    assert_eq!(fx, Vec::<Effect>::new());
    assert_eq!(s.playback, Playback::Playing);
    update(&mut s, key('n'));
    update(
        &mut s,
        Action::Player(PlayerEvent::Started { id: "zzz".into() }),
    );
    assert_eq!(s.playback, Playback::Loading);
}

#[test]
fn queued_track_still_promotes_after_failed_skip() {
    let mut s = playing();
    {
        let a = reply(&s, Intent::Next, vec![serving("b")]);
        update(&mut s, a)
    };
    update(&mut s, key('n'));
    {
        let a = reply(&s, Intent::Now, vec![]);
        update(&mut s, a)
    };
    update(
        &mut s,
        Action::Player(PlayerEvent::Started { id: "b".into() }),
    );
    assert_eq!(s.now.as_ref().unwrap().track.id, "b");
}

#[test]
fn loading_watchdog_gives_up() {
    let mut s = ready();
    let g = gen_of(&update(&mut s, key(' ')));
    update(&mut s, Action::Tick(Duration::from_secs(31)));
    assert_eq!(s.playback, Playback::Idle);
    assert!(s.toast.as_ref().unwrap().text.contains("Timed out"));
    let fx = update(
        &mut s,
        Action::Api(ApiReply::Servings {
            intent: Intent::Now,
            generation: g,
            items: vec![serving("a")],
        }),
    );
    assert_eq!(fx, Vec::<Effect>::new());
}

// --- review round 2: loading/generation ---

fn session_effect(fx: &[Effect], intent: Intent) -> u64 {
    fx.iter()
        .find_map(|e| match e {
            Effect::StartSession {
                intent: i,
                generation,
                ..
            } if *i == intent => Some(*generation),
            _ => None,
        })
        .expect("StartSession")
}

#[test]
fn near_end_during_skip_does_not_drop_the_skip() {
    let mut s = playing();
    let now_gen = session_effect(&update(&mut s, key('n')), Intent::Now);
    let fx = update(
        &mut s,
        Action::Player(PlayerEvent::NearEnd { id: "a".into() }),
    );
    assert!(
        !has_session(&fx, Intent::Next),
        "no prefetch while a skip is loading"
    );
    let fx = update(
        &mut s,
        Action::Api(ApiReply::Servings {
            intent: Intent::Now,
            generation: now_gen,
            items: vec![serving("c")],
        }),
    );
    assert!(matches!(fx.as_slice(), [Effect::Player(PlayerCommand::Load { id, .. })] if id == "c"));
}

#[test]
fn failed_load_after_natural_end_goes_idle() {
    let mut s = playing();
    update(
        &mut s,
        Action::Player(PlayerEvent::Ended { id: "a".into() }),
    );
    assert_eq!(s.playback, Playback::Loading);
    let generation = s.session_gen;
    update(
        &mut s,
        Action::Api(ApiReply::SessionFailed {
            generation,
            error: ApiErrorKind::Other("500".into()),
        }),
    );
    assert_eq!(s.playback, Playback::Idle);
    assert!(
        has_session(&update(&mut s, key(' ')), Intent::Now),
        "space retries"
    );
}

#[test]
fn library_pick_cancels_inflight_skip() {
    let mut s = ready();
    update(
        &mut s,
        Action::Api(ApiReply::Library {
            favorites: vec![serving("f1")],
            recents: vec![],
        }),
    );
    let mut s2 = s.clone();
    s2.panel.visible = true;
    let skip_gen = session_effect(&update(&mut s2, key('n')), Intent::Now);
    s2.loading_for = Duration::from_secs(29);
    update(&mut s2, code(KeyCode::Enter));
    assert_eq!(s2.loading_for, Duration::ZERO);
    let fx = update(
        &mut s2,
        Action::Api(ApiReply::Servings {
            intent: Intent::Now,
            generation: skip_gen,
            items: vec![serving("random")],
        }),
    );
    assert_eq!(
        fx,
        Vec::<Effect>::new(),
        "stale skip must not replace the picked favorite"
    );
    update(
        &mut s2,
        Action::Player(PlayerEvent::Started { id: "f1".into() }),
    );
    assert_eq!(s2.now.as_ref().unwrap().track.id, "f1");
}

#[test]
fn loading_track_is_pending_until_started() {
    let mut s = playing();
    update(&mut s, key('n'));
    {
        let a = reply(&s, Intent::Now, vec![serving("c")]);
        update(&mut s, a)
    };
    assert_eq!(
        s.now.as_ref().unwrap().track.id,
        "a",
        "old track still shown while c loads"
    );
    update(
        &mut s,
        Action::Player(PlayerEvent::Started { id: "c".into() }),
    );
    assert_eq!(s.now.as_ref().unwrap().track.id, "c");
}

#[test]
fn timeout_cancels_engine_load_and_ignores_late_events() {
    let mut s = ready();
    update(&mut s, key(' '));
    {
        let a = reply(&s, Intent::Now, vec![serving("c")]);
        update(&mut s, a)
    };
    let fx = update(&mut s, Action::Tick(Duration::from_secs(31)));
    assert!(fx.contains(&Effect::Player(PlayerCommand::CancelPending)));
    assert_eq!(s.playback, Playback::Idle);
    let errors = s.errors_in_a_row;
    let fx = update(
        &mut s,
        Action::Player(PlayerEvent::Error {
            id: "c".into(),
            message: "late".into(),
        }),
    );
    assert_eq!(fx, Vec::<Effect>::new());
    assert_eq!(s.errors_in_a_row, errors);
    update(
        &mut s,
        Action::Player(PlayerEvent::Started { id: "c".into() }),
    );
    assert_eq!(s.playback, Playback::Idle);
}

#[test]
fn stale_session_failure_ignored() {
    let mut s = playing();
    let next_gen = session_effect(
        &update(
            &mut s,
            Action::Player(PlayerEvent::NearEnd { id: "a".into() }),
        ),
        Intent::Next,
    );
    update(&mut s, key('n'));
    update(
        &mut s,
        Action::Api(ApiReply::SessionFailed {
            generation: next_gen,
            error: ApiErrorKind::Other("boom".into()),
        }),
    );
    assert_eq!(
        s.playback,
        Playback::Loading,
        "the newer skip is still pending"
    );
}

#[test]
fn media_volume_sets_player_volume_and_saves() {
    let mut s = ready();
    let fx = update(&mut s, Action::Media(MediaKey::SetVolume(25)));
    assert_eq!(s.config.volume, 25);
    assert!(fx.contains(&Effect::Player(PlayerCommand::SetVolume(0.25))));
    assert!(fx.contains(&Effect::SaveConfig));
}
