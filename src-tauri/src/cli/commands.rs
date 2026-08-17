use std::path::PathBuf;

use clap::{Parser, Subcommand};

use crate::error::PrioError;
use crate::hooks::{self, HookTarget};
use crate::logger::{print_cli_result, print_hook_result, Logger};
use crate::result::PrioResult;
use crate::services::{
    abort, apply, common, cp, mv, pr, prs, push, recover, setup, stack, status, sync, unsetup,
};
use crate::storage::prio_dir;

#[derive(Parser)]
#[command(
    name = "prio",
    display_name = "prio",
    about = "Stacked PR manager — merge-up, no rebase",
    override_usage = "prio                    # no subcommand opens UI\n       prio <COMMAND>            # CLI usage"
)]
pub struct CliApp {
    #[arg(long = "repo", global = true, value_name = "REPO_PATH")]
    repo: Option<PathBuf>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Set up a repository for prio (defaults to current directory)
    Setup {
        #[arg(value_name = "REPO_PATH")]
        repo_path: Option<PathBuf>,
        #[arg(long)]
        mc_path: Option<PathBuf>,
        #[arg(long)]
        work_branch: Option<String>,
    },
    /// Tear down prio for a repository (archive state, rename branches)
    Unsetup {
        #[arg(short = 'y', help = "Skip confirmation prompt")]
        yes: bool,
    },
    /// Show applied branches and unassigned commits
    Status,
    /// Apply branches or PRs to the work area
    Apply { branches: Vec<String> },
    /// Remove branches from the work area
    Unapply { branches: Vec<String> },
    /// Move commits to another branch
    Mv {
        #[arg(required = true)]
        args: Vec<String>,
        #[arg(
            short = 'c',
            help = "Create destination branch if missing (also applies it)"
        )]
        create: bool,
        #[arg(short = 'a', help = "Apply destination branch into the work area")]
        apply: bool,
        #[arg(
            short = 'f',
            help = "Force-push source branch after removing commits (for pushed branches)"
        )]
        force: bool,
    },
    /// Copy commits to another branch (non-destructive; source branch is unchanged)
    Cp {
        #[arg(required = true)]
        args: Vec<String>,
        #[arg(
            short = 'c',
            help = "Create destination branch if missing (also applies it)"
        )]
        create: bool,
        #[arg(short = 'a', help = "Apply destination branch into the work area")]
        apply: bool,
    },
    /// Push a branch
    Push {
        branch: String,
        /// Also push any stacked dependency branches that have not yet been pushed
        #[arg(short = 'p', long = "push-deps")]
        push_deps: bool,
    },
    /// Create a draft PR for a branch
    Pr { branch: String },
    /// List open PRs showing which are applied to the work area
    Prs,
    /// Stack a branch after one or more dependencies: `prio stack <branch> [dep1 dep2 ...]`
    Stack {
        branch: String,
        dependencies: Vec<String>,
    },
    /// Remove stack metadata for a branch, rebasing it off its dependencies
    Unstack {
        branch: String,
        /// Keep all upstream commits (metadata-only unstack, safe for pushed branches)
        #[arg(short = 'k', long = "keep")]
        keep: bool,
        /// Force-push after rebase (rewrites remote history; required for pushed branches)
        #[arg(short = 'f', long = "force")]
        force: bool,
    },
    /// Sync merged branches for current repo
    Sync,
    /// Sync all known repos
    Syncs,
    /// Abort an in-progress prio mv conflict and clean up the partially-created destination branch
    Abort,
    /// Recover work branch to last known good state
    Recover,
    /// Suggest default work branch name (for UI)
    SuggestWorkBranch { repo_path: PathBuf },
    /// Internal: work clone post-commit hook
    #[command(hide = true, alias = "_internal-work-post-commit")]
    InternalWorkPostCommit,
    /// Internal: mc clone post-commit hook
    #[command(hide = true, alias = "_internal-mc-post-commit")]
    InternalMcPostCommit {
        #[arg(long)]
        mc_path: Option<PathBuf>,
    },
}

