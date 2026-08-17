//! Apply/unapply and the shared merge pipeline.
//!
//! **Design:** all merges and cherry-picks run in the `*-prio-mc` clone; the work clone
//! only receives finished results via [`sync_work_clone`] (`git reset --hard`). See `AGENTS.md`.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use crate::error::PrioError;
use crate::git::runner;
use crate::logger::Logger;
use crate::result::{PrioResult, PrioStatus};
use crate::services::common::{
    assert_on_work_branch, branch_tip_ref, cherry_pick_on_mc, mc_path_for_repo, resolve_repo_path,
    CherryPickOnMc,
};
use crate::storage::{repo_state, user_config};
use crate::util;

pub fn run(
    repo_path: Option<PathBuf>,
    branches: Vec<String>,
    unapply: bool,
    logger: &mut Logger,
) -> Result<PrioResult, PrioError> {
    let repo_path = resolve_repo_path(repo_path)?;
    assert_on_work_branch(&repo_path, logger)?;
    let mc_path = mc_path_for_repo(&repo_path, None)?;
    let mut state = repo_state::load_state(&repo_path)?;
    let config = repo_state::load_config(&repo_path)?;

    let resolved: Vec<String> = branches
        .iter()
        .map(|b| runner::resolve_branch_ref(b, &repo_path, logger))
        .collect::<Result<_, _>>()?;

    for branch in &resolved {
        runner::ensure_branch_for_apply(branch, &mc_path, logger)?;
    }

    let mut applied = state.applied_branches.clone();
    for b in resolved {
        if unapply {
            applied.retain(|x| x != &b);
        } else if !applied.contains(&b) {
            applied.push(b);
        }
    }

    reset_mc_to_default(&mc_path, &repo_path, &state.default_branch, logger)?;
    let order = resolve_optimal_order(
        &applied,
        &mc_path,
        &repo_path,
        &state.default_branch,
        logger,
    )?;
    execute_apply_merge(
        &repo_path, &mc_path, &mut state, &config, applied, order, logger,
    )
}

/// Re-apply with an explicit branch merge order (UI drag-and-drop).
pub fn reorder(
    repo_path: Option<PathBuf>,
    order: Vec<String>,
    logger: &mut Logger,
) -> Result<PrioResult, PrioError> {
    let repo_path = resolve_repo_path(repo_path)?;
    assert_on_work_branch(&repo_path, logger)?;
    let mc_path = mc_path_for_repo(&repo_path, None)?;
    let mut state = repo_state::load_state(&repo_path)?;
    let config = repo_state::load_config(&repo_path)?;

    let mut sorted_applied = state.applied_branches.clone();
    let mut sorted_order = order.clone();
    sorted_applied.sort();
    sorted_order.sort();
    if sorted_applied != sorted_order {
        return Err(PrioError::Message(
            "Reorder must include exactly the currently applied branches.".into(),
        ));
    }

    if state.merge_in_progress {
        let n = state.pending_merge_index;
        if order.len() < n || order[..n] != state.pending_merge_branches[..n] {
            return Err(PrioError::Message(
                "Cannot reorder at or below branches already merged in the current apply.".into(),
            ));
        }
    }

    for branch in &order {
        runner::ensure_branch_for_apply(branch, &mc_path, logger)?;
    }

    reset_mc_to_default(&mc_path, &repo_path, &state.default_branch, logger)?;
    execute_apply_merge(
        &repo_path,
        &mc_path,
        &mut state,
        &config,
        order.clone(),
        order,
        logger,
    )
}

pub fn execute_apply_merge(
    repo_path: &Path,
    mc_path: &Path,
    state: &mut repo_state::RepoState,
    config: &repo_state::RepoConfig,
    applied: Vec<String>,
    order: Vec<String>,
    logger: &mut Logger,
) -> Result<PrioResult, PrioError> {
    let merge_branch = if !order.is_empty() {
        let name = format!(
            "prio-mc/{}",
            order
                .iter()
                .map(|b| b.replace('/', "-"))
                .collect::<Vec<_>>()
                .join("+")
        );
        runner::run_git_no_hooks(&["checkout", "-b", &name], mc_path, logger)?;
        name
    } else {
        String::new()
    };

    match merge_branches_in_mc(mc_path, &order, &state.default_branch, repo_path, logger)? {
        MergeOutcome::Complete(head) => {
            sync_work_clone(repo_path, mc_path, &config.work_branch, &head, logger)?;

            // Sync individual branch refs from prio-mc into WORK so that
            // `prio status` can show commits per branch using `git log <branch>`.
            //
            // After apply, each applied branch still exists as a local ref in
            // prio-mc (only `prio-mc/*` merge branches are deleted by
            // reset_mc_to_default).  For branches that have never been pushed to
            // GitHub, WORK's local ref was created at `main` by `prio mv -c` and
            // was never advanced — it must be synced here.
            //
            // For pushed branches we only sync if WORK's local ref is behind
            // prio-mc (e.g. after `prio mv -f` rebase).  Force-fetch with `+`
            // to allow non-fast-forward updates.
            {
                let mc_str = crate::util::path_arg(mc_path);
                for branch in &applied {
                    if branch == &state.default_branch {
                        continue;
                    }
                    let mc_has_branch =
                        runner::run_git(&["rev-parse", "--verify", branch], mc_path, logger)
                            .is_ok();
                    if mc_has_branch {
                        let _ = runner::run_git(
                            &["fetch", &mc_str, &format!("+{branch}:{branch}")],
                            repo_path,
                            logger,
                        );
                    }
                }
            }

            state.applied_branches = applied;
            state.baseline_commit = head;
            state.merge_in_progress = false;
            state.pending_merge_branches.clear();
            state.pending_merge_index = 0;
            repo_state::save_state(repo_path, state)?;

            repo_state::save_mc_state(
                mc_path,
                &repo_state::McState {
                    merge_in_progress: false,
                    pending_merge_branches: vec![],
                    pending_merge_index: 0,
                    default_branch: state.default_branch.clone(),
                    merge_branch: String::new(),
                    ..Default::default()
                },
            )?;

            // Persist as last-good so `prio recover` can restore to this clean
            // apply result — not to some older pre-apply commit.
            let _ = repo_state::save_last_good(
                repo_path,
                &repo_state::LastGoodState {
                    commit_sha: state.baseline_commit.clone(),
                    state: state.clone(),
                },
            );

            crate::services::suggestions::run_and_log(repo_path, mc_path, logger)?;

            let logs = logger.drain();
            Ok(PrioResult::success("Apply completed.", logs))
        }
        MergeOutcome::Conflict { at_index } => {
            state.applied_branches = applied.clone();
            state.merge_in_progress = true;
            state.pending_merge_branches = order.clone();
            state.pending_merge_index = at_index;
            repo_state::save_state(repo_path, state)?;

            let incoming = order.get(at_index).cloned().unwrap_or_default();
            let already_merged = order[..at_index].to_vec();
            let base_desc = if already_merged.is_empty() {
                state.default_branch.clone()
            } else {
                format!("{} + {}", state.default_branch, already_merged.join(" + "))
            };

            let existing_mc = repo_state::load_mc_state(mc_path).unwrap_or_default();
            repo_state::save_mc_state(
                mc_path,
                &repo_state::McState {
                    merge_in_progress: true,
                    pending_merge_branches: order,
                    pending_merge_index: at_index,
                    default_branch: state.default_branch.clone(),
                    merge_branch: merge_branch.clone(),
                    mv_rebase_force: existing_mc.mv_rebase_force,
                    mv_pending_force_push_sources: existing_mc.mv_pending_force_push_sources,
                    ..Default::default()
                },
            )?;

            let logs = logger.drain();
            Ok(PrioResult::warning(
                format!(
                    "Merge conflict in prio-mc: merging {incoming} into ({base_desc}).\n\
                     Resolve conflicts in: {mc_path}\n\
                     Branch: {merge_branch}\n\
                     Then run: git -C \"{mc_path}\" commit --no-edit",
                    mc_path = mc_path.display(),
                ),
                logs,
            ))
        }
    }
}

