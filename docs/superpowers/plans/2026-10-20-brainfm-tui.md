# brainfm TUI/CLI Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A Rust terminal client for Brain.fm. It has a full-screen TUI that mirrors the desktop app and a scriptable CLI, and it plays audio in-process.

**Architecture:** One binary crate. A typed `api` client talks to `api.brain.fm` with a sliding JWT. A `player` thread streams mp3 into a memory buffer and plays it through rodio. A pure reducer (`app::update`) turns actions into state changes plus a list of effects, and a tokio event loop runs those effects and redraws a ratatui UI.

**Tech Stack:** Rust 2024 (rustc 1.97), ratatui 0.30, crossterm 0.29, tui-big-text 0.8, tokio 1, reqwest 0.13 (rustls), serde, rodio 0.23, stream-download 0.24, souvlaki 0.8, clap 4, directories 6, toml, thiserror 2, anyhow, tracing; tests use wiremock 0.6, insta 1, tempfile.

**Spec:** `docs/superpowers/specs/2026-10-20-brainfm-tui-design.md`

## Global Constraints

- API base `https://api.brain.fm`. Auth and user endpoints are under `/v2`, music endpoints under `/v3`, each as the spec lists it.
- Every request sends `Origin: https://my.brain.fm`, `Referer: https://my.brain.fm/`, `Accept: */*`, a Firefox `User-Agent`, and `Authorization: Bearer <jwt>` when a token is present.
- If any response body has a non-empty `token`, it replaces the stored token and is persisted.
- An id interpolated into a URL path must match `^[A-Za-z0-9_-]+$`. Anything else is rejected before sending.
- Never write audio to disk: stream-download uses its memory storage provider only.
- Never store the password. Never log or print the token or the password. `Debug` for `Session` redacts the token.
- The session file is `~/.local/state/brainfm/session.json` (the `directories` state dir), mode `0600`, inside a `0700` directory, written atomically (temp file + rename).
- The config file is `~/.config/brainfm/config.toml`.
- Logs go to a file under the state dir through `tracing-appender`, never to stdout or stderr while the TUI is up.
- Neural effect levels go over the wire as `"Low" | "Medium" | "High"`. A float level maps to Low ≤ 0.33 < Medium ≤ 0.66 < High.
- Palette: bg `#0d0c11`, surface `#1d1926`, surface-hi `#322c44`, text `#ffffff`, text-dim `#d4d2ea`, pink `#f34675`, purple `#9130bb`, orange `#f39946`, cyan `#0bb9ff`.
- No telemetry: never call `/listening/*`.
- Commit messages follow Conventional Commits with no agent attribution trailers. Do not create a CHANGELOG.
- `unsafe_code = "forbid"`. Code must pass `cargo fmt --check` and `cargo clippy --all-targets -- -D warnings`.

## Review Focus

1. **The token expires mid-session** (paused for an hour, then skip). A 401 must open the login prompt in the TUI and tell the CLI user to run `brainfm login`. It must never crash or silently stop. Pinned in Task 6 (`unauthorized_opens_login`) and Task 2 (`maps_401_to_unauthorized`).
2. **The stream breaks or a track URL fails.** The player reports an error, the app shows a toast and starts the next track. It must not hang on "Loading". Pinned in Task 6 (`player_error_skips_to_next`).
3. **The terminal is tiny or gets resized** (e.g. 40×10). No panic; show "Terminal too small (need 60×20)". Pinned in Task 7 (`renders_too_small_message`).
4. **API shape drift**, such as extra fields, a missing `imageUrl`, or a null `tokenedUrl`. It must still deserialize, and the stream URL falls back to `https://audio.brain.fm/{url}`. Pinned in Task 2 (`tolerates_extra_and_missing_fields`).
5. **The filters return no servings, or the config file is malformed.** Show a toast ("No tracks match these filters"). A malformed config means the TUI starts on defaults with a toast naming the file and never overwrites it, and the CLI exits with an error naming the file. Pinned in Task 6 (`empty_session_shows_toast`) and Task 3 (`malformed_config_is_error_with_path`).

---

## File Structure

