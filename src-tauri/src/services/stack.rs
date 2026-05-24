use std::path::PathBuf;

use crate::error::PrioError;
use crate::git::runner;
use crate::logger::Logger;
use crate::result::PrioResult;
use crate::services::apply;
use crate::services::common::{assert_on_work_branch, mc_path_for_repo, parse_dependencies, resolve_repo_path};
use crate::services::suggestions;
use crate::storage::repo_state;

pub fn run_stack(
    repo_path: Option<PathBuf>,
    dependencies: String,
    branch: String,
    logger: &mut Logger,
) -> Result<PrioResult, PrioError> {
    let repo_path = resolve_repo_path(repo_path)?;
    assert_on_work_branch(&repo_path, logger)?;

    let branch = runner::resolve_branch_ref(&branch, &repo_path, logger)?;
    let deps = parse_dependencies(&dependencies);
    let dep_str = deps.join("+");

    let mut state = repo_state::load_state(&repo_path)?;
    state.stacks.retain(|s| s.branch != branch);
    state.stacks.push(repo_state::StackEntry {
        dependency: dep_str,
        branch: branch.clone(),
    });
    repo_state::save_state(&repo_path, &state)?;

    apply::run(Some(repo_path.clone()), vec![], false, logger)?;

    let mc_path = mc_path_for_repo(&repo_path, None)?;
    suggestions::run_and_log(&repo_path, &mc_path, logger)?;

    let logs = logger.drain();
    Ok(PrioResult::success(
        format!("Stacked {branch} after {dependencies}."),
        logs,
    ))
}

pub fn run_unstack(
    repo_path: Option<PathBuf>,
    branch: String,
    logger: &mut Logger,
) -> Result<PrioResult, PrioError> {
    let repo_path = resolve_repo_path(repo_path)?;
    assert_on_work_branch(&repo_path, logger)?;

    let branch = runner::resolve_branch_ref(&branch, &repo_path, logger)?;
    let mut state = repo_state::load_state(&repo_path)?;
    state.stacks.retain(|s| s.branch != branch);
    repo_state::save_state(&repo_path, &state)?;

    let mc_path = mc_path_for_repo(&repo_path, None)?;
    suggestions::run_and_log(&repo_path, &mc_path, logger)?;

    let logs = logger.drain();
    Ok(PrioResult::success(format!("Unstacked {branch}."), logs))
}