pub enum MergeOutcome {
    Complete(String),
    Conflict { at_index: usize },
}

/// Tip ref to merge — same rules as [`crate::services::common::branch_tip_ref`].
fn merge_ref_for_branch(
    branch: &str,
    mc_path: &Path,
    logger: &mut Logger,
) -> Result<String, PrioError> {
    // In prio-mc, local branch refs are always the authoritative post-rebase state.
    // origin/<branch> can be stale (it tracks what was last fetched from the work clone,
    // which may not yet reflect mc's rebased branches).  Prefer local when it exists;
    // fall back to origin/* only for pushed branches that prio-mc hasn't created locally.
    let local = branch.to_string();
    if runner::run_git(&["rev-parse", "--verify", &local], mc_path, logger).is_ok() {
        return Ok(local);
    }
    let origin = format!("origin/{branch}");
    if runner::run_git(&["rev-parse", "--verify", &origin], mc_path, logger).is_ok() {
        return Ok(origin);
    }
    Err(PrioError::Message(format!(
        "Branch '{branch}' not found in prio-mc (checked local and origin/{branch})"
    )))
}

/// Tracks whether the work-clone github.com fetch has already run this process.
/// `main` does not change during a single `prio` invocation, so one fetch is enough.
static WORK_ORIGIN_FETCHED: AtomicBool = AtomicBool::new(false);

/// Reset the prio-mc clone to a clean `origin/<default_branch>` (start of a merge operation).
///
/// Aborts any in-progress merge or cherry-pick first, so this is safe to call from any state.
/// Also removes any leftover `prio-mc/*` branches from previous apply runs.
///
/// `repo_path` is the WORK clone.  Because prio-mc's `origin` is the WORK clone, its
/// `origin/<default_branch>` tracks WORK's **local** default branch — which can be stale after an
/// upstream push fetched by `prio sync`.  We force-update WORK's local default branch from
/// github.com **once per process** so that prio-mc's `git fetch origin` picks up the latest base
/// commit without redundant network round-trips on every internal reset.
pub fn reset_mc_to_default(
    mc_path: &Path,
    repo_path: &Path,
    default_branch: &str,
    logger: &mut Logger,
) -> Result<(), PrioError> {
    // Abort any in-progress cherry-pick or merge (no-op when nothing is in progress).
    let _ = runner::run_git(&["cherry-pick", "--abort"], mc_path, logger);
    let _ = runner::run_git(&["merge", "--abort"], mc_path, logger);
    // Force-update WORK's local default branch from its GitHub remote so that when
    // prio-mc fetches from WORK it sees the latest base commit.
    // Guard: this is a network call; run it at most once per process invocation.
    // `main` cannot change during the lifetime of a single `prio` command.
    if !WORK_ORIGIN_FETCHED.swap(true, Ordering::Relaxed) {
        let _ = runner::run_git(
            &[
                "fetch",
                "origin",
                &format!("+refs/heads/{default_branch}:refs/heads/{default_branch}"),
            ],
            repo_path,
            logger,
        );
    }
    runner::run_git(&["fetch", "origin"], mc_path, logger)?;
    // Force checkout so a stalled operation with a clean worktree doesn't block us.
    runner::run_git(&["checkout", "-f", default_branch], mc_path, logger)?;
    runner::run_git(
        &["reset", "--hard", &format!("origin/{default_branch}")],
        mc_path,
        logger,
    )?;
    // Delete ALL local branch refs except the default branch.  This removes both
    // leftover prio-mc/* merge branches and any stale per-feature refs from previous
    // operations.  The work clone is the source of truth for branch state; prio-mc
    // must fetch from origin (the work clone) to get current tips rather than relying
    // on potentially-stale local refs it accumulated across runs.
    if let Ok(out) = runner::run_git(&["branch", "--list"], mc_path, logger) {
        for b in out
            .lines()
            .map(|l| l.trim().trim_start_matches('*').trim())
            .filter(|b| !b.is_empty() && *b != default_branch)
        {
            let _ = runner::run_git(&["branch", "-D", b], mc_path, logger);
        }
    }
    Ok(())
}

/// Merge branches in **prio-mc only**. Never run conflicting merges on the work clone.
pub fn merge_branches_in_mc(
    mc_path: &Path,
    order: &[String],
    default_branch: &str,
    _work_repo: &Path,
    logger: &mut Logger,
) -> Result<MergeOutcome, PrioError> {
    let mut head = runner::run_git(&["rev-parse", "HEAD"], mc_path, logger)?
        .trim()
        .to_string();

    for (i, branch) in order.iter().enumerate() {
        let merge_ref = merge_ref_for_branch(branch, mc_path, logger)?;
        let merge_result = runner::run_git_no_hooks(
            &[
                "merge",
                &merge_ref,
                "--no-ff",
                "-m",
                &format!("prio: merge {branch}"),
            ],
            mc_path,
            logger,
        );

        if merge_result.is_err() {
            return Ok(MergeOutcome::Conflict { at_index: i });
        }

        head = runner::run_git(&["rev-parse", "HEAD"], mc_path, logger)?
            .trim()
            .to_string();

        record_conflict_resolution(mc_path, branch, default_branch, &head, i + 1, logger)?;
    }

    Ok(MergeOutcome::Complete(head))
}

