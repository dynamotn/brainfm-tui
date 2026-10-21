//! One listening session shared by every TUI on the machine.
//!
//! The first `brainfm` binds a Unix socket and becomes the host: it owns the
//! player, the API client and the state. Later TUIs attach as views: they
//! send key presses and draw whatever state the host publishes, so every
//! window shows the same session and only one audio stream ever plays.
//! `brainfm play` holds the socket too, turning TUIs away with a reason.
//!
//! The protocol is one JSON object per line.

use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crossterm::event::KeyEvent;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::watch;

use crate::app::state::AppState;
use crate::app::update::Action;
pub(crate) use crate::app::update::is_quit_key as is_local_quit;

const VERSION: &str = env!("CARGO_PKG_VERSION");
/// How long a new view waits for the host's first state or answer.
const HELLO_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "t")]
enum ToHost {
    Hello { version: String },
    Key { key: KeyEvent },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "t")]
enum ToClient {
    State { state: Box<AppState> },
    Busy { reason: String },
}

/// `$XDG_RUNTIME_DIR/brainfm/tui.sock`, else `<state dir>/tui.sock`.
pub(crate) fn socket_path_in(runtime_dir: Option<PathBuf>, state_dir: &Path) -> PathBuf {
    match runtime_dir.filter(|d| !d.as_os_str().is_empty()) {
        Some(d) => d.join("brainfm").join("tui.sock"),
        None => state_dir.join("tui.sock"),
    }
}

pub(crate) fn socket_path() -> anyhow::Result<PathBuf> {
    Ok(socket_path_in(
        std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from),
        &crate::config::state_dir()?,
    ))
}

fn line<T: Serialize>(msg: &T) -> String {
    let mut s = serde_json::to_string(msg).expect("protocol messages serialize");
    s.push('\n');
    s
}

/// The listening side; held by the TUI that owns the session or by `play`.
#[derive(Debug)]
pub(crate) struct Host {
    listener: UnixListener,
}

impl Host {
    /// Binds `path` in a private directory. A socket file left by a crashed
    /// process is replaced; a live one is reported as `AddrInUse`.
    pub(crate) fn bind(path: &Path) -> io::Result<Self> {
        if let Some(dir) = path.parent() {
            crate::session::ensure_private_dir(dir).map_err(io::Error::other)?;
        }
        if path.exists() {
            if std::os::unix::net::UnixStream::connect(path).is_ok() {
                return Err(io::Error::new(
                    io::ErrorKind::AddrInUse,
                    "another brainfm is serving this socket",
                ));
            }
            std::fs::remove_file(path)?;
        }
        Ok(Self {
            listener: UnixListener::bind(path)?,
        })
    }

    /// Serves views until the process ends: each gets every published state
    /// and its keys arrive as [`Action::RemoteKey`].
    pub(crate) async fn serve(
        self,
        states: watch::Receiver<String>,
        actions: UnboundedSender<Action>,
    ) {
        loop {
            match self.listener.accept().await {
                Ok((stream, _)) => {
                    tokio::spawn(serve_view(stream, states.clone(), actions.clone()));
                }
                Err(e) => tracing::warn!("accepting a view: {e}"),
            }
        }
    }

    /// Answers every view with `Busy(reason)` (used by `brainfm play`).
    pub(crate) async fn refuse(self, reason: impl Into<String>) {
        let reply = line(&ToClient::Busy {
            reason: reason.into(),
        });
        loop {
            if let Ok((mut stream, _)) = self.listener.accept().await {
                let _ = stream.write_all(reply.as_bytes()).await;
            }
        }
    }
}

fn state_line(json: &str) -> String {
    // States are published pre-serialized, so wrap them by hand.
    format!("{{\"t\":\"State\",\"state\":{json}}}\n")
}

async fn serve_view(
    stream: UnixStream,
    mut states: watch::Receiver<String>,
    actions: UnboundedSender<Action>,
) {
    let (read, mut write) = stream.into_split();
    let mut lines = BufReader::new(read).lines();
    match tokio::time::timeout(HELLO_TIMEOUT, lines.next_line()).await {
        Ok(Ok(Some(l))) => match serde_json::from_str::<ToHost>(&l) {
            Ok(ToHost::Hello { version }) if version == VERSION => {}
            Ok(ToHost::Hello { version }) => {
                let reason = format!("brainfm {VERSION} is running; this is {version}");
                let _ = write
                    .write_all(line(&ToClient::Busy { reason }).as_bytes())
                    .await;
                return;
            }
            _ => return,
        },
        _ => return,
    }
    let writer = tokio::spawn(async move {
        loop {
            let json = states.borrow_and_update().clone();
            if !json.is_empty() && write.write_all(state_line(&json).as_bytes()).await.is_err() {
                break;
            }
            if states.changed().await.is_err() {
                break;
            }
        }
    });
    while let Ok(Some(l)) = lines.next_line().await {
        if let Ok(ToHost::Key { key }) = serde_json::from_str(&l)
            && actions.send(Action::RemoteKey(key)).is_err()
        {
            break;
        }
    }
    writer.abort();
}

