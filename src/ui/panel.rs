//! Right-hand side panel with Library / Explore / Profile tabs.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use super::Theme;
use crate::app::state::{AppState, PanelTab};

pub fn draw(f: &mut Frame, area: Rect, s: &AppState, t: &Theme) {
    let block = Block::new()
        .borders(Borders::LEFT)
        .border_style(Style::new().fg(t.surface_hi))
        .style(Style::new().bg(t.surface));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let [tabs, _, _body] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Fill(1),
    ])
    .areas(inner);

    let mut spans = vec![Span::raw(" ")];
    for tab in PanelTab::ALL {
        let style = if tab == s.panel.tab {
            Style::new()
                .fg(t.text)
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
        } else {
            t.dim()
        };
        spans.push(Span::styled(tab.label(), style));
        spans.push(Span::raw("  "));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), tabs);
}
