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

fn install_hint_gh() -> String {
    match std::env::consts::OS {
        "windows" => "Run: winget install --id GitHub.cli".into(),
        "macos" => "Run: brew install gh".into(),
        _ => "See https://cli.github.com/packages for the official apt one-liner".into(),
    }
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

pub fn ensure_gh_authenticated(cwd: &Path, logger: &mut Logger) -> Result<(), PrioError> {
    ensure_gh()?;
    let output = Command::new("gh")
        .args(["auth", "status"])
        .current_dir(cwd)
        .output()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                PrioError::ToolNotInstalled {
                    tool: "gh".into(),
                    install_hint: install_hint_gh(),
                }
            } else {
                PrioError::Io(e)
            }
        })?;
    if !output.status.success() {
        logger.warning("GitHub CLI is not authenticated. Run: gh auth login");
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
) -> Result<String, PrioError> {
    let cwd = util::absolute_path(cwd);
    let cmd_str = format!("{} {}", program, args.join(" "));
    let dir = util::path_arg(&cwd);
    logger.log(LogEntry::info(format!("{dir} $ {cmd_str}")));
    let mut command = Command::new(program);
    command.args(args).current_dir(&cwd);
    if skip_prio_hooks {
        command.env("PRIO_AUTOMATED", "1");
    }
    let output = command
        .output()
        .map_err(|e| {
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
            stderr: if stderr.is_empty() {
                stdout
            } else {
                stderr
            },
        })
    }
}

pub fn run_git(args: &[&str], cwd: &Path, logger: &mut Logger) -> Result<String, PrioError> {
    ensure_git()?;
    run_command("git", args, cwd, logger, false)
}

/// Git for automated apply/mv in prio-mc — skips post-commit hooks (avoids deadlock with repo lock).
pub fn run_git_no_hooks(args: &[&str], cwd: &Path, logger: &mut Logger) -> Result<String, PrioError> {
    ensure_git()?;
    run_command("git", args, cwd, logger, true)
}

pub fn run_gh(args: &[&str], cwd: &Path, logger: &mut Logger) -> Result<String, PrioError> {
    ensure_gh()?;
    run_command("gh", args, cwd, logger, false)
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
    Ok(run_git(
        &["rev-parse", "--abbrev-ref", "HEAD"],
        repo_path,
        logger,
    )?
    .trim()
    .to_string())
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
