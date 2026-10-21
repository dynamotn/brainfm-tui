//! Renders README screenshots from sample data with the real UI code and the
//! truecolor palette: `cargo test screenshots -- --ignored`, then
//! `scripts/screenshots.sh` turns the SVGs into PNGs.

use std::fmt::Write as _;
use std::time::Duration;

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::style::{Color, Modifier};
use serde_json::json;

use super::{Theme, draw};
use crate::api::{Activity, EffectLevel, MentalState, Serving};
use crate::app::state::{AppState, Overlay, PanelTab};
use crate::app::tests::reply;
use crate::app::update::{Action, ApiReply, Intent, update};
use crate::config::{Config, StatePrefs};
use crate::player::PlayerEvent;
use crate::timer::{Timer, TimerMode};

// Iosevka advances 0.5em; matching it keeps text tight and blocks aligned.
const CELL_W: f64 = 8.5;
const CELL_H: f64 = 21.0;
const FONT_SIZE: f64 = 17.0;
const PAD: f64 = 22.0;
const TITLE_BAR: f64 = 34.0;
const FONT: &str = "'Iosevka Dynamo', 'Iosevka', 'JetBrains Mono', monospace";

fn track(id: &str, name: &str, genre: &str, moods: &[&str], level: f64) -> Serving {
    let mut tags = vec![json!({"type": "genre", "value": genre})];
    tags.extend(moods.iter().map(|m| json!({"type": "mood", "value": m})));
    serde_json::from_value(json!({
        "track": {"id": id, "name": name, "tags": tags},
        "trackVariation": {"id": format!("v{id}"), "tokenedUrl": "https://cdn/x.mp3",
                           "lengthInSeconds": 478, "neuralEffectLevel": level}
    }))
    .expect("sample serving")
}

fn sample(state_idx: usize) -> AppState {
    let mut config = Config::default();
    config.states.insert(
        "focus".into(),
        StatePrefs {
            activity_id: Some("deep".into()),
            genres: vec!["Electronic".into(), "Piano".into()],
            effects: vec![EffectLevel::High],
        },
    );
    let mut s = AppState::new(config, true, Some("demo".into()));
    s.states = ["Focus", "Relax", "Sleep", "Meditate"]
        .iter()
        .map(|n| MentalState {
            id: n.to_lowercase(),
            display_value: (*n).to_owned(),
            default_activity: None,
        })
        .collect();
    s.state_idx = state_idx;
    s.activities = [
        ("deep", "Deep Work"),
        ("creative", "Creativity"),
        ("learn", "Learning"),
        ("light", "Light Work"),
        ("motivation", "Motivation"),
    ]
    .iter()
    .map(|(id, name)| Activity {
        id: (*id).into(),
        display_value: (*name).into(),
    })
    .collect();
    s.genres = [
        "Electronic",
        "Piano",
        "Lofi",
        "Acoustic",
        "Cinematic",
        "Post Rock",
        "Grooves",
        "Atmospheric",
        "Rain",
    ]
    .iter()
    .map(|g| (*g).to_owned())
    .collect();
    update(
        &mut s,
        Action::Api(ApiReply::Library {
            favorites: vec![
                track("f1", "Lumen", "Piano", &["Calm"], 0.5),
                track("f2", "Night Drive", "Electronic", &["Upbeat"], 0.9),
                track("f3", "Paper Boats", "Acoustic", &["Bright"], 0.3),
                track("a", "Ascent", "Electronic", &["Upbeat", "Bright"], 0.9),
            ],
            recents: vec![
                track("r1", "Drift", "Lofi", &["Chill"], 0.6),
                track("r2", "Tidal", "Atmospheric", &["Dreamy"], 0.4),
                track("r3", "Copper Sky", "Post Rock", &["Epic"], 0.8),
                track("r4", "Slow Bloom", "Cinematic", &["Warm"], 0.5),
            ],
        }),
    );
    s
}

fn playing(state_idx: usize, now: Serving, timer: TimerMode, elapsed: Duration) -> AppState {
    let mut s = sample(state_idx);
    let id = now.track.id.clone();
    let a = reply(&s, Intent::Now, vec![now]);
    update(&mut s, a);
    update(
        &mut s,
        Action::Player(PlayerEvent::Started { id: id.clone() }),
    );
    update(
        &mut s,
        Action::Player(PlayerEvent::Position {
            id,
            pos: Duration::from_secs(192),
        }),
    );
    s.timer = Timer::new(timer);
    s.timer.start();
    s.timer.tick(elapsed);
    s
}

