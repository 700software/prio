use std::fs;
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
    logger: &mut Logger,
) -> Result<PrioResult, PrioError> {
    let repo_path = resolve_repo_path(repo_path)?;
    assert_on_work_branch(&repo_path, logger)?;
    runner::ensure_gh_authenticated(&repo_path, logger)?;

    let branch = runner::resolve_branch_ref(&branch, &repo_path, logger)?;

    ensure_pr_md_not_in_commit(&repo_path, logger)?;

    runner::run_git(&["push", "-u", "origin", &branch], &repo_path, logger)?;

    let body = build_pr_body(&repo_path, &branch, logger)?;
    let title = branch.clone();

    let url = runner::run_gh(
        &[
            "pr",
            "create",
            "--draft",
            "--title",
            &title,
            "--body",
            &body,
            "--head",
            &branch,
        ],
        &repo_path,
        logger,
    )?;

    let mc_path = mc_path_for_repo(&repo_path, None)?;
    suggestions::run_and_log(&repo_path, &mc_path, logger)?;

    let logs = logger.drain();
    Ok(PrioResult::success(format!("Created draft PR: {url}"), logs))
}

fn ensure_pr_md_not_in_commit(repo_path: &PathBuf, logger: &mut Logger) -> Result<(), PrioError> {
    let pr_md = repo_path.join("PR.md");
    if !pr_md.exists() {
        return Ok(());
    }

    let tracked = runner::run_git(
        &["ls-files", "--error-unmatch", "PR.md"],
        repo_path,
        logger,
    )
    .is_ok();

    if tracked {
        let _ = runner::run_git(&["reset", "HEAD", "PR.md"], repo_path, logger);
    }

    let staged = runner::run_git(
        &["diff", "--cached", "--name-only"],
        repo_path,
        logger,
    )?;
    if staged.lines().any(|l| l == "PR.md") {
        runner::run_git(
            &["commit", "--amend", "--no-edit", "--", ":!PR.md"],
            repo_path,
            logger,
        )?;
    }

    Ok(())
}

fn build_pr_body(repo_path: &PathBuf, branch: &str, logger: &mut Logger) -> Result<String, PrioError> {
    let mut body = String::new();
    let pr_md = repo_path.join("PR.md");
    if pr_md.exists() {
        body = fs::read_to_string(&pr_md)?;
    }

    let state = repo_state::load_state(repo_path)?;
    let stack_lines = build_stack_lines(&state, branch, repo_path, logger);
    if !stack_lines.is_empty() {
        if !body.is_empty() {
            body.push_str("\n\n");
        }
        body.push_str("## Stacked after\n");
        body.push_str(&stack_lines);
    }

    Ok(body)
}

fn build_stack_lines(
    state: &repo_state::RepoState,
    branch: &str,
    repo_path: &PathBuf,
    logger: &mut Logger,
) -> String {
    let mut lines = Vec::new();
    for entry in &state.stacks {
        if entry.branch != branch {
            continue;
        }
        for dep in &entry.dependencies {
            let dep = dep.trim();
            if dep.is_empty() {
                continue;
            }
            if let Some(num) = pr_number_for_branch(dep, repo_path, logger) {
                lines.push(format!("- #{num}"));
            } else {
                let pushed = branch_pushed(dep, repo_path, logger);
                if pushed {
                    lines.push(format!("- {dep}"));
                } else {
                    lines.push(format!("- {dep} (not pushed yet)"));
                }
            }
        }
    }
    lines.join("\n")
}

fn pr_number_for_branch(branch: &str, repo_path: &PathBuf, logger: &mut Logger) -> Option<u32> {
    let json = runner::run_gh(
        &["pr", "list", "--head", branch, "--json", "number", "--limit", "1"],
        repo_path,
        logger,
    )
    .ok()?;
    let v: serde_json::Value = serde_json::from_str(&json).ok()?;
    v.as_array()?
        .first()?
        .get("number")?
        .as_u64()
        .map(|n| n as u32)
}

fn branch_pushed(branch: &str, repo_path: &PathBuf, logger: &mut Logger) -> bool {
    runner::run_git(
        &["rev-parse", &format!("origin/{branch}")],
        repo_path,
        logger,
    )
    .is_ok()
}