fn record_conflict_resolution(
    mc_path: &Path,
    branch: &str,
    default_branch: &str,
    commit: &str,
    depth: usize,
    _logger: &mut Logger,
) -> Result<(), PrioError> {
    let mut history = repo_state::load_conflict_history(mc_path)?;
    history.push(repo_state::ConflictHistoryEntry {
        branch_a: default_branch.to_string(),
        branch_b: branch.to_string(),
        resolution_commit: commit.to_string(),
        depth,
        resolved_at: util::now_ms(),
        stale: false,
    });
    history.sort_by(|a, b| b.depth.cmp(&a.depth));
    repo_state::save_conflict_history(mc_path, &history)?;
    Ok(())
}

/// Copy a completed prio-mc result onto the work branch.
///
/// prio-mc and the work repo are separate local clones with independent object stores.
/// Merge commits created inside prio-mc don't automatically exist in the work repo, so we
/// fetch from prio-mc first — bringing the objects across — before `reset --hard`.
pub fn sync_work_clone(
    repo_path: &Path,
    mc_path: &Path,
    work_branch: &str,
    head: &str,
    logger: &mut Logger,
) -> Result<(), PrioError> {
    let mc_str = crate::util::path_arg(mc_path);
    runner::run_git(&["fetch", &mc_str, "HEAD"], repo_path, logger)?;
    runner::run_git(&["checkout", work_branch], repo_path, logger)?;
    runner::run_git(&["reset", "--hard", head], repo_path, logger)?;
    Ok(())
}

pub fn resolve_optimal_order(
    branches: &[String],
    mc_path: &Path,
    repo_path: &Path,
    default_branch: &str,
    logger: &mut Logger,
) -> Result<Vec<String>, PrioError> {
    if branches.len() <= 1 {
        return Ok(branches.to_vec());
    }

    let cache_key = branches.join(",");
    let mut cache = repo_state::load_apply_cache(mc_path)?;
    if let Some(&score) = cache.entries.get(&cache_key) {
        if score == 0 {
            return Ok(branches.to_vec());
        }
    }

    let perms = permutations(branches, 720);
    let mut best_score = usize::MAX;
    let mut best_order = branches.to_vec();

    for perm in perms {
        let score = score_merge_order(&perm, mc_path, repo_path, default_branch, logger)?;
        if score < best_score {
            best_score = score;
            best_order = perm;
        }
    }

    cache.entries.insert(cache_key, best_score);
    repo_state::save_apply_cache(mc_path, &cache)?;

    Ok(best_order)
}

pub fn score_merge_order(
    order: &[String],
    mc_path: &Path,
    repo_path: &Path,
    default_branch: &str,
    logger: &mut Logger,
) -> Result<usize, PrioError> {
    let trial = format!("prio-trial-{}", util::now_ms());
    reset_mc_to_default(mc_path, repo_path, default_branch, logger)?;
    runner::run_git(&["checkout", "-b", &trial], mc_path, logger)?;

    let mut conflicts = 0;
    for branch in order {
        let merge_ref = merge_ref_for_branch(branch, mc_path, logger)?;
        let result =
            runner::run_git_no_hooks(&["merge", "--no-commit", &merge_ref], mc_path, logger);
        if result.is_err() {
            conflicts += count_conflict_markers(mc_path, logger)?;
            let _ = runner::run_git(&["merge", "--abort"], mc_path, logger);
        } else {
            let _ = runner::run_git(&["merge", "--abort"], mc_path, logger);
        }
    }

    reset_mc_to_default(mc_path, repo_path, default_branch, logger)?;
    let _ = runner::run_git(&["branch", "-D", &trial], mc_path, logger);

    Ok(conflicts)
}

fn count_conflict_markers(mc_path: &Path, logger: &mut Logger) -> Result<usize, PrioError> {
    let diff = runner::run_git(&["diff", "--check"], mc_path, logger).unwrap_or_default();
    let count = diff
        .lines()
        .filter(|l| l.contains("leftover conflict marker"))
        .count();
    Ok(count.max(1))
}

fn permutations(items: &[String], max: usize) -> Vec<Vec<String>> {
    let mut result = Vec::new();
    permute(items, 0, &mut result, max);
    result
}

fn permute(items: &[String], start: usize, out: &mut Vec<Vec<String>>, max: usize) {
    if out.len() >= max {
        return;
    }
    if start >= items.len() {
        out.push(items.to_vec());
        return;
    }
    for i in start..items.len() {
        let mut v = items.to_vec();
        v.swap(start, i);
        permute(&v, start + 1, out, max);
    }
}

pub fn mc_post_commit(
    mc_path: Option<PathBuf>,
    logger: &mut Logger,
) -> Result<PrioResult, PrioError> {
    let mc_path = mc_path.unwrap_or_else(|| std::env::current_dir().expect("cwd"));
    let mc_state = repo_state::load_mc_state(&mc_path)?;

    // ── mv rebase continuation (prio mv -f source-branch rebase) ─────────────
    if mc_state.mv_rebase_in_progress {
        return mc_post_commit_rebase_continue(&mc_path, mc_state, logger);
    }

    if !mc_state.merge_in_progress {
        let logs = logger.drain();
        return Ok(PrioResult::success("No merge in progress.", logs));
    }

    let repo = user_config::load_repos()?
        .into_iter()
        .find(|r| r.mc_clone_path == mc_path.to_string_lossy())
        .ok_or_else(|| PrioError::Message("Could not find work clone for this mc repo.".into()))?;

    let work_path = PathBuf::from(repo.path);
    let order = mc_state.pending_merge_branches.clone();
    let start = mc_state.pending_merge_index + 1;

    if start >= order.len() {
        let head = runner::run_git(&["rev-parse", "HEAD"], &mc_path, logger)?
            .trim()
            .to_string();
        // If this merge was triggered by a cross-branch unassign, cherry-pick the
        // unassigned commits in mc on top of the merge result before syncing to work.
        if !mc_state.mv_rebase_unassign_commits.is_empty() {
            return continue_unassign_picks_after_merge(
                &mc_path, &work_path, mc_state, head, logger,
            );
        }
        return finish_merge_continuation(&mc_path, &work_path, &mc_state, &head, logger);
    }

    let remaining = &order[start..];
    match merge_branches_in_mc(
        &mc_path,
        remaining,
        &mc_state.default_branch,
        &work_path,
        logger,
    )? {
        MergeOutcome::Complete(head) => {
            // Same unassign check: cherry-pick unassigned commits before syncing.
            if !mc_state.mv_rebase_unassign_commits.is_empty() {
                return continue_unassign_picks_after_merge(
                    &mc_path, &work_path, mc_state, head, logger,
                );
            }
            finish_merge_continuation(&mc_path, &work_path, &mc_state, &head, logger)
        }
        MergeOutcome::Conflict { at_index } => {
            let mut mc_state = mc_state;
            mc_state.pending_merge_index = start + at_index;
            repo_state::save_mc_state(&mc_path, &mc_state)?;
            let _logs = logger.drain();
            Err(PrioError::MergeConflict { mc_path })
        }
    }
}

