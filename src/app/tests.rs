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
    update(
        &mut s,
        Action::Api(ApiReply::Servings {
            intent: Intent::Now,
            items: vec![serving("a")],
        }),
    );
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
    let fx = update(
        &mut s,
        Action::Api(ApiReply::Servings {
            intent: Intent::Now,
            items: vec![serving("a")],
        }),
    );
    assert!(fx.contains(&Effect::Player(PlayerCommand::Load {
        id: "a".into(),
        url: "https://cdn/a.mp3".into(),
        length: Duration::from_secs(300),
    })));
    assert_eq!(s.now.as_ref().unwrap().track.id, "a");
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
    let fx = update(
        &mut s,
        Action::Api(ApiReply::Servings {
            intent: Intent::Next,
            items: vec![serving("b")],
        }),
    );
    assert!(
        matches!(fx.as_slice(), [Effect::Player(PlayerCommand::Queue { id, .. })] if id == "b")
    );
}

#[test]
fn ended_promotes_next_to_now() {
    let mut s = playing();
    update(
        &mut s,
        Action::Api(ApiReply::Servings {
            intent: Intent::Next,
            items: vec![serving("b")],
        }),
    );
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
    let fx = update(
        &mut s,
        Action::Api(ApiReply::Servings {
            intent: Intent::Now,
            items: vec![],
        }),
    );
    assert!(fx.is_empty());
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
        TimerMode::Countdown(Duration::from_secs(30 * 60))
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
