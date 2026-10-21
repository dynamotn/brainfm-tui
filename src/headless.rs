//! `brainfm play`: plays a mental state without the TUI.

use std::io::Write;
use std::time::{Duration, Instant};

use anyhow::{Context, bail};

use crate::api::{ApiClient, EffectLevel, Serving, SessionRequest, is_unauthorized};
use crate::app::state::Playback;
use crate::cli::{PlayArgs, client_from_store, resolve_activity, resolve_state};
use crate::config::{Config, StatePrefs};
use crate::media::{Media, MediaKey, Mirror};
use crate::player::{self, PlayerCommand, PlayerEvent};
use crate::session::SessionStore;
use crate::timer::{Phase, Timer, TimerEvent, TimerMode, format_clock, parse_intervals};

const TICK: Duration = Duration::from_millis(250);
const MAX_CONSECUTIVE_ERRORS: u32 = 3;

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

/// Why headless playback is silent: the user paused it, or an interval break.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Pauses {
    pub(crate) user: bool,
    pub(crate) rest: bool,
}

/// What the engine should do after a media key or a phase change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KeyEffect {
    Nothing,
    Play,
    Pause,
    Skip,
    Volume(u8),
}

impl Pauses {
    pub(crate) fn paused(self) -> bool {
        self.user || self.rest
    }

    /// Enters or leaves an interval break without overriding a user pause.
    pub(crate) fn set_rest(&mut self, rest: bool) -> KeyEffect {
        let was = self.paused();
        self.rest = rest;
        match (was, self.paused()) {
            (false, true) => KeyEffect::Pause,
            (true, false) => KeyEffect::Play,
            _ => KeyEffect::Nothing,
        }
    }

    fn set_user(&mut self, user: bool) -> KeyEffect {
        let was = self.paused();
        self.user = user;
        match (was, self.paused()) {
            (false, true) => KeyEffect::Pause,
            (true, false) => KeyEffect::Play,
            _ => KeyEffect::Nothing,
        }
    }
}

pub(crate) fn on_media_key(p: &mut Pauses, key: MediaKey) -> KeyEffect {
    match key {
        MediaKey::Toggle => {
            let user = !p.paused();
            p.set_user(user)
        }
        MediaKey::Play => p.set_user(false),
        MediaKey::Pause => p.set_user(true),
        MediaKey::Next => KeyEffect::Skip,
        MediaKey::SetVolume(v) => KeyEffect::Volume(v.min(100)),
    }
}

