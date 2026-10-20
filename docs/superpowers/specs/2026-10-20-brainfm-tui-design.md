# brainfm — a Rust TUI/CLI client for Brain.fm that mirrors the desktop app

## Context

The user wants to listen to Brain.fm from the terminal, with an experience as close as possible to the
official desktop app (which has no Linux build). The repo `~/Working/personal/tools/brainfm` is empty
(no commits). The user skipped the Q&A, so every choice below is **my call** and open to override.

Research (my.brain.fm JS bundle, fetched 2026-10-20; prior art `quintrino/brainfm-cli`) found:
- API: `https://api.brain.fm/v2` (auth, users) and `/v3` (music). Responses `{status, result, messages?, token?}`.
- Login `POST /v2/auth/email-login` → JWT in `result` (payload `_id` = user id). Bearer auth. Token
  slides: any response carrying `token` replaces the stored one (~15 min lifetime).
- `GET /v3/mentalStates/dynamic`, `/mentalStates/dynamic/{id}/activities|genres|moods`.
- Play: `POST /v3/users/{uid}/sessions?platform=web` body
  `{dynamicActivityId, version:3, neuralEffectLevels:[...], genreNames:[...]}` →
  `servings[0].track.name`, `servings[0].trackVariation.tokenedUrl` (signed mp3 URL, fallback `audio.brain.fm/{url}`).
- Favorites `GET /users/{uid}/servings/favorites?version=2`, `POST|DELETE /users/{uid}/tracks/{id}/favorites`;
  recents `/users/{uid}/servings/recent`, recommended, `/tracks/{id}/similar`.
- Headers used by working clients: `Origin/Referer: https://my.brain.fm`, browser UA, JSON content type.
- Desktop UI: mental-state selector (Focus/Relax/Sleep/Meditate), activity picker, big centered timer,
  track card (name, genre, neural effect level, moods), controls (play/pause, skip, ♥, 👎, volume),
  side panel tabs Library/Explore/Profile, timer modes Infinite / Timer (30m/1h/2h + 3 custom) /
  Intervals (25/5 with chime). Palette: bg `#0d0c11`, surfaces `#1d1926` `#322c44`, text `#ffffff` `#d4d2ea`,
  accents pink `#f34675`, purple `#9130bb`, orange `#f39946`, cyan `#0bb9ff`.
- Verified in the bundle on 2026-10-20:
  - Login body is the form values `{email, password}`.
  - Dislike: `POST /v2/users/{uid}/dislikes?version=3` `{trackId}`, `GET` lists them, `DELETE` undoes.
  - Neural effect levels in requests are `"Low" | "Medium" | "High"`. A track's
    `trackVariation.neuralEffectLevel` is a float, shown as Low ≤ 0.33 < Medium ≤ 0.66 < High.
  - A track's genre is the first `track.tags[]` entry with `type == "genre"` and a value other than
    `"Nature"`. Its moods are the tags with `type == "mood"`. Duration is
    `trackVariation.lengthInSeconds`.
  - `GET /v2/users/me` returns the user. Mental-state and activity lists come back as bare arrays in `result`.
- Still unverified: how long a `tokenedUrl` stays valid, and the audio codec (assumed mp3).

## Goals / non-goals / anti-goals

- **Goals:** full-screen TUI that looks and flows like the desktop app; reliable in-process playback with
  seamless track-to-track continuation; Infinite/Timer/Intervals; per-mental-state preferences
  (activity, genres, neural effect); favorites/recents; MPRIS media keys; a scriptable CLI too.
- **Non-goals:** offline mode, Apple/Google/Facebook login (token paste covers those accounts), telemetry
  (`/listening/events` not sent), Windows/macOS polish (should build, not tested).
- **Anti-goals:** saving audio to disk (ToS: no copying content); storing the account password;
  depending on an external player (mpv/vlc).

## Tech choices (my calls)

| Concern | Choice |
|---|---|
| TUI | `ratatui` + `crossterm`, `tui-big-text` for the timer |
| Async / HTTP | `tokio`, `reqwest` (rustls, json), `serde` |
| Audio | `rodio` (symphonia mp3) fed by `stream-download` (progressive HTTP stream, in-memory buffer) |
| Media keys | `souvlaki` (MPRIS on Linux) |
| CLI | `clap` derive |
| Config/state | `directories` → `~/.config/brainfm/config.toml` (prefs), `~/.local/state/brainfm/session.json` (JWT, mode 0600) |
| Errors/logs | `anyhow`/`thiserror`, `tracing` to a log file (never to the TUI, never logging tokens) |
| Tests | `wiremock` (API), `insta` + ratatui `TestBackend` (UI snapshots) |

