//! The centre of the screen: filters, big timer, track card and controls.

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use tui_big_text::{BigText, PixelSize};

use super::Theme;
use crate::api::EffectLevel;
use crate::app::state::{AppState, Playback};
use crate::timer::{Phase, TimerMode, format_clock};

const FULL_TIMER_MIN_HEIGHT: u16 = 26;

pub fn draw(f: &mut Frame, area: Rect, s: &AppState, t: &Theme) {
    let area = area.inner(ratatui::layout::Margin {
        horizontal: 3,
        vertical: 1,
    });
    let big = if area.height >= FULL_TIMER_MIN_HEIGHT {
        8
    } else {
        4
    };
    let [
        filters,
        _,
        timer,
        status,
        chips,
        _,
        name,
        meta,
        _,
        progress,
        controls,
    ] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(big),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(area);

    draw_filters(f, filters, s, t);
    draw_timer(f, timer, s, t, big);
    draw_status(f, status, s, t);
    draw_chips(f, chips, s, t);
    draw_track(f, name, meta, s, t);
    draw_progress(f, progress, s, t);
    draw_controls(f, controls, s, t);
}

fn effect_meter(effects: &[EffectLevel], t: &Theme) -> Vec<Span<'static>> {
    let top = effects.iter().max().copied();
    let filled = match top {
        None => 0,
        Some(EffectLevel::Low) => 1,
        Some(EffectLevel::Medium) => 2,
        Some(EffectLevel::High) => 3,
    };
    let mut spans: Vec<Span> = (0..3)
        .map(|i| {
            if i < filled {
                Span::styled("■", Style::new().fg(t.gradient(f64::from(i) / 2.0)))
            } else {
                Span::styled("□", t.dim())
            }
        })
        .collect();
    let label = if effects.is_empty() {
        "Any".to_owned()
    } else {
        effects
            .iter()
            .map(|e| e.label())
            .collect::<Vec<_>>()
            .join(", ")
    };
    spans.push(Span::styled(format!(" {label}"), Style::new().fg(t.text)));
    spans
}

