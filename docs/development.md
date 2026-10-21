# Development

## Toolchain

`rust-toolchain.toml` pins the compiler, so local checks and CI see the same
lints. The minimum supported version for users is in `Cargo.toml`
(`rust-version`), and CI checks it separately.

On Linux you need the ALSA headers (`alsa-lib` on Arch, `libasound2-dev` on
Debian/Ubuntu). The `pipewire` feature also needs libpipewire 0.3.53 or newer
and clang.

Version managers such as mise can export `RUSTUP_TOOLCHAIN`, which silently
overrides the pin. `scripts/pinned-cargo.sh` runs cargo with the pinned
toolchain whatever the environment says:

```sh
scripts/pinned-cargo.sh clippy --all-targets -- -D warnings
```

## Checks

The pre-commit hooks (run with `prek`) do the same as CI's main job:

```sh
prek run --all-files    # fmt, clippy -D warnings, tests — on the pinned toolchain
cargo deny check        # advisories, licences, sources, banned crates
```

Lints are strict: `clippy::pedantic`, `unreachable_pub`, `unsafe_code =
"forbid"` and `unwrap_used` outside tests. A lint is allowed only with a
comment explaining why, next to the `allow`.

## Tests

| Suite | Command | Needs |
|---|---|---|
| Unit tests, including the reducer, the headless state machine and UI snapshots | `cargo test` | nothing |
| API client against a mock server | `cargo test --test api` | nothing |
| Real audio: streaming, gapless queue, stalled streams, shutdown | `cargo test --test player_smoke -- --ignored --test-threads=1` | ffmpeg and a sound card |
| MPRIS over a real D-Bus | `dbus-run-session -- cargo test --lib serves_mpris -- --ignored` | dbus |

UI snapshots live in `src/ui/snapshots`. After an intended visual change,
review and accept them with `cargo insta review`.

The audio tests honour `BRAINFM_AUDIO_HOST`; run them with `pulse` and with
`alsa` when you touch the engine.

## Screenshots

`scripts/screenshots.sh --live` captures the README screenshots from the
brainfm running on your machine. Start it and play something first. It
attaches as a view, draws a copy of the real state with the app's own drawing
code (`src/ui/screenshots.rs`), and converts the result to PNG with `resvg`.
Popups are opened only on that copy, never in your session, and the Profile
tab (which shows the account id) is never captured. Without `--live` the
script renders made-up sample data instead.

## Continuous integration

`.github/workflows/ci.yml` runs on pushes to `main` and on pull requests:

| Job | What it does |
|---|---|
| `check` | fmt, clippy, tests and docs on the pinned toolchain. |
| `msrv` | `cargo check` on the minimum supported Rust. |
| `pipewire` | clippy with the `pipewire` feature on Ubuntu 24.04. |
| `macos` | clippy and tests on macOS, where the Now Playing backend lives. |
| `deny` | `cargo deny check`. |

Every action is pinned to a commit SHA.

## Releasing

1. Bump `version` in `Cargo.toml` and commit.
2. Push to `main`, and wait for CI.
3. Tag and push: `git tag v0.2.0 && git push origin v0.2.0`.

`.github/workflows/release.yml` then:

- runs CI;
- checks that the tag matches `Cargo.toml`;
- builds Linux x86_64 and aarch64 and macOS arm64 without a build cache;
- publishes `.tar.gz` archives with a `SHA256SUMS` file.

If the commit that adds or changes a workflow and the tag are pushed
together, GitHub may not run the workflow for that tag. Push the branch
first, then the tag.
