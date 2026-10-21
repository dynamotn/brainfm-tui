//! Session timer: Infinite (counts up), Countdown, or focus/rest Intervals.

use std::time::Duration;

use anyhow::{Context, bail};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TimerMode {
    Infinite,
    Countdown(Duration),
    Intervals { focus: Duration, rest: Duration },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Phase {
    Focus,
    Rest,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimerEvent {
    Finished,
    PhaseChanged(Phase),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Timer {
    mode: TimerMode,
    elapsed: Duration,
    phase: Phase,
    running: bool,
    finished: bool,
}

impl Timer {
    pub fn new(mode: TimerMode) -> Self {
        Self {
            mode,
            elapsed: Duration::ZERO,
            phase: Phase::Focus,
            running: false,
            finished: false,
        }
    }

    pub fn start(&mut self) {
        if !self.finished {
            self.running = true;
        }
    }

    pub fn pause(&mut self) {
        self.running = false;
    }

    pub fn reset(&mut self) {
        *self = Self::new(self.mode);
    }

    pub fn mode(&self) -> &TimerMode {
        &self.mode
    }

    pub fn phase(&self) -> Phase {
        self.phase
    }

    pub fn is_running(&self) -> bool {
        self.running
    }

    fn phase_length(&self) -> Option<Duration> {
        match self.mode {
            TimerMode::Infinite => None,
            TimerMode::Countdown(d) => Some(d),
            TimerMode::Intervals { focus, rest } => Some(match self.phase {
                Phase::Focus => focus,
                Phase::Rest => rest,
            }),
        }
    }

    pub fn tick(&mut self, dt: Duration) -> Option<TimerEvent> {
        if !self.running {
            return None;
        }
        self.elapsed += dt;
        let len = self.phase_length()?;
        if self.elapsed < len {
            return None;
        }
        match self.mode {
            TimerMode::Countdown(_) => {
                self.elapsed = len;
                self.running = false;
                self.finished = true;
                Some(TimerEvent::Finished)
            }
            TimerMode::Intervals { .. } => {
                self.elapsed -= len;
                self.phase = match self.phase {
                    Phase::Focus => Phase::Rest,
                    Phase::Rest => Phase::Focus,
                };
                Some(TimerEvent::PhaseChanged(self.phase))
            }
            TimerMode::Infinite => None,
        }
    }

    /// Elapsed time for Infinite; time left in the current phase otherwise.
    pub fn display(&self) -> Duration {
        match self.phase_length() {
            None => self.elapsed,
            Some(len) => len.saturating_sub(self.elapsed),
        }
    }

    pub fn progress(&self) -> Option<f64> {
        let len = self.phase_length()?;
        if len.is_zero() {
            return Some(1.0);
        }
        Some((self.elapsed.as_secs_f64() / len.as_secs_f64()).min(1.0))
    }
}

/// `MM:SS`, or `H:MM:SS` from one hour up.
pub(crate) fn format_clock(d: Duration) -> String {
    let s = d.as_secs();
    let (h, m, s) = (s / 3600, (s / 60) % 60, s % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m:02}:{s:02}")
    }
}

/// Parses `"25m/5m"` into an Intervals mode.
pub(crate) fn parse_intervals(s: &str) -> anyhow::Result<TimerMode> {
    let (f, r) = s
        .split_once('/')
        .context("intervals must look like 25m/5m")?;
    let focus = humantime::parse_duration(f.trim()).context("invalid focus length")?;
    let rest = humantime::parse_duration(r.trim()).context("invalid rest length")?;
    if focus.is_zero() || rest.is_zero() {
        bail!("interval lengths must be greater than zero");
    }
    Ok(TimerMode::Intervals { focus, rest })
}

#[cfg(test)]
mod tests {
    use super::*;

    const S: Duration = Duration::from_secs(1);
    const M: Duration = Duration::from_mins(1);

    #[test]
    fn infinite_counts_up() {
        let mut t = Timer::new(TimerMode::Infinite);
        t.start();
        for _ in 0..3 {
            assert_eq!(t.tick(S), None);
        }
        assert_eq!(t.display(), 3 * S);
        assert_eq!(t.progress(), None);
    }

    #[test]
    fn countdown_finishes_once() {
        let mut t = Timer::new(TimerMode::Countdown(10 * S));
        t.start();
        assert_eq!(t.tick(11 * S), Some(TimerEvent::Finished));
        assert_eq!(t.display(), Duration::ZERO);
        assert!(!t.is_running());
        assert_eq!(t.tick(S), None);
    }

    #[test]
    fn countdown_progress() {
        let mut t = Timer::new(TimerMode::Countdown(10 * S));
        t.start();
        t.tick(5 * S);
        assert_eq!(t.progress(), Some(0.5));
        assert_eq!(t.display(), 5 * S);
    }

    #[test]
    fn intervals_alternate_and_carry() {
        let mut t = Timer::new(TimerMode::Intervals {
            focus: 25 * M,
            rest: 5 * M,
        });
        t.start();
        assert_eq!(
            t.tick(25 * M + 10 * S),
            Some(TimerEvent::PhaseChanged(Phase::Rest))
        );
        assert_eq!(t.phase(), Phase::Rest);
        assert_eq!(t.display(), (5 * M).checked_sub(10 * S).unwrap());
        assert_eq!(t.tick(5 * M), Some(TimerEvent::PhaseChanged(Phase::Focus)));
        assert_eq!(t.display(), (25 * M).checked_sub(10 * S).unwrap());
        assert!(t.is_running());
    }

    #[test]
    fn paused_ignores_ticks() {
        let mut t = Timer::new(TimerMode::Countdown(10 * S));
        assert_eq!(t.tick(20 * S), None);
        assert_eq!(t.display(), 10 * S);
        t.start();
        t.pause();
        t.tick(S);
        assert_eq!(t.display(), 10 * S);
    }

    #[test]
    fn reset_restarts_focus_paused() {
        let mut t = Timer::new(TimerMode::Intervals { focus: M, rest: M });
        t.start();
        t.tick(M + S);
        t.reset();
        assert_eq!(t.phase(), Phase::Focus);
        assert_eq!(t.display(), M);
        assert!(!t.is_running());
    }

    #[test]
    fn format_clock_handles_hours() {
        assert_eq!(format_clock(Duration::from_secs(3725)), "1:02:05");
        assert_eq!(format_clock(Duration::from_secs(65)), "01:05");
        assert_eq!(format_clock(Duration::from_millis(59_900)), "00:59");
    }

    #[test]
    fn parse_intervals_accepts_25m_5m() {
        assert_eq!(
            parse_intervals("25m/5m").unwrap(),
            TimerMode::Intervals {
                focus: 25 * M,
                rest: 5 * M
            }
        );
    }

    #[test]
    fn parse_intervals_rejects_garbage() {
        assert!(parse_intervals("25m").is_err());
        assert!(parse_intervals("x/y").is_err());
        assert!(parse_intervals("0m/5m").is_err());
    }
}
