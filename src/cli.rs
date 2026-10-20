use std::io::Write;

use anyhow::{Context, bail};
use clap::{Args, Parser, Subcommand, ValueEnum};
use url::Url;

use crate::api::client::DEFAULT_BASE;
use crate::api::{Activity, ApiClient, EffectLevel, MentalState, is_unauthorized};
use crate::session::{Session, SessionStore};

/// Listen to Brain.fm from the terminal.
#[derive(Debug, Parser)]
#[command(name = "brainfm", version, about)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Log in with email and password, or paste a token with --token
    Login {
        /// Read a token from stdin instead of logging in with a password
        #[arg(long)]
        token: bool,
    },
    /// Forget the stored session
    Logout,
    /// Show the logged-in account
    Whoami,
    /// List mental states, activities or genres
    List {
        what: ListWhat,
        /// Mental state to list activities or genres for (defaults to focus)
        #[arg(long)]
        state: Option<String>,
    },
    /// Play a mental state without the TUI
    Play(PlayArgs),
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum ListWhat {
    States,
    Activities,
    Genres,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum EffectArg {
    Low,
    Medium,
    High,
}

#[derive(Debug, Args)]
pub struct PlayArgs {
    /// Mental state: focus, relax, sleep or meditate
    pub state: String,
    /// Activity name, e.g. "Deep Work"
    #[arg(long)]
    pub activity: Option<String>,
    /// Neural effect level
    #[arg(long)]
    pub effect: Option<EffectArg>,
    /// Genre filter (repeatable)
    #[arg(long)]
    pub genre: Vec<String>,
    /// Stop after this long, e.g. 25m or 1h30m
    #[arg(long, conflicts_with = "intervals")]
    pub timer: Option<humantime::Duration>,
    /// Pomodoro intervals as focus/rest, e.g. 25m/5m
    #[arg(long)]
    pub intervals: Option<String>,
}

impl From<EffectArg> for EffectLevel {
    fn from(e: EffectArg) -> Self {
        match e {
            EffectArg::Low => EffectLevel::Low,
            EffectArg::Medium => EffectLevel::Medium,
            EffectArg::High => EffectLevel::High,
        }
    }
}

pub fn resolve_state<'a>(states: &'a [MentalState], q: &str) -> Option<&'a MentalState> {
    states
        .iter()
        .find(|s| s.id.eq_ignore_ascii_case(q) || s.display_value.eq_ignore_ascii_case(q))
}

pub fn resolve_activity<'a>(acts: &'a [Activity], q: &str) -> Option<&'a Activity> {
    acts.iter()
        .find(|a| a.id.eq_ignore_ascii_case(q) || a.display_value.eq_ignore_ascii_case(q))
}

/// User-facing text for a command error; a 401 means different things
/// during `login` and everywhere else.
pub fn describe_error(e: &anyhow::Error, during_login: bool) -> String {
    if !is_unauthorized(e) {
        return crate::headless::printable(&format!("error: {e:#}"));
    }
    if during_login {
        "Wrong email or password (or an invalid token)".to_owned()
    } else {
        "Session expired — run `brainfm login`".to_owned()
    }
}

fn base_url() -> Url {
    Url::parse(DEFAULT_BASE).expect("static base url")
}

/// Builds a client from the stored session; refreshed tokens are written back.
pub fn client_from_store(store: &SessionStore) -> anyhow::Result<(ApiClient, Session)> {
    let session = store
        .load()?
        .context("Not logged in — run `brainfm login`")?;
    let writer = store.clone();
    let client = ApiClient::new(base_url())
        .with_token(session.token.clone())
        .on_token_refresh(move |t| writer.refresh_in_background(t));
    Ok((client, session))
}

fn prompt(label: &str) -> anyhow::Result<String> {
    eprint!("{label}");
    std::io::stderr().flush()?;
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    Ok(line.trim().to_owned())
}

async fn login(store: &SessionStore, paste_token: bool) -> anyhow::Result<()> {
    let client = ApiClient::new(base_url());
    let token = if paste_token {
        let t = rpassword::prompt_password("Paste token (input hidden): ")?;
        let t = t.trim().trim_start_matches("Bearer ").to_owned();
        if t.is_empty() {
            bail!("no token given");
        }
        // Validate before saving so a bad paste never replaces a good session.
        ApiClient::new(base_url())
            .with_token(t.clone())
            .me()
            .await?;
        t
    } else {
        let email = prompt("Email: ")?;
        let password = rpassword::prompt_password("Password: ")?;
        client.email_login(&email, &password).await?
    };
    store.save(&token)?;
    let (client, _) = client_from_store(store)?;
    let me = client.me().await?;
    println!(
        "Logged in as {}",
        me.email.as_deref().unwrap_or("(unknown email)")
    );
    Ok(())
}

async fn whoami(store: &SessionStore) -> anyhow::Result<()> {
    let (client, session) = client_from_store(store)?;
    let me = client.me().await?;
    let name = me.name.or(me.first_name).unwrap_or_default();
    println!(
        "{}\t{}\t{}",
        session.user_id,
        me.email.unwrap_or_default(),
        name
    );
    Ok(())
}

async fn list(store: &SessionStore, what: ListWhat, state: Option<String>) -> anyhow::Result<()> {
    let (client, _) = client_from_store(store)?;
    let states = client.mental_states().await?;
    if let ListWhat::States = what {
        for s in &states {
            println!("{}\t{}", s.id, crate::headless::printable(&s.display_value));
        }
        return Ok(());
    }
    let q = state.as_deref().unwrap_or("focus");
    let st = resolve_state(&states, q).with_context(|| format!("unknown mental state {q:?}"))?;
    match what {
        ListWhat::Activities => {
            for a in client.activities(&st.id).await? {
                println!("{}\t{}", a.id, crate::headless::printable(&a.display_value));
            }
        }
        ListWhat::Genres => {
            for g in client.genres(&st.id).await? {
                println!("{}", crate::headless::printable(&g));
            }
        }
        ListWhat::States => unreachable!(),
    }
    Ok(())
}

pub async fn run(cmd: Command) -> anyhow::Result<()> {
    let store = SessionStore::new(SessionStore::default_path()?);
    match cmd {
        Command::Login { token } => login(&store, token).await,
        Command::Logout => {
            store.clear()?;
            println!("Logged out");
            Ok(())
        }
        Command::Whoami => whoami(&store).await,
        Command::List { what, state } => list(&store, what, state).await,
        Command::Play(args) => crate::headless::play(&store, args).await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::{Activity, MentalState};

    fn state(id: &str, name: &str) -> MentalState {
        MentalState {
            id: id.into(),
            display_value: name.into(),
            default_activity: None,
        }
    }

    #[test]
    fn resolve_state_matches_case_insensitively() {
        let states = vec![state("s1", "Focus"), state("s2", "Relax")];
        assert_eq!(resolve_state(&states, "FOCUS").unwrap().id, "s1");
        assert_eq!(resolve_state(&states, "s2").unwrap().display_value, "Relax");
        assert!(resolve_state(&states, "sleep").is_none());
    }

    #[test]
    fn resolve_activity_matches_display_value() {
        let acts = vec![Activity {
            id: "a1".into(),
            display_value: "Deep Work".into(),
        }];
        assert_eq!(resolve_activity(&acts, "deep work").unwrap().id, "a1");
    }
}
