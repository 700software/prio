use std::path::PathBuf;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum PrioError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error(
        "Could not acquire lock at {path} within 30 seconds. \
         Another prio command may be running on this repository, or a git hook fired while prio was busy."
    )]
    LockTimeout { path: PathBuf },

    #[error("{tool} is not installed. {install_hint}")]
    ToolNotInstalled { tool: String, install_hint: String },

    #[error("GitHub CLI is not authenticated. Run: gh auth login")]
    GhNotAuthenticated,

    #[error(
        "'{0}' is not a Git repository.\n\n\
         prio setup must be run inside a clone, or you must pass the path to one.\n\
         • To create a new repository here: git init\n\
         • To use an existing clone: cd into it and run `prio setup` (no path needed)"
    )]
    NotGitRepo(PathBuf),

    #[error("prio is inactive. You are on branch '{current_branch}'.\n  To resume: git checkout {work_branch}\n  Or re-run:  prio setup")]
    Inactive {
        work_branch: String,
        current_branch: String,
    },

    #[error("Repository is not set up for prio. Run: prio setup")]
    NotSetup(PathBuf),

    #[error("Merge conflicts in prio-mc clone at {mc_path}. Resolve conflicts there, commit, then re-run prio apply.")]
    MergeConflict { mc_path: PathBuf },

    #[error(
        "Working tree is not clean. Run `git reset --hard` or `git stash` before prio recover."
    )]
    DirtyWorktree,

    #[error("{0}")]
    Message(String),

    #[error("Command failed: {command}\n{stderr}")]
    CommandFailed { command: String, stderr: String },
}
