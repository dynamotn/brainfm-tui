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

#[test]
fn long_track_names_are_shortened_before_the_genre_column() {
    let mut s = playing();
    s.panel.recents = vec![serving("r9", "Missa Ecce Ancilla Domine Sanctus Agnus")];
    let out = text(&render(&s, 140, 40));
    let line = out
        .lines()
        .find(|l| l.contains("Missa"))
        .expect("the long track is listed");
    assert!(line.contains('…'), "{line}");
    assert!(
        line.contains(" Electronic"),
        "genre keeps its column: {line}"
    );
}

#[test]
fn many_genres_collapse_into_a_count_instead_of_being_cut() {
    let mut s = playing();
    let id = s.current_state().unwrap().id.clone();
    let genres: Vec<String> = [
        "Beach",
        "Classical",
        "Forest",
        "Lofi",
        "Piano",
        "Rain",
        "Rainforest",
        "Thunder",
        "Underwater",
    ]
    .iter()
    .map(|g| (*g).to_owned())
    .collect();
    s.config.states.entry(id).or_default().genres = genres;
    let out = text(&render(&s, 100, 30));
    let line = out.lines().find(|l| l.contains("Neural effect")).unwrap();
    assert!(line.contains("Beach"), "{line}");
    assert!(line.contains('+'), "the rest is summarized: {line}");
    assert!(
        !line.contains("Rainfor") || line.contains("Rainforest"),
        "no word cut in half: {line}"
    );
}

#[test]
fn shorten_keeps_short_text_and_marks_long_text() {
    assert_eq!(super::shorten("Lumen", 10), "Lumen");
    assert_eq!(super::shorten("Missa Ecce Ancilla", 10), "Missa Ecc…");
    assert_eq!(super::shorten("abc", 0), "");
}

#[test]
fn long_track_details_end_with_an_ellipsis_on_narrow_terminals() {
    let mut s = playing();
    let long = serde_json::from_value::<Serving>(json!({
        "track": {"id": "a", "name": "Zephyrline", "tags": [
            {"type": "genre", "value": "Classical"},
            {"type": "mood", "value": "Driving"}, {"type": "mood", "value": "Energizing"},
            {"type": "mood", "value": "Inspiring"}, {"type": "mood", "value": "Optimistic"},
            {"type": "mood", "value": "Upbeat"}, {"type": "mood", "value": "Uplifting"}
        ]},
        "trackVariation": {"id": "v", "tokenedUrl": "https://cdn/x.mp3",
                           "lengthInSeconds": 1800, "neuralEffectLevel": 0.2}
    }))
    .unwrap();
    s.now = Some(long);
    let out = text(&render(&s, 80, 24));
    let line = out.lines().find(|l| l.contains("Classical")).unwrap();
    assert!(line.contains('…'), "{line}");
}