```
Cargo.toml, rustfmt.toml, .gitignore, .pre-commit-config.yaml, README.md
src/main.rs            clap parse → cli::run or app::run
src/cli.rs             Cli/Command definitions + login/logout/whoami/list/play handlers
src/api/mod.rs         re-exports
src/api/models.rs      serde types + EffectLevel + Track helpers
src/api/client.rs      ApiClient, ApiError
src/session.rs         Session, SessionStore, user_id_from_jwt
src/config.rs          Config, StatePrefs, TimerPrefs
src/timer.rs           Timer state machine
src/player/mod.rs      PlayerCommand, PlayerEvent, PlayerHandle::spawn
src/player/engine.rs   rodio + stream-download thread
src/app/mod.rs         run() event loop + effect executor
src/app/state.rs       AppState and sub-structs
src/app/update.rs      Action, Effect, update()
src/ui/mod.rs          draw()
src/ui/theme.rs        Theme, gradient
src/ui/{header,player,panel,overlay,footer}.rs
src/media.rs           souvlaki MPRIS bridge
tests/api.rs           wiremock tests
```

---

### Task 1: Scaffold, tooling, CLI skeleton

**Files:**
- Create: `Cargo.toml`, `rustfmt.toml` (`max_width = 100`), `.gitignore` (`/target`), `.pre-commit-config.yaml`, `src/main.rs`, `src/cli.rs`
- Test: `tests/cli.rs`

**Interfaces:**
- Produces: `cli::Cli` (clap `Parser`) with `command: Option<Command>`. `Command` has these variants:
  - `Login { #[arg(long)] token: bool }`
  - `Logout`
  - `Whoami`
  - `List { what: ListWhat, #[arg(long)] state: Option<String> }`
  - `Play(PlayArgs)`
- `ListWhat` is `States | Activities | Genres`.
- `PlayArgs` is `{ state: String, activity: Option<String>, effect: Option<EffectArg>, genre: Vec<String>, timer: Option<humantime::Duration>, intervals: Option<String> }`. `timer` and `intervals` conflict with each other.
- `pub async fn cli::run(cmd: Command) -> anyhow::Result<()>`. Its arms are `todo!()` until later tasks fill them.

- [ ] **Step 1:** `cargo init --name brainfm`.
  - Set `edition = "2024"`.
  - Add every dependency from Tech Stack at the versions listed:
    - crossterm with `event-stream`.
    - reqwest with `default-features=false, features=["json","rustls-tls"]`. Use whatever feature name 0.13 exposes for rustls.
    - tokio with `full`.
    - rodio with default features plus mp3 through symphonia.
    - stream-download with its `reqwest` feature. If it pins reqwest 0.12, keep both versions rather than downgrade ours.
    - Plus humantime, base64 0.22, url 2, futures.
  - Add `[lints.rust] unsafe_code = "forbid"`.
- [ ] **Step 2:** Write `tests/cli.rs::help_lists_subcommands`. It runs `env!("CARGO_BIN_EXE_brainfm") --help` and asserts stdout contains `login`, `logout`, `whoami`, `list` and `play`.
- [ ] **Step 3:** Run `cargo test --test cli`. Expected: FAIL.
- [ ] **Step 4:** Implement `Cli`/`Command` and `main` (`#[tokio::main]`). No subcommand calls `app::run()`, which is a stub returning `Ok(())` for now.
- [ ] **Step 5:** Run `cargo test --test cli`. Expected: PASS.
- [ ] **Step 6:** Write `.pre-commit-config.yaml` with `repo: local` hooks:
  - `cargo fmt --check`
  - `cargo clippy --all-targets -- -D warnings`
  - `cargo test`

  Each uses `language: system` and `pass_filenames: false`.

  Then run `prek install && prek run --all-files`. Expected: all pass.
- [ ] **Step 7:** Commit everything, including `docs/`: `chore: scaffold brainfm crate and tooling`.

### Task 2: API models and client

**Files:**
- Create: `src/api/mod.rs`, `src/api/models.rs`, `src/api/client.rs`
- Test: `tests/api.rs`, plus unit tests in `models.rs`

**Interfaces:**
- Produces (`models.rs`, all `Deserialize, Clone, Debug, PartialEq`, camelCase, unknown fields ignored, `Option` + `#[serde(default)]` for anything optional):
  - `MentalState { id: String, display_value: String, default_activity: Option<Activity> }`
  - `Activity { id: String, display_value: String }`
  - `EffectLevel { Low, Medium, High }`, also `Serialize`, `Copy`, `Eq`, `Hash`, `Ord`. Provides `fn from_value(v: f64) -> Self` and `fn label(self) -> &'static str`.
  - `Tag { #[serde(rename="type")] kind: String, value: String }`
  - `Track { id: String, name: String, image_url: Option<String>, tags: Vec<Tag> }`, with:
    - `fn genre(&self) -> Option<&str>`: the first tag with `kind == "genre"` whose value is not `"Nature"`.
    - `fn moods(&self) -> Vec<&str>`
  - `TrackVariation { id: String, tokened_url: Option<String>, url: Option<String>, length_in_seconds: f64, neural_effect_level: Option<f64> }`, with:
    - `fn stream_url(&self) -> Option<String>`: `tokened_url`, else `format!("https://audio.brain.fm/{url}")`.
    - `fn length(&self) -> Duration`
    - `fn effect(&self) -> Option<EffectLevel>`
  - `Serving { track: Track, track_variation: TrackVariation }`
  - `SessionRequest { dynamic_activity_id: String, version: u8 /*=3*/, neural_effect_levels: Vec<EffectLevel>, genre_names: Vec<String> }`. `Serialize` with camelCase and `skip_serializing_if = "Vec::is_empty"` on both vecs.
  - `User { email: Option<String>, first_name: Option<String>, name: Option<String> }`
