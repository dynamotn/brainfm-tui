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
use std::sync::{Arc, Mutex};
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
/// How long `request_stop` waits for the host to go away.
const STOP_WAIT: Duration = Duration::from_secs(5);

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "t")]
enum ToHost {
    Hello {
        version: String,
    },
    Key {
        key: KeyEvent,
    },
    /// Frozen wire shape: `{"t":"Stop"}` is valid as the first line of a
    /// connection, with no `Hello` and no version check.
    Stop,
    Reload,
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
        // Serialize check, remove and bind across processes, so a loser of
        // a stale-file race cannot delete the winner's fresh socket.
        let mut lock_name = path.as_os_str().to_owned();
        lock_name.push(".lock");
        let lock = {
            use std::os::unix::fs::OpenOptionsExt;
            std::fs::OpenOptions::new()
                .create(true)
                .truncate(false)
                .write(true)
                .mode(0o600)
                .open(PathBuf::from(lock_name))?
        };
        lock.lock()?;
        if path.exists() {
            if std::os::unix::net::UnixStream::connect(path).is_ok() {
                return Err(io::Error::new(
                    io::ErrorKind::AddrInUse,
                    "another brainfm is serving this socket",
                ));
            }
            std::fs::remove_file(path)?;
        }
        // `lock` is released when it drops, after the bind.
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
        let views = Arc::new(Mutex::new(0));
        loop {
            match self.listener.accept().await {
                Ok((stream, _)) => {
                    tokio::spawn(serve_view(
                        stream,
                        states.clone(),
                        actions.clone(),
                        views.clone(),
                    ));
                }
                Err(e) => tracing::warn!("accepting a view: {e}"),
            }
        }
    }

    /// Answers every view with `Busy(reason)` (used by `brainfm play`),
    /// except a `Stop`, which is forwarded on `stop`.
    pub(crate) async fn refuse(self, reason: impl Into<String>, stop: UnboundedSender<()>) {
        let reply: Arc<str> = line(&ToClient::Busy {
            reason: reason.into(),
        })
        .into();
        loop {
            if let Ok((stream, _)) = self.listener.accept().await {
                tokio::spawn(refuse_one(stream, reply.clone(), stop.clone()));
            }
        }
    }
}

async fn refuse_one(stream: UnixStream, reply: Arc<str>, stop: UnboundedSender<()>) {
    let (read, mut write) = stream.into_split();
    let mut lines = BufReader::new(read).lines();
    if let Ok(Ok(Some(l))) = tokio::time::timeout(HELLO_TIMEOUT, lines.next_line()).await
        && matches!(serde_json::from_str::<ToHost>(&l), Ok(ToHost::Stop))
    {
        let _ = stop.send(());
        return;
    }
    let _ = write.write_all(reply.as_bytes()).await;
}

fn state_line(json: &str) -> String {
    // States are published pre-serialized, so wrap them by hand.
    format!("{{\"t\":\"State\",\"state\":{json}}}\n")
}

/// Changes the view count and reports it under the same lock, so the
/// reducer sees counts in the order they were reached.
fn count_view(views: &Mutex<usize>, actions: &UnboundedSender<Action>, delta: isize) {
    let mut n = views
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    *n = n.saturating_add_signed(delta);
    let _ = actions.send(Action::Views(*n));
}

async fn serve_view(
    stream: UnixStream,
    mut states: watch::Receiver<String>,
    actions: UnboundedSender<Action>,
    views: Arc<Mutex<usize>>,
) {
    let (read, mut write) = stream.into_split();
    let mut lines = BufReader::new(read).lines();
    match tokio::time::timeout(HELLO_TIMEOUT, lines.next_line()).await {
        Ok(Ok(Some(l))) => match serde_json::from_str::<ToHost>(&l) {
            Ok(ToHost::Hello { version }) if version == VERSION => {}
            Ok(ToHost::Hello { version }) => {
                let reason = format!(
                    "brainfm {VERSION} is running; this is {version}. Run `brainfm stop` first"
                );
                let _ = write
                    .write_all(line(&ToClient::Busy { reason }).as_bytes())
                    .await;
                return;
            }
            Ok(ToHost::Stop) => {
                let _ = actions.send(Action::Shutdown);
                return;
            }
            _ => return,
        },
        _ => return,
    }
    count_view(&views, &actions, 1);
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
        let action = match serde_json::from_str(&l) {
            Ok(ToHost::Key { key }) => Action::RemoteKey(key),
            Ok(ToHost::Stop) => Action::Shutdown,
            Ok(ToHost::Reload) => Action::ReloadSession,
            _ => continue,
        };
        if actions.send(action).is_err() {
            break;
        }
    }
    writer.abort();
    count_view(&views, &actions, -1);
}

