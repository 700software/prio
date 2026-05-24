use std::fs;
use std::path::Path;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use crate::error::PrioError;
use crate::git::runner;
use crate::logger::Logger;
use crate::storage::prio_dir;

pub enum HookTarget {
    WorkClone,
    McClone,
}

const WORK_POST_COMMIT: &str = r#"#!/bin/sh
# prio work-clone post-commit hook
if [ -n "$PRIO_AUTOMATED" ]; then exit 0; fi
if command -v prio >/dev/null 2>&1; then
  prio _internal-work-post-commit
elif [ -f ".git/prio/hooks/post-commit" ]; then
  :
fi
"#;

const MC_POST_COMMIT: &str = r#"#!/bin/sh
# prio mc-clone post-commit hook
if [ -n "$PRIO_AUTOMATED" ]; then exit 0; fi
if command -v prio >/dev/null 2>&1; then
  prio _internal-mc-post-commit
fi
"#;

pub fn install(repo_path: &Path, target: HookTarget) -> Result<(), PrioError> {
    let mut logger = Logger::Cli;
    let dir = prio_dir(repo_path);
    fs::create_dir_all(dir.join("hooks"))?;

    let script = match target {
        HookTarget::WorkClone => WORK_POST_COMMIT,
        HookTarget::McClone => MC_POST_COMMIT,
    };
    let hook_path = dir.join("hooks").join("post-commit");
    fs::write(&hook_path, script)?;

    #[cfg(unix)]
    {
        let mut perms = fs::metadata(&hook_path)?.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&hook_path, perms)?;
    }

    let husky_path = repo_path.join(".husky");
    let gitignore_path = repo_path.join(".gitignore");
    let husky_gitignored = is_gitignored(&gitignore_path, ".husky");

    let current_hooks = runner::run_git(&["config", "core.hooksPath"], repo_path, &mut logger)
        .unwrap_or_default();

    if husky_gitignored {
        fs::create_dir_all(husky_path.join("_"))?;
        let husky_hook = husky_path.join("_/post-commit");
        append_if_absent(&husky_hook, "sh .git/prio/hooks/post-commit\n")?;
        if current_hooks.trim().is_empty() {
            runner::run_git(
                &["config", "core.hooksPath", ".husky"],
                repo_path,
                &mut logger,
            )?;
        }
    } else if current_hooks.trim().is_empty() {
        runner::run_git(
            &["config", "core.hooksPath", ".git/prio/hooks"],
            repo_path,
            &mut logger,
        )?;
    } else if current_hooks.trim() != ".git/prio/hooks" && !current_hooks.contains(".husky") {
        logger.warning(format!(
            "core.hooksPath is set to '{}' — prio hook may not run automatically",
            current_hooks.trim()
        ));
    }

    Ok(())
}

/// Unset `core.hooksPath` when prio configured it (`.git/prio` or `.git/prio/hooks`).
pub fn uninstall(repo_path: &Path, logger: &mut Logger) -> Result<(), PrioError> {
    let current = runner::run_git(&["config", "--get", "core.hooksPath"], repo_path, logger)
        .unwrap_or_default();
    let trimmed = current.trim().replace('\\', "/");
    let is_prio_hooks = trimmed == ".git/prio/hooks"
        || trimmed == ".git/prio"
        || trimmed.ends_with("/.git/prio/hooks")
        || trimmed.ends_with("/.git/prio");
    if is_prio_hooks {
        runner::run_git(&["config", "--unset", "core.hooksPath"], repo_path, logger)?;
        logger.info("Unset core.hooksPath (was managed by prio)");
    }
    Ok(())
}

fn is_gitignored(gitignore_path: &Path, pattern: &str) -> bool {
    if !gitignore_path.exists() {
        return false;
    }
    let content = fs::read_to_string(gitignore_path).unwrap_or_default();
    content.lines().any(|line| {
        let t = line.trim();
        t == pattern || t == format!("{pattern}/")
    })
}

fn append_if_absent(path: &Path, line: &str) -> Result<(), PrioError> {
    let existing = if path.exists() {
        fs::read_to_string(path)?
    } else {
        String::new()
    };
    if !existing.contains(line.trim()) {
        let mut content = existing;
        if !content.ends_with('\n') && !content.is_empty() {
            content.push('\n');
        }
        content.push_str(line);
        fs::write(path, content)?;
    }
    Ok(())
}