- Produces (`client.rs`):
  - `ApiError` (thiserror), with variants:
    - `Unauthorized`
    - `RateLimited`
    - `Status { code: u16, message: String }`
    - `InvalidId(String)`
    - `Network(#[from] reqwest::Error)`
    - `Decode(String)`
  - `pub type ApiResult<T> = Result<T, ApiError>`
  - `#[derive(Clone)] pub struct ApiClient`. The token is shared as `Arc<Mutex<Option<String>>>`.
    - `fn new(base: Url) -> Self`
    - `fn with_token(self, t: String) -> Self`
    - `fn on_token_refresh(self, f: impl Fn(&str) + Send + Sync + 'static) -> Self`
    - `fn token(&self) -> Option<String>`
  - `ApiClient` async methods:
    - `email_login(&self, email, password) -> ApiResult<String>`: POST `/v2/auth/email-login` `{email,password}`. Stores the returned token.
    - `me() -> ApiResult<User>`: `/v2/users/me`
    - `mental_states() -> ApiResult<Vec<MentalState>>`
    - `activities(state_id) -> ApiResult<Vec<Activity>>`
    - `genres(state_id) -> ApiResult<Vec<String>>`. Accepts either an array of strings or an array of objects that have `name` or `displayValue` (an untagged enum).
    - `start_session(uid, &SessionRequest) -> ApiResult<Vec<Serving>>`: `result.servings`
    - `favorites(uid) -> ApiResult<Vec<Serving>>`: `/v2/users/{uid}/servings/favorites?version=2`
    - `recents(uid) -> ApiResult<Vec<Serving>>`: `/v2/users/{uid}/servings/recent?platform=web`
    - `set_favorite(uid, track_id, on: bool) -> ApiResult<()>`: POST or DELETE `/v2/users/{uid}/tracks/{id}/favorites`
    - `dislike(uid, track_id) -> ApiResult<()>`: POST `/v2/users/{uid}/dislikes?version=3` `{trackId}`

  The favorites, recents, favorite and dislike paths default to `/v2`, as in the bundle's `DEFAULT_API_VERSION`.
- Status mapping:
  - 401/403 → `Unauthorized`
  - 429 → `RateLimited`
  - any other status ≥400 → `Status`, with `messages` joined
  - a body whose `result` is missing or null → `Decode("no result")`

- [ ] **Step 1:** Write the unit tests in `models.rs`:
  - `effect_from_value_thresholds`: `0.33→Low`, `0.34→Medium`, `0.66→Medium`, `0.67→High`.
  - `session_request_omits_empty_filters`: the JSON has no `genreNames` key when the vec is empty, and `neuralEffectLevels` serializes as `["High"]`.
  - `tolerates_extra_and_missing_fields`: a serving fixture with an extra `foo` field, no `imageUrl`, `tokenedUrl: null` and `url: "x/y.mp3"`. It deserializes, and `stream_url() == Some("https://audio.brain.fm/x/y.mp3")`.
  - `track_genre_skips_nature`: tags `[genre Nature, genre Piano, mood Calm]`. Assert `genre()==Some("Piano")` and `moods()==["Calm"]`.
- [ ] **Step 2:** Write the wiremock tests in `tests/api.rs`:
  - `sends_browser_headers_and_bearer`: assert `Origin`, `Referer` and `Authorization: Bearer t0` on `GET /v3/mentalStates/dynamic`.
  - `sliding_token_replaces_and_notifies`: the response includes `"token":"t1"`. Assert that `client.token()==Some("t1")` and that the callback received `"t1"`.
  - `maps_401_to_unauthorized`
  - `maps_429_to_rate_limited`
  - `login_posts_email_and_password`: a body matcher on `{"email":"a@b.c","password":"pw"}`; the method returns `"jwt"`.
  - `start_session_posts_body_and_returns_servings`: path `/v3/users/u1/sessions`, query `platform=web`.
  - `rejects_unsafe_ids`: `activities("../x")` returns `Err(InvalidId)` without hitting the server. Mount nothing and expect 0 calls.
