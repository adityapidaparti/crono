//! On-disk session storage: no background process, auth happens on demand.
//!
//! Layout: `~/.config/crono/session.json` holds the auth token from the last
//! login; `~/.config/crono/credentials.json` (0600) optionally holds the
//! username/password so an expired session can be refreshed without prompting.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub token: String,
    pub user_id: Option<i64>,
    #[serde(default)]
    pub timezone: Option<String>,
    #[serde(default)]
    pub saved_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Credentials {
    pub username: String,
    pub password: String,
}

fn config_dir() -> Result<PathBuf> {
    let dirs = directories::ProjectDirs::from("", "", "crono")
        .context("could not determine config directory")?;
    Ok(dirs.config_dir().to_path_buf())
}

fn read_json<T: serde::de::DeserializeOwned>(name: &str) -> Result<Option<T>> {
    let path = config_dir()?.join(name);
    match fs::read_to_string(&path) {
        Ok(s) => Ok(Some(
            serde_json::from_str(&s).with_context(|| format!("corrupt {}", path.display()))?,
        )),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

fn write_json<T: Serialize>(name: &str, value: &T) -> Result<PathBuf> {
    let dir = config_dir()?;
    fs::create_dir_all(&dir)?;
    let path = dir.join(name);
    fs::write(&path, serde_json::to_vec_pretty(value)?)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(path)
}

pub fn load_session() -> Result<Option<Session>> {
    read_json("session.json")
}

pub fn save_session(s: &Session) -> Result<PathBuf> {
    write_json("session.json", s)
}

pub fn clear_session() -> Result<()> {
    let path = config_dir()?.join("session.json");
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

pub fn load_credentials() -> Result<Option<Credentials>> {
    read_json("credentials.json")
}

pub fn save_credentials(c: &Credentials) -> Result<PathBuf> {
    write_json("credentials.json", c)
}
