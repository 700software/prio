use std::path::{Path, PathBuf};

use crate::error::PrioError;
use crate::git::runner;
use crate::logger::Logger;
use crate::storage::{repo_state, user_config};
use crate::util;

pub fn resolve_repo_path(repo_path: Option<PathBuf>) -> Result<PathBuf, PrioError> {
    let path = repo_path
        .unwrap_or_else(|| std::env::current_dir().expect("current directory should exist"));
    let abs = util::absolute_path(path);
    runner::ensure_git_work_tree(&abs, &mut Logger::Cli)?;
    Ok(abs)
}

pub fn mc_path_for_repo(
    repo_path: &Path,
    mc_override: Option<PathBuf>,
) -> Result<PathBuf, PrioError> {
    if let Some(p) = mc_override {
        return Ok(p);
    }
    if let Some(record) = user_config::find_repo_by_path(repo_path)? {
        return Ok(PathBuf::from(record.mc_clone_path));
    }
    let name = repo_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("repo");
    Ok(util::absolute_path(
        repo_path
            .parent()
            .unwrap_or(repo_path)
            .join(format!("{name}-prio-mc")),
    ))
}

pub fn assert_on_work_branch(repo_path: &Path, logger: &mut Logger) -> Result<(), PrioError> {
    let config = repo_state::load_config(repo_path)?;
    let current = runner::current_branch(repo_path, logger)?;
    if current != config.work_branch {
        return Err(PrioError::Inactive {
            work_branch: config.work_branch,
            current_branch: current,
        });
    }
    Ok(())
}

pub fn default_mc_path(repo_path: &Path) -> PathBuf {
    let name = repo_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("repo");
    repo_path
        .parent()
        .unwrap_or(repo_path)
        .join(format!("{name}-prio-mc"))
}

pub fn expand_commit_args(
    commits: &[String],
    repo_path: &Path,
    logger: &mut Logger,
) -> Result<Vec<String>, PrioError> {
    let mut out = Vec::new();
    for c in commits {
        if c.contains("..") {
            let shas = runner::run_git(&["rev-list", "--reverse", c], repo_path, logger)?;
            for sha in shas.lines() {
                if !sha.is_empty() {
                    out.push(sha.to_string());
                }
            }
        } else {
            out.push(c.clone());
        }
    }
    Ok(out)
}

/// True when `sha` is on the work branch after the current baseline.
pub fn is_work_area_commit(
    sha: &str,
    baseline: &str,
    repo_path: &Path,
    logger: &mut Logger,
) -> Result<bool, PrioError> {
    let out = runner::run_git(
        &["merge-base", "--is-ancestor", baseline, sha],
        repo_path,
        logger,
    );
    Ok(out.is_ok())
}

/// Resolve the git ref whose tip best represents branch history in the work repo.
///
/// When `prio mv` pushes from prio-mc, it updates `refs/heads/<branch>` in the work
/// repo but not `refs/remotes/origin/<branch>` (which tracks github.com). Prefer the
/// ref that contains the other when both exist.
pub fn branch_tip_ref(
    branch: &str,
    repo_path: &Path,
    logger: &mut Logger,
) -> Result<String, PrioError> {
    let local = branch.to_string();
    let origin = format!("origin/{branch}");
    let has_local = runner::run_git(&["rev-parse", "--verify", &local], repo_path, logger).is_ok();
    let has_origin =
        runner::run_git(&["rev-parse", "--verify", &origin], repo_path, logger).is_ok();

    match (has_local, has_origin) {
        (true, true) => {
            let origin_ancestor_of_local = runner::run_git(
                &["merge-base", "--is-ancestor", &origin, &local],
                repo_path,
                logger,
            )
            .is_ok();
            if origin_ancestor_of_local {
                Ok(local)
            } else {
                let local_ancestor_of_origin = runner::run_git(
                    &["merge-base", "--is-ancestor", &local, &origin],
                    repo_path,
                    logger,
                )
                .is_ok();
                if local_ancestor_of_origin {
                    Ok(origin)
                } else {
                    // Diverged — local reflects prio-mc pushes into the work repo.
                    Ok(local)
                }
            }
        }
        (true, false) => Ok(local),
        (false, true) => Ok(origin),
        (false, false) => Err(PrioError::Message(format!(
            "Branch '{branch}' not found locally or at origin/{branch}"
        ))),
    }
}

/// Find which applied branch a commit belongs to.
///
/// Returns the first applied branch whose tip is a descendant of `sha` (i.e. `sha` is in
/// the branch's history).  Returns `None` when the commit is not on any applied branch.
pub fn find_commit_source_branch(
    sha: &str,
    applied_branches: &[String],
    repo_path: &Path,
    logger: &mut Logger,
) -> Option<String> {
    for branch in applied_branches {
        if let Ok(tip) = branch_tip_ref(branch, repo_path, logger) {
            if runner::run_git(
                &["merge-base", "--is-ancestor", sha, &tip],
                repo_path,
                logger,
            )
            .is_ok()
            {
                return Some(branch.clone());
            }
        }
    }
    None
}

/// Assignment for a work-area commit, following [`RepoState::commit_map`] after cherry-pick.
pub fn assignment_for_work_sha<'a>(sha: &str, state: &'a repo_state::RepoState) -> &'a str {
    if let Some(branch) = state.commit_assignments.get(sha) {
        return branch.as_str();
    }
    if let Some(mapped) = state.commit_map.get(sha) {
        if let Some(branch) = state.commit_assignments.get(mapped) {
            return branch.as_str();
        }
    }
    "."
}

