use ratatui::Terminal;
use ratatui::backend::TestBackend;
use serde_json::json;

use super::*;
use crate::api::{Activity, MentalState, Serving};
use crate::app::state::AppState;
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
    update(
        &mut s,
        Action::Api(ApiReply::Servings {
            intent: Intent::Now,
            items: vec![serving("a", "Ascent")],
        }),
    );
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
