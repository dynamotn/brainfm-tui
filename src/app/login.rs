//! The login form, run in the TUI's own process so the password never
//! reaches the daemon or the socket.

use crossterm::event::{Event, EventStream};
use futures::StreamExt;
use ratatui::DefaultTerminal;
use tokio::sync::mpsc::unbounded_channel;
use tokio::sync::watch;

use super::state::{AppState, Overlay};
use super::update::{Action, ApiReply, Effect, update};
use crate::api::ApiClient;
use crate::config::Config;
use crate::secret::Secret;
use crate::session::SessionStore;
use crate::ui::{self, Theme};

/// Shown in the form once another window has logged in meanwhile.
pub(super) const ELSEWHERE: &str = "Logged in from another window — press Esc";

/// How the form ended.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Outcome {
    /// Logged in here; the session is saved.
    LoggedIn(String),
    /// Closed with Esc (only when cancellable).
    Closed,
    /// Ctrl-C, or the terminal went away.
    Quit,
}

/// What the form wants done after an action.
#[derive(Debug, PartialEq)]
pub(super) enum Step {
    Continue,
    LogIn { email: String, password: Secret },
    Done(Outcome),
}

/// The login overlay on a local state: no effect but the login itself is
/// ever acted on, so every key stays in this process.
#[derive(Debug)]
pub(super) struct Form {
    state: AppState,
}

impl Form {
    pub(super) fn new(cancellable: bool) -> Self {
        // The overlay closes on Esc only when a user id is set.
        let mut state = AppState::new(Config::default(), false, cancellable.then(String::new));
        state.overlay = Some(Overlay::login());
        Self { state }
    }

    pub(super) fn state(&self) -> &AppState {
        &self.state
    }

    pub(super) fn handle(&mut self, action: Action) -> Step {
        if let Action::Api(ApiReply::LoggedIn { user_id }) = action {
            return Step::Done(Outcome::LoggedIn(user_id));
        }
        let effects = update(&mut self.state, action);
        if self.state.quit {
            return Step::Done(Outcome::Quit);
        }
        if self.state.overlay.is_none() {
            return Step::Done(Outcome::Closed);
        }
        // Everything else (fetches, the player) belongs to the daemon.
        effects
            .into_iter()
            .find_map(|e| match e {
                Effect::Login { email, password } => Some(Step::LogIn { email, password }),
                _ => None,
            })
            .unwrap_or(Step::Continue)
    }

    /// Another window logged in: say so, but leave closing to the user, so
    /// keys typed meanwhile still land in this form.
    pub(super) fn logged_in_elsewhere(&mut self) {
        if let Some(Overlay::Login { error, .. }) = &mut self.state.overlay {
            *error = Some(ELSEWHERE.into());
        }
    }
}

/// Shows the login form until it ends. Once `elsewhere` turns true the
/// form says another window logged in (pass a receiver whose sender is
/// dropped when that cannot happen).
pub(super) async fn run(
    terminal: &mut DefaultTerminal,
    theme: &Theme,
    store: &SessionStore,
    client: &ApiClient,
    cancellable: bool,
    mut elsewhere: watch::Receiver<bool>,
) -> anyhow::Result<Outcome> {
    let mut form = Form::new(cancellable);
    let (tx, mut replies) = unbounded_channel::<Action>();
    let mut events = EventStream::new();

    loop {
        terminal.draw(|f| ui::draw(f, form.state(), theme))?;
        let action = tokio::select! {
            ev = events.next() => match ev {
                Some(Ok(Event::Key(k))) => Action::Key(k),
                Some(Ok(_)) => continue,
                Some(Err(e)) => return Err(e.into()),
                None => return Ok(Outcome::Quit),
            },
            Some(reply) = replies.recv() => reply,
            Ok(()) = elsewhere.changed() => {
                if *elsewhere.borrow_and_update() {
                    form.logged_in_elsewhere();
                }
                continue;
            }
        };
        match form.handle(action) {
            Step::Continue => {}
            Step::Done(outcome) => return Ok(outcome),
            Step::LogIn { email, password } => {
                let (client, store, tx) = (client.clone(), store.clone(), tx.clone());
                tokio::spawn(async move {
                    let _ = tx.send(super::log_in(&client, &store, &email, &password).await);
                });
            }
        }
    }
}
