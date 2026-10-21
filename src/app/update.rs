//! The reducer: `update(state, action) -> effects`. No I/O and no clock reads.

use std::time::Duration;

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use super::state::{AppState, LoginField, Overlay, PanelTab, Playback, Toast};
use crate::api::{Activity, EffectLevel, MentalState, Serving, SessionRequest};
use crate::player::{PlayerCommand, PlayerEvent};
use crate::timer::{Phase, Timer, TimerEvent};

const MAX_ERRORS_IN_A_ROW: u32 = 3;
const END_FADE: Duration = Duration::from_secs(3);
const QUIT_FADE: Duration = Duration::from_millis(500);
const VOLUME_STEP: u8 = 10;
const LOADING_TIMEOUT: Duration = Duration::from_secs(30);

pub use crate::media::MediaKey;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Intent {
    /// Replace what is playing.
    Now,
    /// Queue after the current track.
    Next,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApiErrorKind {
    Unauthorized,
    Other(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum ApiReply {
    States(Vec<MentalState>),
    Activities {
        state_id: String,
        items: Vec<Activity>,
    },
    Genres {
        state_id: String,
        items: Vec<String>,
    },
    Servings {
        intent: Intent,
        generation: u64,
        items: Vec<Serving>,
    },
    Library {
        favorites: Vec<Serving>,
        recents: Vec<Serving>,
    },
    LoggedIn {
        user_id: String,
    },
    /// A `StartSession` failed; carries its generation so stale ones are ignored.
    SessionFailed {
        generation: u64,
        error: ApiErrorKind,
    },
    Failed {
        what: &'static str,
        error: ApiErrorKind,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Key(KeyEvent),
    Tick(Duration),
    Player(PlayerEvent),
    Api(ApiReply),
    Media(MediaKey),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    FetchStates,
    /// Activities and genres for a mental state id.
    FetchFilters(String),
    StartSession {
        req: SessionRequest,
        intent: Intent,
        generation: u64,
    },
    Player(PlayerCommand),
    SetFavorite {
        track_id: String,
        on: bool,
    },
    Dislike {
        track_id: String,
    },
    FetchLibrary,
    Login {
        email: String,
        password: String,
    },
    SaveConfig,
}

pub fn update(s: &mut AppState, action: Action) -> Vec<Effect> {
    let mut fx = Vec::new();
    match action {
        Action::Key(k) if k.kind != KeyEventKind::Release => on_key(s, k, &mut fx),
        Action::Key(_) => {}
        Action::Tick(dt) => on_tick(s, dt, &mut fx),
        Action::Player(e) => on_player(s, e, &mut fx),
        Action::Api(r) => on_api(s, r, &mut fx),
        Action::Media(m) => match m {
            MediaKey::Toggle => toggle_play(s, &mut fx),
            MediaKey::Play if s.playback != Playback::Playing => toggle_play(s, &mut fx),
            MediaKey::Pause if s.playback == Playback::Playing => toggle_play(s, &mut fx),
            MediaKey::Next => skip(s, &mut fx),
            MediaKey::SetVolume(v) => set_volume(s, v, &mut fx),
            _ => {}
        },
    }
    fx
}

fn toast(s: &mut AppState, text: impl Into<String>) {
    s.toast = Some(Toast::new(text));
}

fn save_config(s: &AppState, fx: &mut Vec<Effect>) {
    if s.config_writable {
        fx.push(Effect::SaveConfig);
    }
}

fn start_session(s: &mut AppState, intent: Intent, fx: &mut Vec<Effect>) {
    match s.session_request() {
        Some(req) => {
            s.session_gen += 1;
            if intent == Intent::Now {
                begin_loading(s);
            }
            fx.push(Effect::StartSession {
                req,
                intent,
                generation: s.session_gen,
            });
        }
        None => toast(s, "Still loading mental states…"),
    }
}

/// Enters `Loading`, remembering what the engine is doing meanwhile.
fn begin_loading(s: &mut AppState) {
    if s.playback != Playback::Loading {
        s.resume_to = s.playback;
    }
    s.playback = Playback::Loading;
    s.loading_for = Duration::ZERO;
}

/// Starts a replacement track when the engine has gone silent (track ended
/// or failed); if that fails there is nothing to resume.
fn start_after_silence(s: &mut AppState, fx: &mut Vec<Effect>) {
    start_session(s, Intent::Now, fx);
    if s.playback == Playback::Loading {
        s.resume_to = Playback::Idle;
    }
}

/// A `Loading` request failed: go back to whatever the engine is still doing.
fn abort_loading(s: &mut AppState, fx: &mut Vec<Effect>) {
    if s.pending.take().is_some() {
        fx.push(Effect::Player(PlayerCommand::CancelPending));
    }
    if s.playback == Playback::Loading {
        s.playback = s.resume_to;
    }
}

fn load(s: &mut AppState, serving: Serving, fx: &mut Vec<Effect>) {
    let v = &serving.track_variation;
    if let Some(url) = v.stream_url() {
        fx.push(Effect::Player(PlayerCommand::Load {
            id: serving.track.id.clone(),
            url,
            length: v.length(),
        }));
        s.playback = Playback::Loading;
        s.pending = Some(serving);
    } else {
        s.errors_in_a_row += 1;
        abort_loading(s, fx);
        toast(s, "That track has no audio URL");
    }
}

fn toggle_play(s: &mut AppState, fx: &mut Vec<Effect>) {
    match s.playback {
        Playback::Idle => start_session(s, Intent::Now, fx),
        Playback::Loading => {}
        Playback::Playing => {
            fx.push(Effect::Player(PlayerCommand::Pause));
            s.timer.pause();
            s.playback = Playback::Paused;
        }
        Playback::Paused => {
            fx.push(Effect::Player(PlayerCommand::Play));
            s.timer.start();
            s.playback = Playback::Playing;
        }
    }
}

fn skip(s: &mut AppState, fx: &mut Vec<Effect>) {
    start_session(s, Intent::Now, fx);
}

fn is_active(s: &AppState) -> bool {
    s.playback != Playback::Idle
}

/// Re-runs the session with new filters if something is playing.
fn prefs_changed(s: &mut AppState, fx: &mut Vec<Effect>) {
    save_config(s, fx);
    if is_active(s) {
        skip(s, fx);
    }
}

fn select_state(s: &mut AppState, idx: usize, fx: &mut Vec<Effect>) {
    if idx >= s.states.len() || idx == s.state_idx && !s.activities.is_empty() {
        return;
    }
    s.state_idx = idx;
    s.activities.clear();
    s.genres.clear();
    let id = s.states[idx].id.clone();
    s.config.last_state = Some(id.clone());
    fx.push(Effect::FetchFilters(id));
    prefs_changed(s, fx);
}

fn on_key(s: &mut AppState, k: KeyEvent, fx: &mut Vec<Effect>) {
    if k.modifiers.contains(KeyModifiers::CONTROL) && k.code == KeyCode::Char('c') {
        quit(s, fx);
        return;
    }
    if s.overlay.is_some() {
        on_overlay_key(s, k, fx);
        return;
    }
    let n = s.states.len();
    match k.code {
        KeyCode::Char(' ') => toggle_play(s, fx),
        KeyCode::Char('n') => skip(s, fx),
        KeyCode::Char('f') => {
            if let Some(id) = s.now.as_ref().map(|n| n.track.id.clone()) {
                let on = !s.favorite_ids.contains(&id);
                if on {
                    s.favorite_ids.insert(id.clone());
                } else {
                    s.favorite_ids.remove(&id);
                }
                fx.push(Effect::SetFavorite { track_id: id, on });
            }
        }
        KeyCode::Char('d') => {
            if let Some(id) = s.now.as_ref().map(|n| n.track.id.clone()) {
                fx.push(Effect::Dislike { track_id: id });
                toast(s, "Got it — you won't hear that track again");
                skip(s, fx);
            }
        }
        KeyCode::Tab if n > 0 => select_state(s, (s.state_idx + 1) % n, fx),
        KeyCode::BackTab if n > 0 => select_state(s, (s.state_idx + n - 1) % n, fx),
        KeyCode::Char(c @ '1'..='9') => select_state(s, c as usize - '1' as usize, fx),
        KeyCode::Char('a') => s.overlay = Some(Overlay::Activity { sel: 0 }),
        KeyCode::Char('g') => {
            let chosen = s.current_prefs().genres.into_iter().collect();
            s.overlay = Some(Overlay::Genres { sel: 0, chosen });
        }
        KeyCode::Char('e') => {
            let chosen = s.current_prefs().effects.into_iter().collect();
            s.overlay = Some(Overlay::Effect { sel: 0, chosen });
        }
        KeyCode::Char('t') => s.overlay = Some(Overlay::Timer { sel: 0 }),
        KeyCode::Char('?') => s.overlay = Some(Overlay::Help),
        KeyCode::Char('+' | '=') => set_volume(s, s.config.volume.saturating_add(VOLUME_STEP), fx),
        KeyCode::Char('-') => set_volume(s, s.config.volume.saturating_sub(VOLUME_STEP), fx),
        KeyCode::Char('p') => s.panel.visible = !s.panel.visible,
        KeyCode::Char(']') => cycle_tab(s, 1),
        KeyCode::Char('[') => cycle_tab(s, PanelTab::ALL.len() - 1),
        KeyCode::Char('j') | KeyCode::Down if s.panel.visible => {
            let len = panel_len(s);
            if len > 0 {
                s.panel.selected = (s.panel.selected + 1).min(len - 1);
            }
        }
        KeyCode::Char('k') | KeyCode::Up if s.panel.visible => {
            s.panel.selected = s.panel.selected.saturating_sub(1);
        }
        KeyCode::Enter if s.panel.visible => panel_enter(s, fx),
        KeyCode::Char('q') => quit(s, fx),
        _ => {}
    }
}

fn quit(s: &mut AppState, fx: &mut Vec<Effect>) {
    s.quit = true;
    fx.push(Effect::Player(PlayerCommand::Stop { fade: QUIT_FADE }));
}

fn set_volume(s: &mut AppState, v: u8, fx: &mut Vec<Effect>) {
    s.config.volume = v.min(100);
    fx.push(Effect::Player(PlayerCommand::SetVolume(
        f32::from(s.config.volume) / 100.0,
    )));
    save_config(s, fx);
}

fn cycle_tab(s: &mut AppState, by: usize) {
    let i = PanelTab::ALL
        .iter()
        .position(|t| *t == s.panel.tab)
        .unwrap_or(0);
    s.panel.tab = PanelTab::ALL[(i + by) % PanelTab::ALL.len()];
    s.panel.selected = 0;
}

fn panel_len(s: &AppState) -> usize {
    match s.panel.tab {
        PanelTab::Library => s.panel.favorites.len() + s.panel.recents.len(),
        PanelTab::Explore => s.activities.len(),
        PanelTab::Profile => 0,
    }
}

fn panel_enter(s: &mut AppState, fx: &mut Vec<Effect>) {
    match s.panel.tab {
        PanelTab::Library => {
            let picked = s.library().nth(s.panel.selected).cloned();
            if let Some(serving) = picked {
                // Supersede any session request still in flight.
                s.session_gen += 1;
                begin_loading(s);
                load(s, serving, fx);
            }
        }
        PanelTab::Explore => {
            if let Some(id) = s.activities.get(s.panel.selected).map(|a| a.id.clone()) {
                choose_activity(s, id, fx);
            }
        }
        PanelTab::Profile => {}
    }
}

fn choose_activity(s: &mut AppState, id: String, fx: &mut Vec<Effect>) {
    if let Some(p) = s.prefs_mut() {
        p.activity_id = Some(id);
    }
    prefs_changed(s, fx);
}

fn move_sel(sel: &mut usize, code: KeyCode, len: usize) {
    match code {
        KeyCode::Char('j') | KeyCode::Down => *sel = (*sel + 1).min(len.saturating_sub(1)),
        KeyCode::Char('k') | KeyCode::Up => *sel = sel.saturating_sub(1),
        _ => {}
    }
}

fn toggle<T: Ord + Clone>(set: &mut std::collections::BTreeSet<T>, item: Option<&T>) {
    if let Some(item) = item
        && !set.remove(item)
    {
        set.insert(item.clone());
    }
}

fn on_overlay_key(s: &mut AppState, k: KeyEvent, fx: &mut Vec<Effect>) {
    let Some(mut overlay) = s.overlay.take() else {
        return;
    };
    let mut keep = true;
    match &mut overlay {
        Overlay::Login {
            email,
            password,
            field,
            error,
        } => match k.code {
            KeyCode::Esc => keep = s.user_id.is_none(),
            KeyCode::Tab | KeyCode::BackTab | KeyCode::Up | KeyCode::Down => {
                *field = match field {
                    LoginField::Email => LoginField::Password,
                    LoginField::Password => LoginField::Email,
                };
            }
            KeyCode::Enter if *field == LoginField::Email => *field = LoginField::Password,
            KeyCode::Enter => {
                *error = None;
                fx.push(Effect::Login {
                    email: email.clone(),
                    password: password.clone(),
                });
            }
            KeyCode::Backspace => {
                match field {
                    LoginField::Email => email.pop(),
                    LoginField::Password => password.pop(),
                };
            }
            KeyCode::Char(c) => match field {
                LoginField::Email => email.push(c),
                LoginField::Password => password.push(c),
            },
            _ => {}
        },
        Overlay::Help => keep = false,
        _ if k.code == KeyCode::Esc || k.code == KeyCode::Char('q') => keep = false,
        Overlay::Activity { sel } => {
            move_sel(sel, k.code, s.activities.len());
            if k.code == KeyCode::Enter {
                keep = false;
                if let Some(id) = s.activities.get(*sel).map(|a| a.id.clone()) {
                    choose_activity(s, id, fx);
                }
            }
        }
        Overlay::Genres { sel, chosen } => {
            move_sel(sel, k.code, s.genres.len());
            match k.code {
                KeyCode::Char(' ') => toggle(chosen, s.genres.get(*sel)),
                KeyCode::Enter => {
                    keep = false;
                    let genres = chosen.iter().cloned().collect();
                    if let Some(p) = s.prefs_mut() {
                        p.genres = genres;
                    }
                    prefs_changed(s, fx);
                }
                _ => {}
            }
        }
        Overlay::Effect { sel, chosen } => {
            move_sel(sel, k.code, EffectLevel::ALL.len());
            match k.code {
                KeyCode::Char(' ') => toggle(chosen, EffectLevel::ALL.get(*sel)),
                KeyCode::Enter => {
                    keep = false;
                    let effects = chosen.iter().copied().collect();
                    if let Some(p) = s.prefs_mut() {
                        p.effects = effects;
                    }
                    prefs_changed(s, fx);
                }
                _ => {}
            }
        }
        Overlay::Timer { sel } => {
            let opts = s.timer_options();
            move_sel(sel, k.code, opts.len());
            if k.code == KeyCode::Enter {
                keep = false;
                if let Some((_, mode)) = opts.get(*sel) {
                    s.timer = Timer::new(*mode);
                    if s.playback == Playback::Playing {
                        s.timer.start();
                    }
                }
            }
        }
    }
    if keep {
        s.overlay = Some(overlay);
    }
}

fn on_tick(s: &mut AppState, dt: Duration, fx: &mut Vec<Effect>) {
    if let Some(t) = &mut s.toast {
        t.ttl = t.ttl.saturating_sub(dt);
        if t.ttl.is_zero() {
            s.toast = None;
        }
    }
    if s.playback == Playback::Loading {
        s.loading_for += dt;
        if s.loading_for >= LOADING_TIMEOUT {
            s.session_gen += 1;
            s.errors_in_a_row += 1;
            abort_loading(s, fx);
            toast(s, "Timed out loading a track — press space to retry");
        }
    }
    match s.timer.tick(dt) {
        Some(TimerEvent::Finished) => {
            fx.push(Effect::Player(PlayerCommand::Stop { fade: END_FADE }));
            s.playback = Playback::Idle;
            s.session_gen += 1;
            s.next = None;
            s.pending = None;
            toast(s, "Session complete");
        }
        Some(TimerEvent::PhaseChanged(phase)) => {
            fx.push(Effect::Player(PlayerCommand::Chime));
            if phase == Phase::Rest {
                fx.push(Effect::Player(PlayerCommand::Pause));
                s.playback = Playback::Paused;
                toast(s, "Break time");
            } else {
                fx.push(Effect::Player(PlayerCommand::Play));
                s.playback = Playback::Playing;
                toast(s, "Back to focus");
            }
        }
        None => {}
    }
}

fn on_player(s: &mut AppState, e: PlayerEvent, fx: &mut Vec<Effect>) {
    match e {
        PlayerEvent::Started { id } => {
            if s.pending.as_ref().is_some_and(|p| p.track.id == id) {
                s.now = s.pending.take();
                s.next = None;
            } else if s.next.as_ref().is_some_and(|n| n.track.id == id) {
                s.now = s.next.take();
            } else if s.now.as_ref().is_none_or(|n| n.track.id != id) {
                return;
            }
            s.playback = Playback::Playing;
            s.position = Duration::ZERO;
            s.errors_in_a_row = 0;
            s.timer.start();
        }
        PlayerEvent::Position { id, pos } => {
            if s.now.as_ref().is_some_and(|n| n.track.id == id) {
                s.position = pos;
            }
        }
        PlayerEvent::NearEnd { .. } => {
            // While a replacement is loading, prefetching would supersede it.
            if s.next.is_none() && s.playback == Playback::Playing {
                start_session(s, Intent::Next, fx);
            }
        }
        PlayerEvent::Ended { .. } => {
            if s.next.is_none() && s.playback == Playback::Playing {
                start_after_silence(s, fx);
            }
        }
        PlayerEvent::Error { id, message } => {
            if s.next.as_ref().is_some_and(|n| n.track.id == id) {
                // The queued track failed; a fresh one is fetched when this one ends.
                s.next = None;
                return;
            }
            let was_pending = s.pending.as_ref().is_some_and(|p| p.track.id == id);
            if !was_pending && s.now.as_ref().is_none_or(|n| n.track.id != id) {
                return;
            }
            if was_pending {
                // The engine never switched; whatever played before still does.
                s.pending = None;
            }
            s.errors_in_a_row += 1;
            if s.errors_in_a_row >= MAX_ERRORS_IN_A_ROW {
                abort_loading(s, fx);
                if !was_pending {
                    s.playback = Playback::Idle;
                }
                toast(s, format!("Playback keeps failing: {message}"));
            } else {
                toast(s, format!("Skipping track: {message}"));
                if was_pending {
                    start_session(s, Intent::Now, fx);
                } else {
                    start_after_silence(s, fx);
                }
            }
        }
    }
}

fn on_api(s: &mut AppState, r: ApiReply, fx: &mut Vec<Effect>) {
    match r {
        ApiReply::States(states) => {
            s.states = states;
            s.state_idx = s
                .config
                .last_state
                .as_ref()
                .and_then(|id| s.states.iter().position(|st| &st.id == id))
                .unwrap_or(0);
            if let Some(st) = s.current_state() {
                fx.push(Effect::FetchFilters(st.id.clone()));
            }
        }
        ApiReply::Activities { state_id, items } => {
            if s.current_state().is_some_and(|st| st.id == state_id) {
                s.activities = items;
            }
        }
        ApiReply::Genres { state_id, items } => {
            if s.current_state().is_some_and(|st| st.id == state_id) {
                s.genres = items;
            }
        }
        ApiReply::Servings { generation, .. } if generation != s.session_gen => {}
        ApiReply::Servings {
            intent: Intent::Next,
            ..
        } if s.next.is_some() || !matches!(s.playback, Playback::Playing | Playback::Paused) => {}
        ApiReply::Servings { intent, items, .. } => match (intent, items.into_iter().next()) {
            (Intent::Now, Some(serving)) => load(s, serving, fx),
            (Intent::Next, Some(serving)) => {
                if let Some(url) = serving.track_variation.stream_url() {
                    fx.push(Effect::Player(PlayerCommand::Queue {
                        id: serving.track.id.clone(),
                        url,
                        length: serving.track_variation.length(),
                    }));
                    s.next = Some(serving);
                }
            }
            (intent, None) => {
                toast(s, "No tracks match these filters");
                if intent == Intent::Now {
                    abort_loading(s, fx);
                }
            }
        },
        ApiReply::Library { favorites, recents } => {
            s.favorite_ids = favorites.iter().map(|f| f.track.id.clone()).collect();
            s.panel.favorites = favorites;
            s.panel.recents = recents;
        }
        ApiReply::LoggedIn { user_id } => {
            s.user_id = Some(user_id);
            s.overlay = None;
            fx.push(Effect::FetchStates);
            fx.push(Effect::FetchLibrary);
        }
        ApiReply::SessionFailed { generation, .. } if generation != s.session_gen => {}
        ApiReply::SessionFailed { error, .. } => on_api(
            s,
            ApiReply::Failed {
                what: "session",
                error,
            },
            fx,
        ),
        ApiReply::Failed { what, error } => on_failed(s, what, error, fx),
    }
}

fn on_failed(s: &mut AppState, what: &'static str, error: ApiErrorKind, fx: &mut Vec<Effect>) {
    match error {
        ApiErrorKind::Unauthorized => {
            abort_loading(s, fx);
            if s.playback == Playback::Playing {
                fx.push(Effect::Player(PlayerCommand::Pause));
                s.timer.pause();
                s.playback = Playback::Paused;
            }
            let message = (what == "login").then(|| "Wrong email or password".to_owned());
            match &mut s.overlay {
                Some(Overlay::Login { error, .. }) => {
                    *error = message.or(Some("Session expired — log in again".into()));
                }
                _ => {
                    s.overlay = Some(Overlay::Login {
                        email: String::new(),
                        password: String::new(),
                        field: LoginField::Email,
                        error: Some("Session expired — log in again".into()),
                    });
                }
            }
        }
        ApiErrorKind::Other(msg) => {
            if let Some(Overlay::Login { error, .. }) = &mut s.overlay
                && what == "login"
            {
                *error = Some(msg);
                return;
            }
            if what == "session" {
                abort_loading(s, fx);
            }
            toast(s, format!("Could not load {what}: {msg}"));
        }
    }
}
