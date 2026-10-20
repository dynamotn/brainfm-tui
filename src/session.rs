//! Persisted login session. Only the JWT is stored; never the password.

use std::fmt;
use std::fs;
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::{Deserialize, Serialize};

#[derive(Clone, PartialEq, Eq)]
pub struct Session {
    pub token: String,
    pub user_id: String,
}

impl fmt::Debug for Session {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Session")
            .field("token", &"<redacted>")
            .field("user_id", &self.user_id)
            .finish()
    }
}

/// Reads the `_id` claim from a JWT payload without verifying it.
pub fn user_id_from_jwt(token: &str) -> anyhow::Result<String> {
    let payload = token.split('.').nth(1).context("token is not a JWT")?;
    let bytes = URL_SAFE_NO_PAD
        .decode(payload.trim_end_matches('='))
        .context("token payload is not base64url")?;
    let json: serde_json::Value =
        serde_json::from_slice(&bytes).context("token payload is not JSON")?;
    match json.get("_id").and_then(|v| v.as_str()) {
        Some(id) if !id.is_empty() => Ok(id.to_owned()),
        _ => bail!("token has no user id"),
    }
}

/// Creates `dir` (and parents) and makes sure it is mode 0700 even if it existed.
pub fn ensure_private_dir(dir: &Path) -> anyhow::Result<()> {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
        .with_context(|| format!("creating {}", dir.display()))?;
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700))
        .with_context(|| format!("restricting {}", dir.display()))
}

#[derive(Serialize, Deserialize)]
struct SessionFile {
    token: String,
}

#[derive(Debug, Clone)]
pub struct SessionStore {
    path: PathBuf,
}

impl SessionStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn default_path() -> anyhow::Result<PathBuf> {
        Ok(crate::config::state_dir()?.join("session.json"))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn load(&self) -> anyhow::Result<Option<Session>> {
        let text = match fs::read_to_string(&self.path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e).with_context(|| format!("reading {}", self.path.display())),
        };
        let file: SessionFile = serde_json::from_str(&text)
            .with_context(|| format!("parsing {}", self.path.display()))?;
        let user_id = user_id_from_jwt(&file.token)?;
        Ok(Some(Session {
            token: file.token,
            user_id,
        }))
    }

    /// Writes the token atomically with mode 0600 inside a 0700 directory.
    pub fn save(&self, token: &str) -> anyhow::Result<Session> {
        let user_id = user_id_from_jwt(token)?;
        let dir = self.path.parent().context("session path has no parent")?;
        ensure_private_dir(dir)?;
        // A unique temp file per write: token refreshes can race each other.
        let mut tmp = tempfile::NamedTempFile::new_in(dir)
            .with_context(|| format!("writing into {}", dir.display()))?;
        tmp.as_file()
            .set_permissions(fs::Permissions::from_mode(0o600))?;
        let body = serde_json::to_string(&SessionFile {
            token: token.to_owned(),
        })?;
        tmp.write_all(body.as_bytes())?;
        tmp.as_file().sync_all()?;
        tmp.persist(&self.path)
            .with_context(|| format!("writing {}", self.path.display()))?;
        Ok(Session {
            token: token.to_owned(),
            user_id,
        })
    }

    pub fn clear(&self) -> anyhow::Result<()> {
        match fs::remove_file(&self.path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                Err(e).with_context(|| format!("removing {}", self.path.display()))
            }
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use std::os::unix::fs::PermissionsExt;

    fn jwt(uid: &str) -> String {
        let payload = URL_SAFE_NO_PAD.encode(format!(r#"{{"_id":"{uid}","exp":1}}"#));
        format!("h.{payload}.s")
    }

    #[test]
    fn user_id_from_jwt_reads_underscore_id() {
        assert_eq!(user_id_from_jwt(&jwt("u1")).unwrap(), "u1");
        assert!(user_id_from_jwt("garbage").is_err());
    }

    #[test]
    fn save_sets_0600_and_roundtrips() {
        let dir = tempfile::tempdir().unwrap();
        let store = SessionStore::new(dir.path().join("state/session.json"));
        assert!(store.load().unwrap().is_none());
        let token = jwt("u9");
        let saved = store.save(&token).unwrap();
        assert_eq!(saved.user_id, "u9");
        let mode = std::fs::metadata(store.path())
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
        let parent = std::fs::metadata(store.path().parent().unwrap()).unwrap();
        assert_eq!(parent.permissions().mode() & 0o777, 0o700);
        assert_eq!(store.load().unwrap().unwrap().token, token);
        store.clear().unwrap();
        assert!(store.load().unwrap().is_none());
    }

    #[test]
    fn concurrent_saves_never_corrupt() {
        let dir = tempfile::tempdir().unwrap();
        let store = SessionStore::new(dir.path().join("session.json"));
        let short = jwt("u1");
        let long = jwt(&"u".repeat(400));
        std::thread::scope(|sc| {
            for i in 0..8 {
                let (store, t) = (&store, if i % 2 == 0 { &short } else { &long });
                sc.spawn(move || {
                    for _ in 0..50 {
                        store.save(t).unwrap();
                    }
                });
            }
        });
        let loaded = store.load().unwrap().unwrap();
        assert!(loaded.token == short || loaded.token == long);
    }

    #[test]
    fn ensure_private_dir_tightens_existing_dir() {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("state");
        std::fs::create_dir(&state).unwrap();
        std::fs::set_permissions(&state, std::fs::Permissions::from_mode(0o755)).unwrap();
        ensure_private_dir(&state).unwrap();
        let mode = std::fs::metadata(&state).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
    }

    #[test]
    fn debug_redacts_token() {
        let s = Session {
            token: "secret-token".into(),
            user_id: "u1".into(),
        };
        let dbg = format!("{s:?}");
        assert!(!dbg.contains("secret-token"));
        assert!(dbg.contains("u1"));
    }
}
