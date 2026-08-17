//! Stack / unstack branch dependencies.
//!
//! # `prio stack <branch> [dep1 dep2 ...]`
//! Records that `branch` depends on `dep1`, `dep2`, … and re-applies the work area so
//! merge order reflects the declared dependency.
//!
//! # `prio unstack <branch> [-k | -f]`
//! Removes the stack relationship.  By default the branch is **rebased** in prio-mc
//! so it sits directly on `<default_branch>` (dependency commits stripped out).
//!
//! | Branch state | Default | `-k` | `-f` |
//! |---|---|---|---|
//! | Local-only (never pushed) | Rebase in prio-mc, update WORK local ref, re-apply | Metadata-only | N/A |
//! | Pushed (`origin/<branch>` exists) | **Error** — choose `-k` or `-f` | Metadata-only (safe) | Rebase + force-push |
//!
//! The rebase cherry-picks each commit that is unique to the branch
//! (not reachable from the default branch or from any dependency tip)
//! onto a fresh branch rooted at `<default_branch>` inside prio-mc.

use std::path::{Path, PathBuf};

use crate::error::PrioError;
use crate::git::runner;
use crate::logger::Logger;
use crate::result::{PrioResult, PrioStatus};
use crate::services::apply;
use crate::services::common::{
    assert_on_work_branch, branch_tip_ref, mc_path_for_repo, resolve_repo_path,
};
use crate::services::suggestions;
use crate::storage::repo_state;

/// Record that `branch` is stacked after `dependencies` (in order), then re-apply.
pub fn run_stack(
    repo_path: Option<PathBuf>,
    branch: String,
    dependencies: Vec<String>,
    logger: &mut Logger,
) -> Result<PrioResult, PrioError> {
    let repo_path = resolve_repo_path(repo_path)?;
    assert_on_work_branch(&repo_path, logger)?;

    let branch = runner::resolve_branch_ref(&branch, &repo_path, logger)?;
    let deps: Vec<String> = dependencies
        .iter()
        .map(|d| runner::resolve_branch_ref(d, &repo_path, logger))
        .collect::<Result<_, _>>()?;

    let mut state = repo_state::load_state(&repo_path)?;
    state.stacks.retain(|s| s.branch != branch);
    state.stacks.push(repo_state::StackEntry {
        branch: branch.clone(),
        dependencies: deps.clone(),
    });
    repo_state::save_state(&repo_path, &state)?;

    let mc_path = mc_path_for_repo(&repo_path, None)?;
    let apply_result = apply_and_suggest(&repo_path, &mc_path, logger)?;

    // If apply hit a conflict or failed, surface that result directly — do not
    // override it with a misleading "Stacked … SUCCESS" message.
    if apply_result.status != PrioStatus::Success {
        return Ok(apply_result);
    }

    let logs = logger.drain();
    let dep_desc = if deps.is_empty() {
        "(no dependencies)".to_string()
    } else {
        deps.join(", ")
    };
    Ok(PrioResult::success(
        format!("Stacked {branch} after {dep_desc}."),
        logs,
    ))
}

/// Remove stack metadata for `branch`, optionally rebasing it off its dependencies.
///
/// - `keep`  (`-k`): metadata-only; safe for pushed branches (no git changes).
/// - `force` (`-f`): rebase then force-push; rewrites the remote branch history.
/// - neither: rebase (local-only branches) or error (pushed branches without `-k`/`-f`).
pub fn run_unstack(
    repo_path: Option<PathBuf>,
    branch: String,
    keep: bool,
    force: bool,
    logger: &mut Logger,
) -> Result<PrioResult, PrioError> {
    if keep && force {
        return Err(PrioError::Message(
            "-k (keep) and -f (force) are mutually exclusive.".into(),
        ));
    }

    let repo_path = resolve_repo_path(repo_path)?;
    assert_on_work_branch(&repo_path, logger)?;
    let mc_path = mc_path_for_repo(&repo_path, None)?;

    let branch = runner::resolve_branch_ref(&branch, &repo_path, logger)?;
    let mut state = repo_state::load_state(&repo_path)?;

    // Capture the dependencies before removing the stack entry.
    let deps: Vec<String> = state
        .stacks
        .iter()
        .find(|s| s.branch == branch)
        .map(|e| e.dependencies.clone())
        .unwrap_or_default();

    let default_branch = state.default_branch.clone();

    // Check whether the branch has already been pushed.
    let is_pushed = runner::run_git(
        &["rev-parse", "--verify", &format!("origin/{branch}")],
        &repo_path,
        logger,
    )
    .is_ok();

    // Guard: pushed branches require an explicit flag so the user acknowledges
    // that remote history will either be kept as-is (-k) or rewritten (-f).
    if is_pushed && !keep && !force {
        let logs = logger.drain();
        return Ok(PrioResult::failure(
            format!(
                "Branch '{branch}' has been pushed to origin. \
                 Unstacking requires either:\n  \
                 -k  keep all upstream commits (metadata-only unstack)\n  \
                 -f  force-push after rebase (rewrites remote history)"
            ),
            logs,
        ));
    }

    // Perform the rebase unless the user asked to keep upstream commits.
    let rebased = if !keep && !deps.is_empty() {
        rebase_off_deps(
            &repo_path,
            &mc_path,
            &branch,
            &deps,
            &default_branch,
            logger,
        )?
    } else {
        false
    };

    // Remove the stack entry.
    state.stacks.retain(|s| s.branch != branch);
    repo_state::save_state(&repo_path, &state)?;

    if rebased {
        // Bring the rebased branch from prio-mc into the WORK clone's local ref.
        let mc_str = crate::util::path_arg(&mc_path);
        runner::run_git(
            &["fetch", &mc_str, &format!("+{branch}:{branch}")],
            &repo_path,
            logger,
        )?;

        if force && is_pushed {
            runner::run_git(&["push", "--force", "origin", &branch], &repo_path, logger)?;
        }

        // Re-apply so the work area reflects the rebased branch.
        let apply_result = apply_and_suggest(&repo_path, &mc_path, logger)?;
        if apply_result.status != PrioStatus::Success {
            return Ok(apply_result);
        }
    } else {
        suggestions::run_and_log(&repo_path, &mc_path, logger)?;
    }

    let logs = logger.drain();
    let msg = if rebased {
        format!("Unstacked {branch} and rebased onto {default_branch}.")
    } else {
        format!("Unstacked {branch}.")
    };
    Ok(PrioResult::success(msg, logs))
}

