//! `brainfm play`: plays a mental state without the TUI.

mod machine;

use machine::{Cmd, FetchError, FetchKind, Machine};

use std::io::Write;
use std::time::{Duration, Instant};

use anyhow::{Context, bail};

use crate::api::{ApiClient, EffectLevel, Serving, SessionRequest, is_unauthorized};
use crate::cli::{PlayArgs, client_from_store, resolve_activity, resolve_state};
use crate::config::{Config, StatePrefs};
use crate::media::{Media, MediaKey, Mirror};
use crate::player::{self, PlayerCommand, PlayerEvent};
use crate::session::SessionStore;
use crate::timer::{Phase, Timer, TimerMode, format_clock, parse_intervals};

const TICK: Duration = Duration::from_millis(250);

/// Command-line filters win over the saved per-state preferences.
pub(crate) fn build_request(
    activity_id: &str,
    effect: Option<EffectLevel>,
    genres: &[String],
    prefs: &StatePrefs,
) -> SessionRequest {
    let effects = effect.map_or_else(|| prefs.effects.clone(), |e| vec![e]);
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
pub(crate) async fn retry<T, F, Fut>(attempts: u32, delay: Duration, mut f: F) -> anyhow::Result<T>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = anyhow::Result<T>>,
{
    let mut wait = delay;
    let mut last = None;
    for attempt in 1..=attempts {
        match f().await {
            Ok(v) => return Ok(v),
            Err(e) if is_unauthorized(&e) => return Err(e),
            Err(e) => {
                tracing::warn!("attempt {attempt} failed: {e:#}");
                last = Some(e);
                if attempt < attempts {
                    tokio::time::sleep(wait).await;
                    wait *= 2;
                }
            }
        }
    }
    Err(last.unwrap_or_else(|| anyhow::anyhow!("no attempts made")))
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

fn status_line(s: &Serving, pos: Duration, timer: &Timer, paused: bool) -> String {
    let t = &s.track;
    let effect = s.track_variation.effect().map_or("—", EffectLevel::label);
    let icon = if paused { "⏸" } else { "▶" };
    let phase = match (timer.mode(), timer.phase()) {
        (TimerMode::Intervals { .. }, Phase::Rest) => " (break)",
        _ => "",
    };
    format!(
        "{icon} {} · {} · {effect}  {}/{}  ⏱ {}{phase}",
        printable(&t.name),
        printable(t.genre().unwrap_or("—")),
        format_clock(pos),
        format_clock(s.track_variation.length()),
        format_clock(timer.display()),
    )
}

/// Resolves the mental state, activity and filters into a session request.
async fn resolve_request(
    client: &ApiClient,
    config: &Config,
    args: &PlayArgs,
) -> anyhow::Result<SessionRequest> {
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
    Ok(build_request(
        &activity_id,
        args.effect.map(Into::into),
        &args.genre,
        &prefs,
    ))
}

fn timer_mode(args: &PlayArgs) -> anyhow::Result<TimerMode> {
    Ok(match (&args.timer, &args.intervals) {
        (Some(d), _) => TimerMode::Countdown((*d).into()),
        (None, Some(s)) => parse_intervals(s)?,
        (None, None) => TimerMode::Infinite,
    })
}

pub(crate) async fn play(store: &SessionStore, args: PlayArgs) -> anyhow::Result<()> {
    #[cfg(unix)]
    let _socket = claim_session().await?;
    let (client, session) = client_from_store(store)?;
    let config = Config::load(&Config::default_path()?)?;
    let req = resolve_request(&client, &config, &args).await?;
    let mode = timer_mode(&args)?;

    let (player, mut events) = player::spawn(tokio::runtime::Handle::current())?;
    let first = retry(RETRIES, RETRY_DELAY, || {
        next_serving(&client, &session.user_id, &req)
    })
    .await?;
    let (mut m, cmds) = Machine::new(first, mode, config.volume);

    let (key_tx, mut keys) = tokio::sync::mpsc::unbounded_channel::<MediaKey>();
    let media = Media::start(key_tx).await;
    let mut mirror = Mirror::default();
    let (fetch_tx, mut fetch_results) = tokio::sync::mpsc::unbounded_channel();
    let fetcher = Fetcher {
        client: client.clone(),
        uid: session.user_id.clone(),
        req: req.clone(),
        tx: fetch_tx,
    };

    let mut ticker = tokio::time::interval(TICK);
    let mut last_tick = Instant::now();
    let mut out = std::io::stderr();
    let mut pending = cmds;
    let ending: anyhow::Result<Duration> = loop {
        // Carry out what the machine asked for.
        let mut stop = None;
        for cmd in pending.drain(..) {
            match cmd {
                Cmd::Player(c) => player.send(c),
                Cmd::Fetch { kind, epoch } => fetcher.spawn(kind, epoch),
                Cmd::Notice(text) => writeln!(out, "\r\x1b[2K{text}")?,
                Cmd::Stop { fade } => stop = Some(Ok(fade)),
                Cmd::Fail(reason) => stop = Some(Err(anyhow::anyhow!(reason))),
            }
        }
        if let Some(end) = stop {
            break end;
        }
        pending = tokio::select! {
            _ = tokio::signal::ctrl_c() => vec![Cmd::Stop { fade: machine::QUIT_FADE }],
            Some(ev) = events.recv() => {
                let pos_report = matches!(ev, PlayerEvent::Position { .. });
                let cmds = m.on_player(ev);
                if pos_report && let Some(media) = &media {
                    media.set_position(m.pos).await;
                }
                cmds
            }
            Some((kind, epoch, result)) = fetch_results.recv() => m.on_fetched(kind, epoch, result),
            Some(key) = keys.recv() => m.on_key(key),
            _ = ticker.tick() => {
                let now_tick = Instant::now();
                let cmds = m.on_tick(now_tick - last_tick);
                last_tick = now_tick;
                if let Some(media) = &media {
                    mirror.sync(media, Some((&m.now, m.serial)), m.status(), m.volume).await;
                }
                write!(out, "\r\x1b[2K{}", status_line(&m.now, m.pos, &m.timer, m.pauses.paused()))?;
                out.flush()?;
                cmds
            }
        };
    };
    // Clear the status line so any message prints on a line of its own.
    write!(out, "\r\x1b[2K")?;
    let fade = match ending {
        Ok(fade) => fade,
        Err(e) => {
            player.send(PlayerCommand::Stop {
                fade: Duration::ZERO,
            });
            return Err(e);
        }
    };
    player.send(PlayerCommand::Stop { fade });
    // Let the engine finish its fade before the process exits.
    tokio::time::sleep(fade + Duration::from_millis(200)).await;
    if m.timer.display().is_zero() && !matches!(m.timer.mode(), TimerMode::Infinite) {
        eprintln!("Session complete");
    }
    Ok(())
}

/// Runs track fetches in the background and reports them back tagged.
struct Fetcher {
    client: ApiClient,
    uid: String,
    req: SessionRequest,
    tx: tokio::sync::mpsc::UnboundedSender<(FetchKind, u64, Result<Serving, FetchError>)>,
}

impl Fetcher {
    fn spawn(&self, kind: FetchKind, epoch: u64) {
        let (c, uid, req, tx) = (
            self.client.clone(),
            self.uid.clone(),
            self.req.clone(),
            self.tx.clone(),
        );
        tokio::spawn(async move {
            let result = retry(RETRIES, RETRY_DELAY, || next_serving(&c, &uid, &req))
                .await
                .map_err(|e| {
                    if is_unauthorized(&e) {
                        FetchError::Unauthorized
                    } else {
                        FetchError::Other(format!("{e:#}"))
                    }
                });
            let _ = tx.send((kind, epoch, result));
        });
    }
}

/// Holds the shared-session socket so a TUI opened meanwhile explains why it
/// cannot start, instead of playing a second stream.
#[cfg(unix)]
async fn claim_session() -> anyhow::Result<Option<crate::share::SocketGuard>> {
    use crate::share::{Attach, Host, SocketGuard, attach, socket_path};
    let path = socket_path()?;
    match attach(&path).await? {
        Attach::Session(_) => {
            bail!("A brainfm TUI is already playing; control it there, or quit it first")
        }
        Attach::Busy(reason) => bail!("{reason}"),
        Attach::NoHost => match Host::bind(&path) {
            Ok(host) => {
                // Wiring `brainfm stop` to end `play` comes later; for now the
                // receiver is dropped, so a Stop is ignored.
                let (stop, _) = tokio::sync::mpsc::unbounded_channel();
                tokio::spawn(host.refuse(
                    "`brainfm play` is running in another terminal; stop it there first (Ctrl-C)",
                    stop,
                ));
                Ok(Some(SocketGuard(path)))
            }
            Err(e) => {
                tracing::warn!("cannot claim the session socket: {e}");
                Ok(None)
            }
        },
    }
}

/// Drops control characters so API text cannot drive the terminal.
pub(crate) fn printable(text: &str) -> String {
    text.chars().filter(|c| !c.is_control()).collect()
}

#[cfg(test)]
mod tests {
    use super::machine::{KeyEffect, Pauses, on_media_key};
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    #[test]
    fn toggle_pauses_then_resumes() {
        let mut p = Pauses::default();
        assert_eq!(on_media_key(&mut p, MediaKey::Toggle), KeyEffect::Pause);
        assert!(p.paused());
        assert_eq!(on_media_key(&mut p, MediaKey::Toggle), KeyEffect::Play);
        assert!(!p.paused());
    }

    #[test]
    fn play_and_pause_are_idempotent() {
        let mut p = Pauses::default();
        assert_eq!(on_media_key(&mut p, MediaKey::Play), KeyEffect::Nothing);
        assert_eq!(on_media_key(&mut p, MediaKey::Pause), KeyEffect::Pause);
        assert_eq!(on_media_key(&mut p, MediaKey::Pause), KeyEffect::Nothing);
    }

    #[test]
    fn user_play_during_a_break_keeps_the_break_silent() {
        let mut p = Pauses {
            user: true,
            rest: true,
        };
        assert_eq!(on_media_key(&mut p, MediaKey::Play), KeyEffect::Nothing);
        assert!(p.paused(), "still the rest phase");
        assert!(!p.user);
    }

    #[test]
    fn break_ending_does_not_override_a_user_pause() {
        let mut p = Pauses {
            user: true,
            rest: true,
        };
        assert_eq!(p.set_rest(false), KeyEffect::Nothing);
        let mut p = Pauses {
            user: false,
            rest: true,
        };
        assert_eq!(p.set_rest(false), KeyEffect::Play);
        assert_eq!(p.set_rest(true), KeyEffect::Pause);
    }

    #[test]
    fn next_and_volume_pass_through() {
        let mut p = Pauses::default();
        assert_eq!(on_media_key(&mut p, MediaKey::Next), KeyEffect::Skip);
        assert_eq!(
            on_media_key(&mut p, MediaKey::SetVolume(30)),
            KeyEffect::Volume(30)
        );
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
    async fn retry_does_not_sleep_after_last_attempt() {
        let start = std::time::Instant::now();
        let r: anyhow::Result<()> = retry(2, Duration::from_millis(200), || async {
            anyhow::bail!("down")
        })
        .await;
        assert!(r.is_err());
        assert!(
            start.elapsed() < Duration::from_millis(350),
            "{:?}",
            start.elapsed()
        );
    }

    #[test]
    fn status_text_strips_control_characters() {
        assert_eq!(
            printable("Asc\x1b]52;c;ZXZpbA==\x07ent\n"),
            "Asc]52;c;ZXZpbA==ent"
        );
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