fn finish_merge_continuation(
    mc_path: &Path,
    work_path: &Path,
    mc_state: &repo_state::McState,
    head: &str,
    logger: &mut Logger,
) -> Result<PrioResult, PrioError> {
    let config = repo_state::load_config(work_path)?;
    sync_work_clone(work_path, mc_path, &config.work_branch, head, logger)?;
    force_push_pending_sources(work_path, mc_state, logger);

    let mut state = repo_state::load_state(work_path)?;
    state.baseline_commit = head.to_string();
    state.merge_in_progress = false;
    repo_state::save_state(work_path, &state)?;

    repo_state::save_mc_state(
        mc_path,
        &repo_state::McState {
            merge_in_progress: false,
            pending_merge_branches: vec![],
            pending_merge_index: 0,
            default_branch: mc_state.default_branch.clone(),
            merge_branch: String::new(),
            ..Default::default()
        },
    )?;

    let logs = logger.drain();
    Ok(PrioResult::success("Merge continuation completed.", logs))
}

fn force_push_pending_sources(
    work_path: &Path,
    mc_state: &repo_state::McState,
    logger: &mut Logger,
) {
    if !mc_state.mv_rebase_force || mc_state.mv_pending_force_push_sources.is_empty() {
        return;
    }

    for (branch, is_pushed) in decode_unassign_sources(&mc_state.mv_pending_force_push_sources) {
        if is_pushed {
            let _ = runner::run_git(&["push", "--force", "origin", &branch], work_path, logger);
        }
    }
}

fn record_pending_force_push_source(
    mc_path: &Path,
    mc_state: &mut repo_state::McState,
    source_branch: &str,
    force: bool,
    source_is_pushed: bool,
) -> Result<(), PrioError> {
    if force && source_is_pushed {
        mc_state.mv_rebase_force = true;
        mc_state.mv_pending_force_push_sources = vec![format!("{source_branch}:true")];
    } else {
        mc_state.mv_rebase_force = false;
        mc_state.mv_pending_force_push_sources.clear();
    }
    repo_state::save_mc_state(mc_path, mc_state)
}

/// Decode a list of "branch:is_pushed" encoded source entries into typed tuples.
pub(crate) fn decode_unassign_sources(encoded: &[String]) -> Vec<(String, bool)> {
    encoded
        .iter()
        .filter_map(|s| {
            let mut parts = s.rsplitn(2, ':');
            let is_pushed = parts.next()? == "true";
            let branch = parts.next()?.to_string();
            Some((branch, is_pushed))
        })
        .collect()
}

/// After a cross-branch unassign apply-merge completes, cherry-pick the unassigned
/// commits on top of the merge result in prio-mc before syncing to the work clone.
///
/// `merge_head` is the merge result SHA — it becomes `state.baseline_commit` so the
/// cherry-picked commits appear above the baseline in `prio status`.
fn continue_unassign_picks_after_merge(
    mc_path: &Path,
    work_path: &Path,
    mut mc_state: repo_state::McState,
    merge_head: String,
    logger: &mut Logger,
) -> Result<PrioResult, PrioError> {
    let all_shas = mc_state.mv_rebase_unassign_commits.clone();
    let sources_encoded = mc_state.mv_rebase_unassign_all_sources.clone();
    let force = mc_state.mv_rebase_force;

    let config = repo_state::load_config(work_path)?;
    let mut state = repo_state::load_state(work_path)?;

    for (i, sha) in all_shas.iter().enumerate() {
        match cherry_pick_on_mc(mc_path, sha, logger)? {
            CherryPickOnMc::Applied | CherryPickOnMc::SkippedEmpty => {
                let new_sha = runner::run_git(&["rev-parse", "HEAD"], mc_path, logger)?
                    .trim()
                    .to_string();
                state.commit_map.insert(sha.clone(), new_sha.clone());
                state.commit_assignments.insert(new_sha, ".".to_string());
                repo_state::save_state(work_path, &state)?;
            }
            CherryPickOnMc::Conflict => {
                let remaining = all_shas[i + 1..].to_vec();
                state.mv_rebase_conflict = Some(repo_state::MvRebaseConflict {
                    source_branch: String::new(),
                    dest_branch: ".".to_string(),
                    conflicting_commit: sha.clone(),
                    source_is_pushed: false,
                    mc_path: mc_path.to_string_lossy().into_owned(),
                    phase: "unassign-pick".to_string(),
                });
                repo_state::save_state(work_path, &state)?;

                mc_state.mv_rebase_in_progress = true;
                mc_state.mv_rebase_phase = "unassign-pick".to_string();
                mc_state.mv_rebase_remaining_commits = remaining;
                mc_state.mv_rebase_unassign_baseline = merge_head.clone();
                mc_state.merge_in_progress = false;
                mc_state.pending_merge_branches.clear();
                mc_state.pending_merge_index = 0;
                repo_state::save_mc_state(mc_path, &mc_state)?;

                let logs = logger.drain();
                return Ok(PrioResult::warning(
                    format!(
                        "Cherry-pick conflict for {sha} while replaying unassigned \
                         commits onto the work branch in prio-mc.\n\
                         Resolve the conflict in: {}\n\
                         Then run: git -C \"{}\" commit --no-edit",
                        mc_path.display(),
                        mc_path.display(),
                    ),
                    logs,
                ));
            }
            CherryPickOnMc::Failed => {
                return Err(PrioError::Message(format!(
                    "Cherry-pick of {sha} failed in prio-mc. Run `prio abort` to clean up."
                )));
            }
        }
    }

    // All cherry-picks done. final_head is mc HEAD after cherry-picks.
    let final_head = runner::run_git(&["rev-parse", "HEAD"], mc_path, logger)?
        .trim()
        .to_string();

    finish_unassign_sync(
        mc_path,
        work_path,
        &config,
        &mut state,
        &merge_head,
        &final_head,
        &sources_encoded,
        force,
        logger,
    )
}

