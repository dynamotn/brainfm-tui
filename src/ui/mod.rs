//! Renders [`AppState`] in the layout of the Brain.fm desktop app.

mod footer;
mod header;
mod overlay;
mod panel;
mod player;
pub(crate) mod theme;

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::widgets::{Block, Paragraph};

use crate::app::state::AppState;
pub(crate) use theme::Theme;

pub(crate) const MIN_WIDTH: u16 = 60;
pub(crate) const MIN_HEIGHT: u16 = 20;
const PANEL_WIDTH: u16 = 34;
const PANEL_MIN_SCREEN: u16 = 100;

pub(crate) fn draw(f: &mut Frame<'_>, s: &AppState, t: &Theme) {
    let area = f.area();
    f.render_widget(Block::new().style(t.base()), area);

    if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
        let msg = format!("Terminal too small (need {MIN_WIDTH}×{MIN_HEIGHT})");
        let [_, mid, _] = Layout::vertical([
            Constraint::Fill(1),
            Constraint::Length(1),
            Constraint::Fill(1),
        ])
        .areas(area);
        f.render_widget(
            Paragraph::new(msg)
                .alignment(Alignment::Center)
                .style(t.dim()),
            mid,
        );
        return;
    }

    let [head, body, toast, foot] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Fill(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(area);

    header::draw(f, head, s, t);

    let show_panel = s.panel.visible && area.width >= PANEL_MIN_SCREEN;
    let main = if show_panel {
        let [main, side] =
            Layout::horizontal([Constraint::Fill(1), Constraint::Length(PANEL_WIDTH)]).areas(body);
        panel::draw(f, side, s, t);
        main
    } else {
        body
    };
    player::draw(f, main, s, t);

    if let Some(msg) = &s.toast {
        f.render_widget(
            Paragraph::new(format!(" {} ", msg.text))
                .alignment(Alignment::Center)
                .style(Style::new().fg(t.text).bg(t.surface_hi)),
            centered_line(toast, to_u16(msg.text.chars().count()).saturating_add(2)),
        );
    }
    footer::draw(f, foot, s, t);

    if let Some(o) = &s.overlay {
        overlay::draw(f, o, s, t);
    }
}

/// At most `max` characters, ending in `…` when something was cut.
pub(crate) fn shorten(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    if max == 0 {
        return String::new();
    }
    let mut out: String = text.chars().take(max - 1).collect();
    out.push('…');
    out
}

/// `a, b, c` if it fits in `max` characters, else as many items as fit
/// followed by `+N` for the rest, never cutting an item in half.
pub(crate) fn fit_list(items: &[String], max: usize) -> String {
    let all = items.join(", ");
    if all.chars().count() <= max {
        return all;
    }
    for shown in (1..items.len()).rev() {
        let candidate = format!("{} +{}", items[..shown].join(", "), items.len() - shown);
        if candidate.chars().count() <= max {
            return candidate;
        }
    }
    shorten(&format!("{} genres", items.len()), max)
}

/// Terminal sizes fit in `u16`; anything longer is clipped to the screen anyway.
fn to_u16(n: usize) -> u16 {
    u16::try_from(n).unwrap_or(u16::MAX)
}

fn centered_line(area: Rect, width: u16) -> Rect {
    let w = width.min(area.width);
    Rect {
        x: area.x + (area.width - w) / 2,
        width: w,
        ..area
    }
}

#[cfg(test)]
mod screenshots;
#[cfg(test)]
pub(crate) mod tests;
