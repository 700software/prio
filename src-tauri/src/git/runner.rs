use std::path::Path;
use std::process::Command;

use crate::error::PrioError;
use crate::logger::{LogEntry, Logger};
use crate::util;

fn install_hint_git() -> String {
    match std::env::consts::OS {
        "windows" => "Download from https://git-scm.com/download/win (also installs Git Bash)".into(),
        "macos" => {
            "Run `git --version` in Terminal to trigger Xcode CLI tools, or install from https://git-scm.com/download/mac".into()
        }
        _ => "Run: sudo apt install git-all (or see https://git-scm.com/download/linux)".into(),
    }
}

/// Install instructions for the GitHub CLI (shared by setup and other commands).
pub fn gh_install_hint() -> String {
    match std::env::consts::OS {
        "windows" => "Run: winget install --id GitHub.cli".into(),
        "macos" => "Run: brew install gh".into(),
        _ => "See https://cli.github.com/packages for the official apt one-liner".into(),
    }
}

/// How to authenticate with the GitHub CLI.
pub fn gh_auth_hint() -> String {
    "Run: gh auth login".into()
}

fn install_hint_gh() -> String {
    gh_install_hint()
}

fn command_exists(name: &str) -> bool {
    #[cfg(windows)]
    {
        std::process::Command::new("where")
            .arg(name)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }
    #[cfg(not(windows))]
    {
        std::process::Command::new("which")
            .arg(name)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }
}

fn ensure_tool(name: &str, hint: String) -> Result<(), PrioError> {
    if !command_exists(name) {
        return Err(PrioError::ToolNotInstalled {
            tool: name.to_string(),
            install_hint: hint,
        });
    }
    Ok(())
}

pub fn ensure_git() -> Result<(), PrioError> {
    ensure_tool("git", install_hint_git())
}

pub fn ensure_gh() -> Result<(), PrioError> {
    ensure_tool("gh", install_hint_gh())
}

pub fn is_gh_installed() -> bool {
    command_exists("gh")
}

pub fn is_gh_authenticated(cwd: &Path) -> bool {
    if !is_gh_installed() {
        return false;
    }
    Command::new("gh")
        .args(["auth", "status"])
        .current_dir(cwd)
        .output()
        .ok()
        .is_some_and(|o| o.status.success())
}

/// Logged-in GitHub username (`gh api user`), when `gh` is installed and authenticated.
pub fn gh_user_login(cwd: &Path, logger: &mut Logger) -> Option<String> {
    if !is_gh_authenticated(cwd) {
        return None;
    }
    let login = run_gh(&["api", "user", "-q", ".login"], cwd, logger).ok()?;
    let login = login.trim();
    if login.is_empty() {
        None
    } else {
        Some(login.to_string())
    }
}

pub fn ensure_gh_authenticated(cwd: &Path, logger: &mut Logger) -> Result<(), PrioError> {
    ensure_gh()?;
    if !is_gh_authenticated(cwd) {
        logger.warning(format!(
            "GitHub CLI is not authenticated. {}",
            gh_auth_hint()
        ));
        return Err(PrioError::GhNotAuthenticated);
    }
    Ok(())
}

fn run_command(
    program: &str,
    args: &[&str],
    cwd: &Path,
    logger: &mut Logger,
    skip_prio_hooks: bool,
    comment: Option<&str>,
) -> Result<String, PrioError> {
    let cwd = util::absolute_path(cwd);
    let cmd_str = format!("{} {}", program, args.join(" "));
    let dir = util::path_arg(&cwd);
    logger.log(LogEntry::cli_command(
        crate::logger::LogLevel::Info,
        &dir,
        &cmd_str,
        comment,
    ));
    let mut command = Command::new(program);
    command.args(args).current_dir(&cwd);
    if skip_prio_hooks {
        command.env("PRIO_AUTOMATED", "1");
    }
    let output = command.output().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            let hint = if program == "git" {
                install_hint_git()
            } else {
                install_hint_gh()
            };
            PrioError::ToolNotInstalled {
                tool: program.to_string(),
                install_hint: hint,
            }
        } else {
            PrioError::Io(e)
        }
    })?;

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();

    if output.status.success() {
        Ok(stdout)
    } else {
        Err(PrioError::CommandFailed {
            command: cmd_str,
            stderr: if stderr.is_empty() { stdout } else { stderr },
        })
    }
}

pub fn run_git(args: &[&str], cwd: &Path, logger: &mut Logger) -> Result<String, PrioError> {
    ensure_git()?;
    run_command("git", args, cwd, logger, false, None)
}

