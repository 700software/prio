use std::path::PathBuf;

use crate::error::PrioError;
use crate::git::runner;
use crate::logger::Logger;
use crate::result::PrioResult;
use crate::services::common::{assert_on_work_branch, mc_path_for_repo, resolve_repo_path};
use crate::services::suggestions;

pub fn run(
    repo_path: Option<PathBuf>,
    branch: String,
    logger: &mut Logger,
) -> Result<PrioResult, PrioError> {
    let repo_path = resolve_repo_path(repo_path)?;
    assert_on_work_branch(&repo_path, logger)?;
    runner::ensure_gh_authenticated(&repo_path, logger)?;

    let branch = runner::resolve_branch_ref(&branch, &repo_path, logger)?;
    runner::run_git(&["push", "-u", "origin", &branch], &repo_path, logger)?;

    let mc_path = mc_path_for_repo(&repo_path, None)?;
    suggestions::run_and_log(&repo_path, &mc_path, logger)?;

    let logs = logger.drain();
    Ok(PrioResult::success(format!("Pushed branch {branch}."), logs))
}