/// A failed *queued* track only drops the prefetch; the current track plays on.
pub(crate) fn error_needs_reload(id: &str, next: &mut Option<Serving>) -> bool {
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

// One `select!` loop over player events, prefetches and ticks; its arms share
// a dozen locals across awaits, which reads worse when split up.
#[allow(clippy::too_many_lines)]
pub(crate) async fn play(store: &SessionStore, args: PlayArgs) -> anyhow::Result<()> {
    #[cfg(unix)]
    let _socket = claim_session().await?;
    let (client, session) = client_from_store(store)?;
    let config = Config::load(&Config::default_path()?)?;
    let req = resolve_request(&client, &config, &args).await?;
    let mut timer = Timer::new(timer_mode(&args)?);

    let (player, mut events) = player::spawn(tokio::runtime::Handle::current())?;
    player.send(PlayerCommand::SetVolume(f32::from(config.volume) / 100.0));

    let mut now = next_serving(&client, &session.user_id, &req).await?;
    player.send(command_for(&now, false)?);
    let mut next: Option<Serving> = None;
    let mut pos = Duration::ZERO;
    let mut pauses = Pauses::default();
    let mut volume = config.volume.min(100);
    let (key_tx, mut keys) = tokio::sync::mpsc::unbounded_channel::<MediaKey>();
    let media = Media::start(key_tx).await;
    let mut mirror = Mirror::default();
    let mut errors = 0u32;
    let mut ticker = tokio::time::interval(TICK);
    let mut out = std::io::stderr();
    let fetch = || {
        retry(RETRIES, RETRY_DELAY, || {
            next_serving(&client, &session.user_id, &req)
        })
    };
    // Prefetches run in the background so the loop keeps ticking and Ctrl-C
    // stays responsive while the network is slow.
    let (prefetch_tx, mut prefetched) = tokio::sync::mpsc::unbounded_channel();
    let mut prefetching = false;
    let mut last_tick = Instant::now();
    let mut fade = Duration::ZERO;

    let result: anyhow::Result<()> = async {
        loop {
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {
                    fade = Duration::from_secs(1);
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
                    PlayerEvent::Position { pos: p, .. } => {
                        pos = p;
                        if let Some(m) = &media {
                            m.set_position(pos).await;
                        }
                    }
                    PlayerEvent::NearEnd { .. } => {
                        if next.is_none() && !prefetching {
                            prefetching = true;
                            let (c, uid, r, tx) =
                                (client.clone(), session.user_id.clone(), req.clone(), prefetch_tx.clone());
                            tokio::spawn(async move {
                                let s = retry(RETRIES, RETRY_DELAY, || next_serving(&c, &uid, &r)).await;
                                let _ = tx.send(s);
                            });
                        }
                    }
                    PlayerEvent::Ended { .. } => {
                        if next.is_none() {
                            let Some(s) = or_ctrl_c(fetch()).await? else {
                                fade = Duration::ZERO;
                                break;
                            };
                            now = s;
                            player.send(command_for(&now, false)?);
                        }
                    }
                    PlayerEvent::Error { id, message } => {
                        if !error_needs_reload(&id, &mut next) {
                            continue;
                        }
                        errors += 1;
                        let message = printable(&message);
                        if errors >= MAX_CONSECUTIVE_ERRORS {
                            bail!("{message} (giving up after {errors} failed tracks)");
                        }
                        writeln!(out, "\r\x1b[2K! {message}, skipping")?;
                        let Some(s) = or_ctrl_c(fetch()).await? else {
                            fade = Duration::ZERO;
                            break;
                        };
                        now = s;
                        player.send(command_for(&now, false)?);
                    }
                },
                Some(key) = keys.recv() => match on_media_key(&mut pauses, key) {
                    KeyEffect::Nothing => {}
                    KeyEffect::Play => {
                        player.send(PlayerCommand::Play);
                        timer.start();
                    }
                    KeyEffect::Pause => {
                        player.send(PlayerCommand::Pause);
                        timer.pause();
                    }
                    KeyEffect::Skip => {
                        let Some(s) = or_ctrl_c(fetch()).await? else {
                            fade = Duration::ZERO;
                            break;
                        };
                        next = None;
                        now = s;
                        player.send(command_for(&now, false)?);
                    }
                    KeyEffect::Volume(v) => {
                        volume = v;
                        player.send(PlayerCommand::SetVolume(f32::from(v) / 100.0));
                    }
                },
                Some(r) = prefetched.recv() => {
                    prefetching = false;
                    match r {
                        Ok(s) => {
                            player.send(command_for(&s, true)?);
                            next = Some(s);
                        }
                        Err(e) if is_unauthorized(&e) => return Err(e),
                        // Not fatal: another attempt is made when this track ends.
                        Err(e) => tracing::warn!("prefetch failed: {e:#}"),
                    }
                }
                _ = ticker.tick() => {
                    let now_tick = Instant::now();
                    let dt = now_tick - last_tick;
                    last_tick = now_tick;
                    match timer.tick(dt) {
                        Some(TimerEvent::Finished) => {
                            fade = Duration::from_secs(3);
                            break;
                        }
                        Some(TimerEvent::PhaseChanged(phase)) => {
                            player.send(PlayerCommand::Chime);
                            match pauses.set_rest(phase == Phase::Rest) {
                                KeyEffect::Pause => player.send(PlayerCommand::Pause),
                                KeyEffect::Play => player.send(PlayerCommand::Play),
                                _ => {}
                            }
                        }
                        None => {}
                    }
                    if let Some(m) = &media {
                        let status = if pauses.paused() { Playback::Paused } else { Playback::Playing };
                        mirror.sync(m, Some(&now), status, volume).await;
                    }
                    write!(out, "\r\x1b[2K{}", status_line(&now, pos, &timer, pauses.paused()))?;
                    out.flush()?;
                }
            }
        }
        Ok(())
    }
    .await;
    // Clear the status line so any error prints on a line of its own.
    write!(out, "\r\x1b[2K")?;
    player.send(PlayerCommand::Stop { fade });
    result?;
    // Let the engine finish its fade before the process exits.
    tokio::time::sleep(fade + Duration::from_millis(200)).await;
    if timer.display().is_zero() && !matches!(timer.mode(), TimerMode::Infinite) {
        eprintln!("Session complete");
    }
    Ok(())
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
                tokio::spawn(host.refuse(
                    "`brainfm play` is running in another terminal; stop it there first (Ctrl-C)",
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

/// Awaits `fut`, or returns `None` if Ctrl-C arrives first.
async fn or_ctrl_c<T>(fut: impl Future<Output = anyhow::Result<T>>) -> anyhow::Result<Option<T>> {
    tokio::select! {
        r = fut => r.map(Some),
        _ = tokio::signal::ctrl_c() => Ok(None),
    }
}

/// Drops control characters so API text cannot drive the terminal.
pub(crate) fn printable(text: &str) -> String {
    text.chars().filter(|c| !c.is_control()).collect()
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
