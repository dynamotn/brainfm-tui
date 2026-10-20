use clap::{Args, Parser, Subcommand, ValueEnum};

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

pub async fn run(cmd: Command) -> anyhow::Result<()> {
    match cmd {
        Command::Login { .. } => todo!(),
        Command::Logout => todo!(),
        Command::Whoami => todo!(),
        Command::List { .. } => todo!(),
        Command::Play(_) => todo!(),
    }
}
