use std::fs;
use std::path::PathBuf;

use serde::Serialize;

use crate::util;

use crate::error::PrioError;
use crate::git::runner;
use crate::logger::Logger;
use crate::result::PrioResult;
use crate::services::apply;
use crate::services::common::{mc_path_for_repo, resolve_repo_path};
use crate::storage::repo_state;

#[derive(Serialize)]
struct BackupRecord {
    commit_sha: String,
    commits: Vec<String>,
}

pub fn run(repo_path: Option<PathBuf>, logger: &mut Logger) -> Result<PrioResult, PrioError> {
    let repo_path = resolve_repo_path(repo_path)?;

    if !runner::working_tree_clean(&repo_path, logger)? {
        return Err(PrioError::DirtyWorktree);
    }

    let last_good = repo_state::load_last_good(&repo_path)?
        .ok_or_else(|| PrioError::Message("No last known good state recorded.".into()))?;

    let current = runner::run_git(&["rev-parse", "HEAD"], &repo_path, logger)?
        .trim()
        .to_string();

    if current == last_good.commit_sha {
        let logs = logger.drain();
        return Ok(PrioResult::success(
            "Work branch already matches last known good state.",
            logs,
        ));
    }

    let _state = repo_state::load_state(&repo_path)?;
    let new_commits = runner::run_git(
        &[
            "log",
            &format!("{}..HEAD", last_good.state.baseline_commit),
            "--format=%H %s",
        ],
        &repo_path,
        logger,
    )?;

    let mut warning = None;
    if !new_commits.trim().is_empty() {
        let ts = util::now_ms();
        let backup_path = repo_state::backup_dir(&repo_path).join(ts.to_string());
        fs::create_dir_all(&backup_path)?;
        let record = BackupRecord {
            commit_sha: current.clone(),
            commits: new_commits.lines().map(|s| s.to_string()).collect(),
        };
        fs::write(
            backup_path.join("backup.json"),
            serde_json::to_string_pretty(&record)?,
        )?;
        warning = Some(format!(
            "Backup created at .git/prio/backup/{ts} before recover."
        ));
    }

    // Emergency rollback on the work clone only; rebuild applied state through prio-mc.
    runner::run_git(
        &["reset", "--hard", &last_good.commit_sha],
        &repo_path,
        logger,
    )?;
    repo_state::save_state(&repo_path, &last_good.state)?;

    // Clear any mv_rebase_in_progress state in prio-mc so mc-post-commit doesn't
    // try to resume a rebase that was just abandoned.
    if let Ok(mc_path) = mc_path_for_repo(&repo_path, None) {
        if let Ok(mut mc_state) = repo_state::load_mc_state(&mc_path) {
            if mc_state.mv_rebase_in_progress {
                mc_state.mv_rebase_in_progress = false;
                mc_state.mv_rebase_source_branch.clear();
                mc_state.mv_rebase_remaining_commits.clear();
                let _ = repo_state::save_mc_state(&mc_path, &mc_state);
            }
        }
    }

    apply::run(
        Some(repo_path.clone()),
        last_good.state.applied_branches,
        false,
        logger,
    )?;

    let logs = logger.drain();
    if let Some(w) = warning {
        Ok(PrioResult::warning(format!("Recovered. {w}"), logs))
    } else {
        Ok(PrioResult::success(
            "Recovered to last known good state.",
            logs,
        ))
    }
}

pub fn state_matches(repo_path: &PathBuf, logger: &mut Logger) -> Result<bool, PrioError> {
    let state = repo_state::load_state(repo_path)?;
    let head = runner::run_git(&["rev-parse", "HEAD"], repo_path, logger)?
        .trim()
        .to_string();
    Ok(head == state.baseline_commit || head == state.baseline_commit)
}