- [ ] **Step 3:** Run `cargo test`. Expected: FAIL to compile.
- [ ] **Step 4:** Implement models and client. Use one private `async fn request<T: DeserializeOwned>(&self, method, version: u8, path: &str, body: Option<serde_json::Value>) -> ApiResult<T>`, which parses an `Envelope<T> { status, result: Option<T>, messages, token }`.
- [ ] **Step 5:** Run `cargo test`. Expected: PASS.
- [ ] **Step 6:** Commit: `feat(api): add typed Brain.fm API client`.

### Task 3: Session store, config, and auth/list CLI commands

**Files:**
- Create: `src/session.rs`, `src/config.rs`
- Modify: `src/cli.rs`
- Test: unit tests in `session.rs` and `config.rs`

**Interfaces:**
- Consumes: `ApiClient`, `MentalState`, `Activity`, `EffectLevel`.
- Produces:
  - `Session { token: String, user_id: String }`, with a hand-written `Debug` that prints `token: "<redacted>"`.
  - `fn user_id_from_jwt(token: &str) -> anyhow::Result<String>`: base64url-decodes the payload, no padding, and reads `_id`.
  - `SessionStore`:
    - `fn new(path: PathBuf) -> Self`
    - `fn default_path() -> anyhow::Result<PathBuf>`
    - `fn load(&self) -> anyhow::Result<Option<Session>>`: a missing file gives `None`. The file is `{"token": "..."}` and the user id is derived from the token.
    - `fn save(&self, token: &str) -> anyhow::Result<Session>`
    - `fn clear(&self) -> anyhow::Result<()>`
  - `Config { volume: u8 /*default 70*/, last_state: Option<String>, states: BTreeMap<String, StatePrefs>, timer: TimerPrefs }`
  - `StatePrefs { activity_id: Option<String>, genres: Vec<String>, effects: Vec<EffectLevel> }`
  - `TimerPrefs { custom_minutes: Vec<u32> /*≤3*/, focus_minutes: u32 /*25*/, break_minutes: u32 /*5*/ }`
  - `Config` methods: `fn default_path()`, `fn load(path: &Path) -> anyhow::Result<Config>`, `fn save(&self, path: &Path) -> anyhow::Result<()>`.
  - `pub fn cli::client_from_store(store: &SessionStore) -> anyhow::Result<(ApiClient, Session)>`. It wires `on_token_refresh` to `store.save`. With no session, it errors with "Not logged in — run `brainfm login`".
  - `pub fn cli::resolve_state<'a>(states: &'a [MentalState], q: &str) -> Option<&'a MentalState>`: matches `id` or `display_value` case-insensitively.
  - `cli::resolve_activity` does the same over `[Activity]`.

- [ ] **Step 1:** Write the tests:
  - `user_id_from_jwt_reads_underscore_id`: build `h.{base64url({"_id":"u1"})}.s` and expect `"u1"`.
  - `save_sets_0600_and_roundtrips`: uses tempfile. Mode `& 0o777 == 0o600`, and `load()` returns the same token.
  - `debug_redacts_token`: `format!("{:?}", s)` does not contain the token.
  - `missing_config_is_default`: volume is 70 and focus is 25.
  - `malformed_config_is_error_with_path`: the error string contains the file path.
  - `resolve_state_matches_case_insensitively`: `"FOCUS"` finds the display value `"Focus"`.
- [ ] **Step 2:** Run `cargo test`. Expected: FAIL.
- [ ] **Step 3:** Implement `session.rs` and `config.rs`. Then fill in the CLI arms:
  - `login`:
    - Prompt for the email on stdin.
    - Read the password with `rpassword` (add the dependency) so it is never echoed.
    - Call `email_login` and save the result.
    - `--token` reads one token line from stdin with no echo.
    - Print "Logged in as <email>" using `me()`.
  - `logout` calls `clear()`.
  - `whoami` prints the email and name.
  - `list states|activities|genres` prints one row per item: `id<TAB>displayValue`.
  - `Unauthorized` prints "Session expired — run `brainfm login`" and exits with code 2.
- [ ] **Step 4:** Run `cargo test` and `cargo run -- list states`. Expected: the tests PASS. Without a session, the command exits non-zero with the "Not logged in" message.
- [ ] **Step 5:** Commit: `feat(cli): add login, logout, whoami and list`.

### Task 4: Timer state machine

**Files:**
- Create: `src/timer.rs` (with unit tests)

