use std::io::{self, Write};
use std::path::{Path, PathBuf};

use uuid::Uuid;

use crate::util;

use crate::error::PrioError;
use crate::git::runner;
use crate::hooks::{self, HookTarget};
use crate::logger::Logger;
use crate::result::{PrioResult, PrioStatus};
use crate::services::apply;
use crate::services::common::{assign_work_commits_to_branch, default_mc_path};
use crate::storage::{repo_state, user_config};

#[derive(Debug, Clone, serde::Serialize)]
pub struct WorkBranchSuggestion {
    pub default_name: String,
    pub explanation: String,
}

pub fn suggest_work_branch(repo_path: &Path) -> Result<WorkBranchSuggestion, PrioError> {
    let default = get_work_branch_default(repo_path, None)?;
    Ok(WorkBranchSuggestion {
        default_name: default.clone(),
        explanation: "Your work branch is where prio dynamically applies and unapplies stacked changes. \
            Use a unique name (e.g. prio/yourname) so an accidental git push won't overwrite anyone else's prio work."
            .into(),
    })
}

pub fn get_work_branch_default(
    repo_path: &Path,
    override_name: Option<String>,
) -> Result<String, PrioError> {
    if let Some(name) = override_name {
        return Ok(name);
    }
    let user_cfg = user_config::load_config()?;
    if let Some(prefix) = user_cfg.work_branch_prefix {
        return Ok(prefix);
    }

    let mut logger = Logger::Cli;
    let email = runner::run_git(&["config", "user.email"], repo_path, &mut logger)
        .unwrap_or_default()
        .trim()
        .to_string();

    if email.is_empty() {
        return Ok("prio/user".to_string());
    }

    let client = reqwest::blocking::Client::builder()
        .user_agent("prio")
        .build()?;
    let url = format!("https://api.github.com/search/users?q={email}");
    let resp: serde_json::Value = client.get(&url).send()?.json().ok().unwrap_or_default();

    if let Some(items) = resp.get("items").and_then(|i| i.as_array()) {
        if let Some(first) = items.first() {
            if let Some(login) = first.get("login").and_then(|l| l.as_str()) {
                return Ok(format!("prio/{login}"));
            }
        }
    }
    Ok("prio/user".to_string())
}

pub fn prompt_work_branch(default: &str) -> Result<String, PrioError> {
    print!("Work branch name [{default}]: ");
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let trimmed = input.trim();
    if trimmed.is_empty() {
        Ok(default.to_string())
    } else {
        Ok(trimmed.to_string())
    }
}

