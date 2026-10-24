# Architecture

brainfm is a single Rust binary. The library target (`src/lib.rs`) exists so
integration tests can reach the internals; only what `main.rs` and `tests/`
use is public.

## Map

| Module | Responsibility |
|---|---|
| `main.rs`, `cli.rs` | Argument parsing, the `login`/`logout`/`whoami`/`list`/`stop` commands, exit codes. On macOS the main thread runs a `CFRunLoop` (Now Playing needs it) and tokio runs on another thread; a panic there exits 101. |
| `api/` | Typed client for `api.brain.fm`: requests, response envelopes, errors. |
| `session.rs`, `secret.rs` | The stored token, and a string type for passwords that never prints. |
| `config.rs` | `config.toml`: loading, atomic and debounced saving. |
| `app/` | State, reducer and event loop. `daemon.rs` is the hidden `brainfm daemon` that owns playback; `view.rs` is the TUI as a view of it; `login.rs` is the local login form; `spawn.rs` starts the daemon. |
| `ui/` | Drawing `AppState` with ratatui; snapshot tests and screenshot rendering. |
| `player/` | The audio engine thread: streaming, decoding, gapless queueing. |
| `timer.rs` | Infinite, countdown and interval timers. |
| `media/` | Desktop media controls: an MPRIS server on Linux, Now Playing on macOS (its commands are registered on the main thread, or Control Center greys out all but play/pause). |
| `share/` | The Unix socket between the daemon and its views, and the stop/reload requests. |
| `headless/` | `brainfm play`: a state machine plus the loop that runs it. |

## The TUI: state, actions, effects

The TUI follows one rule: **`app::update` is a pure function.** It takes the
current `AppState` and one `Action`, changes the state, and returns a list of
`Effect`s describing the I/O to do. It never touches the network, the
terminal, the clock or the audio device. Almost all behaviour is therefore
testable with plain unit tests (`src/app/tests.rs`).

```mermaid
flowchart LR
    term[Terminal keys] --> loop
    tick[250 ms tick] --> loop
    eng[Player events] --> loop
    api[API replies] --> loop
    keys[Media keys] --> loop
    views[Keys from views] --> loop
    loop((event loop)) -->|Action| update[update]
    update -->|new AppState| draw[ui::draw]
    update -->|Effect list| exec[Runtime::exec]
    exec -->|HTTP| apicall[ApiClient]
    exec -->|PlayerCommand| player[Audio engine]
    apicall -.->|ApiReply| api
    player -.->|PlayerEvent| eng
```

`app::run` owns the loop. Each turn it:

1. draws the current state;
2. waits for the next action;
3. runs `update` and executes the returned effects;
4. publishes the state to other windows and the media controls.

Effects that call the API run as tokio tasks and report back as
`Action::Api(...)`.

### Requests that can go stale

Several things can be in flight at once: a skip, a prefetch, a track the
engine is still opening. Two counters keep late answers from undoing newer
ones:

- `session_gen` is bumped by every session request. A reply or failure with
  an older generation is ignored.
- A track sent to the engine waits in `pending` until the engine reports
  `Started`; only then does it become `now`. Events about any other track are
  ignored.

When a request fails, playback returns to what the engine is still doing
(`resume_to`), so the screen never claims to be playing silence. A watchdog
gives up on `Loading` after 30 seconds.

## The audio engine

`player::spawn` starts a dedicated thread that owns the output device and a
rodio `Player`. The rest of the program talks to it through `PlayerCommand`s
(`Load`, `Queue`, `Play`, `Pause`, `Stop`, `SetVolume`, `Chime`,
`CancelPending`) and hears back through `PlayerEvent`s (`Started`,
`Position`, `NearEnd`, `Ended`, `Error`).

- **Streaming.** stream-download fetches the track into memory while it
  plays, so nothing is written to disk. Opening a stream can stall on the
  network, so it runs on tokio's blocking pool. The source comes back tagged
  with a generation, and anything superseded meanwhile is dropped.
- **Gapless.** 20 seconds before a track ends the engine sends `NearEnd`, the
  app fetches the next track, and `Queue` appends it to the same rodio queue.
- **Output.** cpal's default host prefers native PipeWire (when built with
  the `pipewire` feature), then PulseAudio, then ALSA. `BRAINFM_AUDIO_HOST`
  overrides it (`player::choose_host`).
- **Shutdown.** Dropping the last `PlayerHandle` stops the thread and closes
  the device.

## The API client

`api::ApiClient` mirrors what the web app sends:

- the same `Origin` and `Referer` headers;
- version 2 or 3 paths, as the web app uses them;
- a bearer token.

Every response is a `{status, result, messages, token}` envelope. A new
`token` replaces the stored one, which slides the session forward. Ids are
checked against `[A-Za-z0-9_-]` before they go into a URL path.

`ApiError` is the only typed error in the crate, because callers branch on
it: an expired session opens the login form, while other errors show a
message. Everything else uses `anyhow` with context.

## The daemon and its views: `share/`

