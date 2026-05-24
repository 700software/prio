pub mod lock;
pub mod repo_state;
pub mod user_config;

use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;
use tempfile::NamedTempFile;

use crate::error::PrioError;

pub fn write_atomic<T: Serialize>(path: &Path, value: &T) -> Result<(), PrioError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let serialized = serde_json::to_vec_pretty(value)?;
    let mut tmp = NamedTempFile::new_in(
        path.parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from(".")),
    )?;
    std::io::Write::write_all(&mut tmp, &serialized)?;
    tmp.persist(path).map_err(|e| PrioError::Io(e.error))?;
    Ok(())
}

pub fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, PrioError> {
    let data = fs::read_to_string(path)?;
    Ok(serde_json::from_str(&data)?)
}

pub fn read_json_or_default<T: Default + serde::de::DeserializeOwned>(path: &Path) -> Result<T, PrioError> {
    if path.exists() {
        read_json(path)
    } else {
        Ok(T::default())
    }
}

/// Prio metadata under the repository's `.git/prio` (repo root validated at setup / command entry).
pub fn prio_dir(repo_path: &Path) -> PathBuf {
    repo_path.join(".git").join("prio")
}

pub fn repo_lock_path(repo_path: &Path) -> PathBuf {
    prio_dir(repo_path).join("prio.lock")
}

pub fn user_lock_path() -> PathBuf {
    user_config::user_config_dir().join("prio.lock")
}