pub fn run(
    repo_path: Option<PathBuf>,
    mc_path: Option<PathBuf>,
    work_branch_override: Option<String>,
    interactive: bool,
    logger: &mut Logger,
) -> Result<PrioResult, PrioError> {
    let path = repo_path.unwrap_or_else(|| {
        std::env::current_dir().expect("current directory should exist")
    });
    let abs = crate::util::absolute_path(path);

    logger.info(format!(
        "Checking that {} is a Git repository (git rev-parse --is-inside-work-tree)…",
        abs.display()
    ));
    runner::ensure_git_work_tree(&abs, logger)?;

    let repo_path = abs;

    let origin_raw = runner::run_git(&["remote", "get-url", "origin"], &repo_path, logger)?;
    let origin_norm = runner::normalize_origin(&origin_raw);
    if !origin_norm.contains("github.com") {
        logger.warning(
            "This tool has never been tested with non-GitHub repositories — your mileage may vary.",
        );
    }

    let mut mc_path = mc_path.map(crate::util::absolute_path);
    let existing = user_config::find_repo_by_origin(&origin_norm)?;

    if let Some(ref record) = existing {
        logger.info(format!(
            "🎉 Another clone with the same origin is already in prio — reusing merge-conflicts clone at {}",
            record.mc_clone_path
        ));
        mc_path = Some(PathBuf::from(&record.mc_clone_path));
    }

    let mc_path = crate::util::absolute_path(
        mc_path.unwrap_or_else(|| default_mc_path(&repo_path)),
    );

    if !mc_path.exists() {
        clone_mc_from_work_repo(&repo_path, &mc_path, logger)?;
        repo_state::ensure_prio_dirs(&mc_path)?;
    } else {
        logger.info(format!("Using existing merge-conflicts clone at {}", mc_path.display()));
    }

    let default_branch = detect_default_branch(&repo_path, logger)?;
    let user_cfg = user_config::load_config()?;
    let suggested = get_work_branch_default(&repo_path, work_branch_override.clone())?;
    let work_branch = if let Some(wb) = work_branch_override {
        wb
    } else if interactive && user_cfg.work_branch_prefix.is_none() {
        prompt_work_branch(&suggested)?
    } else {
        if user_cfg.work_branch_prefix.is_some() {
            logger.info(format!("Using work branch from user config: {suggested}"));
        }
        suggested
    };

    let mut user_cfg = user_cfg;
    user_cfg.work_branch_prefix = Some(work_branch.clone());
    user_config::save_config(&user_cfg)?;

    repo_state::ensure_prio_dirs(&repo_path)?;
    repo_state::save_config(
        &repo_path,
        &repo_state::RepoConfig {
            work_branch: work_branch.clone(),
            default_branch: default_branch.clone(),
        },
    )?;

    let baseline = runner::run_git(
        &["rev-parse", &format!("origin/{default_branch}")],
        &repo_path,
        logger,
    )
    .or_else(|_| runner::run_git(&["rev-parse", &default_branch], &repo_path, logger))?;
    let baseline = baseline.trim().to_string();

    let branch_at_setup = runner::current_branch(&repo_path, logger)?;
    let initial_applied: Vec<String> = if branch_at_setup != work_branch
        && branch_at_setup != default_branch
    {
        logger.info(format!(
            "You were on branch {branch_at_setup}; it will be recorded as an applied branch"
        ));
        vec![branch_at_setup]
    } else {
        vec![]
    };

    let state = repo_state::RepoState {
        work_branch: work_branch.clone(),
        default_branch: default_branch.clone(),
        baseline_commit: baseline.clone(),
        applied_branches: initial_applied.clone(),
        commit_map: Default::default(),
        commit_assignments: Default::default(),
        stacks: vec![],
        merge_in_progress: false,
        pending_merge_branches: vec![],
        pending_merge_index: 0,
    };
    repo_state::save_state(&repo_path, &state)?;

    checkout_work_branch_at_baseline(&repo_path, &work_branch, &baseline, logger)?;

    if !initial_applied.is_empty() {
        logger.info(format!(
            "Applying {} into the work area…",
            initial_applied.join(", ")
        ));
        let apply_result = apply::run(Some(repo_path.clone()), vec![], false, logger)?;
        if apply_result.status != PrioStatus::Failure {
            for branch in &initial_applied {
                assign_work_commits_to_branch(&repo_path, branch, logger)?;
            }
        }
        let record = user_config::RepoRecord {
            id: existing
                .as_ref()
                .map(|r| r.id.clone())
                .unwrap_or_else(|| Uuid::new_v4().to_string()),
            path: repo_path.to_string_lossy().to_string(),
            origin_normalized: origin_norm,
            mc_clone_path: mc_path.to_string_lossy().to_string(),
            added_at: existing
                .as_ref()
                .map(|r| r.added_at)
                .unwrap_or_else(util::now_ms),
        };
        user_config::upsert_repo(record)?;
        hooks::install(&repo_path, HookTarget::WorkClone)?;
        hooks::install(&mc_path, HookTarget::McClone)?;
        return Ok(PrioResult {
            status: apply_result.status,
            message: format!(
                "Repository set up on work branch {work_branch} with {} applied.",
                initial_applied.join(", ")
            ),
            logs: apply_result.logs,
        });
    }

    hooks::install(&repo_path, HookTarget::WorkClone)?;
    hooks::install(&mc_path, HookTarget::McClone)?;

    let record = user_config::RepoRecord {
        id: existing
            .as_ref()
            .map(|r| r.id.clone())
            .unwrap_or_else(|| Uuid::new_v4().to_string()),
        path: repo_path.to_string_lossy().to_string(),
        origin_normalized: origin_norm,
        mc_clone_path: mc_path.to_string_lossy().to_string(),
        added_at: existing
            .as_ref()
            .map(|r| r.added_at)
            .unwrap_or_else(util::now_ms),
    };
    user_config::upsert_repo(record)?;

    let logs = logger.drain();
    let status = if logs.iter().any(|l| l.level == crate::logger::LogLevel::Warning) {
        PrioStatus::Warning
    } else {
        PrioStatus::Success
    };
    Ok(PrioResult {
        status,
        message: format!("Repository set up. Work branch: {work_branch}"),
        logs,
    })
}

/// Create the merge-conflicts clone from the local work repository (no network required).
fn clone_mc_from_work_repo(
    work_repo: &Path,
    mc_path: &Path,
    logger: &mut Logger,
) -> Result<(), PrioError> {
    logger.info(format!(
        "Cloning local work repository {} to {} (no remote fetch)",
        work_repo.display(),
        mc_path.display()
    ));
    let src = crate::util::path_arg(work_repo);
    let dest = crate::util::path_arg(mc_path);
    runner::run_git(
        &["clone", &src, &dest],
        work_repo.parent().unwrap_or(work_repo),
        logger,
    )?;
    Ok(())
}

fn detect_default_branch(repo_path: &Path, logger: &mut Logger) -> Result<String, PrioError> {
    if let Ok(sym) = runner::run_git(&["symbolic-ref", "refs/remotes/origin/HEAD"], repo_path, logger)
    {
        if let Some(branch) = sym.trim().strip_prefix("refs/remotes/origin/") {
            return Ok(branch.to_string());
        }
    }
    // Re-setup: reuse default branch already stored for this repo.
    if let Ok(cfg) = repo_state::load_config(repo_path) {
        return Ok(cfg.default_branch);
    }
    Ok("main".to_string())
}

/// Create or reset the work branch at the default-branch baseline (not at current HEAD).
fn checkout_work_branch_at_baseline(
    repo_path: &Path,
    work_branch: &str,
    baseline: &str,
    logger: &mut Logger,
) -> Result<(), PrioError> {
    logger.info(format!(
        "Checking out work branch {work_branch} at {baseline} (default branch tip)"
    ));
    runner::run_git(
        &["checkout", "-B", work_branch, baseline],
        repo_path,
        logger,
    )?;
    Ok(())
}
