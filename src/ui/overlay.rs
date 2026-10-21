//! Modal popovers: activity, genres, neural effect, timer, login and help.

use ratatui::Frame;
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};

use super::Theme;
use crate::api::EffectLevel;
use crate::app::state::{AppState, LoginField, Overlay};

const HELP: &[(&str, &str)] = &[
    ("space", "play / pause"),
    ("n", "skip track"),
    ("f", "favorite"),
    ("d", "dislike and skip"),
    ("tab / 1-4", "mental state"),
    ("a", "activity"),
    ("g", "genres"),
    ("e", "neural effect"),
    ("t", "timer"),
    ("+ / -", "volume"),
    ("p", "toggle panel"),
    ("[ / ]", "panel tab"),
    ("j / k", "move"),
    ("enter", "select"),
    ("esc", "close"),
    ("q", "quit"),
];

fn modal(f: &mut Frame<'_>, title: &str, width: u16, height: u16, t: &Theme) -> Rect {
    let area = f.area();
    let [v] = Layout::vertical([Constraint::Length(height.min(area.height))])
        .flex(Flex::Center)
        .areas(area);
    let [r] = Layout::horizontal([Constraint::Length(width.min(area.width))])
        .flex(Flex::Center)
        .areas(v);
    f.render_widget(Clear, r);
    let block = Block::new()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(t.purple))
        .title(Span::styled(format!(" {title} "), t.accent()))
        .style(Style::new().fg(t.text).bg(t.surface_hi));
    let inner = block.inner(r);
    f.render_widget(block, r);
    inner
}

fn item<'a>(label: String, selected: bool, check: Option<bool>, t: &Theme) -> Line<'a> {
    let mut spans = vec![Span::raw(if selected { " › " } else { "   " })];
    if let Some(on) = check {
        let (g, style) = if on {
            (t.glyphs.check_on, Style::new().fg(t.pink))
        } else {
            (t.glyphs.check_off, t.dim())
        };
        spans.push(Span::styled(format!("{g} "), style));
    }
    let style = if selected {
        Style::new().fg(t.text).add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(t.text_dim)
    };
    spans.push(Span::styled(label, style));
    Line::from(spans)
}

fn hint<'a>(text: &str, t: &Theme) -> Line<'a> {
    Line::from(Span::styled(format!(" {text}"), t.dim()))
}

fn list(
    f: &mut Frame<'_>,
    title: &str,
    mut lines: Vec<Line<'_>>,
    sel: usize,
    footer: &str,
    t: &Theme,
) {
    let height = super::to_u16(lines.len()).saturating_add(4).min(22);
    let inner = modal(f, title, 44, height, t);
    let [body, _, foot] = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(inner);
    let skip = (sel + 1).saturating_sub(body.height as usize);
    lines.drain(..skip.min(lines.len()));
    f.render_widget(Paragraph::new(lines), body);
    f.render_widget(Paragraph::new(hint(footer, t)), foot);
}

pub(super) fn draw(f: &mut Frame<'_>, o: &Overlay, s: &AppState, t: &Theme) {
    match o {
        Overlay::Activity { sel } => {
            let current = s.current_activity_id();
            let lines = s
                .activities
                .iter()
                .enumerate()
                .map(|(i, a)| {
                    item(
                        a.display_value.clone(),
                        i == *sel,
                        Some(current.as_deref() == Some(a.id.as_str())),
                        t,
                    )
                })
                .collect();
            list(f, "Activity", lines, *sel, "enter select · esc close", t);
        }
        Overlay::Genres { sel, chosen } => {
            let lines = s
                .genres
                .iter()
                .enumerate()
                .map(|(i, g)| item(g.clone(), i == *sel, Some(chosen.contains(g)), t))
                .collect();
            list(
                f,
                "Genres",
                lines,
                *sel,
                "space toggle · enter apply · none = all",
                t,
            );
        }
        Overlay::Effect { sel, chosen } => {
            let lines = EffectLevel::ALL
                .iter()
                .enumerate()
                .map(|(i, e)| item(e.label().to_owned(), i == *sel, Some(chosen.contains(e)), t))
                .collect();
            list(
                f,
                "Neural effect",
                lines,
                *sel,
                "space toggle · enter apply",
                t,
            );
        }
        Overlay::Timer { sel } => {
            let lines = s
                .timer_options()
                .into_iter()
                .enumerate()
                .map(|(i, (label, _))| item(label, i == *sel, None, t))
                .collect();
            list(f, "Timer", lines, *sel, "enter select · esc close", t);
        }
        Overlay::Help => draw_help(f, t),
        Overlay::Login {
            email,
            password,
            field,
            error,
        } => draw_login(f, email, password.char_count(), *field, error.as_deref(), t),
    }
}

fn draw_help(f: &mut Frame<'_>, t: &Theme) {
    let lines: Vec<Line<'_>> = HELP
        .iter()
        .map(|(k, v)| {
            Line::from(vec![
                Span::styled(format!("  {k:<11}"), Style::new().fg(t.pink)),
                Span::styled(*v, Style::new().fg(t.text)),
            ])
        })
        .collect();
    let height = super::to_u16(lines.len()).saturating_add(2);
    let inner = modal(f, "Keys", 40, height, t);
    f.render_widget(Paragraph::new(lines), inner);
}

fn draw_login(
    f: &mut Frame<'_>,
    email: &str,
    password_len: usize,
    field: LoginField,
    error: Option<&str>,
    t: &Theme,
) {
    let inner = modal(f, "Log in to Brain.fm", 56, 11, t);
    let input = |label: &str, value: String, active: bool| {
        let style = if active {
            Style::new()
                .fg(t.text)
                .bg(t.surface)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::new().fg(t.text_dim).bg(t.surface)
        };
        let cursor = if active { "▏" } else { " " };
        vec![
            Line::from(Span::styled(format!(" {label}"), t.dim())),
            Line::from(Span::styled(format!(" {value}{cursor:<40}"), style)),
        ]
    };
    let mut lines = input("Email", email.to_owned(), field == LoginField::Email);
    lines.extend(input(
        "Password",
        "•".repeat(password_len),
        field == LoginField::Password,
    ));
    lines.push(Line::default());
    lines.push(match error {
        Some(e) => Line::from(Span::styled(format!(" {e}"), Style::new().fg(t.orange))),
        None => Line::default(),
    });
    lines.push(hint("tab switch field · enter log in", t));
    lines.push(hint("Google/Apple account? run `brainfm login --token`", t));
    f.render_widget(Paragraph::new(lines), inner);
}