pub fn execute() -> Result<(), i32> {
    let CliApp { repo, command } = CliApp::parse();
    let is_hook = matches!(
        command,
        Commands::InternalWorkPostCommit | Commands::InternalMcPostCommit { .. }
    );
    let mut logger = if is_hook { Logger::Hook } else { Logger::Cli };

    // Verify hooks are in place on every user-facing run.  Skip for commands that manage
    // hooks themselves (setup/unsetup) and for internal hook callbacks and utility commands.
    let skip_hook_check = is_hook
        || matches!(
            command,
            Commands::Setup { .. } | Commands::Unsetup { .. } | Commands::SuggestWorkBranch { .. }
        );
    if !skip_hook_check {
        check_hooks(&repo);
    }

    let result = match command {
        Commands::Setup {
            repo_path,
            mc_path,
            work_branch,
        } => setup::run(
            repo_path.or(repo),
            mc_path,
            work_branch.clone(),
            work_branch.is_none(),
            &mut logger,
        ),
        Commands::Unsetup { yes } => unsetup::run(repo, true, yes, &mut logger),
        Commands::Status => match status::run(repo, &mut logger) {
            Ok(s) => {
                status::print_cli(&s);
                Ok(s.prio_result)
            }
            Err(e) => Err(e),
        },
        Commands::Apply { branches } => apply::run(repo, branches, false, &mut logger),
        Commands::Unapply { branches } => apply::run(repo, branches, true, &mut logger),
        Commands::Mv {
            args,
            create,
            apply,
            force,
        } => parse_mv(args, create, apply, force, repo, &mut logger),
        Commands::Cp {
            args,
            create,
            apply,
        } => parse_cp(args, create, apply, repo, &mut logger),
        Commands::Abort => abort::run(repo, &mut logger),
        Commands::Push { branch, push_deps } => push::run(repo, branch, push_deps, &mut logger),
        Commands::Pr { branch } => pr::run(repo, branch, &mut logger),
        Commands::Prs => match prs::run(repo, &mut logger) {
            Ok(()) => return Ok(()),
            Err(e) => Err(e),
        },
        Commands::Stack {
            branch,
            dependencies,
        } => stack::run_stack(repo, branch, dependencies, &mut logger),
        Commands::Unstack {
            branch,
            keep,
            force,
        } => stack::run_unstack(repo, branch, keep, force, &mut logger),
        Commands::Sync => sync::run(repo, &mut logger),
        Commands::Syncs => sync::run_syncs(&mut logger),
        Commands::Recover => recover::run(repo, &mut logger),
        Commands::SuggestWorkBranch { repo_path } => match setup::suggest_work_branch(&repo_path) {
            Ok(s) => {
                println!("{}", serde_json::to_string(&s).unwrap());
                return Ok(());
            }
            Err(e) => Err(e),
        },
        Commands::InternalWorkPostCommit => apply::work_post_commit(repo, &mut logger),
        Commands::InternalMcPostCommit { mc_path } => apply::mc_post_commit(mc_path, &mut logger),
    };

    match result {
        Ok(r) => {
            if is_hook {
                print_hook_result(&r);
            } else {
                print_cli_result(&r);
            }
            if r.status == crate::result::PrioStatus::Failure {
                return Err(1);
            }
            Ok(())
        }
        Err(e) => {
            let r = PrioResult::from_error(e, logger.drain());
            if is_hook {
                print_hook_result(&r);
            } else {
                print_cli_result(&r);
            }
            Err(1)
        }
    }
}

fn parse_mv(
    args: Vec<String>,
    create: bool,
    apply: bool,
    force: bool,
    repo: Option<PathBuf>,
    logger: &mut Logger,
) -> Result<PrioResult, PrioError> {
    if args.len() < 2 {
        return Err(PrioError::Message(
            "Usage: prio mv [-c] [-a] [-f] <commit>... <destination>".into(),
        ));
    }

    let destination = args.last().unwrap().clone();
    let mut commits = args[..args.len() - 1].to_vec();

    if destination == "-c" || destination == "-a" || destination == "-f" {
        return Err(PrioError::Message(
            "Destination required before flags".into(),
        ));
    }

    while matches!(
        commits.last().map(|s| s.as_str()),
        Some("-c") | Some("-a") | Some("-f")
    ) {
        commits.pop();
    }

    mv::run(repo, commits, destination, create, apply, force, logger)
}

fn parse_cp(
    args: Vec<String>,
    create: bool,
    apply: bool,
    repo: Option<PathBuf>,
    logger: &mut Logger,
) -> Result<PrioResult, PrioError> {
    if args.len() < 2 {
        return Err(PrioError::Message(
            "Usage: prio cp [-c] [-a] <commit>... <destination>".into(),
        ));
    }

    let destination = args.last().unwrap().clone();
    let mut commits = args[..args.len() - 1].to_vec();

    if destination == "-c" || destination == "-a" {
        return Err(PrioError::Message(
            "Destination required before flags".into(),
        ));
    }

    while matches!(commits.last().map(|s| s.as_str()), Some("-c") | Some("-a")) {
        commits.pop();
    }

    cp::run(repo, commits, destination, create, apply, logger)
}

/// Verify that prio hooks are installed on both the work repo and the prio-mc clone.
/// Reinstalls any that are missing or stale and prints a warning line for each.
/// Errors are silently swallowed — a missing hook is not a hard failure.
fn check_hooks(repo: &Option<PathBuf>) {
    let repo_path = crate::util::absolute_path(
        repo.clone()
            .unwrap_or_else(|| std::env::current_dir().expect("current dir")),
    );

    // Only verify if prio has been set up for this repo (config.json must exist).
    if !prio_dir(&repo_path).join("config.json").exists() {
        return;
    }

    let mut reinstalled: Vec<&str> = Vec::new();

    if hooks::verify_and_reinstall(&repo_path, HookTarget::WorkClone) {
        reinstalled.push("work clone");
    }

    if let Ok(mc_path) = common::mc_path_for_repo(&repo_path, None) {
        if mc_path.join(".git").exists()
            && hooks::verify_and_reinstall(&mc_path, HookTarget::McClone)
        {
            reinstalled.push("prio-mc clone");
        }
    }

    if !reinstalled.is_empty() {
        eprintln!(
            "\x1b[33m⚠\x1b[0m  Hooks reinstalled ({})",
            reinstalled.join(", ")
        );
    }
}
