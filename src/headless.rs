//! `brainfm play`: plays a mental state without the TUI.

use std::io::Write;
use std::time::Duration;

use anyhow::{Context, bail};

use crate::api::{ApiClient, ApiError, EffectLevel, Serving, SessionRequest};
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

const RETRIES: u32 = 3;
const RETRY_DELAY: Duration = Duration::from_secs(2);

/// Runs `f` up to `attempts` times with a doubling delay. An expired session
/// is returned at once since retrying cannot fix it.
pub async fn retry<T, F, Fut>(attempts: u32, delay: Duration, mut f: F) -> anyhow::Result<T>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = anyhow::Result<T>>,
{
    let mut wait = delay;
    let mut last = None;
    for _ in 0..attempts {
        match f().await {
            Ok(v) => return Ok(v),
            Err(e) if matches!(e.downcast_ref::<ApiError>(), Some(ApiError::Unauthorized)) => {
                return Err(e);
            }
            Err(e) => {
                tracing::warn!("retrying after: {e:#}");
                last = Some(e);
                tokio::time::sleep(wait).await;
                wait *= 2;
            }
        }
    }
    Err(last.unwrap_or_else(|| anyhow::anyhow!("no attempts made")))
}

/// A failed *queued* track only drops the prefetch; the current track plays on.
pub fn error_needs_reload(id: &str, next: &mut Option<Serving>) -> bool {
    if next.as_ref().is_some_and(|n| n.track.id == id) {
        *next = None;
        return false;
    }
    true
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
    let fetch = || {
        retry(RETRIES, RETRY_DELAY, || {
            next_serving(&client, &session.user_id, &req)
        })
    };

    let result: anyhow::Result<()> = async { loop {
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
                PlayerEvent::NearEnd { .. } => match fetch().await {
                    Ok(s) => {
                        player.send(command_for(&s, true)?);
                        next = Some(s);
                    }
                    // Not fatal: another attempt is made when this track ends.
                    Err(e) if !matches!(e.downcast_ref::<ApiError>(), Some(ApiError::Unauthorized)) => {
                        tracing::warn!("prefetch failed: {e:#}");
                    }
                    Err(e) => return Err(e),
                },
                PlayerEvent::Ended { .. } => {
                    if next.is_none() {
                        now = fetch().await?;
                        player.send(command_for(&now, false)?);
                    }
                }
                PlayerEvent::Error { id, message } => {
                    if !error_needs_reload(&id, &mut next) {
                        continue;
                    }
                    errors += 1;
                    if errors >= MAX_CONSECUTIVE_ERRORS {
                        bail!("{message} (giving up after {errors} failed tracks)");
                    }
                    writeln!(out, "\r\x1b[2K! {message}, skipping")?;
                    now = fetch().await?;
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
    } Ok(()) }.await;
    // Clear the status line so any error prints on a line of its own.
    write!(out, "\r\x1b[2K")?;
    if result.is_err() {
        player.send(PlayerCommand::Stop {
            fade: Duration::ZERO,
        });
        return result;
    }
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
    use std::sync::atomic::{AtomicU32, Ordering};

    fn serving(id: &str) -> Serving {
        serde_json::from_value(serde_json::json!({
            "track": {"id": id, "name": id, "tags": []},
            "trackVariation": {"id": "v", "url": "x.mp3", "lengthInSeconds": 10}
        }))
        .unwrap()
    }

    #[test]
    fn queued_error_only_clears_next() {
        let mut next = Some(serving("b"));
        assert!(!error_needs_reload("b", &mut next));
        assert!(next.is_none());
        assert!(error_needs_reload("a", &mut next));
    }

    #[tokio::test]
    async fn retry_recovers_after_transient_failures() {
        let calls = AtomicU32::new(0);
        let out = retry(3, Duration::ZERO, || async {
            if calls.fetch_add(1, Ordering::SeqCst) < 2 {
                anyhow::bail!("transient")
            }
            Ok(7)
        })
        .await
        .unwrap();
        assert_eq!(out, 7);
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn retry_gives_up() {
        let calls = AtomicU32::new(0);
        let r: anyhow::Result<()> = retry(3, Duration::ZERO, || async {
            calls.fetch_add(1, Ordering::SeqCst);
            anyhow::bail!("down")
        })
        .await;
        assert!(r.is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }
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
