use std::path::PathBuf;

use clap::{Parser, Subcommand};

use crate::error::PrioError;
use crate::logger::{print_cli_result, Logger};
use crate::result::PrioResult;
use crate::services::{
    apply, mv, pr, prs, push, recover, setup, stack, status, sync, unsetup,
};

#[derive(Parser)]
#[command(
    name = "prio",
    display_name = "prio",
    about = "Stacked PR manager — merge-up, no rebase",
    override_usage = "prio                    # no subcommand opens UI\n       prio <COMMAND>            # CLI usage",
)]
pub struct CliApp {
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
        #[arg(long)]
        repo_path: Option<PathBuf>,
        #[arg(short = 'y', help = "Skip confirmation prompt")]
        yes: bool,
    },
    /// Show applied branches and unassigned commits
    Status {
        #[arg(long)]
        repo_path: Option<PathBuf>,
    },
    /// Apply branches or PRs to the work area
    Apply {
        branches: Vec<String>,
        #[arg(long)]
        repo_path: Option<PathBuf>,
    },
    /// Remove branches from the work area
    Unapply {
        branches: Vec<String>,
        #[arg(long)]
        repo_path: Option<PathBuf>,
    },
    /// Move commits to another branch
    Mv {
        #[arg(required = true)]
        args: Vec<String>,
        #[arg(short = 'c', help = "Create destination branch if missing (also applies it)")]
        create: bool,
        #[arg(short = 'a', help = "Apply destination branch into the work area")]
        apply: bool,
        #[arg(long)]
        repo_path: Option<PathBuf>,
    },
    /// Push a branch
    Push {
        branch: String,
        #[arg(long)]
        repo_path: Option<PathBuf>,
    },
    /// Create a draft PR for a branch
    Pr {
        branch: String,
        #[arg(long)]
        repo_path: Option<PathBuf>,
    },
    /// List open PRs showing which are applied to the work area
    Prs {
        #[arg(long)]
        repo_path: Option<PathBuf>,
    },
    /// Stack a branch after dependencies (use + for multiple deps)
    Stack {
        dependencies: String,
        branch: String,
        #[arg(long)]
        repo_path: Option<PathBuf>,
    },
    /// Remove stack metadata for a branch
    Unstack {
        branch: String,
        #[arg(long)]
        repo_path: Option<PathBuf>,
    },
    /// Sync merged branches for current repo
    Sync {
        #[arg(long)]
        repo_path: Option<PathBuf>,
    },
    /// Sync all known repos
    Syncs,
    /// Recover work branch to last known good state
    Recover {
        #[arg(long)]
        repo_path: Option<PathBuf>,
    },
    /// Suggest default work branch name (for UI)
    SuggestWorkBranch {
        repo_path: PathBuf,
    },
    /// Internal: work clone post-commit hook
    #[command(hide = true)]
    InternalWorkPostCommit {
        #[arg(long)]
        repo_path: Option<PathBuf>,
    },
    /// Internal: mc clone post-commit hook
    #[command(hide = true)]
    InternalMcPostCommit {
        #[arg(long)]
        mc_path: Option<PathBuf>,
    },
}

pub fn execute() -> Result<(), i32> {
    let app = CliApp::parse();
    let mut logger = Logger::Cli;

    let result = match app.command {
        Commands::Setup {
            repo_path,
            mc_path,
            work_branch,
        } => setup::run(
            repo_path,
            mc_path,
            work_branch.clone(),
            work_branch.is_none(),
            &mut logger,
        ),
        Commands::Unsetup { repo_path, yes } => unsetup::run(repo_path, true, yes, &mut logger),
        Commands::Status { repo_path } => match status::run(repo_path, &mut logger) {
            Ok(s) => {
                status::print_cli(&s);
                Ok(s.prio_result)
            }
            Err(e) => Err(e),
        },
        Commands::Apply { branches, repo_path } => {
            apply::run(repo_path, branches, false, &mut logger)
        }
        Commands::Unapply { branches, repo_path } => {
            apply::run(repo_path, branches, true, &mut logger)
        }
        Commands::Mv {
            args,
            create,
            apply,
            repo_path,
        } => parse_mv(args, create, apply, repo_path, &mut logger),
        Commands::Push { branch, repo_path } => push::run(repo_path, branch, &mut logger),
        Commands::Pr { branch, repo_path } => pr::run(repo_path, branch, &mut logger),
        Commands::Prs { repo_path } => match prs::run(repo_path, &mut logger) {
            Ok(()) => return Ok(()),
            Err(e) => Err(e),
        },
        Commands::Stack {
            dependencies,
            branch,
            repo_path,
        } => stack::run_stack(repo_path, dependencies, branch, &mut logger),
        Commands::Unstack { branch, repo_path } => {
            stack::run_unstack(repo_path, branch, &mut logger)
        }
        Commands::Sync { repo_path } => sync::run(repo_path, &mut logger),
        Commands::Syncs => sync::run_syncs(&mut logger),
        Commands::Recover { repo_path } => recover::run(repo_path, &mut logger),
        Commands::SuggestWorkBranch { repo_path } => match setup::suggest_work_branch(&repo_path) {
            Ok(s) => {
                println!("{}", serde_json::to_string(&s).unwrap());
                return Ok(());
            }
            Err(e) => Err(e),
        },
        Commands::InternalWorkPostCommit { repo_path } => {
            apply::work_post_commit(repo_path, &mut logger)
        }
        Commands::InternalMcPostCommit { mc_path } => {
            apply::mc_post_commit(mc_path, &mut logger)
        }
    };

    match result {
        Ok(r) => {
            print_cli_result(&r);
            if r.status == crate::result::PrioStatus::Failure {
                return Err(1);
            }
            Ok(())
        }
        Err(e) => {
            let r = PrioResult::from_error(e, logger.drain());
            print_cli_result(&r);
            Err(1)
        }
    }
}

fn parse_mv(
    args: Vec<String>,
    create: bool,
    apply: bool,
    repo_path: Option<PathBuf>,
    logger: &mut Logger,
) -> Result<PrioResult, PrioError> {
    if args.len() < 2 {
        return Err(PrioError::Message(
            "Usage: prio mv <commit>... <destination> [-c] [-a]".into(),
        ));
    }

    let destination = args.last().unwrap().clone();
    let mut commits = args[..args.len() - 1].to_vec();

    if destination == "-c" || destination == "-a" {
        return Err(PrioError::Message(
            "Destination required before -c or -a".into(),
        ));
    }

    while commits.last().map(|s| s.as_str()) == Some("-c") || commits.last().map(|s| s.as_str()) == Some("-a")
    {
        commits.pop();
    }

    mv::run(repo_path, commits, destination, create, apply, logger)
}