## Architecture

Single binary crate, modules:

```
src/
  main.rs            clap entry; no subcommand → TUI
  cli.rs             subcommands
  api/{mod,client,models,auth}.rs   typed client, sliding-token middleware, envelope unwrapping
  session.rs         token store (0600), user id from JWT payload
  config.rs          prefs per mental state, timer customs, volume
  player/{mod,engine,queue}.rs      audio thread: rodio Sink, prefetch next serving ~20s before end, fade in/out
  timer.rs           pure state machine: Infinite | Countdown | Intervals{focus,break,phase}
  app/{mod,state,action,event}.rs   single AppState, Action enum, reducer; tokio event loop
  ui/{mod,theme,home,player,panel,picker,help}.rs
  media.rs           souvlaki ↔ Action bridge
```

Flow: terminal events / ticks / player events / API results → `Action` → reducer mutates `AppState`
→ side effects spawned as tasks → `ui::draw(&state)`. Player runs on its own thread, talks via channels.

## Screen design (mimics desktop)

```
┌ brain.fm ──────────[ FOCUS ]  Relax   Sleep   Meditate ───────────────────────┐
│                                                     │ Library  Explore  Profile│
│        Deep Work ▾        Neural effect: ■■■ High    │ ♥ Favorites               │
│                                                     │   Ascent      Electronic  │
│              ██  ███    ███  ███                    │   Lumen       Piano       │
│              ██ █  █  █ █  █   █   (big timer)       │ ◷ Recent                  │
│              ██  ███    ███  ███                    │   ...                     │
│          Infinite · [Timer 25:00] · Intervals        │                           │
│                                                     │                           │
│   Ascent  — Electronic · High · Upbeat, Bright       │                           │
│   ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━──────── 03:12/07:58 │                           │
│          ⏮   ⏯   ⏭      ♥     👎     🔊 ▮▮▮▮▮▯▯ 70%   │                           │
└ space play · n skip · f fav · d dislike · tab state · a activity · t timer · ? ┘
```

- Theme module holds the palette above; truecolor gradient (pink→purple) on the active tab, timer and
  progress bar; 256-color fallback when `COLORTERM` lacks truecolor; Nerd Font glyphs optional with ASCII fallback.
- Overlays (modal pickers, like the desktop popovers): activity, genres (multi-select), neural effect,
  timer settings (presets + custom), login.
- Panel collapses below ~100 columns; layout must render at 80×24.
- Keys: `space` play/pause, `n` skip, `f` favorite, `d` dislike (calls the dislike endpoint, then skips),
  `tab`/`1-4` mental state, `a` activity, `g` genres, `e` neural effect, `t` timer, `+/-` volume,
  `p` toggle panel, `enter` play item in panel, `?` help, `q` quit.

## CLI

- `brainfm` — TUI.
- `brainfm login` — email + password prompt (password never stored, only the JWT); `--token` to paste one
  from the browser (covers Google/Apple accounts).
- `brainfm logout`, `brainfm whoami`.
- `brainfm play <focus|relax|sleep|meditate> [--activity <name>] [--effect low|medium|high] [--genre <g>...] [--timer 25m | --intervals 25m/5m]` — headless playback with a one-line status, Ctrl-C to stop.
- `brainfm list <states|activities|genres> [--state <s>]` for scripting.

## Implementation order

1. Scaffold crate, `cargo` lints, `rustfmt.toml`, prek hooks (fmt, clippy, test); commit design doc to
   `docs/superpowers/specs/2026-10-20-brainfm-tui-design.md`.
2. `api` + `session` with wiremock tests (envelope, sliding token, 401 → re-login prompt).
3. `player` engine: stream + play + prefetch/continue; manual smoke via `brainfm play`.
4. `timer` state machine (unit tests, incl. interval phase switches + chime).
5. `app` reducer + event loop; `ui` screens with insta snapshots at 80×24 and 140×40.
6. Overlays, panel (favorites/recents), MPRIS, config persistence.
7. README (install, login, keys, ToS caveat).

## Verification

- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test` (API via wiremock, timer,
  reducer, UI snapshots), `prek run --all-files`.
- Live check needs a real account — **I won't handle credentials or tokens**, so the user runs:
  `cargo run -- login`, then `cargo run -- play focus --timer 1m` and `cargo run` (TUI), confirming audio,
  track continuation, skip, favorite, timer end fade-out, and media keys (`playerctl play-pause`).
- Finish with local commits only; the user pushes.