/// Final sync step for cross-branch unassign: sync source refs + work branch to work clone.
///
/// `merge_head` becomes `state.baseline_commit`; `final_head` (= merge_head + cherry-picks)
/// is where the work branch lands, so the unassigned commits appear above baseline.
pub(crate) fn finish_unassign_sync(
    mc_path: &Path,
    work_path: &Path,
    config: &repo_state::RepoConfig,
    state: &mut repo_state::RepoState,
    merge_head: &str,
    final_head: &str,
    sources_encoded: &[String],
    force: bool,
    logger: &mut Logger,
) -> Result<PrioResult, PrioError> {
    let mc_str = crate::util::path_arg(mc_path);

    // Sync all applied branch refs from mc to work (includes rebased source branches).
    for branch in &state.applied_branches {
        if branch == &state.default_branch {
            continue;
        }
        let mc_has = runner::run_git(&["rev-parse", "--verify", branch], mc_path, logger).is_ok();
        if mc_has {
            let _ = runner::run_git(
                &["fetch", &mc_str, &format!("+{branch}:{branch}")],
                work_path,
                logger,
            );
        }
    }

    // Force-push source branches to GitHub if -f was specified.
    let sources = decode_unassign_sources(sources_encoded);
    for (source, is_pushed) in &sources {
        if force && *is_pushed {
            runner::run_git(&["push", "--force", "origin", source], work_path, logger)?;
        }
    }

    // Sync work branch to final mc result (merge + unassigned cherry-picks on top).
    sync_work_clone(work_path, mc_path, &config.work_branch, final_head, logger)?;

    // baseline_commit = merge HEAD (before cherry-picks) so unassigned commits are above baseline.
    state.baseline_commit = merge_head.to_string();
    state.merge_in_progress = false;
    state.pending_merge_branches.clear();
    state.pending_merge_index = 0;
    state.mv_rebase_conflict = None;
    repo_state::save_state(work_path, state)?;

    repo_state::save_mc_state(
        mc_path,
        &repo_state::McState {
            merge_in_progress: false,
            pending_merge_branches: vec![],
            pending_merge_index: 0,
            default_branch: state.default_branch.clone(),
            merge_branch: String::new(),
            ..Default::default()
        },
    )?;

    let _ = repo_state::save_last_good(
        work_path,
        &repo_state::LastGoodState {
            commit_sha: state.baseline_commit.clone(),
            state: state.clone(),
        },
    );

    crate::services::suggestions::run_and_log(work_path, mc_path, logger)?;
    let logs = logger.drain();
    Ok(PrioResult::success(
        "Unassigned commits moved to work area.",
        logs,
    ))
}

