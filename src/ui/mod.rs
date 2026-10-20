//! Renders [`AppState`] in the layout of the Brain.fm desktop app.

mod footer;
mod header;
mod panel;
mod player;
pub mod theme;

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::widgets::{Block, Paragraph};

use crate::app::state::AppState;
pub use theme::Theme;

pub const MIN_WIDTH: u16 = 60;
pub const MIN_HEIGHT: u16 = 20;
const PANEL_WIDTH: u16 = 34;
const PANEL_MIN_SCREEN: u16 = 100;

pub fn draw(f: &mut Frame, s: &AppState, t: &Theme) {
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
            centered_line(toast, msg.text.chars().count() as u16 + 2),
        );
    }
    footer::draw(f, foot, s, t);
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
pub(crate) mod tests;
