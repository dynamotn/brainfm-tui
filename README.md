<div align="center">

# brainfm

**Brain.fm focus music, right in your terminal.**

A desktop-style TUI and a scriptable CLI for [Brain.fm](https://www.brain.fm),
written in Rust. No browser, no Electron, no external player.

[![CI](https://github.com/dynamotn/brainfm-tui/actions/workflows/ci.yml/badge.svg)](https://github.com/dynamotn/brainfm-tui/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/dynamotn/brainfm-tui?color=f34675)](https://github.com/dynamotn/brainfm-tui/releases)
[![License: MIT](https://img.shields.io/badge/license-MIT-9130bb)](LICENSE)
[![Rust 1.91+](https://img.shields.io/badge/rust-1.91%2B-f39946?logo=rust)](https://www.rust-lang.org)

<img src="docs/screenshots/player.png" alt="brainfm playing a Focus session: mental-state tabs, a large interval timer, the current track and the library panel" width="900">

</div>

## Why

- **Feels like the desktop app.** Same layout: mental-state tabs, a big
  session timer, the track card, and a Library / Explore / Profile panel, in
  Brain.fm's own colours.
- **Built for deep work.** Infinite, countdown or Pomodoro-style intervals
  with a chime at each switch. Music pauses for breaks and fades out when the
  session ends.
- **Never runs dry.** The next track is fetched 20 seconds before the current
  one ends, so playback continues without a gap.
- **Your filters, per state.** Activity, genres and neural effect level are
  remembered separately for Focus, Relax, Sleep and Meditate.
- **Plays nicely with your desktop.** Shows up in your desktop's media
  widget, and media keys and `playerctl` control it, both in the TUI and in
  `brainfm play`. Audio goes through PulseAudio or PipeWire, so it gets its
  own volume slider in your mixer.
- **Music keeps playing after the TUI closes.** Playback lives in a
  background process: close the window with `q` and it carries on, reopen
  brainfm to pick the session back up, or run `brainfm stop`. Any number of
  windows can show the same session.
- **Scriptable.** `brainfm play focus --intervals 25m/5m` runs a headless
  session from a shell, a status bar or a cron job.
- **Light and private.** One small binary. Audio streams into memory and is
  never written to disk. Only the session token is stored, never your
  password.

## Screenshots

<table>
  <tr>
    <td width="50%"><img src="docs/screenshots/filters.png" alt="Genre picker overlay over the player"></td>
    <td width="50%"><img src="docs/screenshots/explore.png" alt="The Explore tab listing the activities of the current state"></td>
  </tr>
  <tr>
    <td align="center"><sub>Pick genres, activity and neural effect from popovers</sub></td>
    <td align="center"><sub>The Explore tab: activities for the current state</sub></td>
  </tr>
  <tr>
    <td colspan="2" align="center"><img src="docs/screenshots/compact.png" alt="The player on an 80 by 24 terminal, with the side panel hidden" width="60%"></td>
  </tr>
  <tr>
    <td colspan="2" align="center"><sub>Still usable at 80×24: the side panel tucks away on narrow terminals</sub></td>
  </tr>
</table>

<sub>Captured from a real listening session with
`scripts/screenshots.sh --live`.</sub>

## Quick start

```sh
brainfm login     # once
brainfm           # open the player, press space
```

## Install

### Prebuilt binaries

Download the archive for your platform from
[Releases](https://github.com/dynamotn/brainfm-tui/releases): Linux x86_64 or
aarch64, or macOS on Apple Silicon. Then verify and unpack it:

```sh
sha256sum -c SHA256SUMS --ignore-missing
tar -xzf brainfm-v*-x86_64-unknown-linux-gnu.tar.gz
```

On Linux the binary only needs `libasound.so.2`, which every desktop has.

### From source

You need Rust 1.91 or newer. On Linux you also need the ALSA headers:
`alsa-lib` on Arch, `libasound2-dev` on Debian/Ubuntu.

```sh
cargo install --git https://github.com/dynamotn/brainfm-tui
# or, to talk to PipeWire natively (needs libpipewire >= 0.3.53 and clang):
cargo install --git https://github.com/dynamotn/brainfm-tui --features pipewire
```

## Using it

```sh
brainfm                                   # the player; press ? for keys
brainfm play focus --intervals 25m/5m     # no TUI: status line, media keys
brainfm list activities --state relax     # names for --activity
playerctl -p brainfm next                 # control it from anywhere
```

## Documentation

- [User guide](docs/user-guide.md): logging in (including Google and Apple
  accounts), every key, filters, timers, several windows, media keys, sound
  output, `brainfm play`, troubleshooting.
- [Configuration reference](docs/configuration.md): files, `config.toml`,
  environment variables, build features.
- [Architecture](docs/architecture.md) and [development](docs/development.md):
  how it works, how to test and release it.

## Disclaimer

This is an unofficial client: Brain.fm has not made, endorsed or supported
it. It talks to the same private API as the web app, so a change on their side
can break it. You need a Brain.fm subscription. Use it at your own risk and
within [Brain.fm's Terms](https://www.brain.fm/terms).

## License

[MIT](LICENSE). Brain.fm is a trademark of its owner.