/// Like [`run_git`], but logs an optional trailing comment (shown in gray on the CLI).
pub fn run_git_with_comment(
    args: &[&str],
    cwd: &Path,
    logger: &mut Logger,
    comment: &str,
) -> Result<String, PrioError> {
    ensure_git()?;
    run_command("git", args, cwd, logger, false, Some(comment))
}

/// Git for automated apply/mv in prio-mc — skips post-commit hooks (avoids deadlock with repo lock).
pub fn run_git_no_hooks(
    args: &[&str],
    cwd: &Path,
    logger: &mut Logger,
) -> Result<String, PrioError> {
    ensure_git()?;
    run_command("git", args, cwd, logger, true, None)
}

pub fn run_gh(args: &[&str], cwd: &Path, logger: &mut Logger) -> Result<String, PrioError> {
    ensure_gh()?;
    run_command("gh", args, cwd, logger, false, None)
}

/// Returns true when `path` is the root of a Git working tree (`git rev-parse --is-inside-work-tree`).
pub fn is_git_repo(path: &Path) -> bool {
    ensure_git_work_tree(path, &mut Logger::Cli).is_ok()
}

/// Verifies `path` is a Git working tree using the Git CLI.
pub fn ensure_git_work_tree(path: &Path, logger: &mut Logger) -> Result<(), PrioError> {
    ensure_git()?;
    match run_git(&["rev-parse", "--is-inside-work-tree"], path, logger) {
        Ok(out) if out.trim() == "true" => Ok(()),
        Ok(_) | Err(PrioError::CommandFailed { .. }) => {
            Err(PrioError::NotGitRepo(path.to_path_buf()))
        }
        Err(e) => Err(e),
    }
}

pub fn normalize_origin(url: &str) -> String {
    let mut s = url.trim().to_string();
    if s.ends_with(".git") {
        s.truncate(s.len() - 4);
    }
    if let Some(rest) = s.strip_prefix("git@") {
        if let Some((host, path)) = rest.split_once(':') {
            return format!("{host}/{path}");
        }
    }
    if let Some(rest) = s.strip_prefix("https://") {
        return rest.to_string();
    }
    if let Some(rest) = s.strip_prefix("http://") {
        return rest.to_string();
    }
    s
}

pub fn current_branch(repo_path: &Path, logger: &mut Logger) -> Result<String, PrioError> {
    Ok(
        run_git(&["rev-parse", "--abbrev-ref", "HEAD"], repo_path, logger)?
            .trim()
            .to_string(),
    )
}

fn branch_ref_exists(
    reference: &str,
    repo_path: &Path,
    logger: &mut Logger,
) -> Result<bool, PrioError> {
    Ok(run_git(&["rev-parse", "--verify", reference], repo_path, logger).is_ok())
}

/// Prepare a branch for `prio apply` in the prio-mc clone.
///
/// When the branch has no local ref, fetch from origin — either to discover the branch
/// or to refresh a stale `origin/<branch>` before merge.
pub fn ensure_branch_for_apply(
    branch: &str,
    repo_path: &Path,
    logger: &mut Logger,
) -> Result<(), PrioError> {
    let local_ref = format!("refs/heads/{branch}");
    if branch_ref_exists(&local_ref, repo_path, logger)? {
        return Ok(());
    }

    let remote_ref = format!("refs/remotes/origin/{branch}");
    let comment = if branch_ref_exists(&remote_ref, repo_path, logger)? {
        "to ensure the latest commit from origin is applied"
    } else {
        "to see if the branch is found at origin"
    };

    run_git_with_comment(&["fetch"], repo_path, logger, comment)?;

    if branch_ref_exists(&local_ref, repo_path, logger)?
        || branch_ref_exists(&remote_ref, repo_path, logger)?
    {
        return Ok(());
    }

    Err(PrioError::Message(format!(
        "Branch '{branch}' not found locally or at origin/{branch}"
    )))
}

pub fn resolve_branch_ref(
    reference: &str,
    repo_path: &Path,
    logger: &mut Logger,
) -> Result<String, PrioError> {
    if let Some(num) = reference.strip_prefix("pr-") {
        let json = run_gh(
            &["pr", "view", num, "--json", "headRefName"],
            repo_path,
            logger,
        )?;
        let v: serde_json::Value = serde_json::from_str(&json)?;
        return v
            .get("headRefName")
            .and_then(|x| x.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| PrioError::Message(format!("Could not resolve PR #{num}")));
    }
    Ok(reference.to_string())
}

pub fn working_tree_clean(repo_path: &Path, logger: &mut Logger) -> Result<bool, PrioError> {
    let out = run_git(&["status", "--porcelain"], repo_path, logger)?;
    Ok(out.trim().is_empty())
}
