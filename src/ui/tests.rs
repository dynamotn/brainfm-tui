use ratatui::Terminal;
use ratatui::backend::TestBackend;
use serde_json::json;

use super::*;
use crate::api::{Activity, MentalState, Serving};
use crate::app::state::AppState;
use crate::app::tests::reply;
use crate::app::update::{Action, ApiReply, Intent, update};
use crate::config::Config;
use crate::player::PlayerEvent;

fn serving(id: &str, name: &str) -> Serving {
    serde_json::from_value(json!({
        "track": {"id": id, "name": name, "tags": [
            {"type": "genre", "value": "Electronic"},
            {"type": "mood", "value": "Upbeat"},
            {"type": "mood", "value": "Bright"}
        ]},
        "trackVariation": {"id": "v", "tokenedUrl": "https://cdn/x.mp3",
                           "lengthInSeconds": 478, "neuralEffectLevel": 0.9}
    }))
    .unwrap()
}

pub(crate) fn ready() -> AppState {
    let mut s = AppState::new(Config::default(), true, Some("u1".into()));
    s.states = ["Focus", "Relax", "Sleep", "Meditate"]
        .iter()
        .map(|n| MentalState {
            id: n.to_lowercase(),
            display_value: n.to_string(),
            default_activity: None,
        })
        .collect();
    s.activities = vec![
        Activity {
            id: "deep".into(),
            display_value: "Deep Work".into(),
        },
        Activity {
            id: "learn".into(),
            display_value: "Learning".into(),
        },
    ];
    s.genres = vec!["Piano".into(), "Electronic".into(), "Lofi".into()];
    s
}

pub(crate) fn playing() -> AppState {
    let mut s = ready();
    update(
        &mut s,
        Action::Api(ApiReply::Library {
            favorites: vec![serving("f1", "Lumen")],
            recents: vec![serving("r1", "Drift")],
        }),
    );
    {
        let a = reply(&s, Intent::Now, vec![serving("a", "Ascent")]);
        update(&mut s, a)
    };
    update(
        &mut s,
        Action::Player(PlayerEvent::Started { id: "a".into() }),
    );
    update(
        &mut s,
        Action::Player(PlayerEvent::Position {
            id: "a".into(),
            pos: std::time::Duration::from_secs(192),
        }),
    );
    s
}

pub(crate) fn render(s: &AppState, w: u16, h: u16) -> Terminal<TestBackend> {
    let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
    let theme = Theme::ascii();
    t.draw(|f| draw(f, s, &theme)).unwrap();
    t
}

pub(crate) fn text(t: &Terminal<TestBackend>) -> String {
    t.backend().to_string()
}

#[test]
fn renders_idle_80x24() {
    let t = render(&ready(), 80, 24);
    let out = text(&t);
    assert!(out.contains("brain.fm"));
    assert!(out.contains("Focus") && out.contains("Meditate"));
    assert!(out.contains("Deep Work"));
    assert!(out.contains("space"));
    insta::assert_snapshot!(t.backend());
}

#[test]
fn renders_playing_140x40() {
    let t = render(&playing(), 140, 40);
    let out = text(&t);
    assert!(out.contains("Ascent"));
    assert!(out.contains("Electronic · High · Upbeat, Bright"));
    assert!(out.contains("03:12 / 07:58"));
    assert!(out.contains("Library"));
    insta::assert_snapshot!(t.backend());
}

#[test]
fn panel_hidden_when_narrow() {
    let out = text(&render(&playing(), 90, 30));
    assert!(!out.contains("Library"));
}

#[test]
fn renders_too_small_message() {
    let out = text(&render(&playing(), 40, 10));
    assert!(out.contains("Terminal too small"), "{out}");
}

#[test]
fn renders_toast() {
    let mut s = ready();
    s.toast = Some(crate::app::state::Toast::new(
        "No tracks match these filters",
    ));
    assert!(text(&render(&s, 80, 24)).contains("No tracks match these filters"));
}

#[test]
fn renders_library_panel() {
    let out = text(&render(&playing(), 140, 40));
    assert!(out.contains("Favorites") && out.contains("Lumen"));
    assert!(out.contains("Recent") && out.contains("Drift"));
}

#[test]
fn renders_explore_panel_activities() {
    let mut s = playing();
    s.panel.tab = crate::app::state::PanelTab::Explore;
    let out = text(&render(&s, 140, 40));
    assert!(out.contains("Learning"));
}

#[test]
fn renders_genres_overlay() {
    let mut s = ready();
    update(
        &mut s,
        Action::Key(crossterm::event::KeyEvent::from(
            crossterm::event::KeyCode::Char('g'),
        )),
    );
    let t = render(&s, 100, 30);
    let out = text(&t);
    assert!(out.contains("Genres") && out.contains("Piano") && out.contains("Lofi"));
    insta::assert_snapshot!(t.backend());
}

#[test]
fn renders_login_overlay_masks_password() {
    let mut s = AppState::new(Config::default(), true, None);
    if let Some(crate::app::state::Overlay::Login {
        email, password, ..
    }) = &mut s.overlay
    {
        *email = "me@example.com".into();
        *password = "hunter2".into();
    }
    let t = render(&s, 100, 30);
    let out = text(&t);
    assert!(out.contains("me@example.com"));
    assert!(!out.contains("hunter2"));
    assert!(out.contains("•••••••"));
    insta::assert_snapshot!(t.backend());
}

#[test]
fn renders_help_overlay() {
    let mut s = ready();
    s.overlay = Some(crate::app::state::Overlay::Help);
    let out = text(&render(&s, 100, 30));
    assert!(out.contains("Keys") && out.contains("dislike"));
}

#[test]
fn renders_timer_overlay() {
    let mut s = ready();
    s.overlay = Some(crate::app::state::Overlay::Timer { sel: 1 });
    let out = text(&render(&s, 100, 30));
    assert!(out.contains("30 min") && out.contains("Intervals 25/5"));
}

#[test]
fn renders_with_out_of_range_volume() {
    let mut s = ready();
    s.config.volume = 150;
    render(&s, 80, 24);
}

#[test]
fn state_round_trips_through_the_wire_and_renders_the_same() {
    let mut s = playing();
    s.overlay = Some(crate::app::state::Overlay::Timer { sel: 2 });
    let wire = serde_json::to_string(&s).unwrap();
    let back: AppState = serde_json::from_str(&wire).unwrap();
    assert_eq!(text(&render(&s, 140, 40)), text(&render(&back, 140, 40)));
}

#[test]
fn the_wire_never_carries_the_password() {
    let mut s = AppState::new(Config::default(), true, None);
    if let Some(crate::app::state::Overlay::Login { password, .. }) = &mut s.overlay {
        *password = "hunter2".into();
    }
    let wire = serde_json::to_string(&s).unwrap();
    assert!(!wire.contains("hunter2"), "{wire}");
    let back: AppState = serde_json::from_str(&wire).unwrap();
    assert_eq!(
        text(&render(&s, 100, 30)),
        text(&render(&back, 100, 30)),
        "same mask length"
    );
}
