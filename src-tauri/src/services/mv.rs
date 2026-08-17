//! Move (assign) commits to branches via prio-mc.
//!
//! # Cherry-pick vs. merge-up policy
//!
//! | Commit origin | Destination state | Strategy |
//! | ------------- | ----------------- | -------- |
//! | Above baseline on work branch | Any local branch | Cherry-pick in prio-mc |
//! | Above baseline on work branch | Pushed AND commit already on tip | Metadata-only |
//! | On an applied branch (cross-branch) | Any dest | Cherry-pick to dest in mc; rebase source in mc to remove commit; apply |
//! | On an applied branch (pushed source, no `-f`) | Any | **Error** — use `-f` or `prio cp` |
//! | On an applied branch (pushed source, `-f`) | Any | Cherry-pick to dest in mc; rebase + force-push source; apply |
//! | Unassign (`.`) — work-area commit | — | Metadata-only |
//! | Unassign (`.`) — applied-branch commit | — | Rebase source in mc; apply merge in mc; cherry-pick onto work branch in mc; sync |
//!
//! When every above-baseline commit is assigned, `prio mv` rebuilds the work area automatically.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::error::PrioError;
use crate::git::runner;
use crate::logger::Logger;
use crate::result::{PrioResult, PrioStatus};
use crate::services::apply::{
    do_unassign_apply_and_picks, execute_apply_merge, reset_mc_to_default,
};
use crate::services::common::{
    assert_on_work_branch, branch_tip_ref, cherry_pick_on_mc, expand_commit_args,
    find_commit_source_branch, is_work_area_commit, mc_path_for_repo, prepare_mc_dest_branch,
    resolve_repo_path, unassigned_work_shas, CherryPickOnMc,
};
use crate::storage::repo_state;

