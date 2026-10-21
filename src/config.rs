//! User preferences persisted as TOML.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Context;
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};

use crate::api::EffectLevel;

fn dirs() -> anyhow::Result<ProjectDirs> {
    ProjectDirs::from("", "", "brainfm").context("cannot determine home directory")
}

/// `~/.local/state/brainfm` on Linux; falls back to the data dir elsewhere.
pub(crate) fn state_dir() -> anyhow::Result<PathBuf> {
    let d = dirs()?;
    Ok(d.state_dir()
        .unwrap_or_else(|| d.data_local_dir())
        .to_path_buf())
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub volume: u8,
    pub last_state: Option<String>,
    pub states: BTreeMap<String, StatePrefs>,
    pub timer: TimerPrefs,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            volume: 70,
            last_state: None,
            states: BTreeMap::new(),
            timer: TimerPrefs::default(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StatePrefs {
    pub activity_id: Option<String>,
    pub genres: Vec<String>,
    pub effects: Vec<EffectLevel>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
// The field names are the config file's keys; the unit suffix is deliberate.
#[allow(clippy::struct_field_names)]
pub struct TimerPrefs {
    /// Up to three saved custom countdown lengths, in minutes.
    pub custom_minutes: Vec<u32>,
    pub focus_minutes: u32,
    pub break_minutes: u32,
}

impl Default for TimerPrefs {
    fn default() -> Self {
        Self {
            custom_minutes: Vec::new(),
            focus_minutes: 25,
            break_minutes: 5,
        }
    }
}

impl Config {
    pub fn default_path() -> anyhow::Result<PathBuf> {
        Ok(dirs()?.config_dir().join("config.toml"))
    }

    pub fn load(path: &Path) -> anyhow::Result<Config> {
        match fs::read_to_string(path) {
            Ok(text) => {
                let mut c: Config = toml::from_str(&text)
                    .with_context(|| format!("invalid config {}", path.display()))?;
                c.volume = c.volume.min(100);
                Ok(c)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
            Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
        }
    }

    /// Writes atomically through a temp file unique to this write, so two
    /// running instances never clobber each other's half-written file.
    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        use std::io::Write;
        let dir = path.parent().unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        let mut tmp = tempfile::NamedTempFile::new_in(dir)
            .with_context(|| format!("writing into {}", dir.display()))?;
        tmp.write_all(toml::to_string_pretty(self)?.as_bytes())?;
        tmp.as_file().sync_all()?;
        tmp.persist(path)
            .with_context(|| format!("writing {}", path.display()))?;
        Ok(())
    }
}

/// How long settings must stay unchanged before they are written, so a
/// dragged volume slider does not rewrite the file dozens of times.
pub(crate) const SAVE_DEBOUNCE: Duration = Duration::from_millis(500);

/// Writes the latest `Some(config)` sent to the returned channel once
/// changes have paused for `debounce`. Writing happens off the async runtime.
pub(crate) fn spawn_writer(
    path: PathBuf,
    debounce: Duration,
) -> tokio::sync::watch::Sender<Option<Config>> {
    let (tx, mut rx) = tokio::sync::watch::channel(None::<Config>);
    tokio::spawn(async move {
        while rx.changed().await.is_ok() {
            // Keep waiting while changes keep coming.
            loop {
                tokio::select! {
                    changed = rx.changed() => if changed.is_err() { break },
                    () = tokio::time::sleep(debounce) => break,
                }
            }
            let Some(config) = rx.borrow_and_update().clone() else {
                continue;
            };
            let path = path.clone();
            let saved = tokio::task::spawn_blocking(move || config.save(&path)).await;
            match saved {
                Ok(Err(e)) => tracing::warn!("saving config: {e:#}"),
                Err(e) => tracing::warn!("saving config: {e}"),
                Ok(Ok(())) => {}
            }
        }
    });
    tx
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::EffectLevel;

    #[test]
    fn missing_config_is_default() {
        let dir = tempfile::tempdir().unwrap();
        let c = Config::load(&dir.path().join("none.toml")).unwrap();
        assert_eq!(c.volume, 70);
        assert_eq!(c.timer.focus_minutes, 25);
        assert_eq!(c.timer.break_minutes, 5);
    }

    #[test]
    fn malformed_config_is_error_with_path() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "volume = [").unwrap();
        let err = format!("{:#}", Config::load(&path).unwrap_err());
        assert!(err.contains(&path.display().to_string()), "{err}");
    }

    #[test]
    fn volume_above_100_is_clamped() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "volume = 150").unwrap();
        assert_eq!(Config::load(&path).unwrap().volume, 100);
    }

    #[test]
    fn concurrent_saves_never_corrupt() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let small = Config::default();
        let mut big = Config::default();
        for i in 0..50 {
            big.states
                .insert(format!("state{i}"), StatePrefs::default());
        }
        std::thread::scope(|sc| {
            for i in 0..8 {
                let (path, c) = (&path, if i % 2 == 0 { &small } else { &big });
                sc.spawn(move || {
                    for _ in 0..30 {
                        c.save(path).unwrap();
                    }
                });
            }
        });
        let loaded = Config::load(&path).unwrap();
        assert!(loaded == small || loaded == big);
    }

    #[tokio::test]
    async fn bursts_of_changes_are_written_once_with_the_latest() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let writer = spawn_writer(path.clone(), Duration::from_millis(80));
        for v in 0..=50u8 {
            writer.send_replace(Some(Config {
                volume: v,
                ..Config::default()
            }));
        }
        assert!(!path.exists(), "nothing written during the burst");
        tokio::time::sleep(Duration::from_millis(400)).await;
        assert_eq!(Config::load(&path).unwrap().volume, 50);
    }

    #[test]
    fn config_roundtrips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub/config.toml");
        let mut c = Config {
            volume: 40,
            ..Config::default()
        };
        c.states.insert(
            "focus".into(),
            StatePrefs {
                activity_id: Some("deep".into()),
                genres: vec!["Piano".into()],
                effects: vec![EffectLevel::High],
            },
        );
        c.save(&path).unwrap();
        assert_eq!(Config::load(&path).unwrap(), c);
    }
}