/// Called from [`mc_post_commit`] when `mc_state.mv_rebase_in_progress` is true.
///
/// The user just committed a conflict resolution in prio-mc.  Continues the in-flight
/// `prio mv` operation depending on the phase stored in `mc_state.mv_rebase_phase`:
///
/// - `"dest"`          → finish remaining cherry-picks onto the destination branch, then
///   transition to source-rebase phase.
/// - `"source"`        → finish remaining cherry-picks for the source-branch rebase.
/// - `"unassign-source"` → finish source-rebase for cross-branch unassign, then do
///   apply merge + unassign cherry-picks in mc.
/// - `"unassign-pick"` → finish cherry-picking unassigned commits in mc, then sync to work.
fn mc_post_commit_rebase_continue(
    mc_path: &Path,
    mut mc_state: repo_state::McState,
    logger: &mut Logger,
) -> Result<PrioResult, PrioError> {
    let repo = user_config::load_repos()?
        .into_iter()
        .find(|r| r.mc_clone_path == mc_path.to_string_lossy())
        .ok_or_else(|| {
            PrioError::Message("Could not find work clone for this prio-mc repo.".into())
        })?;
    let work_path = PathBuf::from(&repo.path);
    let default_branch = mc_state.default_branch.clone();

    if mc_state.mv_rebase_phase == "dest" || mc_state.mv_rebase_phase == "cp" {
        // ── Dest / cp phase: finish remaining cherry-picks onto the destination branch ──
        let dest_branch = mc_state.mv_rebase_dest_branch.clone();
        let remaining_dest = mc_state.mv_rebase_remaining_dest.clone();
        let is_cp = mc_state.mv_rebase_phase == "cp";
        let source_branch = mc_state.mv_rebase_source_branch.clone();
        let source_shas_to_exclude: std::collections::HashSet<String> = mc_state
            .mv_rebase_source_shas_to_exclude
            .iter()
            .cloned()
            .collect();
        let source_is_pushed = mc_state.mv_rebase_source_is_pushed;
        let force = mc_state.mv_rebase_force;

        for (i, sha) in remaining_dest.iter().enumerate() {
            match cherry_pick_on_mc(mc_path, sha, logger)? {
                CherryPickOnMc::Applied | CherryPickOnMc::SkippedEmpty => {
                    let mut state = repo_state::load_state(&work_path)?;
                    let new_sha = runner::run_git(&["rev-parse", "HEAD"], mc_path, logger)?
                        .trim()
                        .to_string();
                    state.commit_map.insert(sha.clone(), new_sha.clone());
                    state
                        .commit_assignments
                        .insert(new_sha, dest_branch.clone());
                    repo_state::save_state(&work_path, &state)?;
                }
                CherryPickOnMc::Conflict => {
                    mc_state.mv_rebase_remaining_dest = remaining_dest[i + 1..].to_vec();
                    repo_state::save_mc_state(mc_path, &mc_state)?;
                    let mut state = repo_state::load_state(&work_path)?;
                    if let Some(ref mut c) = state.mv_rebase_conflict {
                        c.conflicting_commit = sha.clone();
                    }
                    repo_state::save_state(&work_path, &state)?;
                    let logs = logger.drain();
                    return Ok(PrioResult::warning(
                        format!(
                            "Another conflict at {sha} while finishing cherry-picks onto \
                             '{dest_branch}'. Run `prio status` for instructions."
                        ),
                        logs,
                    ));
                }
                CherryPickOnMc::Failed => {
                    return Err(PrioError::Message(format!(
                        "Cherry-pick of {sha} onto '{dest_branch}' failed. \
                         Run `prio abort` to clean up."
                    )));
                }
            }
        }

        if is_cp {
            let mc_str = crate::util::path_arg(mc_path);
            let dest_is_pushed = runner::run_git(
                &["rev-parse", "--verify", &format!("origin/{dest_branch}")],
                &work_path,
                logger,
            )
            .is_ok();
            if !dest_is_pushed {
                runner::run_git(
                    &["fetch", &mc_str, &format!("{dest_branch}:{dest_branch}")],
                    &work_path,
                    logger,
                )?;
            }
            return finalize_cp_after_dest(mc_path, &work_path, &default_branch, mc_state, logger);
        }

        mc_state.mv_rebase_phase = "source".to_string();
        mc_state.mv_rebase_remaining_dest.clear();

        let state_snapshot = repo_state::load_state(&work_path)?;
        // Compute effective excludes from the current state's commit_map so that both
        // the original SHA and its prio-mc cherry-pick counterpart are excluded.
        let source_shas_vec: Vec<String> = source_shas_to_exclude.into_iter().collect();
        let effective_source_excludes =
            crate::services::mv::effective_excludes_for_source(&source_shas_vec, &state_snapshot);
        match crate::services::mv::rebase_filter_shas(
            &work_path,
            mc_path,
            &source_branch,
            &effective_source_excludes,
            &state_snapshot,
            &default_branch,
            logger,
        )? {
            crate::services::mv::RebaseOutcome::Complete => {
                record_pending_force_push_source(
                    mc_path,
                    &mut mc_state,
                    &source_branch,
                    force,
                    source_is_pushed,
                )?;
                let result =
                    finalize_mv_rebase(mc_path, &work_path, &default_branch, mc_state, logger)?;
                if result.status != PrioStatus::Success {
                    return Ok(result);
                }
                if force && source_is_pushed {
                    runner::run_git(
                        &["push", "--force", "origin", &source_branch],
                        &work_path,
                        logger,
                    )?;
                }
                Ok(result)
            }
            crate::services::mv::RebaseOutcome::Conflict { at_sha, remaining } => {
                mc_state.mv_rebase_remaining_commits = remaining;
                repo_state::save_mc_state(mc_path, &mc_state)?;
                let mut state = repo_state::load_state(&work_path)?;
                if let Some(ref mut c) = state.mv_rebase_conflict {
                    c.phase = "source".to_string();
                    c.conflicting_commit = at_sha.clone();
                }
                repo_state::save_state(&work_path, &state)?;
                let logs = logger.drain();
                Ok(PrioResult::warning(
                    format!(
                        "Conflict at {at_sha} rebasing '{source_branch}'. \
                         Run `prio status` for instructions."
                    ),
                    logs,
                ))
            }
        }
    } else if mc_state.mv_rebase_phase == "unassign-source" {
        // ── Unassign-source phase: finish source rebase, then apply merge + cherry-picks ──
        let source_branch = mc_state.mv_rebase_source_branch.clone();
        let remaining = mc_state.mv_rebase_remaining_commits.clone();
        let all_unassign = mc_state.mv_rebase_unassign_commits.clone();
        let force = mc_state.mv_rebase_force;
        let all_sources_encoded = mc_state.mv_rebase_unassign_all_sources.clone();
        // remaining_source_entries[0] = current (conflicting) source; rest = queued.
        let remaining_source_entries = mc_state.mv_rebase_unassign_source_branches.clone();

        for (i, sha) in remaining.iter().enumerate() {
            match cherry_pick_on_mc(mc_path, sha, logger)? {
                CherryPickOnMc::Applied | CherryPickOnMc::SkippedEmpty => {}
                CherryPickOnMc::Conflict => {
                    mc_state.mv_rebase_remaining_commits = remaining[i + 1..].to_vec();
                    repo_state::save_mc_state(mc_path, &mc_state)?;
                    let mut state = repo_state::load_state(&work_path)?;
                    if let Some(ref mut c) = state.mv_rebase_conflict {
                        c.conflicting_commit = sha.clone();
                    }
                    repo_state::save_state(&work_path, &state)?;
                    let logs = logger.drain();
                    return Ok(PrioResult::warning(
                        format!(
                            "Another conflict at {sha} rebasing '{source_branch}'. \
                             Run `prio status` for instructions."
                        ),
                        logs,
                    ));
                }
                CherryPickOnMc::Failed => {
                    return Err(PrioError::Message(format!(
                        "Cherry-pick of {sha} failed. Run `prio abort` to clean up."
                    )));
                }
            }
        }

        // Current source rebase complete. Decode and process remaining queued sources.
        // remaining_source_entries[0] was the source we just finished — skip it.
        let next_sources: Vec<(String, bool)> = if remaining_source_entries.len() > 1 {
            decode_unassign_sources(&remaining_source_entries[1..])
        } else {
            vec![]
        };

        let mut state = repo_state::load_state(&work_path)?;
        let config = repo_state::load_config(&work_path)?;
        let effective_excludes =
            crate::services::mv::effective_excludes_for_source(&all_unassign, &state);

        for (idx, (source, source_is_pushed)) in next_sources.iter().enumerate() {
            match crate::services::mv::rebase_filter_shas(
                &work_path,
                mc_path,
                source,
                &effective_excludes,
                &state,
                &default_branch,
                logger,
            )? {
                crate::services::mv::RebaseOutcome::Complete => {}
                crate::services::mv::RebaseOutcome::Conflict {
                    at_sha,
                    remaining: rem,
                } => {
                    state.mv_rebase_conflict = Some(repo_state::MvRebaseConflict {
                        source_branch: source.clone(),
                        dest_branch: ".".to_string(),
                        conflicting_commit: at_sha.clone(),
                        source_is_pushed: *source_is_pushed,
                        mc_path: mc_path.to_string_lossy().into_owned(),
                        phase: "unassign-source".to_string(),
                    });
                    repo_state::save_state(&work_path, &state)?;

                    let encoded_remaining: Vec<String> = next_sources[idx..]
                        .iter()
                        .map(|(b, p)| format!("{}:{}", b, if *p { "true" } else { "false" }))
                        .collect();
                    mc_state.mv_rebase_source_branch = source.clone();
                    mc_state.mv_rebase_source_is_pushed = *source_is_pushed;
                    mc_state.mv_rebase_remaining_commits = rem;
                    mc_state.mv_rebase_unassign_source_branches = encoded_remaining;
                    repo_state::save_mc_state(mc_path, &mc_state)?;

                    let logs = logger.drain();
                    return Ok(PrioResult::warning(
                        format!(
                            "Cherry-pick conflict while rebasing '{source}': \
                             commit {at_sha} conflicts. \
                             Run `prio status` for instructions."
                        ),
                        logs,
                    ));
                }
            }
        }

        // All source rebases complete. Proceed to apply merge + cherry-picks.
        state.mv_rebase_conflict = None;
        repo_state::save_state(&work_path, &state)?;

        do_unassign_apply_and_picks(
            &work_path,
            mc_path,
            &config,
            &mut state,
            &all_unassign,
            &all_sources_encoded,
            force,
            logger,
        )
    } else if mc_state.mv_rebase_phase == "unassign-pick" {
        // ── Unassign-pick phase: finish cherry-picking unassigned commits in mc, then sync ──
        let remaining = mc_state.mv_rebase_remaining_commits.clone();
        let merge_head = mc_state.mv_rebase_unassign_baseline.clone();
        let sources_encoded = mc_state.mv_rebase_unassign_all_sources.clone();
        let force = mc_state.mv_rebase_force;

        let config = repo_state::load_config(&work_path)?;
        let mut state = repo_state::load_state(&work_path)?;

        for (i, sha) in remaining.iter().enumerate() {
            match cherry_pick_on_mc(mc_path, sha, logger)? {
                CherryPickOnMc::Applied | CherryPickOnMc::SkippedEmpty => {
                    let new_sha = runner::run_git(&["rev-parse", "HEAD"], mc_path, logger)?
                        .trim()
                        .to_string();
                    state.commit_map.insert(sha.clone(), new_sha.clone());
                    state.commit_assignments.insert(new_sha, ".".to_string());
                    repo_state::save_state(&work_path, &state)?;
                }
                CherryPickOnMc::Conflict => {
                    mc_state.mv_rebase_remaining_commits = remaining[i + 1..].to_vec();
                    repo_state::save_mc_state(mc_path, &mc_state)?;
                    if let Some(ref mut c) = state.mv_rebase_conflict {
                        c.conflicting_commit = sha.clone();
                    }
                    repo_state::save_state(&work_path, &state)?;
                    let logs = logger.drain();
                    return Ok(PrioResult::warning(
                        format!(
                            "Another conflict at {sha} while replaying unassigned commits. \
                             Run `prio status` for instructions."
                        ),
                        logs,
                    ));
                }
                CherryPickOnMc::Failed => {
                    return Err(PrioError::Message(format!(
                        "Cherry-pick of {sha} failed. Run `prio abort` to clean up."
                    )));
                }
            }
        }

        let final_head = runner::run_git(&["rev-parse", "HEAD"], mc_path, logger)?
            .trim()
            .to_string();

        finish_unassign_sync(
            mc_path,
            &work_path,
            &config,
            &mut state,
            &merge_head,
            &final_head,
            &sources_encoded,
            force,
            logger,
        )
    } else {
        // ── Source phase: finish remaining cherry-picks for the source-branch rebase ──
        let source_branch = mc_state.mv_rebase_source_branch.clone();
        let remaining = mc_state.mv_rebase_remaining_commits.clone();
        let source_is_pushed = mc_state.mv_rebase_source_is_pushed;
        let force = mc_state.mv_rebase_force;

        for (i, sha) in remaining.iter().enumerate() {
            match cherry_pick_on_mc(mc_path, sha, logger)? {
                CherryPickOnMc::Applied | CherryPickOnMc::SkippedEmpty => {}
                CherryPickOnMc::Conflict => {
                    mc_state.mv_rebase_remaining_commits = remaining[i + 1..].to_vec();
                    repo_state::save_mc_state(mc_path, &mc_state)?;
                    let mut state = repo_state::load_state(&work_path)?;
                    if let Some(ref mut c) = state.mv_rebase_conflict {
                        c.conflicting_commit = sha.clone();
                    }
                    repo_state::save_state(&work_path, &state)?;
                    let logs = logger.drain();
                    return Ok(PrioResult::warning(
                        format!(
                            "Another conflict at {sha} rebasing '{source_branch}'. \
                             Run `prio status` for instructions."
                        ),
                        logs,
                    ));
                }
                CherryPickOnMc::Failed => {
                    return Err(PrioError::Message(format!(
                        "Cherry-pick of {sha} failed. Run `prio abort` to clean up."
                    )));
                }
            }
        }

        // Source rebase complete. Re-apply before syncing or force-pushing.
        record_pending_force_push_source(
            mc_path,
            &mut mc_state,
            &source_branch,
            force,
            source_is_pushed,
        )?;
        let result = finalize_mv_rebase(mc_path, &work_path, &default_branch, mc_state, logger)?;
        if result.status != PrioStatus::Success {
            return Ok(result);
        }
        if force && source_is_pushed {
            runner::run_git(
                &["push", "--force", "origin", &source_branch],
                &work_path,
                logger,
            )?;
        }
        Ok(result)
    }
}

