//! Everything the TUI shows, in one plain struct the reducer mutates.

use std::collections::{BTreeSet, HashSet};
use std::time::Duration;

use crate::api::{Activity, EffectLevel, MentalState, Serving, SessionRequest};
use crate::config::{Config, StatePrefs};
use crate::timer::{Timer, TimerMode};

const TOAST_TTL: Duration = Duration::from_secs(4);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Playback {
    Idle,
    Loading,
    Playing,
    Paused,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelTab {
    Library,
    Explore,
    Profile,
}

impl PanelTab {
    pub const ALL: [PanelTab; 3] = [PanelTab::Library, PanelTab::Explore, PanelTab::Profile];

    pub fn label(self) -> &'static str {
        match self {
            Self::Library => "Library",
            Self::Explore => "Explore",
            Self::Profile => "Profile",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Panel {
    pub visible: bool,
    pub tab: PanelTab,
    pub selected: usize,
    pub favorites: Vec<Serving>,
    pub recents: Vec<Serving>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoginField {
    Email,
    Password,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Overlay {
    Activity {
        sel: usize,
    },
    Genres {
        sel: usize,
        chosen: BTreeSet<String>,
    },
    Effect {
        sel: usize,
        chosen: BTreeSet<EffectLevel>,
    },
    Timer {
        sel: usize,
    },
    Login {
        email: String,
        password: String,
        field: LoginField,
        error: Option<String>,
    },
    Help,
}

impl Overlay {
    pub fn login() -> Self {
        Self::Login {
            email: String::new(),
            password: String::new(),
            field: LoginField::Email,
            error: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Toast {
    pub text: String,
    pub ttl: Duration,
}

impl Toast {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            ttl: TOAST_TTL,
        }
    }
}

#[derive(Debug, Clone)]
pub struct AppState {
    pub states: Vec<MentalState>,
    pub state_idx: usize,
    pub activities: Vec<Activity>,
    pub genres: Vec<String>,
    pub config: Config,
    /// False when the config file was malformed; it is then never overwritten.
    pub config_writable: bool,
    pub now: Option<Serving>,
    /// Sent to the engine with `Load`; becomes `now` once it reports `Started`.
    pub pending: Option<Serving>,
    pub next: Option<Serving>,
    pub playback: Playback,
    pub position: Duration,
    pub timer: Timer,
    pub favorite_ids: HashSet<String>,
    pub panel: Panel,
    pub overlay: Option<Overlay>,
    pub toast: Option<Toast>,
    pub user_id: Option<String>,
    pub errors_in_a_row: u32,
    /// Bumped on every session request; replies carrying an older value are stale.
    pub session_gen: u64,
    /// What `playback` returns to if a `Loading` request fails.
    pub resume_to: Playback,
    pub loading_for: Duration,
    pub quit: bool,
}

impl AppState {
    pub fn new(config: Config, config_writable: bool, user_id: Option<String>) -> Self {
        Self {
            states: Vec::new(),
            state_idx: 0,
            activities: Vec::new(),
            genres: Vec::new(),
            config,
            config_writable,
            now: None,
            pending: None,
            next: None,
            playback: Playback::Idle,
            position: Duration::ZERO,
            timer: Timer::new(TimerMode::Infinite),
            favorite_ids: HashSet::new(),
            panel: Panel {
                visible: true,
                tab: PanelTab::Library,
                selected: 0,
                favorites: Vec::new(),
                recents: Vec::new(),
            },
            overlay: if user_id.is_none() {
                Some(Overlay::login())
            } else {
                None
            },
            toast: None,
            user_id,
            errors_in_a_row: 0,
            session_gen: 0,
            resume_to: Playback::Idle,
            loading_for: Duration::ZERO,
            quit: false,
        }
    }

    pub fn current_state(&self) -> Option<&MentalState> {
        self.states.get(self.state_idx)
    }

    pub fn current_prefs(&self) -> StatePrefs {
        self.current_state()
            .and_then(|s| self.config.states.get(&s.id))
            .cloned()
            .unwrap_or_default()
    }

    pub fn prefs_mut(&mut self) -> Option<&mut StatePrefs> {
        let id = self.current_state()?.id.clone();
        Some(self.config.states.entry(id).or_default())
    }

    /// The activity the next session plays: saved choice, then the state's default.
    pub fn current_activity_id(&self) -> Option<String> {
        let state = self.current_state()?;
        self.current_prefs()
            .activity_id
            .or_else(|| state.default_activity.as_ref().map(|a| a.id.clone()))
            .or_else(|| self.activities.first().map(|a| a.id.clone()))
    }

    pub fn current_activity_name(&self) -> Option<String> {
        let id = self.current_activity_id()?;
        self.activities
            .iter()
            .find(|a| a.id == id)
            .or(self
                .current_state()?
                .default_activity
                .as_ref()
                .filter(|a| a.id == id))
            .map(|a| a.display_value.clone())
    }

    pub fn session_request(&self) -> Option<SessionRequest> {
        let prefs = self.current_prefs();
        Some(SessionRequest::new(
            &self.current_activity_id()?,
            prefs.effects,
            prefs.genres,
        ))
    }

    /// Favorites then recents, as listed in the Library tab.
    pub fn library(&self) -> impl Iterator<Item = &Serving> {
        self.panel.favorites.iter().chain(&self.panel.recents)
    }

    pub fn timer_options(&self) -> Vec<(String, TimerMode)> {
        let min = |m: u32| Duration::from_secs(u64::from(m) * 60);
        let mut opts = vec![
            ("Infinite".to_owned(), TimerMode::Infinite),
            ("30 min".to_owned(), TimerMode::Countdown(min(30))),
            ("1 hr".to_owned(), TimerMode::Countdown(min(60))),
            ("2 hrs".to_owned(), TimerMode::Countdown(min(120))),
        ];
        for m in self.config.timer.custom_minutes.iter().take(3) {
            opts.push((format!("{m} min"), TimerMode::Countdown(min(*m))));
        }
        let t = &self.config.timer;
        opts.push((
            format!("Intervals {}/{}", t.focus_minutes, t.break_minutes),
            TimerMode::Intervals {
                focus: min(t.focus_minutes),
                rest: min(t.break_minutes),
            },
        ));
        opts
    }
}