/// What a new TUI finds at the socket.
#[derive(Debug)]
pub(crate) enum Attach {
    /// Nobody is playing: become the host.
    NoHost,
    /// Something else holds the session and will not share it.
    Busy(String),
    /// Attached to a running session.
    Session(Box<Client>),
}

/// Connects to a running host, if any. A dead socket file is removed.
pub(crate) async fn attach(path: &Path) -> io::Result<Attach> {
    let stream = match UnixStream::connect(path).await {
        Ok(s) => s,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Attach::NoHost),
        Err(e) if e.kind() == io::ErrorKind::ConnectionRefused => {
            let _ = std::fs::remove_file(path);
            return Ok(Attach::NoHost);
        }
        Err(e) => return Err(e),
    };
    let (read, mut write) = stream.into_split();
    let hello = line(&ToHost::Hello {
        version: VERSION.to_owned(),
    });
    write.write_all(hello.as_bytes()).await?;
    let mut client = Client {
        lines: BufReader::new(read).lines(),
        write,
        first: None,
    };
    match tokio::time::timeout(HELLO_TIMEOUT, client.read()).await {
        Ok(Ok(Some(ToClient::State { state }))) => {
            client.first = Some(*state);
            Ok(Attach::Session(Box::new(client)))
        }
        Ok(Ok(Some(ToClient::Busy { reason }))) => Ok(Attach::Busy(reason)),
        Ok(Ok(None)) => Ok(Attach::Busy(
            "the running brainfm closed the connection".into(),
        )),
        Ok(Err(e)) => Err(e),
        Err(_) => Ok(Attach::Busy("the running brainfm is not responding".into())),
    }
}

/// A view attached to someone else's session.
#[derive(Debug)]
pub(crate) struct Client {
    lines: Lines<BufReader<OwnedReadHalf>>,
    write: OwnedWriteHalf,
    first: Option<AppState>,
}

impl Client {
    async fn read(&mut self) -> io::Result<Option<ToClient>> {
        let Some(l) = self.lines.next_line().await? else {
            return Ok(None);
        };
        serde_json::from_str(&l).map(Some).map_err(io::Error::other)
    }

    /// The next state from the host; `None` once the host is gone.
    #[cfg(test)]
    pub(crate) async fn next_state(&mut self) -> io::Result<Option<AppState>> {
        if let Some(s) = self.first.take() {
            return Ok(Some(s));
        }
        match self.read().await? {
            Some(ToClient::State { state }) => Ok(Some(*state)),
            Some(ToClient::Busy { .. }) | None => Ok(None),
        }
    }

    #[cfg(test)]
    pub(crate) async fn send_key(&mut self, key: KeyEvent) -> io::Result<()> {
        send_key(&mut self.write, key).await
    }

    /// Splits into the first state, a stream of later states (`None` when
    /// the host is gone) fed by a background task, and a key sender, so a
    /// view can wait on both at once.
    pub(crate) fn split(
        mut self,
    ) -> (
        Option<AppState>,
        tokio::sync::mpsc::Receiver<io::Result<Option<AppState>>>,
        KeySender,
    ) {
        let first = self.first.take();
        let (tx, rx) = tokio::sync::mpsc::channel(4);
        let mut lines = self.lines;
        tokio::spawn(async move {
            loop {
                let next = match lines.next_line().await {
                    Ok(None) => Ok(None),
                    Ok(Some(l)) => match serde_json::from_str::<ToClient>(&l) {
                        Ok(ToClient::State { state }) => Ok(Some(*state)),
                        Ok(ToClient::Busy { .. }) => Ok(None),
                        Err(e) => Err(io::Error::other(e)),
                    },
                    Err(e) => Err(e),
                };
                let done = !matches!(next, Ok(Some(_)));
                if tx.send(next).await.is_err() || done {
                    break;
                }
            }
        });
        (first, rx, KeySender(self.write))
    }
}

async fn send_key(write: &mut OwnedWriteHalf, key: KeyEvent) -> io::Result<()> {
    write.write_all(line(&ToHost::Key { key }).as_bytes()).await
}

/// The writing half of an attached view.
#[derive(Debug)]
pub(crate) struct KeySender(OwnedWriteHalf);

impl KeySender {
    pub(crate) async fn send(&mut self, key: KeyEvent) -> io::Result<()> {
        send_key(&mut self.0, key).await
    }
}

/// Removes the socket file when the host goes away cleanly.
#[derive(Debug)]
pub(crate) struct SocketGuard(pub(crate) PathBuf);

impl Drop for SocketGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[cfg(test)]
mod tests;