fn ascent() -> Serving {
    track("a", "Ascent", "Electronic", &["Upbeat", "Bright"], 0.9)
}

/// A Relax session: its own activities, filters and a calm track.
fn relax() -> AppState {
    let now = track("r4", "Slow Bloom", "Cinematic", &["Warm", "Calm"], 0.2);
    let mut s = playing(
        1,
        now,
        TimerMode::Infinite,
        Duration::from_secs(41 * 60 + 7),
    );
    s.activities = [
        ("chill", "Chill"),
        ("recharge", "Recharge"),
        ("unwind", "Unwind"),
        ("destress", "Destress"),
    ]
    .iter()
    .map(|(id, name)| Activity {
        id: (*id).into(),
        display_value: (*name).into(),
    })
    .collect();
    s.config.states.insert(
        "relax".into(),
        StatePrefs {
            activity_id: Some("unwind".into()),
            genres: vec!["Piano".into(), "Atmospheric".into()],
            effects: vec![EffectLevel::Low],
        },
    );
    s.panel.tab = PanelTab::Explore;
    s.panel.selected = 1;
    s.playback = crate::app::state::Playback::Paused;
    s
}

fn hex(c: Color, fallback: &str) -> String {
    match c {
        Color::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
        _ => fallback.to_owned(),
    }
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Block and line elements are drawn as rectangles (top, height as a
/// fraction of the cell) so they tile without font gaps.
fn block_rect(sym: &str) -> Option<(f64, f64)> {
    match sym {
        "█" => Some((0.0, 1.0)),
        "▀" => Some((0.0, 0.5)),
        "▄" => Some((0.5, 0.5)),
        "▔" => Some((0.0, 0.14)),
        "━" => Some((0.44, 0.14)),
        "─" => Some((0.48, 0.05)),
        _ => None,
    }
}

/// A terminal window (title bar and rounded frame) around the rendered buffer.
fn to_svg(buf: &Buffer, title: &str, theme: &Theme) -> String {
    let bg = hex(theme.bg, "#0d0c11");
    let fg = hex(theme.text, "#ffffff");
    let w = f64::from(buf.area.width) * CELL_W + PAD * 2.0;
    let h = f64::from(buf.area.height) * CELL_H + PAD * 2.0 + TITLE_BAR;
    let mut out = String::new();
    let _ = write!(
        out,
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="0 0 {w} {h}">
<defs><linearGradient id="edge" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="#f34675" stop-opacity="0.55"/><stop offset="1" stop-color="#9130bb" stop-opacity="0.55"/></linearGradient></defs>
<rect x="0.5" y="0.5" width="{w1}" height="{h1}" rx="14" fill="{bg}" stroke="url(#edge)"/>
<rect x="1" y="1" width="{w2}" height="{tb}" rx="13" fill="#1d1926"/>
<rect x="1" y="{tb2}" width="{w2}" height="14" fill="#1d1926"/>
<circle cx="22" cy="18" r="6" fill="#ff5f57"/><circle cx="42" cy="18" r="6" fill="#febc2e"/><circle cx="62" cy="18" r="6" fill="#28c840"/>
<text x="{cx}" y="22.5" fill="#d4d2ea" font-family={font:?} font-size="13" text-anchor="middle">{title}</text>
<g font-family={font:?} font-size="{FONT_SIZE}" xml:space="preserve">
"##,
        w1 = w - 1.0,
        h1 = h - 1.0,
        w2 = w - 2.0,
        tb = TITLE_BAR,
        tb2 = TITLE_BAR - 13.0,
        cx = w / 2.0,
        font = FONT,
        title = escape(title),
    );
    let ox = PAD;
    let oy = PAD + TITLE_BAR;
    for y in 0..buf.area.height {
        for x in 0..buf.area.width {
            let cell = &buf[(x, y)];
            let (cx, cy) = (ox + f64::from(x) * CELL_W, oy + f64::from(y) * CELL_H);
            let cell_bg = hex(cell.bg, &bg);
            if cell_bg != bg {
                let _ = writeln!(
                    out,
                    r#"<rect x="{cx}" y="{cy}" width="{CELL_W}" height="{CELL_H}" fill="{cell_bg}"/>"#
                );
            }
            let sym = cell.symbol();
            if sym.trim().is_empty() {
                continue;
            }
            let color = hex(cell.fg, &fg);
            if let Some((top, frac)) = block_rect(sym) {
                let _ = writeln!(
                    out,
                    r#"<rect x="{cx}" y="{}" width="{}" height="{}" fill="{color}"/>"#,
                    cy + top * CELL_H,
                    CELL_W + 0.4,
                    frac * CELL_H + 0.4,
                );
                continue;
            }
            let weight = if cell.modifier.contains(Modifier::BOLD) {
                r#" font-weight="700""#
            } else {
                ""
            };
            let deco = if cell.modifier.contains(Modifier::UNDERLINED) {
                r#" text-decoration="underline""#
            } else {
                ""
            };
            let _ = writeln!(
                out,
                r#"<text x="{}" y="{}" fill="{color}"{weight}{deco}>{}</text>"#,
                cx,
                cy + CELL_H * 0.76,
                escape(sym),
            );
        }
    }
    out.push_str("</g>\n</svg>\n");
    out
}

fn shoot(name: &str, title: &str, s: &AppState, w: u16, h: u16) {
    let theme = Theme::truecolor();
    let mut term = Terminal::new(TestBackend::new(w, h)).expect("test backend");
    term.draw(|f| draw(f, s, &theme)).expect("draw");
    let svg = to_svg(term.backend().buffer(), title, &theme);
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/screenshots");
    std::fs::create_dir_all(&dir).expect("screenshot dir");
    std::fs::write(dir.join(format!("{name}.svg")), svg).expect("write svg");
}

#[test]
#[ignore = "writes docs/screenshots; run on demand"]
fn screenshots() {
    let mut main = playing(
        0,
        ascent(),
        TimerMode::Intervals {
            focus: Duration::from_mins(25),
            rest: Duration::from_mins(5),
        },
        Duration::from_secs(6 * 60 + 18),
    );
    main.favorite_ids.insert("a".into());
    shoot("player", "brainfm — Focus", &main, 132, 34);

    let mut genres = playing(
        0,
        ascent(),
        TimerMode::Countdown(Duration::from_hours(1)),
        Duration::from_secs(754),
    );
    genres.overlay = Some(Overlay::Genres {
        sel: 2,
        chosen: ["Electronic".to_owned(), "Piano".to_owned()].into(),
    });
    shoot("filters", "brainfm — pick genres", &genres, 132, 34);

    shoot("explore", "brainfm — Relax", &relax(), 132, 34);

    let compact = playing(
        0,
        ascent(),
        TimerMode::Countdown(Duration::from_mins(30)),
        Duration::from_mins(9),
    );
    shoot("compact", "brainfm — 80×24", &compact, 80, 24);
}

/// Screenshots of a real session: start `brainfm`, play something, then run
/// `scripts/screenshots.sh --live`. Only a local copy of the state is
/// changed (popups, panel tab); the running session is left alone, and the
/// Profile tab (which shows the account id) is never captured.
#[tokio::test]
#[ignore = "attaches to a running brainfm; run on demand"]
async fn live_screenshots() {
    use crate::share::{Attach, attach, socket_path};
    let Attach::Session(client) = attach(&socket_path().expect("socket path"))
        .await
        .expect("socket")
    else {
        panic!("start brainfm and play something first");
    };
    let (first, _, _) = client.split();
    let mut live = first.expect("a state from the running brainfm");
    live.overlay = None;
    live.toast = None;
    live.panel.visible = true;
    live.panel.tab = PanelTab::Library;
    live.panel.selected = 0;
    let state = live.current_state().map_or_else(
        || "brainfm".to_owned(),
        |s| format!("brainfm — {}", s.display_value),
    );
    shoot("player", &state, &live, 132, 34);

    let mut filters = live.clone();
    let chosen = filters.current_prefs().genres.into_iter().collect();
    filters.overlay = Some(Overlay::Genres { sel: 0, chosen });
    shoot("filters", "brainfm — pick genres", &filters, 132, 34);

    let mut explore = live.clone();
    explore.panel.tab = PanelTab::Explore;
    shoot("explore", &format!("{state} · Explore"), &explore, 132, 34);

    shoot("compact", "brainfm — 80×24", &live, 80, 24);
}
