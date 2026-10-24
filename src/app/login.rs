//! The login form, run in the TUI's own process so the password never
//! reaches the daemon or the socket.

use crossterm::event::{Event, EventStream};
use futures::StreamExt;
use ratatui::DefaultTerminal;
use tokio::sync::mpsc::unbounded_channel;

use super::state::{AppState, Overlay};
use super::update::{Action, ApiReply, Effect, update};
use crate::api::ApiClient;
use crate::config::Config;
use crate::session::SessionStore;
use crate::ui::{self, Theme};

/// Shows the login form until the user logs in (`Some(user_id)`, with the
/// session saved to `store`) or gives up: Ctrl-C, or Esc when `cancellable`.
pub(super) async fn run(
    terminal: &mut DefaultTerminal,
    theme: &Theme,
    store: &SessionStore,
    client: &ApiClient,
    cancellable: bool,
) -> anyhow::Result<Option<String>> {
    // The form closes on Esc only when a user id is set.
    let mut state = AppState::new(Config::default(), false, cancellable.then(String::new));
    state.overlay = Some(Overlay::login());
    let (tx, mut replies) = unbounded_channel::<Action>();
    let mut events = EventStream::new();

    loop {
        terminal.draw(|f| ui::draw(f, &state, theme))?;
        let action = tokio::select! {
            ev = events.next() => match ev {
                Some(Ok(Event::Key(k))) => Action::Key(k),
                Some(Ok(_)) => continue,
                Some(Err(e)) => return Err(e.into()),
                None => return Ok(None),
            },
            Some(reply) = replies.recv() => reply,
        };
        if let Action::Api(ApiReply::LoggedIn { user_id }) = action {
            return Ok(Some(user_id));
        }
        for effect in update(&mut state, action) {
            // Everything else (fetches, the player) belongs to the daemon.
            if let Effect::Login { email, password } = effect {
                let (client, store, tx) = (client.clone(), store.clone(), tx.clone());
                tokio::spawn(async move {
                    let _ = tx.send(super::log_in(&client, &store, &email, &password).await);
                });
            }
        }
        if state.quit || state.overlay.is_none() {
            return Ok(None);
        }
    }
}