/// Why [`Attach::NotResponding`] gives up.
pub(crate) const NOT_RESPONDING: &str = "the running brainfm is not responding";

/// What a new TUI finds at the socket.
#[derive(Debug)]
pub(crate) enum Attach {
    /// Nobody is playing: become the host.
    NoHost,
    /// Something else holds the session and will not share it.
    Busy(String),
    /// Something holds the socket but sent nothing in time (it may still be
    /// starting up); [`NOT_RESPONDING`] says so to the user.
    NotResponding,
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
        Err(_) => Ok(Attach::NotResponding),
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

    /// The state the host sent on attaching, until [`Client::split`].
    pub(crate) fn first_state(&self) -> Option<&AppState> {
        self.first.as_ref()
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

/// What `brainfm stop` found.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum StopOutcome {
    NothingRunning,
    Stopped,
    NoAnswer(String),
}

/// Connects to `path`; `None` when nothing is listening (a stale socket
/// file is removed).
async fn connect_live(path: &Path) -> io::Result<Option<UnixStream>> {
    match UnixStream::connect(path).await {
        Ok(s) => Ok(Some(s)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) if e.kind() == io::ErrorKind::ConnectionRefused => {
            let _ = std::fs::remove_file(path);
            Ok(None)
        }
        Err(e) => Err(e),
    }
}

/// Asks the host at `path` to stop and waits until its socket is gone.
pub(crate) async fn request_stop(path: &Path) -> io::Result<StopOutcome> {
    let Some(mut stream) = connect_live(path).await? else {
        return Ok(StopOutcome::NothingRunning);
    };
    stream.write_all(line(&ToHost::Stop).as_bytes()).await?;
    drop(stream);
    let wait = async {
        while UnixStream::connect(path).await.is_ok() {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    };
    Ok(match tokio::time::timeout(STOP_WAIT, wait).await {
        Ok(()) => StopOutcome::Stopped,
        Err(_) => StopOutcome::NoAnswer("brainfm did not stop within 5 s".into()),
    })
}

/// Asks a live host to reload its session; `false` when nothing runs or it
/// turned the request away.
pub(crate) async fn request_reload(path: &Path) -> io::Result<bool> {
    let Some(stream) = connect_live(path).await? else {
        return Ok(false);
    };
    let (read, mut write) = stream.into_split();
    let hello = line(&ToHost::Hello {
        version: VERSION.to_owned(),
    });
    write.write_all(hello.as_bytes()).await?;
    write.write_all(line(&ToHost::Reload).as_bytes()).await?;
    let mut lines = BufReader::new(read).lines();
    let answer = tokio::time::timeout(HELLO_TIMEOUT, lines.next_line()).await;
    Ok(matches!(
        answer
            .ok()
            .and_then(Result::ok)
            .flatten()
            .and_then(|l| serde_json::from_str::<ToClient>(&l).ok()),
        Some(ToClient::State { .. })
    ))
}

/// The writing half of an attached view.
#[derive(Debug)]
pub(crate) struct KeySender(OwnedWriteHalf);

impl KeySender {
    pub(crate) async fn send(&mut self, key: KeyEvent) -> io::Result<()> {
        send_key(&mut self.0, key).await
    }

    /// Asks the host to shut down.
    pub(crate) async fn stop(&mut self) -> io::Result<()> {
        self.0.write_all(line(&ToHost::Stop).as_bytes()).await
    }

    /// Asks the host to reload its session.
    pub(crate) async fn reload(&mut self) -> io::Result<()> {
        self.0.write_all(line(&ToHost::Reload).as_bytes()).await
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