/// Run `apply::run` (empty branch list = re-apply current applied set) then, on
/// success only, emit suggestions.
///
/// This helper lives here rather than in `apply.rs` because `suggestions.rs`
/// imports `apply::score_merge_order`, which would create a circular dependency
/// if apply imported suggestions in turn.
fn apply_and_suggest(
    repo_path: &std::path::Path,
    mc_path: &std::path::Path,
    logger: &mut Logger,
) -> Result<PrioResult, PrioError> {
    let result = apply::run(Some(repo_path.to_path_buf()), vec![], false, logger)?;
    if result.status == PrioStatus::Success {
        suggestions::run_and_log(repo_path, mc_path, logger)?;
    }
    Ok(result)
}

/// Cherry-pick the commits unique to `branch` (not reachable from `default_branch` or
/// any dependency tip) onto a fresh branch rooted at `default_branch` inside prio-mc.
///
/// Returns `true` when the rebase was performed, `false` when there was nothing to do
/// (e.g. the branch had no unique commits, or its tip ref could not be resolved).
fn rebase_off_deps(
    repo_path: &Path,
    mc_path: &Path,
    branch: &str,
    deps: &[String],
    default_branch: &str,
    logger: &mut Logger,
) -> Result<bool, PrioError> {
    // Resolve the branch tip in the WORK clone.
    let tip_ref = match branch_tip_ref(branch, repo_path, logger) {
        Ok(t) => t,
        Err(_) => return Ok(false),
    };

    // Resolve each dependency tip (skip deps we can't find — safe to be inclusive).
    let dep_tips: Vec<String> = deps
        .iter()
        .filter_map(|d| branch_tip_ref(d, repo_path, logger).ok())
        .collect();

    // Find unique commits: reachable from branch tip but not from default_branch or
    // any dep tip.  --reverse gives them in topological (oldest-first) order so we
    // can cherry-pick them in sequence.
    // The trailing `--` prevents git from treating branch names as filesystem paths.
    let mut log_args: Vec<String> = vec![
        "log".into(),
        tip_ref.clone(),
        "--not".into(),
        default_branch.to_string(),
    ];
    log_args.extend(dep_tips);
    log_args.extend([
        "--format=%H".to_string(),
        "--reverse".to_string(),
        "--".to_string(),
    ]);
    let log_refs: Vec<&str> = log_args.iter().map(|s| s.as_str()).collect();

    let unique_out = runner::run_git(&log_refs, repo_path, logger)?;
    let unique_shas: Vec<&str> = unique_out.lines().filter(|l| !l.is_empty()).collect();

    if unique_shas.is_empty() {
        // Nothing unique on the branch — nothing to rebase.
        return Ok(false);
    }

    // Reset prio-mc to a clean state, then build a fresh branch from default_branch.
    apply::reset_mc_to_default(mc_path, repo_path, default_branch, logger)?;

    // Delete any existing local branch by this name so we can re-create it.
    let _ = runner::run_git(&["branch", "-D", branch], mc_path, logger);
    runner::run_git(&["checkout", "-b", branch, default_branch], mc_path, logger)?;

    // Cherry-pick each unique commit in order.
    for sha in &unique_shas {
        let result = runner::run_git_no_hooks(
            &["cherry-pick", "--allow-empty", "--allow-empty-message", sha],
            mc_path,
            logger,
        );
        if result.is_err() {
            let _ = runner::run_git(&["cherry-pick", "--abort"], mc_path, logger);
            let _ = apply::reset_mc_to_default(mc_path, repo_path, default_branch, logger);
            return Err(PrioError::Message(format!(
                "Cherry-pick conflict while rebasing '{branch}': commit {sha} conflicts. \
                 Resolve manually in prio-mc at {}.",
                mc_path.display()
            )));
        }
    }

    Ok(true)
}