Playback lives in `brainfm daemon`, a hidden subcommand that runs the same
reducer and event loop as the TUI but with no terminal. It binds
`$XDG_RUNTIME_DIR/brainfm/tui.sock` and becomes the **host**. Every `brainfm`
window is a **view** (`app/view.rs`): it logs in locally if needed, attaches to
the socket, or spawns the daemon first and then attaches. The daemon, the
socket and spawning are `cfg(unix)`; elsewhere the TUI keeps running as a
single process.

```mermaid
sequenceDiagram
    participant V as View (TUI window)
    participant D as Daemon (host)
    V->>D: Hello {version}
    D-->>V: State {whole AppState}
    V->>D: Key {key event}
    Note over D: Action::RemoteKey → update
    D-->>V: State {…} after every change
    V->>D: Reload (after a login)
    V->>D: Stop (Q)
    Note over D: Shutdown → socket closes → views exit
```

- The protocol is one JSON object per line. `ToHost` is `Hello`, `Key`,
  `Reload` and `Stop`.
- The host publishes the serialized state through a `watch` channel, so a
  slow view only ever receives the latest state.
- Views draw that state with the same `ui::draw`, at their own terminal size.
- Remote keys arrive as `Action::RemoteKey`. The reducer ignores quit keys
  from them, so `q` closes a view and never the host. `Q` is sent as `Stop`.
- `Stop` is accepted as the first line of a connection, with no version
  check. Its shape (`{"t":"Stop"}`) is frozen so that any release can stop any
  other. It becomes `Action::Shutdown`. `request_stop` waits up to 5 seconds
  for the socket to go away and treats a missing or stale socket as nothing to
  stop.
- `Reload` becomes `Action::ReloadSession`: the daemon re-reads the token
  from the session store. It is idempotent, so two views logging in at once is
  harmless.
- The password never crosses the socket. A view logs in on its own
  (`app/login.rs`), the token lands in the session store, and only then does
  it send `Reload`.
- When the daemon's API calls come back `Unauthorized` it sets
  `needs_login`; every view shows the banner, and `L` opens the local form.
  A form that is already open never closes by itself if another view logs in
  first; it says so and waits for `Esc`.
- `Host::bind` takes a `<socket>.lock` file for the whole check-and-bind, so
  two views starting together end up with exactly one daemon; the loser exits
  quietly and both attach to the winner. A view that finds a host of a
  different version gets `Busy` with a request to run `brainfm stop`;
  `Attach::NotResponding` covers a socket that accepts but does not answer.
- `brainfm play` holds the socket as well and answers every view with
  `Busy`, so the TUI and the command line never play two streams. It obeys
  `Stop` and exits 0.

### Lifetime

The daemon starts detached from the terminal that spawned it and ignores
SIGHUP, so closing that terminal does not stop playback. It bails at startup
without a stored session, and logs errors to `brainfm.log` since it has no
stderr. It ends on `Stop`, or on its own after `IDLE_EXIT` (10 minutes)
with no views, nothing playing and no timer running. The idle check lives in
the reducer, driven by the tick, and applies only to the daemon (a
single-process TUI is never idle-closed).

## Media controls: `media/`

`media::Media` has the same async API on every platform: `start`,
`set_track`, `set_playback`, `set_position`, `set_volume`. Commands come back
as `MediaKey`s. `Mirror` remembers what was last pushed and sends only
changes. It keys tracks on (id, play count), so the same track played twice
still counts as a change.

On Linux, `media::mpris` is an MPRIS 2 server written with zbus.

- **Bus name.** It asks for `org.mpris.MediaPlayer2.brainfm` without
  replacing or queueing. If that name is taken, it falls back to
  `…brainfm.instance<pid>`.
- **Track ids.** Each track gets an object path under
  `/io/github/dynamotn/brainfm/track/`.
- **Position.** It is not signalled. Clients poll it, and the server
  interpolates from the last engine report.
- **Capabilities.** The `Can*` properties reflect what actually works: no
  previous, no seek.

## `brainfm play`: `headless/`

The command-line player has the same shape as the TUI, on a smaller scale.

- `headless::machine::Machine` holds the session. It takes events (player
  events, fetch results, media keys, ticks) and returns `Cmd`s.
- `headless::play` runs a `select!` loop that carries those out.

Track fetches run in the background and are tagged with an epoch. A skip, or
a reload after an error or a natural end, bumps the epoch, so late fetches
are dropped. Two independent flags in `Pauses` decide silence: a user pause,
and an interval break. The timer runs except during a user pause.

## Security notes

- The token is stored with mode `0600` in a `0700` directory.
- The password is never stored. It lives in a `Secret`, which never appears
  in `Debug` output and is zeroized on drop.
- Signed audio URLs carry short-lived credentials in their query strings.
  Error messages cut those query strings off (`player::redact_urls`), and
  stream-download's own logging is switched off.
- Text from the API is stripped of control characters before it is printed by
  the command line. The TUI draws through ratatui, which drops them anyway.
- Audio is fetched over HTTPS only.
