use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::PrioError;
use crate::storage::{self, lock, prio_dir, write_atomic};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoConfig {
    pub work_branch: String,
    #[serde(default = "default_main")]
    pub default_branch: String,
}

fn default_main() -> String {
    "main".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct StackEntry {
    pub dependency: String,
    pub branch: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RepoState {
    pub work_branch: String,
    pub default_branch: String,
    pub baseline_commit: String,
    #[serde(default)]
    pub applied_branches: Vec<String>,
    #[serde(default)]
    pub commit_map: HashMap<String, String>,
    /// Work-branch commit SHA → branch name, or `"."` when unassigned (`prio mv <sha> .`).
    #[serde(default)]
    pub commit_assignments: HashMap<String, String>,
    #[serde(default)]
    pub stacks: Vec<StackEntry>,
    #[serde(default)]
    pub merge_in_progress: bool,
    #[serde(default)]
    pub pending_merge_branches: Vec<String>,
    #[serde(default)]
    pub pending_merge_index: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LastGoodState {
    pub commit_sha: String,
    pub state: RepoState,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConflictHistoryEntry {
    pub branch_a: String,
    pub branch_b: String,
    pub resolution_commit: String,
    pub depth: usize,
    pub resolved_at: u64,
    #[serde(default)]
    pub stale: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ApplyCache {
    #[serde(default)]
    pub entries: HashMap<String, usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct McState {
    #[serde(default)]
    pub merge_in_progress: bool,
    #[serde(default)]
    pub pending_merge_branches: Vec<String>,
    #[serde(default)]
    pub pending_merge_index: usize,
    #[serde(default)]
    pub default_branch: String,
    /// Name of the git branch created in prio-mc for this merge operation
    /// (e.g. `prio-mc/feature-a+feature-b`). Empty when no merge is in progress.
    #[serde(default)]
    pub merge_branch: String,
}

fn config_path(repo_path: &Path) -> PathBuf {
    prio_dir(repo_path).join("config.json")
}

fn state_path(repo_path: &Path) -> PathBuf {
    prio_dir(repo_path).join("state.json")
}

fn last_good_path(repo_path: &Path) -> PathBuf {
    prio_dir(repo_path).join("last_good_state.json")
}

pub fn mc_prio_dir(mc_path: &Path) -> PathBuf {
    prio_dir(mc_path)
}

fn mc_conflict_path(mc_path: &Path) -> PathBuf {
    mc_prio_dir(mc_path).join("conflict_history.json")
}

fn mc_apply_cache_path(mc_path: &Path) -> PathBuf {
    mc_prio_dir(mc_path).join("apply_cache.json")
}

fn mc_state_path(mc_path: &Path) -> PathBuf {
    mc_prio_dir(mc_path).join("mc_state.json")
}

pub fn ensure_prio_dirs(repo_path: &Path) -> Result<(), PrioError> {
    let dir = prio_dir(repo_path);
    std::fs::create_dir_all(dir.join("hooks"))?;
    std::fs::create_dir_all(dir.join("backup"))?;
    Ok(())
}

pub fn load_config(repo_path: &Path) -> Result<RepoConfig, PrioError> {
    let path = config_path(repo_path);
    if !path.exists() {
        return Err(PrioError::NotSetup(repo_path.to_path_buf()));
    }
    storage::read_json(&path)
}

pub fn save_config(repo_path: &Path, config: &RepoConfig) -> Result<(), PrioError> {
    ensure_prio_dirs(repo_path)?;
    write_atomic(&config_path(repo_path), config)
}

pub fn load_state(repo_path: &Path) -> Result<RepoState, PrioError> {
    storage::read_json_or_default(&state_path(repo_path))
}

pub fn save_state(repo_path: &Path, state: &RepoState) -> Result<(), PrioError> {
    let _guard = lock::acquire(&storage::repo_lock_path(repo_path))?;
    ensure_prio_dirs(repo_path)?;
    write_atomic(&state_path(repo_path), state)
}

pub fn save_last_good(repo_path: &Path, snapshot: &LastGoodState) -> Result<(), PrioError> {
    ensure_prio_dirs(repo_path)?;
    write_atomic(&last_good_path(repo_path), snapshot)
}

pub fn load_last_good(repo_path: &Path) -> Result<Option<LastGoodState>, PrioError> {
    let path = last_good_path(repo_path);
    if path.exists() {
        Ok(Some(storage::read_json(&path)?))
    } else {
        Ok(None)
    }
}

pub fn load_mc_state(mc_path: &Path) -> Result<McState, PrioError> {
    storage::read_json_or_default(&mc_state_path(mc_path))
}

pub fn save_mc_state(mc_path: &Path, state: &McState) -> Result<(), PrioError> {
    std::fs::create_dir_all(mc_prio_dir(mc_path))?;
    write_atomic(&mc_state_path(mc_path), state)
}

pub fn load_conflict_history(mc_path: &Path) -> Result<Vec<ConflictHistoryEntry>, PrioError> {
    storage::read_json_or_default(&mc_conflict_path(mc_path))
}

pub fn save_conflict_history(
    mc_path: &Path,
    entries: &[ConflictHistoryEntry],
) -> Result<(), PrioError> {
    std::fs::create_dir_all(mc_prio_dir(mc_path))?;
    write_atomic(&mc_conflict_path(mc_path), &entries.to_vec())
}

pub fn load_apply_cache(mc_path: &Path) -> Result<ApplyCache, PrioError> {
    storage::read_json_or_default(&mc_apply_cache_path(mc_path))
}

pub fn save_apply_cache(mc_path: &Path, cache: &ApplyCache) -> Result<(), PrioError> {
    std::fs::create_dir_all(mc_prio_dir(mc_path))?;
    write_atomic(&mc_apply_cache_path(mc_path), cache)
}

pub fn backup_dir(repo_path: &Path) -> PathBuf {
    prio_dir(repo_path).join("backup")
}