/// Complete a `prio cp` after destination cherry-picks finish (including post-conflict).
fn finalize_cp_after_dest(
    mc_path: &Path,
    work_path: &Path,
    default_branch: &str,
    mut mc_state: repo_state::McState,
    logger: &mut Logger,
) -> Result<PrioResult, PrioError> {
    let dest_branch = mc_state.mv_rebase_dest_branch.clone();
    mc_state.mv_rebase_in_progress = false;
    mc_state.mv_rebase_remaining_dest.clear();
    mc_state.mv_rebase_dest_branch.clear();
    mc_state.mv_rebase_phase.clear();
    repo_state::save_mc_state(mc_path, &mc_state)?;

    let mut state = repo_state::load_state(work_path)?;
    state.mv_rebase_conflict = None;
    repo_state::save_state(work_path, &state)?;

    let config = repo_state::load_config(work_path)?;
    reset_mc_to_default(mc_path, work_path, default_branch, logger)?;

    let dest_tip = branch_tip_ref(&dest_branch, work_path, logger).ok();
    let work_branch_tip = runner::run_git(&["rev-parse", "HEAD"], work_path, logger)
        .map(|s| s.trim().to_string())
        .ok();
    let dest_already_in_work = match (&dest_tip, &work_branch_tip) {
        (Some(dt), Some(wt)) => {
            runner::run_git(&["merge-base", "--is-ancestor", dt, wt], work_path, logger).is_ok()
        }
        _ => false,
    };

    if !dest_already_in_work && !state.applied_branches.is_empty() {
        let applied = state.applied_branches.clone();
        return execute_apply_merge(
            work_path,
            mc_path,
            &mut state,
            &config,
            applied.clone(),
            applied,
            logger,
        );
    }

    crate::services::suggestions::run_and_log(work_path, mc_path, logger)?;
    let logs = logger.drain();
    Ok(PrioResult::success(
        format!("Copy to '{dest_branch}' completed."),
        logs,
    ))
}

