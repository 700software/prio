use std::path::{Path, PathBuf};

use crate::error::PrioError;
use crate::git::runner;
use crate::logger::Logger;
use crate::storage::{repo_state, user_config};
use crate::util;

pub fn resolve_repo_path(repo_path: Option<PathBuf>) -> Result<PathBuf, PrioError> {
    let path = repo_path.unwrap_or_else(|| {
        std::env::current_dir().expect("current directory should exist")
    });
    let abs = util::absolute_path(path);
    runner::ensure_git_work_tree(&abs, &mut Logger::Cli)?;
    Ok(abs)
}

pub fn mc_path_for_repo(repo_path: &Path, mc_override: Option<PathBuf>) -> Result<PathBuf, PrioError> {
    if let Some(p) = mc_override {
        return Ok(p);
    }
    if let Some(record) = user_config::find_repo_by_path(repo_path)? {
        return Ok(PathBuf::from(record.mc_clone_path));
    }
    let name = repo_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("repo");
    Ok(util::absolute_path(
        repo_path
            .parent()
            .unwrap_or(repo_path)
            .join(format!("{name}-prio-mc")),
    ))
}

pub fn assert_on_work_branch(repo_path: &Path, logger: &mut Logger) -> Result<(), PrioError> {
    let config = repo_state::load_config(repo_path)?;
    let current = runner::current_branch(repo_path, logger)?;
    if current != config.work_branch {
        return Err(PrioError::Inactive {
            work_branch: config.work_branch,
            current_branch: current,
        });
    }
    Ok(())
}

pub fn default_mc_path(repo_path: &Path) -> PathBuf {
    let name = repo_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("repo");
    repo_path
        .parent()
        .unwrap_or(repo_path)
        .join(format!("{name}-prio-mc"))
}

pub fn parse_dependencies(spec: &str) -> Vec<String> {
    spec.split('+')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

pub fn expand_commit_args(commits: &[String], repo_path: &Path, logger: &mut Logger) -> Result<Vec<String>, PrioError> {
    let mut out = Vec::new();
    for c in commits {
        if c.contains("..") {
            let shas = runner::run_git(
                &["rev-list", "--reverse", c],
                repo_path,
                logger,
            )?;
            for sha in shas.lines() {
                if !sha.is_empty() {
                    out.push(sha.to_string());
                }
            }
        } else {
            out.push(c.clone());
        }
    }
    Ok(out)
}

/// True when `sha` is on the work branch after the current baseline.
pub fn is_work_area_commit(
    sha: &str,
    baseline: &str,
    repo_path: &Path,
    logger: &mut Logger,
) -> Result<bool, PrioError> {
    let out = runner::run_git(
        &["merge-base", "--is-ancestor", baseline, sha],
        repo_path,
        logger,
    );
    Ok(out.is_ok())
}

pub fn assign_work_commits_to_branch(
    repo_path: &Path,
    branch: &str,
    logger: &mut Logger,
) -> Result<(), PrioError> {
    let mut state = repo_state::load_state(repo_path)?;
    let log = runner::run_git(
        &[
            "log",
            &format!("{}..HEAD", state.baseline_commit),
            "--format=%H",
        ],
        repo_path,
        logger,
    )?;
    for sha in log.lines().map(str::trim).filter(|s| !s.is_empty()) {
        state
            .commit_assignments
            .insert(sha.to_string(), branch.to_string());
    }
    repo_state::save_state(repo_path, &state)?;
    Ok(())
}
