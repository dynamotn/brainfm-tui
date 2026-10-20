//! Top bar: wordmark and mental-state tabs with a gradient underline.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::Theme;
use crate::app::state::AppState;

const LOGO: &str = " brain.fm ";

pub(super) fn draw(f: &mut Frame<'_>, area: Rect, s: &AppState, t: &Theme) {
    let mut tabs = vec![Span::styled(LOGO, t.accent()), Span::raw("   ")];
    let mut underline = vec![Span::raw(" ".repeat(LOGO.len() + 3))];

    for (i, st) in s.states.iter().enumerate() {
        let label = format!(" {} ", st.display_value);
        let width = label.chars().count();
        if i == s.state_idx {
            tabs.push(Span::styled(
                label,
                Style::new().fg(t.text).add_modifier(Modifier::BOLD),
            ));
            for c in 0..width {
                let color = t.gradient(c as f64 / width.max(2).saturating_sub(1) as f64);
                underline.push(Span::styled("▔", Style::new().fg(color)));
            }
        } else {
            tabs.push(Span::styled(label, t.dim()));
            underline.push(Span::raw(" ".repeat(width)));
        }
        tabs.push(Span::raw("  "));
        underline.push(Span::raw("  "));
    }
    if s.states.is_empty() {
        tabs.push(Span::styled("loading…", t.dim()));
    }

    f.render_widget(
        Paragraph::new(vec![Line::from(tabs), Line::from(underline)]),
        area,
    );
}
