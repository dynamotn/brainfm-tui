//! A TUI attached to another window's session: it draws the host's state
//! and forwards key presses, so every window shows the same thing.

use crossterm::event::{Event, EventStream, KeyEventKind};
use futures::StreamExt;

use crate::share::{Client, is_local_quit};
use crate::ui::{self, Theme};

enum Ended {
    ByUser,
    HostGone,
}

pub(super) async fn run(client: Box<Client>) -> anyhow::Result<()> {
    let (first, mut states, mut keys) = client.split();
    let Some(mut state) = first else {
        anyhow::bail!("the brainfm session in the other window has ended");
    };
    let theme = Theme::detect();
    let mut terminal = ratatui::init();
    let mut events = EventStream::new();

    let result: anyhow::Result<Ended> = async {
        loop {
            terminal.draw(|f| ui::draw(f, &state, &theme))?;
            tokio::select! {
                ev = events.next() => match ev {
                    Some(Ok(Event::Key(k))) if k.kind != KeyEventKind::Release => {
                        if is_local_quit(&state, &k) {
                            return Ok(Ended::ByUser);
                        }
                        if keys.send(k).await.is_err() {
                            return Ok(Ended::HostGone);
                        }
                    }
                    Some(Ok(_)) => {}
                    Some(Err(e)) => return Err(e.into()),
                    None => return Ok(Ended::ByUser),
                },
                next = states.recv() => match next {
                    Some(Ok(Some(s))) => state = s,
                    Some(Err(e)) => return Err(e.into()),
                    Some(Ok(None)) | None => return Ok(Ended::HostGone),
                },
            }
        }
    }
    .await;

    ratatui::restore();
    if let Ended::HostGone = result? {
        eprintln!("The brainfm session in the other window has ended.");
    }
    Ok(())
}