**Interfaces:**
- Produces:
  - `TimerMode { Infinite, Countdown(Duration), Intervals { focus: Duration, rest: Duration } }`
  - `Phase { Focus, Rest }`
  - `TimerEvent { Finished, PhaseChanged(Phase) }`
  - `Timer`:
    - `fn new(mode: TimerMode) -> Self`, which starts paused in `Focus`.
    - `fn start(&mut self)`, `fn pause(&mut self)`, `fn reset(&mut self)`
    - `fn tick(&mut self, dt: Duration) -> Option<TimerEvent>`. It does nothing while paused, and carries leftover time across a phase change.
    - `fn display(&self) -> Duration`. This is the elapsed time for `Infinite`, and the time remaining in the phase otherwise.
    - `fn progress(&self) -> Option<f64>`, which is `None` for `Infinite`.
    - `fn phase(&self) -> Phase`, `fn mode(&self) -> &TimerMode`, `fn is_running(&self) -> bool`
  - `fn parse_intervals(s: &str) -> anyhow::Result<TimerMode>` parses `"25m/5m"`.

- [ ] **Step 1:** Write the tests:
  - `infinite_counts_up`: three ticks of 1s give `display()==3s`.
  - `countdown_finishes_once`: 10s, one tick of 11s → `Some(Finished)`, `display()==0`. The next tick gives `None`.
  - `intervals_alternate_and_carry`: 25m/5m, one tick of 25m+10s → `PhaseChanged(Rest)` and `display()==5m-10s`. A tick of 5m → `PhaseChanged(Focus)`.
  - `paused_ignores_ticks`
  - `parse_intervals_accepts_25m_5m`
  - `parse_intervals_rejects_garbage`
- [ ] **Step 2:** Run `cargo test timer`. Expected: FAIL.
- [ ] **Step 3:** Implement. Intervals loop forever and never emit `Finished`.
- [ ] **Step 4:** Run `cargo test timer`. Expected: PASS.
- [ ] **Step 5:** Commit: `feat(timer): add infinite, countdown and interval timer`.

### Task 5: Player engine and headless `brainfm play`

**Files:**
- Create: `src/player/mod.rs`, `src/player/engine.rs`
- Modify: `src/cli.rs` (the `play` arm)

**Interfaces:**
- Consumes: `Serving::track_variation.stream_url()`/`length()`, `Timer`, `ApiClient::start_session`, `resolve_state`/`resolve_activity`.
- Produces:
  - `PlayerCommand`, with variants:
    - `Load { id: String, url: String, length: Duration }`: replace the current track and fade in over 1.5s.
    - `Queue { id: String, url: String, length: Duration }`: append for a gapless follow-on.
    - `Play`, `Pause`
    - `Stop { fade: Duration }`
    - `SetVolume(f32) /*0.0–1.0*/`
    - `Chime`: a short two-tone sine built with rodio's `SineWave`, mixed over the music.
  - `PlayerEvent`, with variants:
    - `Started { id }`
    - `Position { id, pos: Duration }`, emitted every 500ms while playing
    - `NearEnd { id }`, emitted once when `length - pos <= 20s`
    - `Ended { id }`
    - `Error { id, message }`
  - `PlayerHandle { fn send(&self, c: PlayerCommand) }` and `fn spawn(rt: tokio::runtime::Handle) -> anyhow::Result<(PlayerHandle, tokio::sync::mpsc::UnboundedReceiver<PlayerEvent>)>`. `spawn` returns `Err` when there is no audio output device.
- Engine approach:
  - It runs on a dedicated `std::thread` that owns the rodio `OutputStream` and the `Sink`.
  - It builds each source with `rt.block_on(StreamDownload::new_http(url, MemoryStorageProvider, Settings::default()))`, wraps that reader in `rodio::Decoder`, and appends it.
  - It tracks a `VecDeque` of `(id, length)` to know which track `sink.get_pos()` belongs to, using `sink.len()` to detect when one has moved on.
  - A decode or HTTP error emits `Error` and does not panic.

- [ ] **Step 1:** Write `player/mod.rs::near_end_fires_once_at_20s` against a pure helper, `fn near_end(length: Duration, pos: Duration, already: bool) -> bool`. The cases are length 100s: pos 79s → false, pos 80s → true, and `already=true` → false. Run it and expect FAIL.
- [ ] **Step 2:** Implement `near_end`, the engine and `spawn`. Run `cargo test player`. Expected: PASS.
- [ ] **Step 3:** Implement the `play` CLI arm:
  - Resolve the state and activity. The default activity is `state.default_activity`.
  - Build the `SessionRequest` from the flags, falling back to the config's `StatePrefs`.
  - On start, `start_session` then `Load`. On `NearEnd`, `start_session` then `Queue`.
  - Timer ticks every 250ms. `Finished` → `Stop{fade:3s}` and exit 0. An interval `PhaseChanged` → `Chime`, plus `Pause` on Rest and `Play` on Focus.
  - Ctrl-C (`tokio::signal::ctrl_c`) → `Stop{fade:1s}`, then exit.
  - Status line, redrawn with `\r`: `▶ {track} · {genre} · {effect}  {pos}/{len}  ⏱ {timer}`.
  - An empty `servings` → error "No tracks match these filters".
