//! A TUI attached to the daemon's session: it draws the daemon's state and
//! forwards key presses, so every window shows the same thing. `Q` stops
//! the daemon; `L` logs in again here when the session has expired.

use crossterm::event::{Event, EventStream, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use futures::StreamExt;
use ratatui::DefaultTerminal;
use tokio::sync::mpsc::Receiver;
use tokio::sync::watch;

use super::login;
use super::state::AppState;
use crate::api::ApiClient;
use crate::session::SessionStore;
use crate::share::{Client, is_local_quit};
use crate::ui::{self, Theme};

/// How a view ended.
pub(super) enum Ended {
    /// `q` or Ctrl-C: this window closed, the daemon plays on.
    ByUser,
    /// The daemon went away after this window asked it to stop.
    Stopped,
    /// The daemon went away on its own.
    HostGone,
}

type States = Receiver<std::io::Result<Option<AppState>>>;

/// `Q` or `L`, as typed with or without Shift.
fn is_capital(k: &KeyEvent, c: char) -> bool {
    k.code == KeyCode::Char(c)
        && !k
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
}

pub(super) async fn run(
    client: Box<Client>,
    terminal: &mut DefaultTerminal,
    theme: &Theme,
    store: &SessionStore,
    api: &ApiClient,
) -> anyhow::Result<Ended> {
    let (first, mut states, mut keys) = client.split();
    let Some(mut state) = first else {
        anyhow::bail!("The background player stopped.");
    };
    let mut events = EventStream::new();
    let mut stopping = false;
    let gone = |stopping: bool| {
        if stopping {
            Ended::Stopped
        } else {
            Ended::HostGone
        }
    };

    loop {
        terminal.draw(|f| ui::draw(f, &state, theme))?;
        tokio::select! {
            ev = events.next() => match ev {
                Some(Ok(Event::Key(k))) if k.kind != KeyEventKind::Release => {
                    if is_local_quit(&state, &k) {
                        return Ok(Ended::ByUser);
                    }
                    let sent = if state.overlay.is_none() && is_capital(&k, 'Q') {
                        stopping = true;
                        keys.stop().await
                    } else if state.overlay.is_none() && is_capital(&k, 'L') && state.needs_login {
                        // Leaves `events` idle while the form reads the terminal.
                        match log_in(terminal, theme, store, api, &mut states, &mut state).await? {
                            Some(login::Outcome::LoggedIn(_)) => keys.reload().await,
                            Some(login::Outcome::Closed) => Ok(()),
                            Some(login::Outcome::Quit) => return Ok(Ended::ByUser),
                            None => return Ok(gone(stopping)),
                        }
                    } else {
                        keys.send(k).await
                    };
                    if sent.is_err() {
                        return Ok(gone(stopping));
                    }
                }
                Some(Ok(_)) => {}
                Some(Err(e)) => return Err(e.into()),
                None => return Ok(Ended::ByUser),
            },
            next = states.recv() => match next {
                Some(Ok(Some(s))) => state = s,
                Some(Err(e)) => return Err(e.into()),
                Some(Ok(None)) | None => return Ok(gone(stopping)),
            },
        }
    }
}

/// Runs the login form while still taking the daemon's states, so the
/// connection stays alive. `None` when the daemon went away meanwhile.
async fn log_in(
    terminal: &mut DefaultTerminal,
    theme: &Theme,
    store: &SessionStore,
    api: &ApiClient,
    states: &mut States,
    state: &mut AppState,
) -> anyhow::Result<Option<login::Outcome>> {
    let (elsewhere, elsewhere_rx) = watch::channel(false);
    let follow = async {
        loop {
            match states.recv().await {
                Some(Ok(Some(s))) => {
                    if !s.needs_login {
                        elsewhere.send_replace(true);
                    }
                    *state = s;
                }
                Some(Err(e)) => return Err(anyhow::Error::from(e)),
                Some(Ok(None)) | None => return Ok(None),
            }
        }
    };
    tokio::select! {
        r = login::run(terminal, theme, store, api, true, elsewhere_rx) => r.map(Some),
        r = follow => r,
    }
}
