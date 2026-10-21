//! An unofficial terminal client for [Brain.fm](https://www.brain.fm).
//!
//! The crate is an application; the library target exists so integration
//! tests can reach the pieces below. Only what `main.rs` and `tests/` use is
//! public.
//!
//! - [`api`]: typed client for `api.brain.fm` (the web app's private API).
//! - [`player`]: the audio engine thread, driven by [`player::PlayerCommand`].
//! - [`cli`]: the `brainfm` subcommands; with none, [`app::run`] starts the TUI.
//!
//! Errors inside the application use [`anyhow`]; only the API boundary has a
//! typed error ([`api::ApiError`]), because that is where callers branch on
//! the kind of failure (expired session, rate limit, network).

pub mod api;
pub mod app;
pub mod cli;
pub(crate) mod config;
pub(crate) mod headless;
pub(crate) mod media;
pub mod player;
pub mod secret;
pub(crate) mod session;
pub(crate) mod timer;
pub(crate) mod ui;
