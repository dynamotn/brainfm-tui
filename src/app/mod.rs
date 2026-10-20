//! The TUI event loop: terminal, timer ticks, player and API results feed
//! [`update::update`], whose effects are executed here.

pub mod state;
pub mod update;

#[cfg(test)]
pub(crate) mod tests;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use crossterm::event::{Event, EventStream};
use futures::StreamExt;
use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use url::Url;

use crate::api::client::DEFAULT_BASE;
use crate::api::{ApiClient, ApiError};
use crate::config::{Config, state_dir};
use crate::media::MediaBridge;
use crate::player::{self, PlayerCommand, PlayerHandle};
use crate::session::SessionStore;
use crate::ui::{self, Theme};
use state::{AppState, Playback, Toast};
use update::{Action, ApiErrorKind, ApiReply, Effect, update};

const TICK: Duration = Duration::from_millis(250);
/// Slightly longer than the quit fade so it can finish before exit.
const QUIT_GRACE: Duration = Duration::from_millis(600);

pub fn error_kind(e: &ApiError) -> ApiErrorKind {
    match e {
        ApiError::Unauthorized => ApiErrorKind::Unauthorized,
        other => ApiErrorKind::Other(other.to_string()),
    }
}

fn failed(what: &'static str, e: &ApiError) -> Action {
    tracing::warn!("{what} failed: {e}");
    Action::Api(ApiReply::Failed {
        what,
        error: error_kind(e),
    })
}

/// Everything effects need besides the state itself.
struct Runtime {
    client: ApiClient,
    store: SessionStore,
    player: PlayerHandle,
    tx: UnboundedSender<Action>,
    config_path: PathBuf,
}

impl Runtime {
    fn spawn<F>(&self, fut: F)
    where
        F: Future<Output = Action> + Send + 'static,
    {
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let _ = tx.send(fut.await);
        });
    }

    /// Fire-and-forget: only a failure is reported back.
    fn spawn_quiet<F>(&self, fut: F)
    where
        F: Future<Output = Option<Action>> + Send + 'static,
    {
        let tx = self.tx.clone();
        tokio::spawn(async move {
            if let Some(a) = fut.await {
                let _ = tx.send(a);
            }
        });
    }

    fn exec(&self, effect: Effect, s: &AppState) {
        let c = self.client.clone();
        let uid = s.user_id.clone().unwrap_or_default();
        match effect {
            Effect::FetchStates => self.spawn(async move {
                match c.mental_states().await {
                    Ok(v) => Action::Api(ApiReply::States(v)),
                    Err(e) => failed("mental states", &e),
                }
            }),
            Effect::FetchFilters(state_id) => {
                let (c2, id2) = (c.clone(), state_id.clone());
                self.spawn(async move {
                    match c.activities(&state_id).await {
                        Ok(items) => Action::Api(ApiReply::Activities { state_id, items }),
                        Err(e) => failed("activities", &e),
                    }
                });
                self.spawn(async move {
                    match c2.genres(&id2).await {
                        Ok(items) => Action::Api(ApiReply::Genres {
                            state_id: id2,
                            items,
                        }),
                        Err(e) => failed("genres", &e),
                    }
                });
            }
            Effect::StartSession {
                req,
                intent,
                generation,
            } => self.spawn(async move {
                match c.start_session(&uid, &req).await {
                    Ok(items) => Action::Api(ApiReply::Servings {
                        intent,
                        generation,
                        items,
                    }),
                    Err(e) => {
                        tracing::warn!("session failed: {e}");
                        Action::Api(ApiReply::SessionFailed {
                            generation,
                            error: error_kind(&e),
                        })
                    }
                }
            }),
            Effect::Player(cmd) => self.player.send(cmd),
            Effect::SetFavorite { track_id, on } => self.spawn(async move {
                match c.set_favorite(&uid, &track_id, on).await {
                    Ok(()) => library(&c, &uid).await,
                    Err(e) => failed("favorite", &e),
                }
            }),
            Effect::Dislike { track_id } => self.spawn_quiet(async move {
                c.dislike(&uid, &track_id)
                    .await
                    .err()
                    .map(|e| failed("dislike", &e))
            }),
            Effect::FetchLibrary => self.spawn(async move { library(&c, &uid).await }),
            Effect::Login { email, password } => {
                let store = self.store.clone();
                self.spawn(async move {
                    let token = match c.email_login(&email, &password).await {
                        Ok(t) => t,
                        Err(e) => return failed("login", &e),
                    };
                    match store.save(&token) {
                        Ok(session) => Action::Api(ApiReply::LoggedIn {
                            user_id: session.user_id,
                        }),
                        Err(e) => Action::Api(ApiReply::Failed {
                            what: "login",
                            error: ApiErrorKind::Other(format!("{e:#}")),
                        }),
                    }
                });
            }
            Effect::SaveConfig => {
                if let Err(e) = s.config.save(&self.config_path) {
                    tracing::warn!("saving config: {e:#}");
                    let _ = self.tx.send(Action::Api(ApiReply::Failed {
                        what: "config",
                        error: ApiErrorKind::Other(format!("{e:#}")),
                    }));
                }
            }
        }
    }
}

