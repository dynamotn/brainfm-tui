# brainfm

An unofficial terminal client for [Brain.fm](https://www.brain.fm). It has a
full-screen TUI laid out like the desktop app, plus a scriptable CLI. Audio
plays in-process, so you don't need mpv or vlc.

```
 brain.fm     Focus    Relax    Sleep    Meditate
             ▔▔▔▔▔▔▔
   Deep Work ▾    Neural effect ■■■ High    All genres          │ Library  Explore  Profile
                                                                │ ♥ Favorites
              ██████   ██████       ██████   ██████             │ › Lumen          Electronic
                         (big session timer)                    │ ◷ Recent
                 Infinite  ·  Timer 30:00  ·  Intervals         │   Drift          Piano
                              Ascent
               Electronic · High · Upbeat, Bright
    ━━━━━━━━━━━━━━━━━━━━━━━━──────────────── 03:12 / 07:58
         ⏸ space   ⏭ n   ♥ f   ⊘ d   🔊 ▮▮▮▮▮▮▮▯▯▯ 70%
 space play  n skip  f fav  d dislike  tab state  a activity  g genres  ...
```

## Install

You need Rust 1.91 or newer. On Linux you also need the ALSA and D-Bus
development headers: `alsa-lib` and `dbus` on Arch, or `libasound2-dev` and
`libdbus-1-dev` on Debian/Ubuntu.

```sh
cargo install --path .
```

### Prebuilt binaries

Each `v*` tag publishes binaries for Linux (x86_64, aarch64) and macOS (Apple
Silicon) on the GitHub Releases page, with a `SHA256SUMS` file. On Linux they
need `libasound.so.2` and `libdbus-1.so.3` at runtime.

```sh
sha256sum -c SHA256SUMS --ignore-missing
tar -xzf brainfm-v0.1.0-x86_64-unknown-linux-gnu.tar.gz
```

## Log in

```sh
brainfm login            # email + password; only the session token is stored
brainfm login --token    # paste a token instead
brainfm whoami
brainfm logout
```

If your account signs in with Google or Apple, use `--token`:

1. Open my.brain.fm in your browser and open DevTools.
2. Copy the `Authorization: Bearer …` value from any request to `api.brain.fm`.
3. Paste it at the prompt.

The token is saved with mode `0600`. Brain.fm slides it forward on every
request, so the stored copy is refreshed automatically.

## TUI

Run `brainfm` with no arguments.

| Key | Action |
|---|---|
| `space` | play / pause |
| `n` | skip track |
| `f` | favorite / unfavorite |
| `d` | dislike (never play again) and skip |
| `tab` / `shift-tab` / `1`–`4` | switch mental state |
| `a` | pick activity |
| `g` | pick genres (none selected = all) |
| `e` | pick neural effect level |
| `t` | timer: Infinite, 30 min / 1 hr / 2 hrs, custom, Intervals |
| `+` / `-` | volume |
| `p` | show / hide the side panel |
| `[` / `]` | switch panel tab (Library, Explore, Profile) |
| `j` / `k`, `enter` | move and select in panel and pickers |
| `?` | help |
| `q` / `ctrl-c` | quit |

Each mental state remembers its activity, genres and neural effect level.
Media keys and `playerctl` work through MPRIS.

When the terminal has no truecolor support, the TUI falls back to 256 colors.
Set `BRAINFM_ASCII=1` if your font lacks the symbols.

## CLI

```sh
brainfm list states
brainfm list activities --state focus
brainfm list genres --state relax

brainfm play focus
brainfm play focus --activity "Deep Work" --effect high --intervals 25m/5m
brainfm play sleep --genre Piano --timer 1h30m
```

`play` prints a one-line status. It stops when the timer ends, with a fade out,
or when you press Ctrl-C.

## Files

| What | Where (Linux) |
|---|---|
| Session token | `~/.local/state/brainfm/session.json` |
| Preferences | `~/.config/brainfm/config.toml` |
| Log | `~/.local/state/brainfm/brainfm.log` |

Audio is streamed into memory only and never written to disk.

## Disclaimer

Brain.fm has not made, endorsed or supported this client. It talks to the
same private API as the web app, so it can break whenever that API changes.
You need a Brain.fm subscription. Use it at your own risk and within
[Brain.fm's Terms](https://www.brain.fm/terms).

## Development

```sh
cargo test                                            # unit, API (wiremock) and UI snapshot tests
cargo test --test player_smoke -- --ignored           # real audio; needs ffmpeg + a sound card
prek run --all-files                                  # fmt, clippy, tests
cargo deny check                                      # advisories, licenses, sources
```

To release: bump `version` in `Cargo.toml`, commit, then push a matching tag
(`git tag v0.2.0 && git push origin v0.2.0`). The Release workflow runs CI,
checks the tag against `Cargo.toml`, builds and publishes the binaries.
