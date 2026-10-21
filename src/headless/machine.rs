//! The logic of `brainfm play`, kept free of I/O so it can be tested: events
//! go in, [`Cmd`]s come out, and `mod.rs` carries them out.
//!
//! Track fetches run in the background and are tagged with an epoch. A skip,
//! a reload after an error or a natural end bumps the epoch, so a fetch that
//! finishes after it no longer applies is dropped.

use std::time::Duration;

use crate::api::Serving;
use crate::app::state::Playback;
use crate::media::MediaKey;
use crate::player::{PlayerCommand, PlayerEvent};
use crate::timer::{Phase, Timer, TimerEvent, TimerMode};

const MAX_ERRORS_IN_A_ROW: u32 = 3;
const END_FADE: Duration = Duration::from_secs(3);
/// Fade used when the user stops `brainfm play` with Ctrl-C.
pub(crate) const QUIT_FADE: Duration = Duration::from_secs(1);

/// Why playback is silent: the user paused it, or an interval break.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Pauses {
    pub(crate) user: bool,
    pub(crate) rest: bool,
}

/// What the engine should do after a media key or a phase change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KeyEffect {
    Nothing,
    Play,
    Pause,
    Skip,
    Volume(u8),
}

impl Pauses {
    pub(crate) fn paused(self) -> bool {
        self.user || self.rest
    }

    fn change(&mut self, f: impl FnOnce(&mut Self)) -> KeyEffect {
        let was = self.paused();
        f(self);
        match (was, self.paused()) {
            (false, true) => KeyEffect::Pause,
            (true, false) => KeyEffect::Play,
            _ => KeyEffect::Nothing,
        }
    }

    /// Enters or leaves an interval break without overriding a user pause.
    pub(crate) fn set_rest(&mut self, rest: bool) -> KeyEffect {
        self.change(|p| p.rest = rest)
    }
}

