use std::collections::VecDeque;
use std::path::PathBuf;

use crate::error::PrioError;
use crate::git::runner;
use crate::logger::Logger;
use crate::result::PrioResult;
use crate::services::common::{assert_on_work_branch, mc_path_for_repo, resolve_repo_path};
use crate::services::suggestions;
use crate::storage::repo_state;

pub fn run(
    repo_path: Option<PathBuf>,
    branch: String,
    push_deps: bool,
    logger: &mut Logger,
) -> Result<PrioResult, PrioError> {
    let repo_path = resolve_repo_path(repo_path)?;
    assert_on_work_branch(&repo_path, logger)?;
    runner::ensure_gh_authenticated(&repo_path, logger)?;

    let branch = runner::resolve_branch_ref(&branch, &repo_path, logger)?;
    let mc_path = mc_path_for_repo(&repo_path, None)?;
    let state = repo_state::load_state(&repo_path)?;

    // ── Stack dependency enforcement ──────────────────────────────────────────
    // Publishing a stacked branch before its dependency is pushed would force
    // reviewers to apply the dependency themselves and would make the PR diff
    // misleading.  Collect any transitive deps that are not yet pushed and either
    // block or push them first depending on the -p flag.
    let unpushed_deps = collect_unpushed_deps(&branch, &state, &repo_path, logger);

    if !unpushed_deps.is_empty() {
        if !push_deps {
            let dep_list = unpushed_deps.join(", ");
            let logs = logger.drain();
            return Ok(PrioResult::failure(
                format!(
                    "Cannot push '{branch}': stacked dependency branch(es) \
                     [{dep_list}] have not been pushed yet.\n\
                     Use -p / --push-deps to push all dependency branches first."
                ),
                logs,
            ));
        }

        // Push each unpushed dependency in dependency order (topological, deps first).
        for dep in &unpushed_deps {
            logger.info(format!("Pushing dependency branch: {dep}"));
            push_single(&repo_path, &mc_path, dep, &state, logger)?;
        }
    }

    // ── Push the target branch ────────────────────────────────────────────────
    push_single(&repo_path, &mc_path, &branch, &state, logger)?;

    suggestions::run_and_log(&repo_path, &mc_path, logger)?;

    let logs = logger.drain();
    Ok(PrioResult::success(
        format!("Pushed branch {branch}."),
        logs,
    ))
}

/// Collect transitive stack dependencies of `branch` that have not yet been
/// pushed to origin, in topological order (outermost dependency first).
fn collect_unpushed_deps(
    branch: &str,
    state: &repo_state::RepoState,
    repo_path: &std::path::Path,
    _logger: &mut Logger,
) -> Vec<String> {
    // BFS / topological traversal through the stack graph.
    let mut visited = std::collections::HashSet::new();
    let mut ordered: Vec<String> = Vec::new();
    let mut queue: VecDeque<String> = VecDeque::new();

    // Seed with direct dependencies.
    if let Some(entry) = state.stacks.iter().find(|s| s.branch == branch) {
        for dep in &entry.dependencies {
            if visited.insert(dep.clone()) {
                queue.push_back(dep.clone());
            }
        }
    }

    while let Some(current) = queue.pop_front() {
        // Enqueue transitive deps (depth-first via queue prepend would be
        // cleaner but BFS is fine for the shallow stacks prio supports).
        if let Some(entry) = state.stacks.iter().find(|s| s.branch == current) {
            for dep in &entry.dependencies {
                if visited.insert(dep.clone()) {
                    queue.push_front(dep.clone()); // prepend so deeper deps sort first
                }
            }
        }
        ordered.push(current);
    }

    // Filter to those not yet pushed.
    ordered
        .into_iter()
        .filter(|dep| {
            runner::run_git(
                &["rev-parse", "--verify", &format!("origin/{dep}")],
                repo_path,
                &mut Logger::Cli,
            )
            .is_err()
        })
        .collect()
}

/// Sync a single branch from prio-mc into WORK if needed, then `git push -u origin`.
fn push_single(
    repo_path: &std::path::Path,
    mc_path: &std::path::Path,
    branch: &str,
    state: &repo_state::RepoState,
    logger: &mut Logger,
) -> Result<(), PrioError> {
    let origin_exists = runner::run_git(
        &["rev-parse", "--verify", &format!("origin/{branch}")],
        repo_path,
        logger,
    )
    .is_ok();

    let default_origin_ref = format!("origin/{}", state.default_branch);
    let local_has_own_commits = runner::run_git(
        &[
            "log",
            "--oneline",
            &format!("{default_origin_ref}..{branch}"),
            "--",
        ],
        repo_path,
        logger,
    )
    .map(|out| !out.trim().is_empty())
    .unwrap_or(false);

    if !origin_exists && !local_has_own_commits {
        // Branch is local-only and has no unique commits in WORK's local ref.
        // Sync from prio-mc's version before pushing.
        let mc_sha = runner::run_git(&["rev-parse", "--verify", branch], mc_path, logger)
            .ok()
            .map(|s| s.trim().to_string());
        let default_sha = runner::run_git(&["rev-parse", &default_origin_ref], repo_path, logger)
            .ok()
            .map(|s| s.trim().to_string());

        let mc_tip_is_ahead = mc_sha.is_some() && mc_sha != default_sha;
        if mc_tip_is_ahead {
            let mc_str = crate::util::path_arg(mc_path);
            runner::run_git(
                &["fetch", &mc_str, &format!("{branch}:{branch}")],
                repo_path,
                logger,
            )?;
        }
    }

    runner::run_git(&["push", "-u", "origin", branch], repo_path, logger)?;
    Ok(())
}