pub fn run(
    repo_path: Option<PathBuf>,
    commits: Vec<String>,
    destination: String,
    create: bool,
    apply: bool,
    force: bool,
    logger: &mut Logger,
) -> Result<PrioResult, PrioError> {
    let repo_path = resolve_repo_path(repo_path)?;
    assert_on_work_branch(&repo_path, logger)?;

    let config = repo_state::load_config(&repo_path)?;
    let mut state = repo_state::load_state(&repo_path)?;
    let mc_path = mc_path_for_repo(&repo_path, None)?;

    // Expand ranges and resolve every SHA to its full 40-char form so that
    // commit_map / commit_assignments comparisons are consistent.
    let commit_shas: Vec<String> = expand_commit_args(&commits, &repo_path, logger)?
        .into_iter()
        .map(|sha| {
            runner::run_git(&["rev-parse", &sha], &repo_path, logger)
                .map(|s| s.trim().to_string())
                .unwrap_or(sha)
        })
        .collect();

    // ── Classify each commit ───────────────────────────────────────────────────
    let mut cross_branch: Vec<(String, String)> = Vec::new(); // (full_sha, source_branch)
    let mut work_area: Vec<String> = Vec::new();
    let mut not_found: Vec<String> = Vec::new();

    for sha in &commit_shas {
        // Check commit_map FIRST: if this SHA was previously cherry-picked to an applied
        // branch by `prio mv`, the mapping is authoritative — the source branch has a
        // prio-mc cherry-pick that must be rebased out.  We must take the cross-branch
        // path even when the original work-area SHA is still technically "above baseline"
        // (e.g. the baseline hasn't advanced past it yet).
        let from_map = state
            .commit_map
            .get(sha.as_str())
            .and_then(|cherry| state.commit_assignments.get(cherry.as_str()))
            .filter(|branch| state.applied_branches.contains(*branch))
            .cloned();

        if let Some(src) = from_map {
            cross_branch.push((sha.clone(), src));
        } else if is_work_area_commit(sha, &state.baseline_commit, &repo_path, logger)? {
            work_area.push(sha.clone());
        } else if let Some(src) =
            find_commit_source_branch(sha, &state.applied_branches, &repo_path, logger)
        {
            // Pushed branches: git ancestor check against origin/* works correctly.
            cross_branch.push((sha.clone(), src));
        } else {
            not_found.push(sha.clone());
        }
    }

    if !not_found.is_empty() {
        let logs = logger.drain();
        return Ok(PrioResult::failure(
            format!(
                "Commit(s) {} are not on the work branch or any applied branch. \
                 They must be reachable from the work branch (above the baseline) \
                 or from an applied branch visible in `prio status`.",
                not_found.join(", ")
            ),
            logs,
        ));
    }

    // Mixed work-area + cross-branch in one call is not supported.
    if !work_area.is_empty() && !cross_branch.is_empty() {
        let logs = logger.drain();
        return Ok(PrioResult::failure(
            "Cannot mix work-branch commits with applied-branch commits in a single \
             `prio mv` call. Use separate calls."
                .to_string(),
            logs,
        ));
    }

    // ── Cross-branch path ──────────────────────────────────────────────────────
    if !cross_branch.is_empty() {
        if destination == "." {
            if create {
                return Err(PrioError::Message(
                    "-c is not allowed with destination `.` (unassign).".into(),
                ));
            }
            return run_cross_branch_unassign(
                repo_path,
                mc_path,
                config,
                state,
                cross_branch,
                force,
                logger,
            );
        }
        return run_cross_branch(
            repo_path,
            mc_path,
            config,
            state,
            cross_branch,
            destination,
            create,
            apply,
            force,
            logger,
        );
    }

    // ── Work-area path ────────────────────────────────────────────────────────

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

    // Check branch existence in the work clone for validation only.
    // Creation (when -c) is deferred to prio-mc after reset_mc_to_default so the
    // work clone is not modified before mc work succeeds.
    let branch_exists = !runner::run_git(&["branch", "--list", &dest], &repo_path, logger)?
        .trim()
        .is_empty();
    let branch_created = create && !branch_exists;

    if !branch_exists && !create {
        return Err(PrioError::Message(format!(
            "Branch {dest} does not exist. Use -c to create it."
        )));
    }

    if destination.starts_with("pr-") && create {
        return Err(PrioError::Message(
            "-c is not allowed for pr-{num} destinations.".into(),
        ));
    }

    if assignment != "." {
        let should_apply = apply || branch_created;
        if should_apply && !state.applied_branches.contains(&dest) {
            state.applied_branches.push(dest.clone());
        }
    }

    let branch_is_pushed = destination != "."
        && runner::run_git(
            &["rev-parse", "--verify", &format!("origin/{dest}")],
            &repo_path,
            logger,
        )
        .is_ok();

    let tip_ref = if destination != "." {
        branch_tip_ref(&dest, &repo_path, logger).ok()
    } else {
        None
    };

    let all_already_on_branch = destination != "."
        && tip_ref.is_some()
        && commit_shas.iter().all(|sha| {
            runner::run_git(
                &[
                    "merge-base",
                    "--is-ancestor",
                    sha,
                    tip_ref.as_ref().unwrap(),
                ],
                &repo_path,
                logger,
            )
            .is_ok()
        });

    if all_already_on_branch {
        for sha in &commit_shas {
            state
                .commit_assignments
                .insert(sha.clone(), assignment.clone());
        }
        repo_state::save_state(&repo_path, &state)?;
        let logs = logger.drain();
        return Ok(PrioResult::success(
            format!(
                "Tagged {} commit(s) as {assignment} (already on {dest}).",
                commit_shas.len()
            ),
            logs,
        ));
    }

    // Unassign (`.`): commits are already in the work area above baseline.
    if destination == "." {
        for sha in &commit_shas {
            state
                .commit_assignments
                .insert(sha.clone(), ".".to_string());
        }
        repo_state::save_state(&repo_path, &state)?;
        crate::services::suggestions::run_and_log(&repo_path, &mc_path, logger)?;
        let logs = logger.drain();
        return Ok(PrioResult::success(
            format!(
                "Unassigned {} commit(s) — they remain in the work area.",
                commit_shas.len()
            ),
            logs,
        ));
    }

    // ── All mc work first ──────────────────────────────────────────────────
    reset_mc_to_default(&mc_path, &repo_path, &state.default_branch, logger)?;

    // Create the branch in mc (deferred from work clone to avoid partial state).
    if branch_created {
        runner::run_git(
            &["checkout", "-b", &dest, &state.default_branch],
            &mc_path,
            logger,
        )?;
    } else {
        runner::run_git(&["checkout", &dest], &mc_path, logger)?;
    }

    for sha in &commit_shas {
        let pick = runner::run_git_no_hooks(
            &["cherry-pick", "--allow-empty", "--allow-empty-message", sha],
            &mc_path,
            logger,
        );
        if pick.is_err() {
            let _ = runner::run_git(&["cherry-pick", "--abort"], &mc_path, logger);
            repo_state::save_state(&repo_path, &state)?;
            let logs = logger.drain();
            return Ok(PrioResult::failure(
                format!(
                    "Cherry-pick conflict for {sha} onto {dest} in prio-mc at {}. \
                     The cherry-pick has been aborted — resolve why this commit conflicts \
                     and retry.",
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

    // ── mc cherry-picks complete: now sync to work clone ──────────────────
    repo_state::save_state(&repo_path, &state)?;

    let unassigned = unassigned_work_shas(&repo_path, &state, logger)?;
    if unassigned.is_empty() && !state.applied_branches.is_empty() {
        // Get prio-mc onto the default branch without wiping the freshly-created
        // local dest ref that execute_apply_merge needs to merge.
        let _ = runner::run_git(&["cherry-pick", "--abort"], &mc_path, logger);
        let _ = runner::run_git(&["merge", "--abort"], &mc_path, logger);
        runner::run_git(&["checkout", &state.default_branch], &mc_path, logger)?;

        // execute_apply_merge rebuilds the work branch in mc and syncs all applied
        // branch refs (including dest) to the work clone only after the merge succeeds.
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

    // Unassigned commits remain — apply is not triggered.
    // Sync dest ref from mc to work now (apply won't do it).
    if branch_is_pushed {
        runner::run_git(&["push", "origin", &dest], &mc_path, logger)?;
    } else {
        let mc_str = crate::util::path_arg(&mc_path);
        let _ = runner::run_git(
            &["fetch", &mc_str, &format!("+{dest}:{dest}")],
            &repo_path,
            logger,
        );
    }

    crate::services::suggestions::run_and_log(&repo_path, &mc_path, logger)?;

    let logs = logger.drain();
    let hint = format!(
        " {} unassigned commit(s) remain above the baseline — assign them with \
         `prio mv` before running `prio apply`, or they will be discarded.",
        unassigned.len()
    );
    Ok(PrioResult::success(
        format!(
            "Cherry-picked {} commit(s) to {assignment}.{hint}",
            commit_shas.len()
        ),
        logs,
    ))
}

// ── Cross-branch unassign ─────────────────────────────────────────────────────

/// Unassign commits that live on applied branches.
///
/// **mc-first design**: all cherry-picks, source rebases, and the apply merge run entirely
/// in prio-mc before the work clone is touched.  The work clone is updated atomically at
/// the very end via [`finish_unassign_sync`] once all mc work succeeds.
///
/// On conflict at any stage the work clone is left unchanged; mc is left in the conflict
/// state for the user to resolve and continue via the mc-post-commit hook.
fn run_cross_branch_unassign(
    repo_path: PathBuf,
    mc_path: PathBuf,
    config: repo_state::RepoConfig,
    mut state: repo_state::RepoState,
    cross_branch: Vec<(String, String)>,
    force: bool,
    logger: &mut Logger,
) -> Result<PrioResult, PrioError> {
    let mut source_shas: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();
    for (sha, src) in &cross_branch {
        source_shas
            .entry(src.clone())
            .or_default()
            .push(sha.clone());
    }
    let all_shas: Vec<String> = cross_branch.iter().map(|(s, _)| s.clone()).collect();

    for source in source_shas.keys() {
        let is_pushed = runner::run_git(
            &["rev-parse", "--verify", &format!("origin/{source}")],
            &repo_path,
            logger,
        )
        .is_ok();
        if is_pushed && !force {
            let logs = logger.drain();
            return Ok(PrioResult::failure(
                format!(
                    "Source branch '{source}' has been pushed to origin. \
                     Unassigning commits rewrites remote history. Use `-f` to allow \
                     force-push after rebase."
                ),
                logs,
            ));
        }
    }

    // Compute effective excludes BEFORE modifying commit_map.
    // Using all unassign SHAs for every source is safe: SHAs from other sources simply
    // won't appear in each source's git log and are silently filtered out.
    let effective_excludes = effective_excludes_for_source(&all_shas, &state);

    // Remove from commit_assignments (but keep commit_map — needed for excludes).
    // New "."-assignments will be added with fresh SHAs after cherry-picks in mc.
    for sha in &all_shas {
        if let Some(mapped) = state.commit_map.get(sha.as_str()) {
            state.commit_assignments.remove(mapped);
        }
        state.commit_assignments.remove(sha);
    }
    repo_state::save_state(&repo_path, &state)?;

    // ── All mc work first — work clone untouched until finish_unassign_sync ──

    reset_mc_to_default(&mc_path, &repo_path, &state.default_branch, logger)?;

    // Build ordered source list for tracking progress across continuations.
    let sources_ordered: Vec<(String, bool)> = source_shas
        .keys()
        .map(|src| {
            let is_pushed = runner::run_git(
                &["rev-parse", "--verify", &format!("origin/{src}")],
                &repo_path,
                logger,
            )
            .is_ok();
            (src.clone(), is_pushed)
        })
        .collect();

    // Encode ALL sources for use in the final sync step (even if a later source conflicts).
    let all_sources_encoded: Vec<String> = sources_ordered
        .iter()
        .map(|(b, p)| format!("{}:{}", b, if *p { "true" } else { "false" }))
        .collect();

    for (idx, (source, source_is_pushed)) in sources_ordered.iter().enumerate() {
        let outcome = rebase_filter_shas(
            &repo_path,
            &mc_path,
            source,
            &effective_excludes,
            &state,
            &state.default_branch,
            logger,
        )?;

        match outcome {
            RebaseOutcome::Complete => {}
            RebaseOutcome::Conflict { at_sha, remaining } => {
                // Save remaining sources (this one + those after it) for continuation.
                let remaining_sources_encoded: Vec<String> = sources_ordered[idx..]
                    .iter()
                    .map(|(b, p)| format!("{}:{}", b, if *p { "true" } else { "false" }))
                    .collect();

                state.mv_rebase_conflict = Some(repo_state::MvRebaseConflict {
                    source_branch: source.clone(),
                    dest_branch: ".".to_string(),
                    conflicting_commit: at_sha.clone(),
                    source_is_pushed: *source_is_pushed,
                    mc_path: mc_path.to_string_lossy().into_owned(),
                    phase: "unassign-source".to_string(),
                });
                repo_state::save_state(&repo_path, &state)?;

                let mc_state = repo_state::McState {
                    mv_rebase_in_progress: true,
                    mv_rebase_phase: "unassign-source".to_string(),
                    mv_rebase_source_branch: source.clone(),
                    mv_rebase_dest_branch: ".".to_string(),
                    mv_rebase_remaining_commits: remaining,
                    mv_rebase_source_is_pushed: *source_is_pushed,
                    mv_rebase_force: force,
                    mv_rebase_unassign_commits: all_shas.clone(),
                    mv_rebase_unassign_all_sources: all_sources_encoded.clone(),
                    mv_rebase_unassign_source_branches: remaining_sources_encoded,
                    ..repo_state::load_mc_state(&mc_path).unwrap_or_default()
                };
                repo_state::save_mc_state(&mc_path, &mc_state)?;

                let logs = logger.drain();
                return Ok(PrioResult::warning(
                    format!(
                        "Cherry-pick conflict while rebasing '{source}' to unassign \
                         commit(s): commit {at_sha} conflicts.\n\
                         Run `prio status` for resolution instructions."
                    ),
                    logs,
                ));
            }
        }
    }

    // All source rebases complete in mc.
    state.mv_rebase_conflict = None;
    repo_state::save_state(&repo_path, &state)?;

    // Apply merge + cherry-pick unassigned commits in mc, then sync to work.
    do_unassign_apply_and_picks(
        &repo_path,
        &mc_path,
        &config,
        &mut state,
        &all_shas,
        &all_sources_encoded,
        force,
        logger,
    )
}

// ── Cross-branch move ──────────────────────────────────────────────────────────

/// Move commits that live on applied branches (not above the work-area baseline).
///
/// For each unique source branch:
/// 1. Guard: if pushed and no `-f`, return an error suggesting `-f` or `prio cp`.
/// 2. `-f` only: fetch origin and fast-forward local source before rebasing.
/// 3. Cherry-pick the commits onto the destination in prio-mc.
/// 4. Rebase each source branch in prio-mc (filtering out the moved commits).
/// 5. Sync destination and each rebased source from prio-mc back to WORK.
/// 6. `-f` only: force-push each rebased source to origin.
/// 7. Re-apply to rebuild the work area.
fn run_cross_branch(
    repo_path: PathBuf,
    mc_path: PathBuf,
    config: repo_state::RepoConfig,
    mut state: repo_state::RepoState,
    cross_branch: Vec<(String, String)>, // (sha, source_branch)
    destination: String,
    create: bool,
    apply_flag: bool,
    force: bool,
    logger: &mut Logger,
) -> Result<PrioResult, PrioError> {
    // Collect unique source branches.
    let mut source_shas: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();
    for (sha, src) in &cross_branch {
        source_shas
            .entry(src.clone())
            .or_default()
            .push(sha.clone());
    }
    let all_shas: Vec<String> = cross_branch.iter().map(|(s, _)| s.clone()).collect();

    // Guard: pushed source branches require -f.
    for source in source_shas.keys() {
        let is_pushed = runner::run_git(
            &["rev-parse", "--verify", &format!("origin/{source}")],
            &repo_path,
            logger,
        )
        .is_ok();
        if is_pushed && !force {
            let logs = logger.drain();
            return Ok(PrioResult::failure(
                format!(
                    "Source branch '{source}' has been pushed to origin. \
                     Moving commits off it rewrites remote history. Either:\n  \
                     -f  to allow me to force-push source branch after rebase\n  \
                     prio cp <sha> <dest>  copy without modifying the source branch"
                ),
                logs,
            ));
        }
    }

    // Resolve destination.
    let dest = runner::resolve_branch_ref(&destination, &repo_path, logger)?;
    // Check existence in work clone for validation; creation is deferred to mc.
    let branch_exists = !runner::run_git(&["branch", "--list", &dest], &repo_path, logger)?
        .trim()
        .is_empty();
    let branch_created = create && !branch_exists;

    if !branch_exists && !create {
        return Err(PrioError::Message(format!(
            "Branch {dest} does not exist. Use -c to create it."
        )));
    }

    if apply_flag || branch_created {
        if !state.applied_branches.contains(&dest) {
            state.applied_branches.push(dest.clone());
        }
    }

    // -f: fetch + fast-forward each source branch before rebasing.
    if force {
        for source in source_shas.keys() {
            let is_pushed = runner::run_git(
                &["rev-parse", "--verify", &format!("origin/{source}")],
                &repo_path,
                logger,
            )
            .is_ok();
            if is_pushed {
                runner::run_git(&["fetch", "origin", source], &repo_path, logger)?;
                let origin_ref = format!("origin/{source}");
                // Fast-forward local to origin if local is strictly behind.
                let local_is_behind = runner::run_git(
                    &["merge-base", "--is-ancestor", source, &origin_ref],
                    &repo_path,
                    logger,
                )
                .is_ok();
                if local_is_behind {
                    runner::run_git(&["branch", "-f", source, &origin_ref], &repo_path, logger)?;
                }
            }
        }
    }

    // ── Snapshot per-source effective-excludes BEFORE cherry-picks modify commit_map ──
    // Cherry-picking onto the destination overwrites commit_map[sha] with the new dest SHA,
    // so we must compute what to exclude from each source branch's git log NOW, while the
    // old mappings (original → cherry-pick-on-source) are still intact.
    let source_effective_excludes: std::collections::HashMap<String, HashSet<String>> = source_shas
        .iter()
        .map(|(src, shas)| {
            let mut ex: HashSet<String> = HashSet::new();
            for sha in shas {
                ex.insert(sha.clone());
                // Forward: original SHA → cherry-pick SHA on the source branch.
                if let Some(mapped) = state.commit_map.get(sha.as_str()) {
                    ex.insert(mapped.clone());
                }
            }
            // Reverse: any commit_map value that equals a SHA we're moving.
            for (k, v) in &state.commit_map {
                if shas.contains(v) {
                    ex.insert(k.clone());
                }
            }
            (src.clone(), ex)
        })
        .collect();

    // Cherry-pick all moved commits onto the destination in prio-mc.
    reset_mc_to_default(&mc_path, &repo_path, &state.default_branch, logger)?;

    prepare_mc_dest_branch(&mc_path, &repo_path, &dest, &state.default_branch, logger)?;

    for (i, sha) in all_shas.iter().enumerate() {
        match cherry_pick_on_mc(&mc_path, sha, logger)? {
            CherryPickOnMc::Applied | CherryPickOnMc::SkippedEmpty => {
                let new_sha = runner::run_git(&["rev-parse", "HEAD"], &mc_path, logger)?
                    .trim()
                    .to_string();
                state.commit_map.insert(sha.clone(), new_sha.clone());
                state.commit_assignments.insert(new_sha, dest.clone());
            }
            CherryPickOnMc::Conflict => {
                let remaining_dest: Vec<String> = all_shas[i + 1..].to_vec();
                let source_shas_flat: Vec<String> =
                    source_shas.values().flatten().cloned().collect();
                let first_source = source_shas.keys().next().cloned().unwrap_or_default();
                let first_source_is_pushed = runner::run_git(
                    &["rev-parse", "--verify", &format!("origin/{first_source}")],
                    &repo_path,
                    logger,
                )
                .is_ok();

                state.mv_rebase_conflict = Some(repo_state::MvRebaseConflict {
                    source_branch: first_source.clone(),
                    dest_branch: dest.clone(),
                    conflicting_commit: sha.clone(),
                    source_is_pushed: first_source_is_pushed,
                    mc_path: mc_path.to_string_lossy().into_owned(),
                    phase: "dest".to_string(),
                });
                repo_state::save_state(&repo_path, &state)?;

                let mc_state = repo_state::McState {
                    mv_rebase_in_progress: true,
                    mv_rebase_phase: "dest".to_string(),
                    mv_rebase_dest_branch: dest.clone(),
                    mv_rebase_remaining_dest: remaining_dest,
                    mv_rebase_source_branch: first_source,
                    mv_rebase_source_shas_to_exclude: source_shas_flat,
                    mv_rebase_source_is_pushed: first_source_is_pushed,
                    mv_rebase_force: force,
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
                         Run `prio abort` to clean up stale prio-mc branches.",
                        mc_path.display()
                    ),
                    logs,
                ));
            }
        }
    }

    // Save state — dest cherry-picks are done; clears any stale mv_rebase_conflict.
    state.mv_rebase_conflict = None;
    repo_state::save_state(&repo_path, &state)?;

    // Rebase each source branch in prio-mc, filtering out the moved commits.
    // Ref syncs to the work clone are deferred to after execute_apply_merge so the
    // work clone is never in a partially-updated state.
    for source in source_shas.keys() {
        let exclude = source_effective_excludes
            .get(source)
            .cloned()
            .unwrap_or_default();
        let source_is_pushed = runner::run_git(
            &["rev-parse", "--verify", &format!("origin/{source}")],
            &repo_path,
            logger,
        )
        .is_ok();

        let outcome = rebase_filter_shas(
            &repo_path,
            &mc_path,
            source,
            &exclude,
            &state,
            &state.default_branch.clone(),
            logger,
        )?;

        match outcome {
            RebaseOutcome::Complete => {
                // Ref sync deferred to execute_apply_merge (which syncs all applied branches).
            }
            RebaseOutcome::Conflict { at_sha, remaining } => {
                // Cherry-pick conflict left in prio-mc for user to resolve.
                state.mv_rebase_conflict = Some(repo_state::MvRebaseConflict {
                    source_branch: source.clone(),
                    dest_branch: dest.clone(),
                    conflicting_commit: at_sha.clone(),
                    source_is_pushed,
                    mc_path: mc_path.to_string_lossy().into_owned(),
                    phase: "source".to_string(),
                });
                repo_state::save_state(&repo_path, &state)?;

                let mc_state = repo_state::McState {
                    mv_rebase_in_progress: true,
                    mv_rebase_phase: "source".to_string(),
                    mv_rebase_source_branch: source.clone(),
                    mv_rebase_dest_branch: dest.clone(),
                    mv_rebase_remaining_commits: remaining,
                    mv_rebase_source_is_pushed: source_is_pushed,
                    mv_rebase_force: force,
                    ..repo_state::load_mc_state(&mc_path).unwrap_or_default()
                };
                repo_state::save_mc_state(&mc_path, &mc_state)?;

                let logs = logger.drain();
                return Ok(PrioResult::warning(
                    format!(
                        "Cherry-pick conflict while rebasing '{source}' to remove moved \
                         commits: commit {at_sha} conflicts.\n\
                         Run `prio status` for resolution instructions.",
                    ),
                    logs,
                ));
            }
        }
    }

    // All source rebases complete. Re-apply to rebuild work area; execute_apply_merge
    // syncs ALL applied branch refs (dest + sources) to work atomically.
    let dest_in_applied = state.applied_branches.contains(&dest);
    let dest_is_pushed = runner::run_git(
        &["rev-parse", "--verify", &format!("origin/{dest}")],
        &repo_path,
        logger,
    )
    .is_ok();

    // Get prio-mc onto the default branch so execute_apply_merge can create its
    // prio-mc/* merge branch from there.  Do NOT call reset_mc_to_default here: that
    // would wipe the freshly-created local branch refs (dest cherry-pick, source
    // rebases) that merge_branches_in_mc is about to merge.  Those local refs are the
    // authoritative post-operation state; origin/* in prio-mc still reflects the
    // pre-operation tips until we sync back to the work clone.
    let _ = runner::run_git(&["cherry-pick", "--abort"], &mc_path, logger);
    let _ = runner::run_git(&["merge", "--abort"], &mc_path, logger);
    runner::run_git(&["checkout", &state.default_branch], &mc_path, logger)?;

    let pending_force_push_sources: Vec<String> = source_shas
        .keys()
        .filter_map(|source| {
            let source_is_pushed = runner::run_git(
                &["rev-parse", "--verify", &format!("origin/{source}")],
                &repo_path,
                logger,
            )
            .is_ok();
            if force && source_is_pushed {
                Some(format!("{source}:true"))
            } else {
                None
            }
        })
        .collect();
    let mut pre_apply_mc_state = repo_state::load_mc_state(&mc_path).unwrap_or_default();
    pre_apply_mc_state.mv_rebase_force = !pending_force_push_sources.is_empty();
    pre_apply_mc_state.mv_pending_force_push_sources = pending_force_push_sources;
    repo_state::save_mc_state(&mc_path, &pre_apply_mc_state)?;

    let applied = state.applied_branches.clone();
    let result = execute_apply_merge(
        &repo_path,
        &mc_path,
        &mut state,
        &config,
        applied.clone(),
        applied,
        logger,
    )?;

    if result.status != PrioStatus::Success {
        return Ok(result);
    }

    // Sync dest ref if it was not in applied_branches (execute_apply_merge only syncs
    // applied branches; dest may not be applied if -a was not passed and it pre-existed).
    if !dest_in_applied {
        if dest_is_pushed {
            let _ = runner::run_git(&["push", "origin", &dest], &mc_path, logger);
        } else {
            let mc_str = crate::util::path_arg(&mc_path);
            let _ = runner::run_git(
                &["fetch", &mc_str, &format!("+{dest}:{dest}")],
                &repo_path,
                logger,
            );
        }
    }

    // Force-push source branches to GitHub if -f was specified.
    for source in source_shas.keys() {
        let source_is_pushed = runner::run_git(
            &["rev-parse", "--verify", &format!("origin/{source}")],
            &repo_path,
            logger,
        )
        .is_ok();
        if force && source_is_pushed {
            let _ = runner::run_git(&["push", "--force", "origin", source], &repo_path, logger);
        }
    }

    Ok(result)
}

/// Outcome of [`rebase_filter_shas`].
pub(crate) enum RebaseOutcome {
    /// All commits cherry-picked successfully.
    Complete,
    /// A cherry-pick conflict was left in place for the user to resolve.
    Conflict {
        /// SHA of the commit that caused the conflict.
        at_sha: String,
        /// Commits that still need to be cherry-picked after the conflict is resolved.
        remaining: Vec<String>,
    },
}

/// In prio-mc: rebase `source_branch` onto `default_branch`, cherry-picking every
/// **non-merge** commit from its history (above `default_branch`) **except** those
/// corresponding to `shas_to_exclude`.
///
/// # Merge-commit handling
/// Merge commits (commits with two or more parents) cannot be cherry-picked without `-m`
/// and are **silently skipped**.  Their content was already incorporated by the non-merge
/// commits on either side, so skipping them produces a correct linear history.
///
/// # Content-conflict handling
/// When a cherry-pick fails with a real content conflict (`CHERRY_PICK_HEAD` exists in
/// `.git`), the conflict is **left in place** so the user can resolve it.
/// Returns [`RebaseOutcome::Conflict`] with the remaining commits that still need to
/// be cherry-picked afterwards.
///
/// # Hard failures
/// Any other failure (e.g. the cherry-pick was refused before starting) aborts and
/// returns `Err`.
///
/// The git log is retrieved from prio-mc's perspective (using prio-mc's local branch ref
/// or `origin/<branch>`) to avoid relying on potentially stale WORK local refs.
/// Pre-compute the set of SHAs (in prio-mc space) to exclude from a source branch log.
///
/// Must be called BEFORE any cherry-picks to dest, because those operations
/// overwrite `state.commit_map` entries (the original → prio-mc mappings needed here).
pub(crate) fn effective_excludes_for_source(
    shas_to_move: &[String],
    state: &repo_state::RepoState,
) -> HashSet<String> {
    let mut ex: HashSet<String> = shas_to_move.iter().cloned().collect();
    for sha in shas_to_move {
        if let Some(mapped) = state.commit_map.get(sha.as_str()) {
            ex.insert(mapped.clone());
        }
    }
    for (k, v) in &state.commit_map {
        if shas_to_move.contains(v) {
            ex.insert(k.clone());
        }
    }
    ex
}

pub(crate) fn rebase_filter_shas(
    repo_path: &Path,
    mc_path: &Path,
    source_branch: &str,
    effective_excludes: &HashSet<String>,
    _state: &repo_state::RepoState,
    default_branch: &str,
    logger: &mut Logger,
) -> Result<RebaseOutcome, PrioError> {
    // Get the authoritative tip from prio-mc.
    let mc_tip = match branch_tip_ref(source_branch, mc_path, logger) {
        Ok(t) => t,
        Err(_) => match branch_tip_ref(source_branch, repo_path, logger) {
            Ok(t) => t,
            Err(_) => return Ok(RebaseOutcome::Complete), // nothing to rebase
        },
    };

    // List NON-MERGE commits on source above default_branch (oldest first),
    // including BOTH kept and excluded commits (we need the full chain for squashing).
    let log_out = runner::run_git(
        &[
            "log",
            &mc_tip,
            "--not",
            default_branch,
            "--no-merges",
            "--format=%H %s",
            "--reverse",
            "--",
        ],
        mc_path,
        logger,
    )?;

    // Parse into (sha, subject, is_kept) tuples.
    let full_chain: Vec<(String, String, bool)> = log_out
        .lines()
        .filter(|l| !l.is_empty())
        .filter_map(|line| {
            let mut parts = line.splitn(2, ' ');
            let sha = parts.next()?.to_string();
            let subj = parts.next().unwrap_or("").to_string();
            let kept = !effective_excludes.contains(sha.as_str());
            Some((sha, subj, kept))
        })
        .collect();

    // If nothing to keep, branch is entirely removed — leave it at default_branch tip.
    if full_chain.iter().all(|(_, _, kept)| !kept) {
        // Nothing to rebase; source branch will have no commits above default_branch.
        // Use a targeted abort+checkout instead of reset_mc_to_default so that other
        // local branch refs built up by the current mv operation are not wiped.
        let _ = runner::run_git(&["cherry-pick", "--abort"], mc_path, logger);
        let _ = runner::run_git(&["merge", "--abort"], mc_path, logger);
        runner::run_git(&["checkout", default_branch], mc_path, logger)?;
        let _ = runner::run_git(&["branch", "-D", source_branch], mc_path, logger);
        runner::run_git(
            &["checkout", "-b", source_branch, default_branch],
            mc_path,
            logger,
        )?;
        return Ok(RebaseOutcome::Complete);
    }

    // Build the list of kept commits (for conflict-continuation tracking).
    let kept_commits: Vec<String> = full_chain
        .iter()
        .filter(|(_, _, kept)| *kept)
        .map(|(sha, _, _)| sha.clone())
        .collect();

    // Create a fresh source branch from default_branch, preserving other local refs.
    // Use a targeted abort+checkout instead of reset_mc_to_default so that other
    // local branch refs built up by the current mv operation are not wiped.
    let _ = runner::run_git(&["cherry-pick", "--abort"], mc_path, logger);
    let _ = runner::run_git(&["merge", "--abort"], mc_path, logger);
    runner::run_git(&["checkout", default_branch], mc_path, logger)?;
    let _ = runner::run_git(&["branch", "-D", source_branch], mc_path, logger);
    runner::run_git(
        &["checkout", "-b", source_branch, default_branch],
        mc_path,
        logger,
    )?;

    // Replay the full commit chain with the "squash-excluded-predecessors" strategy:
    //
    // When a kept commit immediately follows one or more excluded commits, those
    // excluded commits provide content that the kept commit's diff depends on.
    // To avoid cherry-pick conflicts we:
    //   1. Cherry-pick excluded commits normally (they commit onto the branch).
    //   2. Cherry-pick the kept commit on top.
    //   3. "Squash" the excluded commits out: `git reset --soft HEAD~(n_excluded+1)`
    //      to unstage all those commits' changes back into the index, then
    //      recommit under the kept commit's subject.
    //
    // This leaves only the kept commits in the final history, each carrying the
    // accumulated content of any excluded predecessors it depended on.
    let mut pending_excluded: usize = 0;

    for (i, (sha, subj, kept)) in full_chain.iter().enumerate() {
        match cherry_pick_on_mc(mc_path, sha, logger)? {
            CherryPickOnMc::Applied => {
                if *kept {
                    if pending_excluded > 0 {
                        // Squash the N excluded commits + this kept commit into one.
                        let squash_count = pending_excluded + 1;
                        runner::run_git(
                            &["reset", "--soft", &format!("HEAD~{squash_count}")],
                            mc_path,
                            logger,
                        )?;
                        runner::run_git(
                            &[
                                "commit",
                                "--allow-empty",
                                "--allow-empty-message",
                                "-m",
                                subj,
                            ],
                            mc_path,
                            logger,
                        )?;
                        pending_excluded = 0;
                    }
                    // else: kept commit with no excluded predecessors — already committed ✓
                } else {
                    pending_excluded += 1;
                }
            }
            CherryPickOnMc::SkippedEmpty => {
                if *kept {
                    // The kept commit was empty (already applied).  Still need to
                    // clear any pending excluded context that preceded it.
                    if pending_excluded > 0 {
                        runner::run_git(
                            &["reset", "--soft", &format!("HEAD~{pending_excluded}")],
                            mc_path,
                            logger,
                        )?;
                        pending_excluded = 0;
                    }
                }
                // An excluded commit that was empty is irrelevant — skip it.
            }
            CherryPickOnMc::Conflict => {
                if *kept {
                    // Conflict on a kept commit.  Report it and leave prio-mc in the
                    // conflict state for the user to resolve.  Pass the remaining kept
                    // commits for continuation.
                    let remaining = kept_commits[kept_commits
                        .iter()
                        .position(|s| s == sha)
                        .map(|p| p + 1)
                        .unwrap_or(kept_commits.len())..]
                        .to_vec();
                    return Ok(RebaseOutcome::Conflict {
                        at_sha: sha.clone(),
                        remaining,
                    });
                } else {
                    // Conflict on an excluded commit.  This means even building the
                    // "foundation" context fails — treat as a conflict on the next kept
                    // commit (if any).
                    let next_kept = full_chain[i + 1..]
                        .iter()
                        .find(|(_, _, k)| *k)
                        .map(|(s, _, _)| s.clone());
                    if let Some(kept_sha) = next_kept {
                        let remaining = kept_commits[kept_commits
                            .iter()
                            .position(|s| *s == kept_sha)
                            .unwrap_or(kept_commits.len())..]
                            .to_vec();
                        return Ok(RebaseOutcome::Conflict {
                            at_sha: kept_sha,
                            remaining,
                        });
                    } else {
                        // No more kept commits — this conflict is on excluded context
                        // we don't need to commit.  Abort and continue.
                        let _ = runner::run_git(&["cherry-pick", "--abort"], mc_path, logger);
                    }
                }
            }
            CherryPickOnMc::Failed => {
                return Err(PrioError::Message(format!(
                    "Could not cherry-pick commit {sha} while rebasing '{source_branch}': \
                     cherry-pick was refused. Check prio-mc at {}.",
                    mc_path.display()
                )));
            }
        }
    }

    // If there are trailing excluded commits with no following kept commit,
    // drop them (reset back, removing any uncommitted work).
    if pending_excluded > 0 {
        runner::run_git(
            &["reset", "--soft", &format!("HEAD~{pending_excluded}")],
            mc_path,
            logger,
        )?;
        // Discard the staged excluded changes — they're being moved to dest.
        runner::run_git(&["checkout", "--", "."], mc_path, logger)?;
    }

    Ok(RebaseOutcome::Complete)
}