pub(crate) fn on_media_key(p: &mut Pauses, key: MediaKey) -> KeyEffect {
    match key {
        // Flips the user's own pause, so it also works during a break.
        MediaKey::Toggle => p.change(|p| p.user = !p.user),
        MediaKey::Play => p.change(|p| p.user = false),
        MediaKey::Pause => p.change(|p| p.user = true),
        MediaKey::Next => KeyEffect::Skip,
        MediaKey::SetVolume(v) => KeyEffect::Volume(v.min(100)),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FetchKind {
    /// Replace what is playing.
    Now,
    /// Queue after the current track.
    Next,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FetchError {
    Unauthorized,
    Other(String),
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Cmd {
    Player(PlayerCommand),
    Fetch {
        kind: FetchKind,
        epoch: u64,
    },
    /// A line for the user above the status line.
    Notice(String),
    /// End the session normally, fading out.
    Stop {
        fade: Duration,
    },
    /// End the session with an error.
    Fail(String),
}

#[derive(Debug)]
pub(crate) struct Machine {
    pub(crate) now: Serving,
    pub(crate) next: Option<Serving>,
    pub(crate) pauses: Pauses,
    pub(crate) timer: Timer,
    pub(crate) pos: Duration,
    pub(crate) volume: u8,
    /// Bumped by every `Started`, so media widgets see a repeat as new.
    pub(crate) serial: u64,
    epoch: u64,
    fetching_now: bool,
    prefetching: bool,
    errors: u32,
    started_once: bool,
}

fn command_for(s: &Serving, queue: bool) -> Option<PlayerCommand> {
    let v = &s.track_variation;
    let url = v.stream_url()?;
    let (id, length) = (s.track.id.clone(), v.length());
    Some(if queue {
        PlayerCommand::Queue { id, url, length }
    } else {
        PlayerCommand::Load { id, url, length }
    })
}

impl Machine {
    /// Starts loading `now`.
    pub(crate) fn new(now: Serving, mode: TimerMode, volume: u8) -> (Self, Vec<Cmd>) {
        let volume = volume.min(100);
        let mut m = Self {
            now,
            next: None,
            pauses: Pauses::default(),
            timer: Timer::new(mode),
            pos: Duration::ZERO,
            volume,
            serial: 0,
            epoch: 0,
            fetching_now: false,
            prefetching: false,
            errors: 0,
            started_once: false,
        };
        let mut cmds = vec![Cmd::Player(PlayerCommand::SetVolume(
            f32::from(volume) / 100.0,
        ))];
        cmds.extend(m.load_now());
        (m, cmds)
    }

    pub(crate) fn status(&self) -> Playback {
        if self.pauses.paused() {
            Playback::Paused
        } else {
            Playback::Playing
        }
    }

    /// The clock runs once music has started, except while the user pauses;
    /// breaks keep counting.
    fn sync_timer(&mut self) {
        if self.started_once && !self.pauses.user {
            self.timer.start();
        } else {
            self.timer.pause();
        }
    }

    fn load_now(&mut self) -> Vec<Cmd> {
        match command_for(&self.now, false) {
            Some(c) => vec![Cmd::Player(c)],
            None => self.track_failed("track has no audio URL"),
        }
    }

    /// Fetches a replacement for the current track; stale fetches are dropped.
    fn reload(&mut self) -> Vec<Cmd> {
        if self.fetching_now {
            return Vec::new();
        }
        self.epoch += 1;
        self.fetching_now = true;
        self.prefetching = false;
        self.next = None;
        vec![Cmd::Fetch {
            kind: FetchKind::Now,
            epoch: self.epoch,
        }]
    }

    fn track_failed(&mut self, message: &str) -> Vec<Cmd> {
        self.errors += 1;
        if self.errors >= MAX_ERRORS_IN_A_ROW {
            return vec![Cmd::Fail(format!(
                "{message} (giving up after {} failed tracks)",
                self.errors
            ))];
        }
        let mut cmds = vec![Cmd::Notice(format!("! {message}, skipping"))];
        cmds.extend(self.reload());
        cmds
    }

    pub(crate) fn on_player(&mut self, ev: PlayerEvent) -> Vec<Cmd> {
        match ev {
            PlayerEvent::Started { id } => {
                if self.next.as_ref().is_some_and(|n| n.track.id == id) {
                    self.now = self.next.take().expect("checked");
                } else if self.now.track.id != id {
                    return Vec::new();
                }
                self.pos = Duration::ZERO;
                self.errors = 0;
                self.serial += 1;
                self.started_once = true;
                self.sync_timer();
                // A fresh track starts audible; keep it silent if paused.
                if self.pauses.paused() {
                    vec![Cmd::Player(PlayerCommand::Pause)]
                } else {
                    Vec::new()
                }
            }
            PlayerEvent::Position { id, pos } => {
                if id == self.now.track.id {
                    self.pos = pos;
                }
                Vec::new()
            }
            PlayerEvent::NearEnd { id } => {
                if id != self.now.track.id
                    || self.next.is_some()
                    || self.prefetching
                    || self.fetching_now
                {
                    return Vec::new();
                }
                self.prefetching = true;
                vec![Cmd::Fetch {
                    kind: FetchKind::Next,
                    epoch: self.epoch,
                }]
            }
            PlayerEvent::Ended { id } => {
                if id == self.now.track.id && self.next.is_none() {
                    self.reload()
                } else {
                    Vec::new()
                }
            }
            PlayerEvent::Error { id, message } => {
                if self.next.as_ref().is_some_and(|n| n.track.id == id) {
                    // The queued track failed or was dropped; a fresh one is
                    // fetched when this one ends.
                    self.next = None;
                    return Vec::new();
                }
                if id != self.now.track.id {
                    return Vec::new();
                }
                self.track_failed(&crate::headless::printable(&message))
            }
        }
    }

    pub(crate) fn on_fetched(
        &mut self,
        kind: FetchKind,
        epoch: u64,
        result: Result<Serving, FetchError>,
    ) -> Vec<Cmd> {
        if epoch != self.epoch {
            return Vec::new();
        }
        match kind {
            FetchKind::Now => {
                self.fetching_now = false;
                match result {
                    Ok(s) => {
                        self.now = s;
                        self.next = None;
                        self.pos = Duration::ZERO;
                        self.load_now()
                    }
                    Err(FetchError::Unauthorized) => {
                        vec![Cmd::Fail("Session expired — run `brainfm login`".into())]
                    }
                    Err(FetchError::Other(m)) => vec![Cmd::Fail(m)],
                }
            }
            FetchKind::Next => {
                self.prefetching = false;
                match result {
                    Ok(s) if self.next.is_none() => match command_for(&s, true) {
                        Some(c) => {
                            self.next = Some(s);
                            vec![Cmd::Player(c)]
                        }
                        None => Vec::new(),
                    },
                    Err(FetchError::Unauthorized) => {
                        vec![Cmd::Fail("Session expired — run `brainfm login`".into())]
                    }
                    // A prefetch is already queued, or it failed: not fatal,
                    // another attempt is made when this track ends.
                    Ok(_) | Err(FetchError::Other(_)) => Vec::new(),
                }
            }
        }
    }

    pub(crate) fn on_key(&mut self, key: MediaKey) -> Vec<Cmd> {
        let cmds = match on_media_key(&mut self.pauses, key) {
            KeyEffect::Nothing => Vec::new(),
            KeyEffect::Play => vec![Cmd::Player(PlayerCommand::Play)],
            KeyEffect::Pause => vec![Cmd::Player(PlayerCommand::Pause)],
            KeyEffect::Skip => self.reload(),
            KeyEffect::Volume(v) => {
                self.volume = v;
                vec![Cmd::Player(PlayerCommand::SetVolume(f32::from(v) / 100.0))]
            }
        };
        self.sync_timer();
        cmds
    }

    pub(crate) fn on_tick(&mut self, dt: Duration) -> Vec<Cmd> {
        match self.timer.tick(dt) {
            Some(TimerEvent::Finished) => vec![Cmd::Stop { fade: END_FADE }],
            Some(TimerEvent::PhaseChanged(phase)) => {
                let mut cmds = vec![Cmd::Player(PlayerCommand::Chime)];
                match self.pauses.set_rest(phase == Phase::Rest) {
                    KeyEffect::Pause => cmds.push(Cmd::Player(PlayerCommand::Pause)),
                    KeyEffect::Play => cmds.push(Cmd::Player(PlayerCommand::Play)),
                    _ => {}
                }
                cmds
            }
            None => Vec::new(),
        }
    }
}

#[cfg(test)]
#[path = "machine_tests.rs"]
mod tests;