- [ ] **Step 4:** Run `cargo clippy --all-targets -- -D warnings && cargo test`. Expected: PASS.
  - Live playback needs the user's account. The user checks it by running `cargo run -- login` and then `cargo run -- play focus --timer 2m`.
  - Expected result: audio plays, the status line updates, and playback fades out at 2m.
  - Record this step as unverified until the user confirms it.
- [ ] **Step 5:** Commit: `feat(player): stream and play tracks, add headless play`.

### Task 6: App state and reducer

**Files:**
- Create: `src/app/state.rs`, `src/app/update.rs` (with unit tests)

**Interfaces:**
- Consumes: the models, `Config`, `Session`, `Timer`, `PlayerCommand`, `PlayerEvent`, `ApiError`.
- Produces (`state.rs`):
  - `AppState`, with fields:
    - `states: Vec<MentalState>`, `state_idx: usize`
    - `activities: Vec<Activity>`, `genres: Vec<String>`
    - `config: Config`, `config_writable: bool`
    - `now: Option<Serving>`, `next: Option<Serving>`
    - `playback: Playback`, `position: Duration`
    - `timer: Timer`
    - `favorite_ids: HashSet<String>`
    - `panel: Panel`, `overlay: Option<Overlay>`, `toast: Option<Toast>`
    - `user_id: Option<String>`, `quit: bool`
  - `fn AppState::new(config: Config, config_writable: bool, user_id: Option<String>) -> Self`
  - `Playback { Idle, Loading, Playing, Paused }`
  - `Panel { visible: bool, tab: PanelTab, selected: usize, favorites: Vec<Serving>, recents: Vec<Serving> }`
  - `PanelTab { Library, Explore, Profile }`
  - `Overlay`, with variants:
    - `Activity { sel: usize }`
    - `Genres { sel: usize, chosen: BTreeSet<String> }`
    - `Effect { sel: usize, chosen: BTreeSet<EffectLevel> }`
    - `Timer { sel: usize }`
    - `Login { email: String, password: String, field: LoginField, error: Option<String> }`
    - `Help`
  - `Toast { text: String, ttl: Duration }`
  - `fn current_prefs(&self) -> StatePrefs`
  - `fn session_request(&self) -> Option<SessionRequest>`
- Produces (`update.rs`):
  - `Action`, with variants:
    - `Key(crossterm::event::KeyEvent)`
    - `Tick(Duration)`
    - `Player(PlayerEvent)`
    - `Api(ApiReply)`
    - `Media(MediaKey)`
  - `MediaKey { Play, Pause, Toggle, Next }`
  - `ApiReply`, with variants:
    - `States(Vec<MentalState>)`
    - `Activities { state_id, items }`
    - `Genres { state_id, items }`
    - `Servings { intent: Intent, items: Vec<Serving> }`
    - `Library { favorites, recents }`
    - `LoggedIn { token: String }`
    - `Failed { what: &'static str, error: ApiErrorKind }`
  - `ApiErrorKind { Unauthorized, Other(String) }`
  - `Intent { Now, Next }`
  - `Effect`, with variants:
    - `FetchStates`
    - `FetchFilters(String /*state id*/)`
    - `StartSession { req: SessionRequest, intent: Intent }`
    - `Player(PlayerCommand)`
    - `SetFavorite { track_id, on }`
    - `Dislike { track_id }`
    - `FetchLibrary`
    - `Login { email, password }`
    - `SaveConfig`
  - `pub fn update(s: &mut AppState, a: Action) -> Vec<Effect>`
- Key map (from the spec):

  | Key | Action |
  |---|---|
  | `space` | play/pause |
  | `n` | skip |
  | `f` | favorite |
  | `d` | dislike + skip |
  | `tab` / `1-4` | mental state |
  | `a` / `g` / `e` / `t` | overlays |
  | `+` / `-` | volume ±10 |
  | `p` | panel |
  | `[` / `]` | panel tab |
  | `j` / `k` / `↑` / `↓` | move |
  | `enter` | select |
  | `esc` | close overlay |
  | `?` | help |
  | `q` | quit |

  Changing state, activity, genres or effect → `SaveConfig`, plus `StartSession{intent:Now}` if something was playing.

