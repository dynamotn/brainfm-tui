//! The TUI event loop: terminal, timer ticks, player and API results feed
//! [`update::update`], whose effects are executed here.

pub mod state;
pub mod update;
#[cfg(unix)]
mod view;

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
use crate::media::{Media, MediaKey, Mirror};
use crate::player::{self, PlayerCommand, PlayerHandle};
use crate::session::SessionStore;
use crate::ui::{self, Theme};
use state::{AppState, Toast};
use update::{Action, ApiErrorKind, ApiReply, Effect, update};

const TICK: Duration = Duration::from_millis(250);
/// Slightly longer than the quit fade so it can finish before exit.
const QUIT_GRACE: Duration = Duration::from_millis(600);

#[must_use]
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
    config_writer: tokio::sync::watch::Sender<Option<Config>>,
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
                    Err(e) => {
                        tracing::warn!("favorite failed: {e}");
                        Action::Api(ApiReply::FavoriteFailed {
                            track_id,
                            on,
                            error: error_kind(&e),
                        })
                    }
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
                    let token = match c.email_login(&email, password.expose()).await {
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
            // Written by a debounced background task (see `spawn_writer`).
            Effect::SaveConfig => {
                self.config_writer.send_replace(Some(s.config.clone()));
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

#[cfg(unix)]
enum Start {
    /// This window was a view of another session, which has ended.
    Joined,
    /// This window owns the session (`None` if the socket is unusable).
    Host(Option<(crate::share::Host, crate::share::SocketGuard)>),
}

/// Attaches to a running session as a view, or claims the socket as host.
#[cfg(unix)]
async fn join_or_host() -> anyhow::Result<Start> {
    use crate::share::{Attach, Host, SocketGuard, attach, socket_path};
    let path = socket_path()?;
    for _ in 0..2 {
        match attach(&path).await? {
            Attach::Session(client) => {
                view::run(client).await?;
                return Ok(Start::Joined);
            }
            Attach::Busy(reason) => anyhow::bail!("{reason}"),
            Attach::NoHost => match Host::bind(&path) {
                Ok(h) => return Ok(Start::Host(Some((h, SocketGuard(path))))),
                // Another window started at the same moment; join it.
                Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {}
                Err(e) => {
                    tracing::warn!("cannot share the session: {e}");
                    break;
                }
            },
        }
    }
    Ok(Start::Host(None))
}

struct Setup {
    config_path: PathBuf,
    store: SessionStore,
    client: ApiClient,
    state: AppState,
}

/// Loads config and session and builds the API client and starting state.
fn setup() -> anyhow::Result<Setup> {
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
    let writer = store.clone();
    let mut client = ApiClient::new(Url::parse(DEFAULT_BASE)?)
        .on_token_refresh(move |t| writer.refresh_in_background(t));
    if let Some(s) = &session {
        client = client.with_token(&s.token);
    }
    let mut state = AppState::new(config, config_writable, session.map(|s| s.user_id));
    if let Some(e) = config_error {
        state.toast = Some(Toast {
            ttl: Duration::from_secs(10),
            ..Toast::new(format!("Using defaults — {e}"))
        });
    }
    Ok(Setup {
        config_path,
        store,
        client,
        state,
    })
}

/// Pushes what changed to the desktop's media controls.
async fn mirror_to_media(
    m: &Media,
    mirror: &mut Mirror,
    shown_position: &mut Duration,
    state: &AppState,
) {
    mirror
        .sync(
            m,
            state.now.as_ref().map(|n| (n, state.started)),
            state.playback,
            state.config.volume,
        )
        .await;
    // Only real reports from the engine; MPRIS interpolates between them.
    if state.position != *shown_position {
        *shown_position = state.position;
        m.set_position(state.position).await;
    }
}

pub async fn run() -> anyhow::Result<()> {
    let _log = init_logging()?;

    // A second window joins the running session instead of starting its own.
    #[cfg(unix)]
    let host = match join_or_host().await? {
        Start::Joined => return Ok(()),
        Start::Host(host) => host,
    };

    let Setup {
        config_path,
        store,
        client,
        mut state,
    } = setup()?;

    // Fail before touching the terminal if there is no audio device.
    let (player, mut player_rx) = player::spawn(tokio::runtime::Handle::current())?;
    player.send(PlayerCommand::SetVolume(
        f32::from(state.config.volume) / 100.0,
    ));

    let (tx, mut rx) = unbounded_channel::<Action>();
    let rt = Runtime {
        client,
        store,
        player,
        tx: tx.clone(),
        config_writer: crate::config::spawn_writer(
            config_path.clone(),
            crate::config::SAVE_DEBOUNCE,
        ),
    };
    let (state_tx, state_rx) = tokio::sync::watch::channel(serde_json::to_string(&state)?);
    #[cfg(unix)]
    let _socket = host.map(|(h, guard)| {
        tokio::spawn(h.serve(state_rx, tx.clone()));
        guard
    });
    #[cfg(not(unix))]
    drop(state_rx);

    let (key_tx, mut key_rx) = unbounded_channel::<MediaKey>();
    let media = Media::start(key_tx).await;
    let mut mirror = Mirror::default();
    let mut shown_position = Duration::ZERO;

    if state.user_id.is_some() {
        rt.exec(Effect::FetchStates, &state);
        rt.exec(Effect::FetchLibrary, &state);
    }

    let theme = Theme::detect();
    let mut terminal = ratatui::init();
    let mut events = EventStream::new();
    let mut ticker = tokio::time::interval(TICK);
    let mut last_tick = Instant::now();

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
                Some(k) = key_rx.recv() => Some(Action::Media(k)),
            };
            let Some(action) = action else { continue };
            for effect in update(&mut state, action) {
                rt.exec(effect, &state);
            }
            let json = serde_json::to_string(&state)?;
            state_tx.send_if_modified(|current| {
                let changed = *current != json;
                if changed {
                    *current = json;
                }
                changed
            });

            if let Some(m) = &media {
                mirror_to_media(m, &mut mirror, &mut shown_position, &state).await;
            }
            if state.quit {
                break;
            }
        }
        Ok(())
    }
    .await;

    // A change made just before quitting may still sit in the debounce.
    if state.config_writable
        && let Err(e) = state.config.save(&config_path)
    {
        tracing::warn!("saving config: {e:#}");
    }
    tokio::time::sleep(QUIT_GRACE).await;
    ratatui::restore();
    result
}
