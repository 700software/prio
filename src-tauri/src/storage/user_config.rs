use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::PrioError;
use crate::storage::{self, lock, write_atomic};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UserConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub work_branch_prefix: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UiState {
    #[serde(default)]
    pub tab_order: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoRecord {
    pub id: String,
    pub path: String,
    pub origin_normalized: String,
    pub mc_clone_path: String,
    pub added_at: u64,
}

pub fn user_config_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("PRIO_CONFIG_DIR") {
        return PathBuf::from(dir);
    }
    #[cfg(target_os = "windows")]
    {
        let base = std::env::var("APPDATA").unwrap_or_else(|_| ".".into());
        return PathBuf::from(base).join("prio");
    }
    #[cfg(target_os = "macos")]
    {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
        return PathBuf::from(home).join("Library/Application Support/prio");
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
            return PathBuf::from(xdg).join("prio");
        }
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
        PathBuf::from(home).join(".config/prio")
    }
}

fn config_path() -> PathBuf {
    user_config_dir().join("config.json")
}

fn ui_state_path() -> PathBuf {
    user_config_dir().join("ui-state.json")
}

fn repos_path() -> PathBuf {
    user_config_dir().join("repos.json")
}

pub fn load_config() -> Result<UserConfig, PrioError> {
    storage::read_json_or_default(&config_path())
}

pub fn save_config(config: &UserConfig) -> Result<(), PrioError> {
    let _guard = lock::acquire(&storage::user_lock_path())?;
    std::fs::create_dir_all(user_config_dir())?;
    write_atomic(&config_path(), config)
}

pub fn load_ui_state() -> Result<UiState, PrioError> {
    storage::read_json_or_default(&ui_state_path())
}

pub fn save_ui_state(state: &UiState) -> Result<(), PrioError> {
    let _guard = lock::acquire(&storage::user_lock_path())?;
    std::fs::create_dir_all(user_config_dir())?;
    write_atomic(&ui_state_path(), state)
}

pub fn load_repos() -> Result<Vec<RepoRecord>, PrioError> {
    storage::read_json_or_default(&repos_path())
}

pub fn save_repos(repos: &[RepoRecord]) -> Result<(), PrioError> {
    let _guard = lock::acquire(&storage::user_lock_path())?;
    std::fs::create_dir_all(user_config_dir())?;
    write_atomic(&repos_path(), &repos.to_vec())
}

pub fn find_repo_by_origin(origin: &str) -> Result<Option<RepoRecord>, PrioError> {
    Ok(load_repos()?
        .into_iter()
        .find(|r| r.origin_normalized == origin))
}

pub fn find_repo_by_path(path: &Path) -> Result<Option<RepoRecord>, PrioError> {
    let abs = crate::util::absolute_path(path);
    let abs_str = abs.to_string_lossy().to_string();
    Ok(load_repos()?
        .into_iter()
        .find(|r| r.path == abs_str))
}

pub fn upsert_repo(record: RepoRecord) -> Result<(), PrioError> {
    let mut repos = load_repos()?;
    if let Some(idx) = repos.iter().position(|r| r.id == record.id) {
        repos[idx] = record;
    } else {
        repos.push(record);
    }
    save_repos(&repos)
}

pub fn remove_repo_by_path(path: &Path) -> Result<(), PrioError> {
    let abs = crate::util::absolute_path(path);
    let abs_str = abs.to_string_lossy().to_string();
    let mut repos = load_repos()?;
    let before = repos.len();
    repos.retain(|r| r.path != abs_str);
    if repos.len() < before {
        save_repos(&repos)?;
    }
    Ok(())
}
