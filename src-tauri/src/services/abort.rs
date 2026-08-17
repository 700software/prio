use std::path::PathBuf;

use crate::error::PrioError;
use crate::git::runner;
use crate::logger::Logger;
use crate::result::PrioResult;
use crate::services::apply::reset_mc_to_default;
use crate::services::common::{cleanup_orphan_mc_branches, mc_path_for_repo, resolve_repo_path};
use crate::storage::repo_state;

/// Abort an in-progress `prio mv` cherry-pick or rebase conflict.
///
/// - Aborts any active cherry-pick in prio-mc.
/// - Resets prio-mc to default.
/// - Deletes the destination branch from prio-mc (it will have been left in a
///   partially-applied or conflicted state).
/// - If the destination branch is not in `applied_branches` (i.e. it was newly
///   created by this mv and never properly applied), removes it from the WORK
///   local branch list too.
/// - Clears `mv_rebase_conflict` from `state.json` and `mv_rebase_in_progress`
///   from `mc_state.json`.
pub fn run(repo_path: Option<PathBuf>, logger: &mut Logger) -> Result<PrioResult, PrioError> {
    let repo_path = resolve_repo_path(repo_path)?;
    let mc_path = mc_path_for_repo(&repo_path, None)?;

    let mut state = repo_state::load_state(&repo_path)?;

    let conflict = match state.mv_rebase_conflict.take() {
        Some(c) => c,
        None => {
            let orphans =
                cleanup_orphan_mc_branches(&mc_path, &repo_path, &state.default_branch, logger)?;
            if !orphans.is_empty() {
                let _ = runner::run_git(&["cherry-pick", "--abort"], &mc_path, logger);
                let _ = reset_mc_to_default(&mc_path, &repo_path, &state.default_branch, logger);
                let logs = logger.drain();
                return Ok(PrioResult::success(
                    format!(
                        "Removed stale prio-mc branch(es) not present in work clone: {}.",
                        orphans.join(", ")
                    ),
                    logs,
                ));
            }

            let cherry_pick_active = runner::run_git(
                &["rev-parse", "--verify", "CHERRY_PICK_HEAD"],
                &mc_path,
                logger,
            )
            .is_ok();
            if cherry_pick_active {
                let _ = runner::run_git(&["cherry-pick", "--abort"], &mc_path, logger);
                let _ = reset_mc_to_default(&mc_path, &repo_path, &state.default_branch, logger);
                let logs = logger.drain();
                return Ok(PrioResult::success(
                    "Aborted stray cherry-pick in prio-mc (no recorded prio operation in progress).",
                    logs,
                ));
            }

            let logs = logger.drain();
            return Ok(PrioResult::success("Nothing to abort.", logs));
        }
    };

    let dest_branch = conflict.dest_branch.clone();
    let default_branch = state.default_branch.clone();

    // Abort any cherry-pick and reset prio-mc.
    let _ = runner::run_git(&["cherry-pick", "--abort"], &mc_path, logger);
    reset_mc_to_default(&mc_path, &repo_path, &default_branch, logger)?;

    // Delete the destination branch from prio-mc (it may have partial cherry-picks).
    let _ = runner::run_git(&["branch", "-D", &dest_branch], &mc_path, logger);

    // If the dest branch is not currently applied, remove it from the WORK local refs too.
    // (It was likely created by the -c flag of the failed mv.)
    if !state.applied_branches.contains(&dest_branch) {
        let _ = runner::run_git(&["branch", "-D", &dest_branch], &repo_path, logger);
    }

    // Clear mv_rebase_in_progress in mc_state.
    if let Ok(mut mc_state) = repo_state::load_mc_state(&mc_path) {
        if mc_state.mv_rebase_in_progress {
            mc_state.mv_rebase_in_progress = false;
            mc_state.mv_rebase_phase.clear();
            mc_state.mv_rebase_source_branch.clear();
            mc_state.mv_rebase_dest_branch.clear();
            mc_state.mv_rebase_remaining_dest.clear();
            mc_state.mv_rebase_source_shas_to_exclude.clear();
            mc_state.mv_rebase_remaining_commits.clear();
            let _ = repo_state::save_mc_state(&mc_path, &mc_state);
        }
    }

    repo_state::save_state(&repo_path, &state)?;

    let logs = logger.drain();
    Ok(PrioResult::success(
        format!(
            "Aborted prio mv conflict. \
             Destination branch '{dest_branch}' cleaned up in prio-mc{}.",
            if !conflict.dest_branch.is_empty() && !state.applied_branches.contains(&dest_branch) {
                " and removed from WORK"
            } else {
                ""
            }
        ),
        logs,
    ))
}
