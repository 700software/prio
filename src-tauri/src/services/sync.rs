use std::path::PathBuf;

use crate::error::PrioError;
use crate::git::runner;
use crate::logger::Logger;
use crate::result::PrioResult;
use crate::services::apply;
use crate::services::common::{assert_on_work_branch, resolve_repo_path};
use crate::storage::{repo_state, user_config};

pub fn run(repo_path: Option<PathBuf>, logger: &mut Logger) -> Result<PrioResult, PrioError> {
    let repo_path = resolve_repo_path(repo_path)?;
    assert_on_work_branch(&repo_path, logger)?;

    let mut state = repo_state::load_state(&repo_path)?;
    let old_baseline = state.baseline_commit.clone();

    // Fetch FIRST so all merge checks below use up-to-date remote refs.
    runner::run_git(&["fetch", "origin"], &repo_path, logger)?;

    // Advance baseline to the latest origin/<default>.
    if let Ok(sha) = runner::run_git(
        &["rev-parse", &format!("origin/{}", state.default_branch)],
        &repo_path,
        logger,
    ) {
        state.baseline_commit = sha.trim().to_string();
    }

    // Remove any applied branch whose commits are all reachable from origin/<default>.
    let mut removed = Vec::new();
    for branch in state.applied_branches.clone() {
        if branch_fully_merged(&branch, &state.default_branch, &repo_path, logger)? {
            removed.push(branch.clone());
            state.applied_branches.retain(|b| b != &branch);
            let _ = runner::run_git(&["branch", "-d", &branch], &repo_path, logger);
        }
    }

    repo_state::save_state(&repo_path, &state)?;

    // Re-apply whenever the baseline advanced or applied branches changed so
    // the work clone reflects the current state.
    let baseline_changed = state.baseline_commit != old_baseline;
    if baseline_changed || !removed.is_empty() {
        apply::run(
            Some(repo_path.clone()),
            state.applied_branches.clone(),
            false,
            logger,
        )?;
    }

    let logs = logger.drain();
    let msg = if removed.is_empty() {
        "Sync complete.".to_string()
    } else {
        format!(
            "Sync complete. Removed from applied (merged into {}): {}",
            state.default_branch,
            removed.join(", ")
        )
    };
    Ok(PrioResult::success(msg, logs))
}

pub fn run_syncs(logger: &mut Logger) -> Result<PrioResult, PrioError> {
    let repos = user_config::load_repos()?;
    if repos.is_empty() {
        let logs = logger.drain();
        return Ok(PrioResult::warning(
            "No repositories configured. Run prio setup first.",
            logs,
        ));
    }

    let mut messages = Vec::new();
    for repo in repos {
        logger.info(format!("Syncing {}", repo.path));
        match run(Some(PathBuf::from(&repo.path)), logger) {
            Ok(r) => messages.push(r.message),
            Err(e) => messages.push(e.to_string()),
        }
    }

    let logs = logger.drain();
    Ok(PrioResult::success(messages.join("\n"), logs))
}

/// Returns `true` when every commit on `branch` is already reachable from
/// `origin/<default_branch>`, meaning the branch has been merged and can safely
/// be removed from `applied_branches`.
///
/// Three checks are tried in order:
/// 1. Local branch tip is an ancestor of `origin/<default>`.
/// 2. Remote tracking branch tip (`origin/<branch>`) is an ancestor.
/// 3. GitHub reports the PR as merged (catches squash / rebase merges where the
///    original commits were rewritten and are NOT direct ancestors).
fn branch_fully_merged(
    branch: &str,
    default_branch: &str,
    repo_path: &std::path::Path,
    logger: &mut Logger,
) -> Result<bool, PrioError> {
    let default_ref = format!("origin/{default_branch}");

    // Guard: if the branch has no unique commits above the default branch its local ref is
    // just a baseline marker (common for local-only branches created by `prio mv -c`).
    // Such branches are NOT merged — they were never pushed.
    let has_own_commits = runner::run_git(
        &[
            "log",
            "--oneline",
            &format!("{default_ref}..{branch}"),
            "--",
        ],
        repo_path,
        logger,
    )
    .map(|out| !out.trim().is_empty())
    .unwrap_or(false);

    // Check 1: local branch — only valid when the branch actually has unique commits
    if has_own_commits
        && runner::run_git(
            &["merge-base", "--is-ancestor", branch, &default_ref],
            repo_path,
            logger,
        )
        .is_ok()
    {
        return Ok(true);
    }

    // Check 2: remote tracking branch (handles local deletion before sync)
    let remote_ref = format!("origin/{branch}");
    if runner::run_git(&["rev-parse", "--verify", &remote_ref], repo_path, logger).is_ok()
        && runner::run_git(
            &["merge-base", "--is-ancestor", &remote_ref, &default_ref],
            repo_path,
            logger,
        )
        .is_ok()
    {
        return Ok(true);
    }

    // Check 3: GitHub PR state — handles squash / rebase merges.
    // gh pr list defaults to --state open; we need --state merged explicitly.
    if let Ok(json) = runner::run_gh(
        &[
            "pr", "list", "--head", branch, "--state", "merged", "--json", "number", "--limit", "1",
        ],
        repo_path,
        logger,
    ) {
        let v: serde_json::Value = serde_json::from_str(&json).unwrap_or_default();
        if v.as_array().map(|a| !a.is_empty()).unwrap_or(false) {
            return Ok(true);
        }
    }

    Ok(false)
}
