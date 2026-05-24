use std::io::{self, Write};
use std::path::{Path, PathBuf};

use crate::error::PrioError;
use crate::git::runner;
use crate::hooks;
use crate::logger::Logger;
use crate::result::{PrioResult, PrioStatus};
use crate::services::common::{mc_path_for_repo, resolve_repo_path};
use crate::storage::{lock, prio_dir, repo_state, user_config};
use crate::util;

pub fn run(
    repo_path: Option<PathBuf>,
    interactive: bool,
    assume_yes: bool,
    logger: &mut Logger,
) -> Result<PrioResult, PrioError> {
    let repo_path = resolve_repo_path(repo_path)?;

    if interactive && !assume_yes && !confirm_unsetup(&repo_path)? {
        let logs = logger.drain();
        return Ok(PrioResult {
            status: PrioStatus::Warning,
            message: "Unsetup cancelled.".into(),
            logs,
        });
    }

    let config = repo_state::load_config(&repo_path)?;
    let work_branch = config.work_branch.clone();
    let mc_path = mc_path_for_repo(&repo_path, None)?;

    let ts = util::now_ms();
    logger.info(format!("Unsetup timestamp: {ts}"));

    let _guard = lock::acquire(&crate::storage::repo_lock_path(&repo_path))?;

    remove_remote_from_work_branch(&repo_path, &work_branch, logger)?;
    rename_work_branch(&repo_path, &work_branch, ts, logger)?;
    hooks::uninstall(&repo_path, logger)?;
    archive_prio_data(&repo_path, ts, logger)?;
    rename_mc_clone(&mc_path, ts, logger)?;
    user_config::remove_repo_by_path(&repo_path)?;

    let backup_branch = backup_branch_name(ts);
    let logs = logger.drain();
    Ok(PrioResult {
        status: PrioStatus::Success,
        message: format!(
            "Prio unsetup complete. Work branch renamed to {backup_branch}; \
             data archived under .git/prio/backup-{ts}"
        ),
        logs,
    })
}

fn backup_branch_name(ts: u64) -> String {
    format!("prio/backup/{ts}")
}

fn confirm_unsetup(repo_path: &Path) -> Result<bool, PrioError> {
    print!(
        "Are you sure you want to remove prio setup for {}? [y/n] ",
        repo_path.display()
    );
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let trimmed = input.trim().to_lowercase();
    Ok(trimmed == "y" || trimmed == "yes")
}

/// Unset upstream to origin and delete `origin/<work_branch>` on the remote if present.
fn remove_remote_from_work_branch(
    repo_path: &Path,
    work_branch: &str,
    logger: &mut Logger,
) -> Result<(), PrioError> {
    let remote = runner::run_git(
        &["config", "--get", &format!("branch.{work_branch}.remote")],
        repo_path,
        logger,
    )
    .unwrap_or_default();

    if remote.trim() == "origin" {
        runner::run_git(&["branch", "--unset-upstream", work_branch], repo_path, logger)?;
        logger.info(format!("Unset upstream for branch {work_branch}"));
    }

    let remote_ref = format!("refs/heads/{work_branch}");
    let ls = runner::run_git(
        &["ls-remote", "--heads", "origin", &remote_ref],
        repo_path,
        logger,
    )
    .unwrap_or_default();

    if !ls.trim().is_empty() {
        logger.info(format!("Deleting remote branch origin/{work_branch}"));
        runner::run_git(
            &["push", "origin", &format!(":{work_branch}")],
            repo_path,
            logger,
        )?;
    }

    Ok(())
}

fn rename_work_branch(
    repo_path: &Path,
    work_branch: &str,
    ts: u64,
    logger: &mut Logger,
) -> Result<(), PrioError> {
    let backup_branch = backup_branch_name(ts);
    let branches = runner::run_git(&["branch", "--list", work_branch], repo_path, logger)?;
    if branches.trim().is_empty() {
        logger.warning(format!("Work branch {work_branch} not found; skipping rename"));
        return Ok(());
    }

    let current = runner::current_branch(repo_path, logger)?;
    if current == work_branch {
        runner::run_git(&["branch", "-m", &backup_branch], repo_path, logger)?;
    } else {
        runner::run_git(
            &["branch", "-m", work_branch, &backup_branch],
            repo_path,
            logger,
        )?;
    }

    logger.info(format!(
        "Renamed branch {work_branch} → {backup_branch}"
    ));
    Ok(())
}

/// Move everything under `.git/prio/` into `.git/prio/backup-{ts}/`.
fn archive_prio_data(repo_path: &Path, ts: u64, logger: &mut Logger) -> Result<(), PrioError> {
    let prio = prio_dir(repo_path);
    if !prio.exists() {
        logger.info(".git/prio not found; skipping data archive");
        return Ok(());
    }

    let backup_dir_name = format!("backup-{ts}");
    let backup_dest = prio.join(&backup_dir_name);
    if backup_dest.exists() {
        return Err(PrioError::Message(format!(
            "Archive path already exists: {}",
            backup_dest.display()
        )));
    }
    std::fs::create_dir_all(&backup_dest)?;

    for entry in std::fs::read_dir(&prio)? {
        let entry = entry?;
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if name_str == backup_dir_name || name_str == "prio.lock" {
            continue;
        }
        let dest = backup_dest.join(&name);
        std::fs::rename(entry.path(), dest)?;
    }

    logger.info(format!(
        "Archived .git/prio contents to .git/prio/{backup_dir_name}/"
    ));
    Ok(())
}

fn rename_mc_clone(mc_path: &Path, ts: u64, logger: &mut Logger) -> Result<(), PrioError> {
    if !mc_path.exists() {
        logger.info(format!(
            "Merge-conflicts clone not found at {}; skipping rename",
            mc_path.display()
        ));
        return Ok(());
    }

    let new_path = mc_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(format!("prio-mc-backup-{ts}"));

    if new_path.exists() {
        return Err(PrioError::Message(format!(
            "Backup path already exists: {}",
            new_path.display()
        )));
    }

    std::fs::rename(mc_path, &new_path)?;
    logger.info(format!(
        "Renamed merge-conflicts clone {} → {}",
        mc_path.display(),
        new_path.display()
    ));
    Ok(())
}