/// Complete a cross-branch unassign after all source-branch rebases have finished.
///
/// Merges all applied branches in prio-mc, cherry-picks the unassigned commits on top of
/// the merge result, then syncs everything to the work clone atomically.
///
/// `sources_encoded_all` contains ALL source branches (format: `"branch:is_pushed"`), used
/// for force-pushing rebased branches and syncing their refs to the work clone at the end.
pub(crate) fn do_unassign_apply_and_picks(
    work_path: &Path,
    mc_path: &Path,
    config: &repo_state::RepoConfig,
    state: &mut repo_state::RepoState,
    unassign_shas: &[String],
    sources_encoded_all: &[String],
    force: bool,
    logger: &mut Logger,
) -> Result<PrioResult, PrioError> {
    reset_mc_to_default(mc_path, work_path, &state.default_branch, logger)?;

    let applied = state.applied_branches.clone();
    let order = resolve_optimal_order(&applied, mc_path, work_path, &state.default_branch, logger)?;

    let merge_branch_name = if !order.is_empty() {
        let name = format!(
            "prio-mc/{}",
            order
                .iter()
                .map(|b| b.replace('/', "-"))
                .collect::<Vec<_>>()
                .join("+")
        );
        runner::run_git_no_hooks(&["checkout", "-b", &name], mc_path, logger)?;
        name
    } else {
        String::new()
    };

    let merge_head =
        match merge_branches_in_mc(mc_path, &order, &state.default_branch, work_path, logger)? {
            MergeOutcome::Complete(head) => head,
            MergeOutcome::Conflict { at_index } => {
                state.applied_branches = applied.clone();
                state.merge_in_progress = true;
                state.pending_merge_branches = order.clone();
                state.pending_merge_index = at_index;
                repo_state::save_state(work_path, state)?;

                repo_state::save_mc_state(
                    mc_path,
                    &repo_state::McState {
                        merge_in_progress: true,
                        pending_merge_branches: order.clone(),
                        pending_merge_index: at_index,
                        default_branch: state.default_branch.clone(),
                        merge_branch: merge_branch_name.clone(),
                        mv_rebase_unassign_commits: unassign_shas.to_vec(),
                        mv_rebase_unassign_all_sources: sources_encoded_all.to_vec(),
                        mv_rebase_force: force,
                        ..Default::default()
                    },
                )?;

                let incoming = order.get(at_index).cloned().unwrap_or_default();
                let already_merged = order[..at_index].to_vec();
                let base_desc = if already_merged.is_empty() {
                    state.default_branch.clone()
                } else {
                    format!("{} + {}", state.default_branch, already_merged.join(" + "))
                };
                let logs = logger.drain();
                return Ok(PrioResult::warning(
                    format!(
                        "Merge conflict in prio-mc: merging {incoming} into ({base_desc}).\n\
                     Resolve conflicts in: {mc_path}\n\
                     Branch: {merge_branch_name}\n\
                     Then run: git -C \"{mc_path}\" commit --no-edit",
                        mc_path = mc_path.display(),
                    ),
                    logs,
                ));
            }
        };

    // Merge complete. Cherry-pick unassigned commits in mc on top of the merge result.
    for (i, sha) in unassign_shas.iter().enumerate() {
        match cherry_pick_on_mc(mc_path, sha, logger)? {
            CherryPickOnMc::Applied | CherryPickOnMc::SkippedEmpty => {
                let new_sha = runner::run_git(&["rev-parse", "HEAD"], mc_path, logger)?
                    .trim()
                    .to_string();
                state.commit_map.insert(sha.clone(), new_sha.clone());
                state.commit_assignments.insert(new_sha, ".".to_string());
                repo_state::save_state(work_path, state)?;
            }
            CherryPickOnMc::Conflict => {
                let remaining = unassign_shas[i + 1..].to_vec();
                state.mv_rebase_conflict = Some(repo_state::MvRebaseConflict {
                    source_branch: String::new(),
                    dest_branch: ".".to_string(),
                    conflicting_commit: sha.clone(),
                    source_is_pushed: false,
                    mc_path: mc_path.to_string_lossy().into_owned(),
                    phase: "unassign-pick".to_string(),
                });
                repo_state::save_state(work_path, state)?;

                repo_state::save_mc_state(
                    mc_path,
                    &repo_state::McState {
                        mv_rebase_in_progress: true,
                        mv_rebase_phase: "unassign-pick".to_string(),
                        mv_rebase_remaining_commits: remaining,
                        mv_rebase_unassign_baseline: merge_head.clone(),
                        mv_rebase_unassign_all_sources: sources_encoded_all.to_vec(),
                        mv_rebase_unassign_commits: unassign_shas.to_vec(),
                        mv_rebase_force: force,
                        default_branch: state.default_branch.clone(),
                        ..Default::default()
                    },
                )?;

                let logs = logger.drain();
                return Ok(PrioResult::warning(
                    format!(
                        "Cherry-pick conflict for {sha} while replaying unassigned \
                         commits onto the work branch in prio-mc.\n\
                         Resolve the conflict in: {mc_path}\n\
                         Then run: git -C \"{mc_path}\" commit --no-edit",
                        mc_path = mc_path.display(),
                    ),
                    logs,
                ));
            }
            CherryPickOnMc::Failed => {
                return Err(PrioError::Message(format!(
                    "Cherry-pick of {sha} failed in prio-mc. Run `prio abort` to clean up."
                )));
            }
        }
    }

    let final_head = runner::run_git(&["rev-parse", "HEAD"], mc_path, logger)?
        .trim()
        .to_string();

    finish_unassign_sync(
        mc_path,
        work_path,
        config,
        state,
        &merge_head,
        &final_head,
        sources_encoded_all,
        force,
        logger,
    )
}

/// Clear mv rebase state, re-apply branches, return success.
fn finalize_mv_rebase(
    mc_path: &Path,
    work_path: &Path,
    default_branch: &str,
    mut mc_state: repo_state::McState,
    logger: &mut Logger,
) -> Result<PrioResult, PrioError> {
    mc_state.mv_rebase_in_progress = false;
    mc_state.mv_rebase_remaining_commits.clear();
    mc_state.mv_rebase_remaining_dest.clear();
    mc_state.mv_rebase_source_branch.clear();
    mc_state.mv_rebase_dest_branch.clear();
    mc_state.mv_rebase_source_shas_to_exclude.clear();
    mc_state.mv_rebase_phase.clear();
    repo_state::save_mc_state(mc_path, &mc_state)?;

    let mut state = repo_state::load_state(work_path)?;
    state.mv_rebase_conflict = None;
    repo_state::save_state(work_path, &state)?;

    let config = repo_state::load_config(work_path)?;

    // Preserve local prio-mc refs created by the mv continuation. Resetting here
    // would wipe the freshly rebased source/dest refs before the apply merge can use them.
    let _ = runner::run_git(&["cherry-pick", "--abort"], mc_path, logger);
    let _ = runner::run_git(&["merge", "--abort"], mc_path, logger);
    runner::run_git(&["checkout", default_branch], mc_path, logger)?;

    let applied = state.applied_branches.clone();
    execute_apply_merge(
        work_path,
        mc_path,
        &mut state,
        &config,
        applied.clone(),
        applied,
        logger,
    )
}

pub fn work_post_commit(
    repo_path: Option<PathBuf>,
    logger: &mut Logger,
) -> Result<PrioResult, PrioError> {
    let repo_path = resolve_repo_path(repo_path)?;
    let head = runner::run_git(&["rev-parse", "HEAD"], &repo_path, logger)?
        .trim()
        .to_string();
    let state = repo_state::load_state(&repo_path)?;
    repo_state::save_last_good(
        &repo_path,
        &repo_state::LastGoodState {
            commit_sha: head,
            state,
        },
    )?;
    let logs = logger.drain();
    Ok(PrioResult::success("Recorded last known good state.", logs))
}