- [ ] **Step 1:** Write the tests:
  - `space_from_idle_starts_session`: the effects contain `StartSession{intent:Now}`, and playback is `Loading`.
  - `servings_now_loads_player`: `Api(Servings{Now,[s]})` → `Player(Load{url: s.stream_url, ..})`.
  - `near_end_requests_next_then_queues`: `NearEnd` → `StartSession{Next}`. Then `Servings{Next}` → `Player(Queue)`.
  - `ended_promotes_next_to_now`
  - `player_error_skips_to_next`: shows a toast and returns `StartSession{Now}`.
  - `empty_session_shows_toast`: the toast text is `"No tracks match these filters"`, and playback is `Idle`.
  - `unauthorized_opens_login`: the overlay becomes `Login`, and playback pauses.
  - `countdown_finished_fades_out`: `Player(Stop{fade:3s})`.
  - `interval_phase_change_chimes_and_pauses_on_rest`
  - `tab_cycles_state_and_refetches_filters`: `FetchFilters(id)` and `SaveConfig`.
  - `favorite_toggles_optimistically`
  - `volume_clamps_0_100`
  - `malformed_config_never_saves`: with `config_writable=false`, no `SaveConfig` is emitted.
- [ ] **Step 2:** Run `cargo test app`. Expected: FAIL.
- [ ] **Step 3:** Implement `update`. It is pure: no I/O, no clock reads.
- [ ] **Step 4:** Run `cargo test app`. Expected: PASS.
- [ ] **Step 5:** Commit: `feat(app): add state and reducer`.

### Task 7: Theme and main screen rendering

**Files:**
- Create: `src/ui/mod.rs`, `src/ui/theme.rs`, `src/ui/header.rs`, `src/ui/player.rs`, `src/ui/footer.rs`
- Test: insta snapshots in `src/ui/mod.rs` tests (`src/ui/snapshots/`)

**Interfaces:**
- Consumes: `AppState`.
- Produces:
  - `Theme { bg, surface, surface_hi, text, text_dim, pink, purple, orange, cyan: Color, ascii: bool }`, with:
    - `fn detect() -> Theme`: RGB when `COLORTERM` is `truecolor` or `24bit`, otherwise the nearest `Color::Indexed`. ASCII glyphs when `BRAINFM_ASCII=1`.
    - `fn gradient(&self, t: f64) -> Color`: pink→purple.
  - `pub fn draw(f: &mut Frame, s: &AppState, t: &Theme)`
- Layout follows the spec's mockup:
  - The header has the `brain.fm` title, then the mental-state tabs. The active tab is bold with a gradient underline.
  - The left body shows, in order:
    - the activity line and the neural effect blocks
    - the `tui-big-text` timer, using `Timer::display` as `MM:SS`, or `H:MM:SS` when an hour or more
    - the timer-mode chips
    - the track card (name in bold, then `genre · effect · moods`)
    - a gradient progress gauge with `pos/len`
    - the control glyphs, with a filled heart for a favorite and volume bars
  - The right panel is 34 columns, shown only when `panel.visible` and the width is ≥100.
  - The footer lists the key hints.
  - Below 60×20, the whole screen is replaced by the centered text "Terminal too small (need 60×20)".
  - A toast is a one-line bar above the footer.

- [ ] **Step 1:** Write the snapshot tests with `TestBackend` and `Theme` in ascii mode:
  - `renders_idle_80x24`
  - `renders_playing_140x40`: a fixture serving with the panel visible.
  - `renders_too_small_message`: at 40×10, assert the buffer contains the message.
  - `big_timer_formats_hours`: `3725s` → `"1:02:05"`, through a `fn format_clock(d: Duration) -> String`.
- [ ] **Step 2:** Run `cargo insta test`. Expected: new snapshots. Review them with `cargo insta review` and check them against the spec's mockup.
- [ ] **Step 3:** Implement until the layout matches. Then accept the snapshots and run `cargo test`. Expected: PASS.
- [ ] **Step 4:** Commit: `feat(ui): render desktop-style main screen`.

### Task 8: Side panel and overlays

**Files:**
- Create: `src/ui/panel.rs`, `src/ui/overlay.rs`
- Modify: `src/ui/mod.rs`