fn draw_filters(f: &mut Frame, area: Rect, s: &AppState, t: &Theme) {
    let prefs = s.current_prefs();
    let activity = s
        .current_activity_name()
        .unwrap_or_else(|| "Activity".into());
    let genres = if prefs.genres.is_empty() {
        "All genres".to_owned()
    } else {
        prefs.genres.join(", ")
    };
    let mut spans = vec![
        Span::styled(
            format!("{activity} {}", t.glyphs.dropdown),
            Style::new().fg(t.text).add_modifier(Modifier::BOLD),
        ),
        Span::styled("    Neural effect ", t.dim()),
    ];
    spans.extend(effect_meter(&prefs.effects, t));
    spans.push(Span::styled(format!("    {genres}"), t.dim()));
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn draw_timer(f: &mut Frame, area: Rect, s: &AppState, t: &Theme, height: u16) {
    let size = if height >= 8 {
        PixelSize::Full
    } else {
        PixelSize::HalfHeight
    };
    let text = format_clock(s.timer.display());
    let big = BigText::builder()
        .pixel_size(size)
        .style(Style::new().fg(t.gradient(0.35)))
        .lines(vec![Line::from(text)])
        .centered()
        .build();
    f.render_widget(big, area);
}

fn draw_status(f: &mut Frame, area: Rect, s: &AppState, t: &Theme) {
    let rest =
        matches!(s.timer.mode(), TimerMode::Intervals { .. }) && s.timer.phase() == Phase::Rest;
    let (text, style) = match s.playback {
        _ if rest => (
            "Break — music resumes with the next focus block",
            Style::new().fg(t.cyan),
        ),
        Playback::Idle => ("Press space to start", t.dim()),
        Playback::Loading => ("Loading…", t.dim()),
        Playback::Playing => ("", t.dim()),
        Playback::Paused => ("Paused", Style::new().fg(t.orange)),
    };
    f.render_widget(
        Paragraph::new(text)
            .alignment(Alignment::Center)
            .style(style),
        area,
    );
}

fn draw_chips(f: &mut Frame, area: Rect, s: &AppState, t: &Theme) {
    let mode = s.timer.mode();
    let chip = |label: String, on: bool| {
        if on {
            Span::styled(
                format!(" {label} "),
                Style::new()
                    .fg(t.text)
                    .bg(t.surface_hi)
                    .add_modifier(Modifier::BOLD),
            )
        } else {
            Span::styled(format!(" {label} "), t.dim())
        }
    };
    let timer_label = match mode {
        TimerMode::Countdown(d) => format!("Timer {}", format_clock(*d)),
        _ => "Timer".to_owned(),
    };
    let interval_label = match mode {
        TimerMode::Intervals { focus, rest } => {
            format!("Intervals {}/{}", focus.as_secs() / 60, rest.as_secs() / 60)
        }
        _ => "Intervals".to_owned(),
    };
    let sep = Span::styled(format!(" {} ", t.glyphs.dot), t.dim());
    let line = Line::from(vec![
        chip("Infinite".into(), matches!(mode, TimerMode::Infinite)),
        sep.clone(),
        chip(timer_label, matches!(mode, TimerMode::Countdown(_))),
        sep,
        chip(interval_label, matches!(mode, TimerMode::Intervals { .. })),
    ]);
    f.render_widget(Paragraph::new(line).alignment(Alignment::Center), area);
}

fn draw_track(f: &mut Frame, name: Rect, meta: Rect, s: &AppState, t: &Theme) {
    let Some(now) = &s.now else {
        let state = s
            .current_state()
            .map(|st| st.display_value.as_str())
            .unwrap_or("Brain.fm");
        f.render_widget(
            Paragraph::new(format!("{state} music, made for your brain"))
                .alignment(Alignment::Center)
                .style(t.dim()),
            name,
        );
        return;
    };
    let track = &now.track;
    f.render_widget(
        Paragraph::new(track.name.as_str())
            .alignment(Alignment::Center)
            .style(Style::new().fg(t.text).add_modifier(Modifier::BOLD)),
        name,
    );
    let mut parts = Vec::new();
    if let Some(g) = track.genre() {
        parts.push(g.to_owned());
    }
    if let Some(e) = now.track_variation.effect() {
        parts.push(e.label().to_owned());
    }
    let moods = track.moods();
    if !moods.is_empty() {
        parts.push(moods.join(", "));
    }
    f.render_widget(
        Paragraph::new(parts.join(" · "))
            .alignment(Alignment::Center)
            .style(t.dim()),
        meta,
    );
}

fn draw_progress(f: &mut Frame, area: Rect, s: &AppState, t: &Theme) {
    let len = s
        .now
        .as_ref()
        .map(|n| n.track_variation.length())
        .unwrap_or_default();
    let label = format!(" {} / {}", format_clock(s.position), format_clock(len));
    let bar_width = (area.width as usize).saturating_sub(label.len() + 2);
    let ratio = if len.is_zero() {
        0.0
    } else {
        (s.position.as_secs_f64() / len.as_secs_f64()).min(1.0)
    };
    let filled = (ratio * bar_width as f64).round() as usize;
    let mut spans: Vec<Span> = (0..bar_width)
        .map(|i| {
            if i < filled {
                let color = t.gradient(i as f64 / bar_width.max(1) as f64);
                Span::styled(t.glyphs.bar_full, Style::new().fg(color))
            } else {
                Span::styled(t.glyphs.bar_empty, Style::new().fg(t.surface_hi))
            }
        })
        .collect();
    spans.push(Span::styled(label, t.dim()));
    f.render_widget(
        Paragraph::new(Line::from(spans)).alignment(Alignment::Center),
        area,
    );
}

fn draw_controls(f: &mut Frame, area: Rect, s: &AppState, t: &Theme) {
    let g = &t.glyphs;
    let playing = s.playback == Playback::Playing;
    let fav = s
        .now
        .as_ref()
        .is_some_and(|n| s.favorite_ids.contains(&n.track.id));
    let key = |k: &'static str| Span::styled(format!(" {k}   "), t.dim());
    let vol = s.config.volume;
    let bars = usize::from(vol / 10);
    let line = Line::from(vec![
        Span::styled(if playing { g.pause } else { g.play }, t.accent()),
        key("space"),
        Span::styled(g.skip, Style::new().fg(t.text)),
        key("n"),
        Span::styled(
            if fav { g.heart } else { g.heart_empty },
            Style::new().fg(if fav { t.pink } else { t.text }),
        ),
        key("f"),
        Span::styled(g.dislike, Style::new().fg(t.text)),
        key("d"),
        Span::styled(format!("{} ", g.volume), Style::new().fg(t.text)),
        Span::styled("▮".repeat(bars), Style::new().fg(t.purple)),
        Span::styled("▯".repeat(10 - bars), Style::new().fg(t.surface_hi)),
        Span::styled(format!(" {vol}%"), t.dim()),
    ]);
    f.render_widget(Paragraph::new(line).alignment(Alignment::Center), area);
}
