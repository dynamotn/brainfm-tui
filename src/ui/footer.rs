//! Bottom line of key hints.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::Theme;
use crate::app::state::AppState;

const HINTS: &[(&str, &str)] = &[
    ("space", "play"),
    ("n", "skip"),
    ("f", "fav"),
    ("d", "dislike"),
    ("tab", "state"),
    ("a", "activity"),
    ("g", "genres"),
    ("e", "effect"),
    ("t", "timer"),
    ("p", "panel"),
    ("?", "help"),
    ("q", "quit"),
];

pub(super) fn draw(f: &mut Frame<'_>, area: Rect, _s: &AppState, t: &Theme) {
    let mut spans = vec![Span::raw(" ")];
    let mut width = 1;
    for (k, label) in HINTS {
        let w = k.len() + label.len() + 3;
        if width + w > area.width as usize {
            break;
        }
        width += w;
        spans.push(Span::styled(*k, Style::new().fg(t.pink)));
        spans.push(Span::styled(format!(" {label}  "), t.dim()));
    }
    f.render_widget(
        Paragraph::new(Line::from(spans)).style(Style::new().bg(t.surface)),
        area,
    );
}
