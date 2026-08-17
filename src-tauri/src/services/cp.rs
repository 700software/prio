//! Copy commits to a branch non-destructively.
//!
//! `prio cp <sha>... <destination> [-c] [-a]`
//!
//! Cherry-picks the given commits onto the destination branch in prio-mc.  The source
//! branch is **never modified** — this is the key difference from `prio mv`.

use std::path::PathBuf;

use crate::error::PrioError;
use crate::git::runner;
use crate::logger::Logger;
use crate::result::PrioResult;
use crate::services::apply::{execute_apply_merge, reset_mc_to_default};
use crate::services::common::{
    assert_on_work_branch, branch_tip_ref, cherry_pick_on_mc, expand_commit_args, mc_path_for_repo,
    prepare_mc_dest_branch, resolve_repo_path, CherryPickOnMc,
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

    let commit_shas: Vec<String> = expand_commit_args(&commits, &repo_path, logger)?
        .into_iter()
        .map(|sha| {
            runner::run_git(&["rev-parse", &sha], &repo_path, logger)
                .map(|s| s.trim().to_string())
                .unwrap_or(sha)
        })
        .collect();

    if commit_shas.is_empty() {
        return Err(PrioError::Message("No commits specified.".into()));
    }

    let dest = runner::resolve_branch_ref(&destination, &repo_path, logger)?;
    let branch_exists = !runner::run_git(&["branch", "--list", &dest], &repo_path, logger)?
        .trim()
        .is_empty();
    let branch_created = create && !branch_exists;

    if !branch_exists {
        if create {
            runner::run_git(
                &["branch", &dest, &state.default_branch],
                &repo_path,
                logger,
            )?;
        } else {
            return Err(PrioError::Message(format!(
                "Branch {dest} does not exist. Use -c to create it."
            )));
        }
    }

    if apply || branch_created {
        if !state.applied_branches.contains(&dest) {
            state.applied_branches.push(dest.clone());
        }
    }

    let dest_is_pushed = runner::run_git(
        &["rev-parse", "--verify", &format!("origin/{dest}")],
        &repo_path,
        logger,
    )
    .is_ok();

    reset_mc_to_default(&mc_path, &repo_path, &state.default_branch, logger)?;
    prepare_mc_dest_branch(&mc_path, &repo_path, &dest, &state.default_branch, logger)?;

    for (i, sha) in commit_shas.iter().enumerate() {
        match cherry_pick_on_mc(&mc_path, sha, logger)? {
            CherryPickOnMc::Applied | CherryPickOnMc::SkippedEmpty => {
                let new_sha = runner::run_git(&["rev-parse", "HEAD"], &mc_path, logger)?
                    .trim()
                    .to_string();
                state.commit_map.insert(sha.clone(), new_sha.clone());
                state.commit_assignments.insert(new_sha, dest.clone());
            }
            CherryPickOnMc::Conflict => {
                let remaining_dest: Vec<String> = commit_shas[i + 1..].to_vec();
                state.mv_rebase_conflict = Some(repo_state::MvRebaseConflict {
                    source_branch: String::new(),
                    dest_branch: dest.clone(),
                    conflicting_commit: sha.clone(),
                    source_is_pushed: false,
                    mc_path: mc_path.to_string_lossy().into_owned(),
                    phase: "cp".to_string(),
                });
                repo_state::save_state(&repo_path, &state)?;

                let mc_state = repo_state::McState {
                    mv_rebase_in_progress: true,
                    mv_rebase_phase: "cp".to_string(),
                    mv_rebase_dest_branch: dest.clone(),
                    mv_rebase_remaining_dest: remaining_dest,
                    ..repo_state::load_mc_state(&mc_path).unwrap_or_default()
                };
                repo_state::save_mc_state(&mc_path, &mc_state)?;

                let logs = logger.drain();
                return Ok(PrioResult::warning(
                    format!(
                        "Cherry-pick conflict for {sha} onto '{dest}' in prio-mc.\n\
                         Run `prio status` for resolution instructions.",
                    ),
                    logs,
                ));
            }
            CherryPickOnMc::Failed => {
                let logs = logger.drain();
                return Ok(PrioResult::failure(
                    format!(
                        "Cherry-pick of {sha} onto '{dest}' failed in prio-mc at {}. \
                         The cherry-pick was refused (not a conflict). \
                         Run `prio abort` to clean up stale prio-mc branches.",
                        mc_path.display()
                    ),
                    logs,
                ));
            }
        }
    }

    if dest_is_pushed {
        runner::run_git(&["push", "origin", &dest], &mc_path, logger)?;
    }

    if !dest_is_pushed {
        let mc_str = crate::util::path_arg(&mc_path);
        runner::run_git(
            &["fetch", &mc_str, &format!("{dest}:{dest}")],
            &repo_path,
            logger,
        )?;
    }

    state.mv_rebase_conflict = None;
    repo_state::save_state(&repo_path, &state)?;

    let dest_tip = branch_tip_ref(&dest, &repo_path, logger).ok();
    let work_branch_tip = runner::run_git(&["rev-parse", "HEAD"], &repo_path, logger)
        .map(|s| s.trim().to_string())
        .ok();
    let dest_already_in_work = match (&dest_tip, &work_branch_tip) {
        (Some(dt), Some(wt)) => {
            runner::run_git(&["merge-base", "--is-ancestor", dt, wt], &repo_path, logger).is_ok()
        }
        _ => false,
    };

    if !dest_already_in_work && !state.applied_branches.is_empty() {
        reset_mc_to_default(&mc_path, &repo_path, &state.default_branch, logger)?;
        let applied = state.applied_branches.clone();
        return execute_apply_merge(
            &repo_path,
            &mc_path,
            &mut state,
            &config,
            applied.clone(),
            applied,
            logger,
        );
    }

    crate::services::suggestions::run_and_log(&repo_path, &mc_path, logger)?;

    let logs = logger.drain();
    Ok(PrioResult::success(
        format!(
            "Copied {} commit(s) to {dest}. Source branch(es) are unchanged.",
            commit_shas.len()
        ),
        logs,
    ))
}