/// SHAs on the work branch above baseline that are still unassigned.
pub fn unassigned_work_shas(
    repo_path: &Path,
    state: &repo_state::RepoState,
    logger: &mut Logger,
) -> Result<Vec<String>, PrioError> {
    let log = runner::run_git(
        &[
            "log",
            &format!("{}..HEAD", state.baseline_commit),
            "--format=%H",
        ],
        repo_path,
        logger,
    )?;
    Ok(log
        .lines()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .filter(|sha| assignment_for_work_sha(sha, state) == ".")
        .map(String::from)
        .collect())
}

/// Recreate `branch` in prio-mc from the authoritative tip in the work repo.
///
/// Deletes any existing `branch` in prio-mc first.  A previous failed `prio mv` / `prio cp`
/// may have left a partial cherry-pick on that branch even when the work clone has no
/// local ref for it.
pub fn prepare_mc_dest_branch(
    mc_path: &Path,
    repo_path: &Path,
    branch: &str,
    default_branch: &str,
    logger: &mut Logger,
) -> Result<(), PrioError> {
    let _ = runner::run_git(&["branch", "-D", branch], mc_path, logger);
    let base_ref =
        branch_tip_ref(branch, repo_path, logger).unwrap_or_else(|_| default_branch.to_string());
    // Resolve to a SHA in the work repo — `base_ref` may be a branch name (e.g.
    // `bryan-prettier`) that does not exist in prio-mc after we deleted it above.
    let base_sha = runner::run_git(&["rev-parse", &base_ref], repo_path, logger)?
        .trim()
        .to_string();
    runner::run_git(&["checkout", "-b", branch, &base_sha], mc_path, logger)?;
    Ok(())
}

/// Result of attempting a cherry-pick in prio-mc.
pub enum CherryPickOnMc {
    /// Cherry-pick created a new commit (or reused changes).
    Applied,
    /// Cherry-pick was empty (commit already present); skipped with `cherry-pick --skip`.
    SkippedEmpty,
    /// Real content conflict — cherry-pick left in progress for the user.
    Conflict,
    /// Cherry-pick refused before starting (not a conflict).
    Failed,
}

/// Cherry-pick `sha` onto the current branch in prio-mc.
///
/// Does **not** abort on conflict; leaves `CHERRY_PICK_HEAD` in place when the worktree
/// has unresolved paths.  Empty duplicate cherry-picks (clean worktree, nothing to commit)
/// are auto-skipped.
pub fn cherry_pick_on_mc(
    mc_path: &Path,
    sha: &str,
    logger: &mut Logger,
) -> Result<CherryPickOnMc, PrioError> {
    let pick = runner::run_git_no_hooks(
        &["cherry-pick", "--allow-empty", "--allow-empty-message", sha],
        mc_path,
        logger,
    );
    if pick.is_ok() {
        return Ok(CherryPickOnMc::Applied);
    }

    let head_active = runner::run_git(
        &["rev-parse", "--verify", "CHERRY_PICK_HEAD"],
        mc_path,
        logger,
    )
    .is_ok();

    if head_active {
        if runner::working_tree_clean(mc_path, logger)? {
            let _ = runner::run_git(&["cherry-pick", "--skip"], mc_path, logger);
            return Ok(CherryPickOnMc::SkippedEmpty);
        }
        return Ok(CherryPickOnMc::Conflict);
    }

    Ok(CherryPickOnMc::Failed)
}

/// Delete branches that exist in prio-mc but not as local branches in the work clone.
///
/// Returns the names of branches that were removed from prio-mc.
pub fn cleanup_orphan_mc_branches(
    mc_path: &Path,
    repo_path: &Path,
    default_branch: &str,
    logger: &mut Logger,
) -> Result<Vec<String>, PrioError> {
    let work_branches: std::collections::HashSet<String> = runner::run_git(
        &["branch", "--list", "--format=%(refname:short)"],
        repo_path,
        logger,
    )?
    .lines()
    .map(str::trim)
    .filter(|s| !s.is_empty())
    .map(String::from)
    .collect();

    let mc_out = runner::run_git(
        &["branch", "--list", "--format=%(refname:short)"],
        mc_path,
        logger,
    )?;
    let mut removed = Vec::new();
    for b in mc_out.lines().map(str::trim).filter(|s| !s.is_empty()) {
        if b == default_branch || b.starts_with("prio-mc/") {
            continue;
        }
        if !work_branches.contains(b) {
            let _ = runner::run_git(&["branch", "-D", b], mc_path, logger);
            removed.push(b.to_string());
        }
    }
    Ok(removed)
}

pub fn assign_work_commits_to_branch(
    repo_path: &Path,
    branch: &str,
    logger: &mut Logger,
) -> Result<(), PrioError> {
    let mut state = repo_state::load_state(repo_path)?;
    let log = runner::run_git(
        &[
            "log",
            &format!("{}..HEAD", state.baseline_commit),
            "--format=%H",
        ],
        repo_path,
        logger,
    )?;
    for sha in log.lines().map(str::trim).filter(|s| !s.is_empty()) {
        state
            .commit_assignments
            .insert(sha.to_string(), branch.to_string());
    }
    repo_state::save_state(repo_path, &state)?;
    Ok(())
}
