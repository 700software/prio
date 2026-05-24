//! Apply/unapply and the shared merge pipeline.
//!
//! **Design:** all merges and cherry-picks run in the `*-prio-mc` clone; the work clone
//! only receives finished results via [`sync_work_clone`] (`git reset --hard`). See `AGENTS.md`.

use std::path::{Path, PathBuf};

use crate::error::PrioError;
use crate::util;
use crate::git::runner;
use crate::logger::Logger;
use crate::result::PrioResult;
use crate::services::common::{assert_on_work_branch, mc_path_for_repo, resolve_repo_path};
use crate::storage::{repo_state, user_config};

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

    let mut applied = state.applied_branches.clone();
    for b in resolved {
        if unapply {
            applied.retain(|x| x != &b);
        } else if !applied.contains(&b) {
            applied.push(b);
        }
    }

    reset_mc_to_default(&mc_path, &state.default_branch, logger)?;
    let order = resolve_optimal_order(&applied, &mc_path, &state.default_branch, logger)?;

    // Create a named branch in prio-mc so the user can see exactly what is being merged
    // and can find the conflict location easily (e.g. `prio-mc/feature-a+feature-b`).
    let merge_branch = if !order.is_empty() {
        let name = format!(
            "prio-mc/{}",
            order.iter().map(|b| b.replace('/', "-")).collect::<Vec<_>>().join("+")
        );
        runner::run_git_no_hooks(&["checkout", "-b", &name], &mc_path, logger)?;
        name
    } else {
        String::new()
    };

    match merge_branches_in_mc(
        &mc_path,
        &order,
        &state.default_branch,
        &repo_path,
        logger,
    )? {
        MergeOutcome::Complete(head) => {
            sync_work_clone(&repo_path, &mc_path, &config.work_branch, &head, logger)?;
            state.applied_branches = applied;
            state.baseline_commit = head;
            state.merge_in_progress = false;
            state.pending_merge_branches.clear();
            state.pending_merge_index = 0;
            repo_state::save_state(&repo_path, &state)?;

            repo_state::save_mc_state(&mc_path, &repo_state::McState {
                merge_in_progress: false,
                pending_merge_branches: vec![],
                pending_merge_index: 0,
                default_branch: state.default_branch.clone(),
                merge_branch: String::new(),
            })?;

            crate::services::suggestions::run_and_log(&repo_path, &mc_path, logger)?;

            let logs = logger.drain();
            Ok(PrioResult::success("Apply completed.", logs))
        }
        MergeOutcome::Conflict { at_index } => {
            state.applied_branches = applied.clone();
            state.merge_in_progress = true;
            state.pending_merge_branches = order.clone();
            state.pending_merge_index = at_index;
            repo_state::save_state(&repo_path, &state)?;

            let incoming = order.get(at_index).cloned().unwrap_or_default();
            let already_merged = order[..at_index].to_vec();
            let base_desc = if already_merged.is_empty() {
                state.default_branch.clone()
            } else {
                format!("{} + {}", state.default_branch, already_merged.join(" + "))
            };

            repo_state::save_mc_state(&mc_path, &repo_state::McState {
                merge_in_progress: true,
                pending_merge_branches: order,
                pending_merge_index: at_index,
                default_branch: state.default_branch,
                merge_branch: merge_branch.clone(),
            })?;

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

/// Prefer `origin/<branch>` when it exists; otherwise merge the local branch (e.g. setup from an unpushed branch).
fn merge_ref_for_branch(branch: &str, mc_path: &Path, logger: &mut Logger) -> String {
    let remote = format!("origin/{branch}");
    if runner::run_git(&["rev-parse", "--verify", &remote], mc_path, logger).is_ok() {
        remote
    } else {
        branch.to_string()
    }
}

/// Reset the prio-mc clone to a clean `origin/<default_branch>` (start of a merge operation).
///
/// Aborts any in-progress merge first so this is safe to call during a conflict (e.g. `prio unapply`).
/// Also removes any leftover `prio-mc/*` branches from previous apply runs.
pub fn reset_mc_to_default(mc_path: &Path, default_branch: &str, logger: &mut Logger) -> Result<(), PrioError> {
    // Abort any in-progress merge (no-op if there isn't one).
    let _ = runner::run_git(&["merge", "--abort"], mc_path, logger);
    runner::run_git(&["fetch", "origin"], mc_path, logger)?;
    runner::run_git(&["checkout", default_branch], mc_path, logger)?;
    runner::run_git(
        &["reset", "--hard", &format!("origin/{default_branch}")],
        mc_path,
        logger,
    )?;
    // Clean up leftover prio-mc merge branches from previous runs.
    if let Ok(out) = runner::run_git(&["branch", "--list", "prio-mc/*"], mc_path, logger) {
        for b in out.lines().map(|l| l.trim().trim_start_matches('*').trim()) {
            if !b.is_empty() {
                let _ = runner::run_git(&["branch", "-D", b], mc_path, logger);
            }
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
    let mut head = runner::run_git(
        &["rev-parse", "HEAD"],
        mc_path,
        logger,
    )?
    .trim()
    .to_string();

    for (i, branch) in order.iter().enumerate() {
        let merge_ref = merge_ref_for_branch(branch, mc_path, logger);
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
        let score = score_merge_order(&perm, mc_path, default_branch, logger)?;
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
    default_branch: &str,
    logger: &mut Logger,
) -> Result<usize, PrioError> {
    let trial = format!("prio-trial-{}", util::now_ms());
    reset_mc_to_default(mc_path, default_branch, logger)?;
    runner::run_git(&["checkout", "-b", &trial], mc_path, logger)?;

    let mut conflicts = 0;
    for branch in order {
        let merge_ref = merge_ref_for_branch(branch, mc_path, logger);
        let result = runner::run_git_no_hooks(
            &["merge", "--no-commit", &merge_ref],
            mc_path,
            logger,
        );
        if result.is_err() {
            conflicts += count_conflict_markers(mc_path, logger)?;
            let _ = runner::run_git(&["merge", "--abort"], mc_path, logger);
        } else {
            let _ = runner::run_git(&["merge", "--abort"], mc_path, logger);
        }
    }

    reset_mc_to_default(mc_path, default_branch, logger)?;
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

pub fn mc_post_commit(mc_path: Option<PathBuf>, logger: &mut Logger) -> Result<PrioResult, PrioError> {
    let mc_path = mc_path.unwrap_or_else(|| {
        std::env::current_dir().expect("cwd")
    });
    let mc_state = repo_state::load_mc_state(&mc_path)?;
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
        let config = repo_state::load_config(&work_path)?;
        sync_work_clone(&work_path, &mc_path, &config.work_branch, &head, logger)?;

        let mut state = repo_state::load_state(&work_path)?;
        state.baseline_commit = head;
        state.merge_in_progress = false;
        repo_state::save_state(&work_path, &state)?;

        let logs = logger.drain();
        return Ok(PrioResult::success("Merge continuation completed.", logs));
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
            let config = repo_state::load_config(&work_path)?;
            sync_work_clone(&work_path, &mc_path, &config.work_branch, &head, logger)?;
            let mut state = repo_state::load_state(&work_path)?;
            state.baseline_commit = head;
            state.merge_in_progress = false;
            repo_state::save_state(&work_path, &state)?;
            let logs = logger.drain();
            Ok(PrioResult::success("Merge continuation completed.", logs))
        }
        MergeOutcome::Conflict { at_index } => {
            let mut mc_state = mc_state;
            mc_state.pending_merge_index = start + at_index;
            repo_state::save_mc_state(&mc_path, &mc_state)?;
            let logs = logger.drain();
            Err(PrioError::MergeConflict { mc_path })
        }
    }
}

pub fn work_post_commit(repo_path: Option<PathBuf>, logger: &mut Logger) -> Result<PrioResult, PrioError> {
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
