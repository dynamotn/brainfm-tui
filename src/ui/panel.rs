//! Right-hand side panel with Library / Explore / Profile tabs.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use super::Theme;
use crate::api::Serving;
use crate::app::state::{AppState, PanelTab};

pub(super) fn draw(f: &mut Frame<'_>, area: Rect, s: &AppState, t: &Theme) {
    let block = Block::new()
        .borders(Borders::LEFT)
        .border_style(Style::new().fg(t.surface_hi))
        .style(Style::new().bg(t.surface));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let [tabs, _, body] = Layout::vertical([
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
    let lines = match s.panel.tab {
        PanelTab::Library => library(s, t, body.width),
        PanelTab::Explore => explore(s, t, body.width),
        PanelTab::Profile => profile(s, t),
    };
    // Keep the selected row on screen.
    let skip = (s.panel.selected + 4).saturating_sub(body.height as usize);
    f.render_widget(Paragraph::new(lines).scroll((super::to_u16(skip), 0)), body);
}

fn row<'a>(text: &str, extra: Option<&str>, selected: bool, t: &Theme, width: u16) -> Line<'a> {
    let marker = if selected { "› " } else { "  " };
    let base = if selected {
        Style::new()
            .fg(t.text)
            .bg(t.surface_hi)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(t.text)
    };
    // Leave room for the right-aligned extra plus a gap of two spaces.
    let reserved = extra.map_or(0, |e| e.chars().count() + 3);
    let text = super::shorten(text, usize::from(width).saturating_sub(3 + reserved));
    let mut spans = vec![Span::styled(format!(" {marker}{text}"), base)];
    if let Some(extra) = extra {
        let used = text.chars().count() + 3;
        let pad = (width as usize).saturating_sub(used + extra.chars().count() + 1);
        spans.push(Span::styled(
            format!("{}{extra} ", " ".repeat(pad)),
            t.dim(),
        ));
    }
    Line::from(spans)
}

fn heading<'a>(text: &str, t: &Theme) -> Line<'a> {
    Line::from(Span::styled(format!(" {text}"), t.accent()))
}

fn servings<'a>(
    items: &[Serving],
    offset: usize,
    s: &AppState,
    t: &Theme,
    width: u16,
) -> Vec<Line<'a>> {
    if items.is_empty() {
        return vec![Line::from(Span::styled("   Nothing here yet", t.dim()))];
    }
    items
        .iter()
        .enumerate()
        .map(|(i, sv)| {
            row(
                &sv.track.name,
                sv.track.genre(),
                s.panel.selected == offset + i,
                t,
                width,
            )
        })
        .collect()
}

fn library<'a>(s: &AppState, t: &Theme, width: u16) -> Vec<Line<'a>> {
    let g = &t.glyphs;
    let mut lines = vec![heading(&format!("{} Favorites", g.favorite_header), t)];
    lines.extend(servings(&s.panel.favorites, 0, s, t, width));
    lines.push(Line::default());
    lines.push(heading(&format!("{} Recent", g.recent_header), t));
    lines.extend(servings(
        &s.panel.recents,
        s.panel.favorites.len(),
        s,
        t,
        width,
    ));
    lines
}

fn explore<'a>(s: &AppState, t: &Theme, width: u16) -> Vec<Line<'a>> {
    let current = s.current_activity_id();
    let mut lines = vec![heading("Activities", t)];
    if s.activities.is_empty() {
        lines.push(Line::from(Span::styled("   loading…", t.dim())));
    }
    for (i, a) in s.activities.iter().enumerate() {
        let mark = if current.as_deref() == Some(a.id.as_str()) {
            Some(t.glyphs.check_on)
        } else {
            None
        };
        lines.push(row(&a.display_value, mark, s.panel.selected == i, t, width));
    }
    lines
}

fn profile<'a>(s: &AppState, t: &Theme) -> Vec<Line<'a>> {
    let config = crate::config::Config::default_path()
        .map_or_else(|_| "unknown".into(), |p| p.display().to_string());
    vec![
        heading("Account", t),
        Line::from(Span::styled(
            format!("   {}", s.user_id.as_deref().unwrap_or("not logged in")),
            Style::new().fg(t.text),
        )),
        Line::default(),
        heading("Config", t),
        Line::from(Span::styled(format!("   {config}"), t.dim())),
        Line::default(),
        Line::from(Span::styled("   brainfm logout  to sign out", t.dim())),
    ]
}