/// Favorites and recents; one list failing to decode does not hide the other.
async fn library(c: &ApiClient, uid: &str) -> Action {
    let (favorites, recents) = tokio::join!(c.favorites(uid), c.recents(uid));
    if [&favorites, &recents]
        .iter()
        .any(|r| matches!(r, Err(ApiError::Unauthorized)))
    {
        return failed("library", &ApiError::Unauthorized);
    }
    let keep = |r: Result<Vec<_>, ApiError>, what: &str| {
        r.unwrap_or_else(|e| {
            tracing::warn!("loading {what}: {e}");
            Vec::new()
        })
    };
    Action::Api(ApiReply::Library {
        favorites: keep(favorites, "favorites"),
        recents: keep(recents, "recents"),
    })
}

fn init_logging() -> anyhow::Result<tracing_appender::non_blocking::WorkerGuard> {
    let dir = state_dir()?;
    crate::session::ensure_private_dir(&dir)?;
    let (writer, guard) =
        tracing_appender::non_blocking(tracing_appender::rolling::never(&dir, "brainfm.log"));
    // stream-download logs full request URLs, and signed audio URLs carry
    // short-lived credentials, so its logs are dropped.
    let filter = tracing_subscriber::filter::Targets::new()
        .with_default(tracing::Level::INFO)
        .with_target("stream_download", tracing::level_filters::LevelFilter::OFF);
    let _ = tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(writer)
                .with_ansi(false),
        )
        .with(filter)
        .try_init();
    Ok(guard)
}

pub async fn run() -> anyhow::Result<()> {
    let _log = init_logging()?;

    let config_path = Config::default_path()?;
    let (config, config_writable, config_error) = match Config::load(&config_path) {
        Ok(c) => (c, true, None),
        Err(e) => (Config::default(), false, Some(format!("{e:#}"))),
    };
    let store = SessionStore::new(SessionStore::default_path()?);
    let session = store.load().unwrap_or_else(|e| {
        tracing::warn!("ignoring stored session: {e:#}");
        None
    });

    // Fail before touching the terminal if there is no audio device.
    let (player, mut player_rx) = player::spawn(tokio::runtime::Handle::current())?;
    player.send(PlayerCommand::SetVolume(f32::from(config.volume) / 100.0));

    let (tx, mut rx) = unbounded_channel::<Action>();
    let writer = store.clone();
    let mut client = ApiClient::new(Url::parse(DEFAULT_BASE)?).on_token_refresh(move |t| {
        if let Err(e) = writer.save(t) {
            tracing::warn!("could not persist refreshed token: {e:#}");
        }
    });
    if let Some(s) = &session {
        client = client.with_token(s.token.clone());
    }

    let mut state = AppState::new(config, config_writable, session.map(|s| s.user_id));
    if let Some(e) = config_error {
        state.toast = Some(Toast {
            ttl: Duration::from_secs(10),
            ..Toast::new(format!("Using defaults — {e}"))
        });
    }
    let rt = Runtime {
        client,
        store,
        player,
        tx: tx.clone(),
        config_path,
    };
    let mut media = MediaBridge::new(tx);

    if state.user_id.is_some() {
        rt.exec(Effect::FetchStates, &state);
        rt.exec(Effect::FetchLibrary, &state);
    }

    let theme = Theme::detect();
    let mut terminal = ratatui::init();
    let mut events = EventStream::new();
    let mut ticker = tokio::time::interval(TICK);
    let mut last_tick = Instant::now();
    let mut shown_track: Option<String> = None;
    let mut shown_playback = Playback::Idle;

    let result: anyhow::Result<()> = async {
        loop {
            terminal.draw(|f| ui::draw(f, &state, &theme))?;
            let action = tokio::select! {
                ev = events.next() => match ev {
                    Some(Ok(Event::Key(k))) => Some(Action::Key(k)),
                    Some(Ok(_)) => None,
                    Some(Err(e)) => return Err(e.into()),
                    None => break,
                },
                _ = ticker.tick() => {
                    let now = Instant::now();
                    let dt = now - last_tick;
                    last_tick = now;
                    Some(Action::Tick(dt))
                }
                Some(ev) = player_rx.recv() => Some(Action::Player(ev)),
                Some(a) = rx.recv() => Some(a),
            };
            let Some(action) = action else { continue };
            for effect in update(&mut state, action) {
                rt.exec(effect, &state);
            }

            if let Some(m) = &mut media {
                let track = state.now.as_ref().map(|n| n.track.id.clone());
                if track != shown_track {
                    if let Some(now) = &state.now {
                        m.set_track(now);
                    }
                    shown_track = track;
                }
                if state.playback != shown_playback {
                    m.set_playback(state.playback);
                    shown_playback = state.playback;
                }
            }
            if state.quit {
                break;
            }
        }
        Ok(())
    }
    .await;

    tokio::time::sleep(QUIT_GRACE).await;
    ratatui::restore();
    result
}
