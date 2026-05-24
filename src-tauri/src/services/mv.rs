//! Move (assign) commits to branches via prio-mc.
//!
//! # Cherry-pick vs. merge-up policy
//!
//! | Destination branch state | Strategy |
//! | ------------------------ | -------- |
//! | Not yet pushed (local only) | Cherry-pick in prio-mc — safe to rewrite local history |
//! | Pushed (`origin/<branch>` exists) or in a PR | **Metadata-only** — update `commit_assignments`, no cherry-pick. History is shared and must not be rewritten. The commit reaches the branch via the normal merge-up when the branch is applied. |
//! | Unassign (`.`) | Cherry-pick is always fine; commit is floating in the work area |

use std::path::PathBuf;

use crate::error::PrioError;
use crate::git::runner;
use crate::logger::Logger;
use crate::result::PrioResult;
use crate::services::apply::{merge_branches_in_mc, reset_mc_to_default, sync_work_clone, MergeOutcome};
use crate::services::common::{
    assert_on_work_branch, expand_commit_args, is_work_area_commit, mc_path_for_repo, resolve_repo_path,
};
use crate::storage::repo_state;

pub fn run(
    repo_path: Option<PathBuf>,
    commits: Vec<String>,
    destination: String,
    create: bool,
    apply: bool,
    logger: &mut Logger,
) -> Result<PrioResult, PrioError> {
    let repo_path = resolve_repo_path(repo_path)?;
    assert_on_work_branch(&repo_path, logger)?;

    let config = repo_state::load_config(&repo_path)?;
    let mut state = repo_state::load_state(&repo_path)?;
    let mc_path = mc_path_for_repo(&repo_path, None)?;

    let commit_shas = expand_commit_args(&commits, &repo_path, logger)?;
    for sha in &commit_shas {
        if !is_work_area_commit(sha, &state.baseline_commit, &repo_path, logger)? {
            return Err(PrioError::Message(format!(
                "Commit {sha} is not on the work branch past baseline and cannot be moved."
            )));
        }
    }

    let dest = if destination == "." {
        config.work_branch.clone()
    } else {
        runner::resolve_branch_ref(&destination, &repo_path, logger)?
    };

    let assignment = if destination == "." {
        ".".to_string()
    } else {
        dest.clone()
    };

    let branch_exists = !runner::run_git(&["branch", "--list", &dest], &repo_path, logger)?
        .trim()
        .is_empty();
    let branch_created = create && !branch_exists;

    if !branch_exists {
        if create {
            runner::run_git(&["branch", &dest], &repo_path, logger)?;
        } else {
            return Err(PrioError::Message(format!(
                "Branch {dest} does not exist. Use -c to create it."
            )));
        }
    }

    if destination.starts_with("pr-") && create {
        return Err(PrioError::Message("-c is not allowed for pr-{num} destinations.".into()));
    }

    if assignment != "." {
        let should_apply = apply || branch_created;
        if should_apply && !state.applied_branches.contains(&dest) {
            state.applied_branches.push(dest.clone());
        }
    }

    // ── Pushed-branch guard ───────────────────────────────────────────────────
    // A branch is "pushed" once origin/<branch> exists.  Rebasing or cherry-
    // picking shared history breaks collaborators; use merge-up instead.
    // For unassign (".") and local-only branches, cherry-pick is safe.
    let is_pushed = destination != "."
        && runner::run_git(
            &["rev-parse", "--verify", &format!("origin/{dest}")],
            &repo_path,
            logger,
        )
        .is_ok();

    if is_pushed {
        // Metadata-only: record the assignment but don't touch prio-mc history.
        for sha in &commit_shas {
            state.commit_assignments.insert(sha.clone(), assignment.clone());
        }

        // If the branch was just added to applied_branches, run a full apply so
        // the work clone picks up the merged result.
        let newly_applied = (apply || branch_created) && {
            // Check against the pre-mutation state: if we pushed to applied above,
            // the branch is now in state.applied_branches.
            state.applied_branches.contains(&dest)
                && !repo_state::load_state(&repo_path)
                    .map(|s| s.applied_branches.contains(&dest))
                    .unwrap_or(false)
        };
        repo_state::save_state(&repo_path, &state)?;

        if newly_applied {
            return crate::services::apply::run(
                Some(repo_path),
                state.applied_branches.clone(),
                false,
                logger,
            );
        }

        let logs = logger.drain();
        return Ok(PrioResult::success(
            format!(
                "Tagged {} commit(s) as {assignment} \
                 (branch is pushed — merge-up, not cherry-pick).",
                commit_shas.len()
            ),
            logs,
        ));
    }

    // ── Local branch or unassign: cherry-pick in prio-mc ─────────────────────
    reset_mc_to_default(&mc_path, &state.default_branch, logger)?;
    runner::run_git(&["checkout", &dest], &mc_path, logger)?;

    for sha in &commit_shas {
        let pick = runner::run_git_no_hooks(&["cherry-pick", sha], &mc_path, logger);
        if pick.is_err() {
            state.merge_in_progress = true;
            repo_state::save_state(&repo_path, &state)?;
            let logs = logger.drain();
            return Ok(PrioResult::warning(
                format!(
                    "Cherry-pick conflict in prio-mc at {}. Resolve, commit, then continue.",
                    mc_path.display()
                ),
                logs,
            ));
        }
        let new_sha = runner::run_git(&["rev-parse", "HEAD"], &mc_path, logger)?
            .trim()
            .to_string();
        state.commit_map.insert(sha.clone(), new_sha.clone());
        state.commit_assignments.remove(sha);
        state.commit_assignments.insert(new_sha, assignment.clone());
    }

    match merge_branches_in_mc(
        &mc_path,
        &state.applied_branches,
        &state.default_branch,
        &repo_path,
        logger,
    )? {
        MergeOutcome::Complete(head) => {
            sync_work_clone(&repo_path, &mc_path, &config.work_branch, &head, logger)?;
            state.baseline_commit = head;
            state.merge_in_progress = false;
            repo_state::save_state(&repo_path, &state)?;

            crate::services::suggestions::run_and_log(&repo_path, &mc_path, logger)?;

            let logs = logger.drain();
            let label = if assignment == "." {
                "unassigned".to_string()
            } else {
                assignment
            };
            Ok(PrioResult::success(
                format!("Moved {} commit(s) to {label}.", commit_shas.len()),
                logs,
            ))
        }
        MergeOutcome::Conflict { .. } => {
            state.merge_in_progress = true;
            repo_state::save_state(&repo_path, &state)?;
            let logs = logger.drain();
            Ok(PrioResult::warning(
                format!(
                    "Merge conflicts in prio-mc at {}. Resolve, commit, then continue.",
                    mc_path.display()
                ),
                logs,
            ))
        }
    }
}
