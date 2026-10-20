//! `brainfm play`: plays a mental state without the TUI.

use std::io::Write;
use std::time::Duration;

use anyhow::{Context, bail};

use crate::api::{ApiClient, EffectLevel, Serving, SessionRequest};
use crate::cli::{PlayArgs, client_from_store, resolve_activity, resolve_state};
use crate::config::{Config, StatePrefs};
use crate::player::{self, PlayerCommand, PlayerEvent};
use crate::session::SessionStore;
use crate::timer::{Phase, Timer, TimerEvent, TimerMode, format_clock, parse_intervals};

const TICK: Duration = Duration::from_millis(250);
const MAX_CONSECUTIVE_ERRORS: u32 = 3;

/// Command-line filters win over the saved per-state preferences.
pub fn build_request(
    activity_id: &str,
    effect: Option<EffectLevel>,
    genres: &[String],
    prefs: &StatePrefs,
) -> SessionRequest {
    let effects = effect
        .map(|e| vec![e])
        .unwrap_or_else(|| prefs.effects.clone());
    let genres = if genres.is_empty() {
        prefs.genres.clone()
    } else {
        genres.to_vec()
    };
    SessionRequest::new(activity_id, effects, genres)
}

async fn next_serving(
    client: &ApiClient,
    uid: &str,
    req: &SessionRequest,
) -> anyhow::Result<Serving> {
    client
        .start_session(uid, req)
        .await?
        .into_iter()
        .next()
        .context("No tracks match these filters")
}

fn command_for(s: &Serving, queue: bool) -> anyhow::Result<PlayerCommand> {
    let v = &s.track_variation;
    let url = v.stream_url().context("track has no stream url")?;
    let (id, length) = (s.track.id.clone(), v.length());
    Ok(if queue {
        PlayerCommand::Queue { id, url, length }
    } else {
        PlayerCommand::Load { id, url, length }
    })
}

fn status_line(s: &Serving, pos: Duration, timer: &Timer, paused: bool) -> String {
    let t = &s.track;
    let effect = s
        .track_variation
        .effect()
        .map(EffectLevel::label)
        .unwrap_or("—");
    let icon = if paused { "⏸" } else { "▶" };
    let phase = match (timer.mode(), timer.phase()) {
        (TimerMode::Intervals { .. }, Phase::Rest) => " (break)",
        _ => "",
    };
    format!(
        "{icon} {} · {} · {effect}  {}/{}  ⏱ {}{phase}",
        t.name,
        t.genre().unwrap_or("—"),
        format_clock(pos),
        format_clock(s.track_variation.length()),
        format_clock(timer.display()),
    )
}

pub async fn play(store: &SessionStore, args: PlayArgs) -> anyhow::Result<()> {
    let (client, session) = client_from_store(store)?;
    let config = Config::load(&Config::default_path()?)?;

    let states = client.mental_states().await?;
    let state = resolve_state(&states, &args.state)
        .with_context(|| format!("unknown mental state {:?}", args.state))?;
    let activities = client.activities(&state.id).await?;
    let prefs = config.states.get(&state.id).cloned().unwrap_or_default();
    let activity_id = match &args.activity {
        Some(q) => resolve_activity(&activities, q)
            .with_context(|| format!("unknown activity {q:?}"))?
            .id
            .clone(),
        None => prefs
            .activity_id
            .clone()
            .or_else(|| state.default_activity.as_ref().map(|a| a.id.clone()))
            .or_else(|| activities.first().map(|a| a.id.clone()))
            .context("this mental state has no activities")?,
    };
    let req = build_request(
        &activity_id,
        args.effect.map(Into::into),
        &args.genre,
        &prefs,
    );

    let mode = match (&args.timer, &args.intervals) {
        (Some(d), _) => TimerMode::Countdown((*d).into()),
        (None, Some(s)) => parse_intervals(s)?,
        (None, None) => TimerMode::Infinite,
    };
    let mut timer = Timer::new(mode);

    let (player, mut events) = player::spawn(tokio::runtime::Handle::current())?;
    player.send(PlayerCommand::SetVolume(f32::from(config.volume) / 100.0));

    let mut now = next_serving(&client, &session.user_id, &req).await?;
    player.send(command_for(&now, false)?);
    let mut next: Option<Serving> = None;
    let mut pos = Duration::ZERO;
    let mut paused = false;
    let mut errors = 0u32;
    let mut ticker = tokio::time::interval(TICK);
    let mut out = std::io::stderr();

    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                player.send(PlayerCommand::Stop { fade: Duration::from_secs(1) });
                break;
            }
            Some(ev) = events.recv() => match ev {
                PlayerEvent::Started { id } => {
                    if next.as_ref().is_some_and(|n| n.track.id == id) {
                        now = next.take().expect("checked");
                    }
                    pos = Duration::ZERO;
                    errors = 0;
                    timer.start();
                }
                PlayerEvent::Position { pos: p, .. } => pos = p,
                PlayerEvent::NearEnd { .. } => {
                    let s = next_serving(&client, &session.user_id, &req).await?;
                    player.send(command_for(&s, true)?);
                    next = Some(s);
                }
                PlayerEvent::Ended { .. } => {
                    if next.is_none() {
                        now = next_serving(&client, &session.user_id, &req).await?;
                        player.send(command_for(&now, false)?);
                    }
                }
                PlayerEvent::Error { message, .. } => {
                    errors += 1;
                    if errors >= MAX_CONSECUTIVE_ERRORS {
                        bail!("{message} (giving up after {errors} failed tracks)");
                    }
                    writeln!(out, "\r\x1b[2K! {message}, skipping")?;
                    now = next_serving(&client, &session.user_id, &req).await?;
                    player.send(command_for(&now, false)?);
                }
            },
            _ = ticker.tick() => {
                match timer.tick(TICK) {
                    Some(TimerEvent::Finished) => {
                        player.send(PlayerCommand::Stop { fade: Duration::from_secs(3) });
                        break;
                    }
                    Some(TimerEvent::PhaseChanged(phase)) => {
                        player.send(PlayerCommand::Chime);
                        paused = phase == Phase::Rest;
                        player.send(if paused { PlayerCommand::Pause } else { PlayerCommand::Play });
                    }
                    None => {}
                }
                write!(out, "\r\x1b[2K{}", status_line(&now, pos, &timer, paused))?;
                out.flush()?;
            }
        }
    }
    writeln!(out)?;
    // Give the engine time to finish its fade before the process exits.
    tokio::time::sleep(Duration::from_millis(3200)).await;
    if timer.display().is_zero() && !matches!(timer.mode(), TimerMode::Infinite) {
        eprintln!("Session complete");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::EffectLevel;
    use crate::config::StatePrefs;

    #[test]
    fn flags_override_prefs() {
        let prefs = StatePrefs {
            activity_id: None,
            genres: vec!["Piano".into()],
            effects: vec![EffectLevel::Low],
        };
        let req = build_request("a1", Some(EffectLevel::High), &[], &prefs);
        assert_eq!(req.neural_effect_levels, vec![EffectLevel::High]);
        assert_eq!(req.genre_names, vec!["Piano"]);
        let req = build_request("a1", None, &["Lofi".into()], &prefs);
        assert_eq!(req.neural_effect_levels, vec![EffectLevel::Low]);
        assert_eq!(req.genre_names, vec!["Lofi"]);
    }
}
