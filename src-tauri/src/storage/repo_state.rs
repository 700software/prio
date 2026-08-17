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
    /// Branch that is stacked (depends on `dependencies`).
    pub branch: String,
    /// Branches this branch is stacked after (its dependencies).
    #[serde(default)]
    pub dependencies: Vec<String>,
}

/// Persisted when a `prio mv` hits a cherry-pick conflict in prio-mc.
/// Cleared by `prio abort`, `prio recover`, or when the operation completes.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MvRebaseConflict {
    /// Branch being rebased (source of the mv).
    pub source_branch: String,
    /// Destination branch.
    pub dest_branch: String,
    /// SHA that caused the cherry-pick conflict in prio-mc.
    pub conflicting_commit: String,
    /// Whether the source branch was pushed to origin.
    pub source_is_pushed: bool,
    /// Absolute path to the prio-mc clone (for display in `prio status`).
    pub mc_path: String,
    /// "dest"   = conflict while cherry-picking commits onto the destination branch.
    /// "source" = conflict while rebasing the source branch to remove the moved commits.
    #[serde(default)]
    pub phase: String,
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
    /// Set when a `prio mv -f` source-branch rebase hits a cherry-pick conflict in prio-mc.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mv_rebase_conflict: Option<MvRebaseConflict>,
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
    /// Set when a `prio mv` hits a cherry-pick conflict in prio-mc.
    /// mc-post-commit uses this to resume the operation after the user resolves.
    #[serde(default)]
    pub mv_rebase_in_progress: bool,
    /// "dest" = conflict while cherry-picking onto destination.
    /// "source" = conflict while rebasing source branch.
    #[serde(default)]
    pub mv_rebase_phase: String,
    #[serde(default)]
    pub mv_rebase_source_branch: String,
    #[serde(default)]
    pub mv_rebase_dest_branch: String,
    /// For "dest" phase: remaining commits still to cherry-pick onto dest.
    #[serde(default)]
    pub mv_rebase_remaining_dest: Vec<String>,
    /// For "dest" phase: shas to exclude from source branch rebase (after dest is done).
    #[serde(default)]
    pub mv_rebase_source_shas_to_exclude: Vec<String>,
    /// For "source" phase: commits still to cherry-pick during source rebase.
    #[serde(default)]
    pub mv_rebase_remaining_commits: Vec<String>,
    #[serde(default)]
    pub mv_rebase_source_is_pushed: bool,
    /// Whether -f was given (determines force-push after source rebase).
    #[serde(default)]
    pub mv_rebase_force: bool,
    /// Source branches to force-push after apply-conflict resolution.
    /// Format: "branch:is_pushed", same encoding as mv_rebase_unassign_all_sources.
    #[serde(default)]
    pub mv_pending_force_push_sources: Vec<String>,

    // ── Cross-branch unassign continuation state ──────────────────────────
    //
    // When `prio mv <sha> .` unassigns commits from applied branches, all
    // conflict-prone work (source rebases, apply merge, unassign cherry-picks)
    // happens in prio-mc before the work clone is touched.  These fields
    // carry state across mc-post-commit continuations.
    /// The original SHAs being unassigned (to cherry-pick on top of apply merge in mc).
    #[serde(default)]
    pub mv_rebase_unassign_commits: Vec<String>,
    /// The apply merge HEAD (before unassign cherry-picks).  Set as
    /// `state.baseline_commit` after sync so the cherry-picked commits appear
    /// above the baseline in `prio status`.
    #[serde(default)]
    pub mv_rebase_unassign_baseline: String,
    /// ALL source branches for the cross-branch unassign (format: "branch:is_pushed").
    /// Used to sync source refs to the work clone once all mc work completes.
    #[serde(default)]
    pub mv_rebase_unassign_all_sources: Vec<String>,
    /// Source branches still to rebase in mc (format: "branch:is_pushed").
    /// First entry is the one currently conflicting; entries after are queued.
    #[serde(default)]
    pub mv_rebase_unassign_source_branches: Vec<String>,
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