**Interfaces:**
- Consumes: `Panel`, `Overlay` (Task 6).
- Produces: `fn draw_panel(f, area, &Panel, &Theme)` and `fn draw_overlay(f, &Overlay, &AppState, &Theme)`.
- Each overlay is a centered modal box on `surface_hi` with a gradient border:
  - **Activity:** a single-select list.
  - **Genres:** multi-select with `[x]` marks. `enter` applies.
  - **Effect:** Low/Medium/High multi-select.
  - **Timer:** Infinite, 30m, 1h, 2h, the custom values, and `Intervals 25/5`.
  - **Login:** email field, a masked password field (`•`), and an error line.
  - **Help:** the key table from Task 6.
- Panel tabs:
  - **Library:** `♥ Favorites` then `◷ Recent`. Each row is `name` plus the genre, dimmed.
  - **Explore:** the current state's activities.
  - **Profile:** the user id and the config path.

- [ ] **Step 1:** Write the snapshot tests:
  - `renders_genres_overlay`
  - `renders_login_overlay_masks_password`: the buffer never contains the fixture password `"hunter2"`.
  - `renders_library_panel`
- [ ] **Step 2:** Run `cargo insta test`, review, implement, accept. Then run `cargo test`. Expected: PASS.
- [ ] **Step 3:** Commit: `feat(ui): add side panel and overlays`.

### Task 9: Event loop, effect executor and MPRIS

**Files:**
- Create: `src/app/mod.rs` (`run`), `src/media.rs`
- Modify: `src/main.rs`

**Interfaces:**
- Consumes: everything above.
- Produces:
  - `pub async fn app::run() -> anyhow::Result<()>`
  - `media::MediaBridge`:
    - `fn new(tx: UnboundedSender<Action>) -> Option<Self>`: logs and returns `None` on failure.
    - `fn set_track(&mut self, s: &Serving)`
    - `fn set_playback(&mut self, p: Playback)`
- `run` does the following:
  - Loads the config. On a malformed config it toasts and sets `config_writable=false`.
  - Loads the session. Without one it opens the `Login` overlay.
  - Spawns the player. On error it exits the TUI with the message, before entering raw mode.
  - Calls `ratatui::init()` and keeps `ratatui::restore()` in a guard.
  - `tokio::select!` over:
    - the crossterm `EventStream`
    - a 250ms `interval` → `Tick`
    - player events
    - an `ApiReply` channel
    - the media channel
  - Each `Action` → `update` → executes the effects. API effects are `tokio::spawn`ed tasks that send an `ApiReply`, mapping `ApiError::Unauthorized` to `ApiErrorKind::Unauthorized`. `SaveConfig` writes `Config::save`.
  - Then `terminal.draw`.
  - Startup effects are `FetchStates` and `FetchLibrary`.
  - On quit: `Stop{fade:500ms}`, wait 600ms, restore the terminal.
- Initialise tracing to `state_dir/brainfm.log` before anything else.

- [ ] **Step 1:** Implement `run` and `MediaBridge`.
  - Keep the bridge `Option<MediaBridge>` throughout `run`, so a missing D-Bus session only means no media keys.
  - Map souvlaki's `MediaControlEvent::{Play, Pause, Toggle, Next}` to `Action::Media`.
  - Write the unit test `media_events_map_to_actions` against a pure `fn map_media(e: MediaControlEvent) -> Option<MediaKey>`.
- [ ] **Step 2:** Run `cargo clippy --all-targets -- -D warnings && cargo test && prek run --all-files`. Expected: PASS.
- [ ] **Step 3:** Hand off the manual check to the user, and record it as unverified until confirmed.
  - The user runs `cargo run`, logs in through the overlay, plays Focus and checks:
    - continuation between tracks
    - `n`, `f`, `d`, `t` → Timer 30m, `p`, and resize
    - `playerctl play-pause` and `playerctl metadata`
  - Then `q` must restore the terminal cleanly.
- [ ] **Step 4:** Commit: `feat(app): wire TUI event loop and media keys`.

### Task 10: README

**Files:**
- Create: `README.md`

- [ ] **Step 1:** Write the README sections:
  - **Install:** `cargo install --path .`, plus the ALSA and D-Bus dev packages on Linux.
  - **Login:** email, or `--token` from the browser DevTools `Authorization` header for Google/Apple accounts.
  - **TUI keys:** the table from Task 6.
  - **CLI examples:** `play focus --activity "Deep Work" --effect high --intervals 25m/5m`, and `list activities --state focus`.
  - **Files:** where the session, config and log live.
  - **Disclaimer:** an unofficial client, the API may change, no audio is cached, and it is used at your own risk under Brain.fm's Terms.
- [ ] **Step 2:** Run `prek run --all-files`. Expected: PASS.
- [ ] **Step 3:** Commit: `docs: add README`.
