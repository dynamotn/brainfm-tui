//! User preferences persisted as TOML.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

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

    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        }
        let tmp = path.with_extension("toml.tmp");
        fs::write(&tmp, toml::to_string_pretty(self)?)
            .with_context(|| format!("writing {}", tmp.display()))?;
        fs::rename(&tmp, path).with_context(|| format!("writing {}", path.display()))
    }
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
