use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::error::PrioError;
use crate::git::runner;
use crate::logger::Logger;
use crate::result::PrioResult;
use crate::services::apply;
use crate::services::common::{
    assert_on_work_branch, assignment_for_work_sha, branch_tip_ref, mc_path_for_repo,
    resolve_repo_path,
};
use crate::storage::repo_state;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BranchInfo {
    pub name: String,
    pub pr_number: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pr_url: Option<String>,
    #[serde(default)]
    pub commits: Vec<CommitInfo>,
    /// Merge pipeline state: "applied" | "merged" | "conflict" | "pending".
    /// "applied" (default/absent) = fully in the work clone.
    /// "merged"  = merged in prio-mc only — waiting for conflict resolution downstream.
    /// "conflict"= this branch is the one with the active merge conflict.
    /// "pending" = queued but not yet merged.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub apply_status: Option<String>,
    /// True when the branch is listed in `applied_branches` (merge-up active).
    pub applied: bool,
    /// Branches this branch is stacked after (from `prio stack`), if any.
    /// Empty when the branch has no declared stack dependencies.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stacked_after: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommitInfo {
    pub sha: String,
    pub message: String,
    pub branch: String,
    /// True when the commit is actually present on the branch's git ref
    /// (i.e. from `git log default..origin/<branch>`).
    /// False when it exists only above the work-area baseline (assigned via
    /// commit_assignments but not yet cherry-picked/merged into the branch).
    #[serde(default)]
    pub on_branch: bool,
}

/// Present only when a merge conflict is in progress in the prio-mc clone.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MergeConflictInfo {
    /// Absolute path to the prio-mc clone.
    pub mc_path: String,
    /// The git branch name inside prio-mc where the conflict lives.
    pub merge_branch: String,
    /// The branch whose merge triggered the conflict.
    pub incoming_branch: String,
    /// Description of what has already been merged (e.g. "main + feature-a").
    pub base_desc: String,
    /// Branches that were successfully merged before the conflict.
    pub branches_merged: Vec<String>,
    /// Branches not yet attempted (after the conflicting one).
    pub branches_pending: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusData {
    pub applied_branches: Vec<BranchInfo>,
    pub unassigned_commits: Vec<CommitInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub merge_conflict: Option<MergeConflictInfo>,
    /// Present when `prio mv -f` left a cherry-pick conflict in prio-mc while
    /// rebasing the source branch.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mv_rebase_conflict: Option<repo_state::MvRebaseConflict>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusResult {
    pub data: StatusData,
    pub prio_result: PrioResult,
}

pub fn run(repo_path: Option<PathBuf>, logger: &mut Logger) -> Result<StatusResult, PrioError> {
    let repo_path = resolve_repo_path(repo_path)?;
    assert_on_work_branch(&repo_path, logger)?;

    let mut state = repo_state::load_state(&repo_path)?;
    let mut stale_conflict_cleared = false;

    // ── Conflict state ────────────────────────────────────────────────────────
    // Load mc_state to get accurate pending_merge_index (it advances with each
    // continued conflict resolution, whereas the work-clone state is only
    // updated when the whole apply finishes).
    //
    // Uses a labeled block so we can freely mutate `state`, probe git, and call
    // apply::run for auto-recovery without the closure ownership restrictions of
    // the previous and_then formulation.
    let mc_conflict: Option<MergeConflictInfo> = 'conflict: {
        if !state.merge_in_progress {
            break 'conflict None;
        }
        let mc_path = match mc_path_for_repo(&repo_path, None) {
            Ok(p) => p,
            Err(_) => break 'conflict None,
        };
        let mc_state = match repo_state::load_mc_state(&mc_path) {
            Ok(s) => s,
            Err(_) => break 'conflict None,
        };

        // ── Stale-flag detection (JSON level) ────────────────────────────────
        // state.merge_in_progress can be left true by interrupted operations.
        // Trust mc_state as ground truth: if mc says no merge and no pending
        // branches, the flag is stale.
        let mc_actually_in_conflict =
            mc_state.merge_in_progress || !mc_state.pending_merge_branches.is_empty();
        if !mc_actually_in_conflict {
            state.merge_in_progress = false;
            state.pending_merge_branches.clear();
            state.pending_merge_index = 0;
            let _ = repo_state::save_state(&repo_path, &state);
            stale_conflict_cleared = true;
            break 'conflict None;
        }

        // ── Git-level probe: is MERGE_HEAD actually present? ─────────────────
        // mc JSON says a merge conflict is in progress, but MERGE_HEAD may be
        // absent for two different reasons:
        // 1. The user resolved and committed the conflict, but the post-commit
        //    hook did not run. Continue the merge from the existing merge branch.
        // 2. Another command wiped prio-mc while state was left dirty. Re-apply
        //    from scratch so the conflict is materialized again.
        let mut temp = Logger::Cli;
        let mc_git_merge_active = runner::run_git(
            &["rev-parse", "--verify", "MERGE_HEAD"],
            &mc_path,
            &mut temp,
        )
        .is_ok();

        if !mc_git_merge_active {
            let current_branch =
                runner::run_git(&["branch", "--show-current"], &mc_path, &mut temp)
                    .unwrap_or_default();
            if !mc_state.merge_branch.is_empty() && current_branch.trim() == mc_state.merge_branch {
                logger.info(
                    "⚠  Conflict was resolved but the prio-mc post-commit hook did not \
                     run. Continuing the merge…"
                        .to_string(),
                );

                let _ = apply::mc_post_commit(Some(mc_path.clone()), logger);

                state = match repo_state::load_state(&repo_path) {
                    Ok(s) => s,
                    Err(_) => break 'conflict None,
                };

                if !state.merge_in_progress {
                    break 'conflict None;
                }

                let mc_state2 = match repo_state::load_mc_state(&mc_path) {
                    Ok(s) => s,
                    Err(_) => break 'conflict None,
                };
                let at = mc_state2.pending_merge_index;
                let order = &mc_state2.pending_merge_branches;
                let incoming = order.get(at).cloned().unwrap_or_default();
                let merged = order[..at].to_vec();
                let pending = order.get(at + 1..).unwrap_or(&[]).to_vec();
                let base_desc = if merged.is_empty() {
                    mc_state2.default_branch.clone()
                } else {
                    format!("{} + {}", mc_state2.default_branch, merged.join(" + "))
                };
                break 'conflict Some(MergeConflictInfo {
                    mc_path: mc_path.to_string_lossy().into_owned(),
                    merge_branch: mc_state2.merge_branch.clone(),
                    incoming_branch: incoming,
                    base_desc,
                    branches_merged: merged,
                    branches_pending: pending,
                });
            }

            logger.info(
                "⚠  Detected stale conflict state: prio-mc was cleaned up without \
                 updating state.  Re-applying to restore the conflict…"
                    .to_string(),
            );

            // Clear stale flags in both clones before re-running apply.
            state.merge_in_progress = false;
            state.pending_merge_branches.clear();
            state.pending_merge_index = 0;
            let _ = repo_state::save_state(&repo_path, &state);

            let mut stale_mc = mc_state.clone();
            stale_mc.merge_in_progress = false;
            stale_mc.pending_merge_branches.clear();
            stale_mc.pending_merge_index = 0;
            let _ = repo_state::save_mc_state(&mc_path, &stale_mc);

            // Re-run apply with the same applied set.  If the conflict still
            // exists it will be re-materialized in prio-mc and state will be
            // updated to merge_in_progress = true again.
            let _ = apply::run(Some(repo_path.clone()), vec![], false, logger);

            // Reload state so the rest of status reflects the new reality.
            state = match repo_state::load_state(&repo_path) {
                Ok(s) => s,
                Err(_) => break 'conflict None,
            };

            if !state.merge_in_progress {
                // Re-apply succeeded (no conflict this time) — status is clean.
                break 'conflict None;
            }

            // Re-apply hit the conflict again — reload mc_state and fall through
            // to the normal conflict-info builder below.
            let mc_state2 = match repo_state::load_mc_state(&mc_path) {
                Ok(s) => s,
                Err(_) => break 'conflict None,
            };
            let at = mc_state2.pending_merge_index;
            let order = &mc_state2.pending_merge_branches;
            let incoming = order.get(at).cloned().unwrap_or_default();
            let merged = order[..at].to_vec();
            let pending = order.get(at + 1..).unwrap_or(&[]).to_vec();
            let base_desc = if merged.is_empty() {
                mc_state2.default_branch.clone()
            } else {
                format!("{} + {}", mc_state2.default_branch, merged.join(" + "))
            };
            break 'conflict Some(MergeConflictInfo {
                mc_path: mc_path.to_string_lossy().into_owned(),
                merge_branch: mc_state2.merge_branch.clone(),
                incoming_branch: incoming,
                base_desc,
                branches_merged: merged,
                branches_pending: pending,
            });
        }

        // Normal path: MERGE_HEAD is present — build conflict info from mc_state.
        let at = mc_state.pending_merge_index;
        let order = &mc_state.pending_merge_branches;
        let incoming = order.get(at).cloned().unwrap_or_default();
        let merged = order[..at].to_vec();
        let pending = order.get(at + 1..).unwrap_or(&[]).to_vec();
        let base_desc = if merged.is_empty() {
            mc_state.default_branch.clone()
        } else {
            format!("{} + {}", mc_state.default_branch, merged.join(" + "))
        };
        Some(MergeConflictInfo {
            mc_path: mc_path.to_string_lossy().into_owned(),
            merge_branch: mc_state.merge_branch.clone(),
            incoming_branch: incoming,
            base_desc,
            branches_merged: merged,
            branches_pending: pending,
        })
    };

    // Map each branch name → apply_status when a conflict is in progress.
    let branch_status: HashMap<String, &'static str> = if let Some(ref info) = mc_conflict {
        let at = state.pending_merge_index;
        state
            .pending_merge_branches
            .iter()
            .enumerate()
            .map(|(i, b)| {
                let s = if i < at {
                    "merged"
                } else if b == &info.incoming_branch {
                    "conflict"
                } else {
                    "pending"
                };
                (b.clone(), s)
            })
            .collect()
    } else {
        HashMap::new()
    };

    // ── Work-area commits above baseline (for unassigned + pending-on-branch) ─
    let log_out = runner::run_git(
        &[
            "log",
            &format!("{}..HEAD", state.baseline_commit),
            "--format=%H|%s",
        ],
        &repo_path,
        logger,
    )?;

    let mut above_baseline: Vec<(String, String, String)> = Vec::new();
    let mut unassigned_commits = Vec::new();

    for line in log_out.lines().filter(|l| !l.is_empty()) {
        let mut parts = line.splitn(2, '|');
        let sha = parts.next().unwrap_or("").to_string();
        let message = parts.next().unwrap_or("").to_string();
        let assignment = assignment_for_work_sha(&sha, &state).to_string();
        if assignment == "." {
            unassigned_commits.push(CommitInfo {
                sha,
                message,
                branch: ".".to_string(),
                on_branch: false,
            });
        } else {
            above_baseline.push((sha, message, assignment));
        }
    }

    // ── Applied branches (git history is source of truth) ─────────────────────
    let mut applied_branches = Vec::new();
    for branch in &state.applied_branches {
        let mut commits = Vec::new();
        let mut on_branch_shas = std::collections::HashSet::new();

        // Collect stack dependencies for this branch (if any).
        let stacked_after: Vec<String> = state
            .stacks
            .iter()
            .find(|s| s.branch == *branch)
            .map(|entry| entry.dependencies.clone())
            .unwrap_or_default();

        if let Ok(tip_ref) = branch_tip_ref(branch, &repo_path, logger) {
            // When stacked, resolve dep tip refs so we can exclude their commits.
            let dep_tips: Vec<String> = stacked_after
                .iter()
                .filter_map(|d| branch_tip_ref(d, &repo_path, logger).ok())
                .collect();

            // Build git log args:
            //   - Unstacked: `git log <default>..<tip> --format=...`
            //     (range notation is unambiguous)
            //   - Stacked:   `git log <tip> --not <default> <dep1> [dep2...] --format=... --`
            //     This excludes all commits reachable from the dependency branches,
            //     showing only commits unique to this branch.
            //     The trailing `--` disambiguates revision refs from filesystem paths
            //     (needed when a branch name matches a directory, e.g. a `tim/` subdir).
            let log_args_owned: Vec<String> = if dep_tips.is_empty() {
                vec![
                    "log".into(),
                    format!("{}..{}", state.default_branch, tip_ref),
                    "--format=%H|%s".into(),
                ]
            } else {
                let mut args = vec![
                    "log".to_string(),
                    tip_ref.clone(),
                    "--not".to_string(),
                    state.default_branch.clone(),
                ];
                args.extend(dep_tips);
                args.push("--format=%H|%s".to_string());
                args.push("--".to_string());
                args
            };
            let log_args: Vec<&str> = log_args_owned.iter().map(|s| s.as_str()).collect();

            if let Ok(branch_log) = runner::run_git(&log_args, &repo_path, logger) {
                for line in branch_log.lines().filter(|l| !l.is_empty()) {
                    let mut parts = line.splitn(2, '|');
                    let sha = parts.next().unwrap_or("").to_string();
                    let message = parts.next().unwrap_or("").to_string();
                    on_branch_shas.insert(sha.clone());
                    commits.push(CommitInfo {
                        sha,
                        message,
                        branch: branch.clone(),
                        on_branch: true,
                    });
                }
            }
        }

        // Work-area commits assigned here but not yet on the branch tip (pre-apply).
        for (sha, message, assignment) in &above_baseline {
            if assignment != branch {
                continue;
            }
            if on_branch_shas.contains(sha) {
                continue;
            }
            if state
                .commit_map
                .get(sha)
                .is_some_and(|mapped| on_branch_shas.contains(mapped))
            {
                continue;
            }
            commits.insert(
                0,
                CommitInfo {
                    sha: sha.clone(),
                    message: message.clone(),
                    branch: branch.clone(),
                    on_branch: false,
                },
            );
        }

        let (pr_number, pr_url) = resolve_pr_info(branch, &repo_path, logger);
        applied_branches.push(BranchInfo {
            name: branch.clone(),
            pr_number,
            pr_url,
            commits,
            apply_status: branch_status.get(branch.as_str()).map(|s| s.to_string()),
            applied: true,
            stacked_after,
        });
    }

    // Branches with assignments but not in applied_branches.
    let mut extra_branches: HashMap<String, Vec<CommitInfo>> = HashMap::new();
    for (sha, message, assignment) in above_baseline {
        if state.applied_branches.contains(&assignment) {
            continue;
        }
        extra_branches
            .entry(assignment.clone())
            .or_default()
            .push(CommitInfo {
                sha,
                message,
                branch: assignment,
                on_branch: false,
            });
    }
    let mut extra: Vec<_> = extra_branches.into_iter().collect();
    extra.sort_by(|a, b| a.0.cmp(&b.0));
    for (name, commits) in extra {
        let (pr_number, pr_url) = resolve_pr_info(&name, &repo_path, logger);
        let stacked_after: Vec<String> = state
            .stacks
            .iter()
            .find(|s| s.branch == name)
            .map(|entry| entry.dependencies.clone())
            .unwrap_or_default();
        applied_branches.push(BranchInfo {
            name: name.clone(),
            pr_number,
            pr_url,
            commits,
            apply_status: None,
            applied: false,
            stacked_after,
        });
    }

    // Commits on an applied branch's git ref are never unassigned.
    let all_branch_shas: std::collections::HashSet<&str> = applied_branches
        .iter()
        .flat_map(|b| b.commits.iter())
        .filter(|c| c.on_branch)
        .map(|c| c.sha.as_str())
        .collect();
    unassigned_commits.retain(|c| !all_branch_shas.contains(c.sha.as_str()));

    // ── mv rebase conflict state ──────────────────────────────────────────────
    // Check for a live cherry-pick in prio-mc that was left by `prio mv -f`.
    // Auto-clear stale flags when prio-mc no longer has an active cherry-pick.
    let mv_rebase_conflict: Option<repo_state::MvRebaseConflict> =
        if let Some(ref conflict) = state.mv_rebase_conflict {
            let mc_path_buf = mc_path_for_repo(&repo_path, None).ok();
            let cherry_pick_still_active = mc_path_buf.as_ref().map_or(false, |mc| {
                let mut temp = crate::logger::Logger::Cli;
                crate::git::runner::run_git(
                    &["rev-parse", "--verify", "CHERRY_PICK_HEAD"],
                    mc,
                    &mut temp,
                )
                .is_ok()
            });
            if cherry_pick_still_active {
                Some(conflict.clone())
            } else {
                // Stale flag — clear it.
                state.mv_rebase_conflict = None;
                let _ = repo_state::save_state(&repo_path, &state);
                None
            }
        } else {
            None
        };

    let logs = logger.drain();
    let message = if stale_conflict_cleared {
        "Stale conflict flag cleared — prio-mc has no active merge or cherry-pick in progress. \
         Status is clean."
    } else {
        "Status loaded."
    };
    Ok(StatusResult {
        data: StatusData {
            applied_branches,
            unassigned_commits,
            merge_conflict: mc_conflict,
            mv_rebase_conflict,
        },
        prio_result: if stale_conflict_cleared {
            PrioResult::warning(message, logs)
        } else {
            PrioResult::success(message, logs)
        },
    })
}

fn resolve_pr_info(
    branch: &str,
    repo_path: &std::path::Path,
    logger: &mut Logger,
) -> (Option<u32>, Option<String>) {
    let json = match runner::run_gh(
        &[
            "pr",
            "list",
            "--head",
            branch,
            "--json",
            "number,url",
            "--limit",
            "1",
        ],
        repo_path,
        logger,
    ) {
        Ok(j) => j,
        Err(_) => return (None, None),
    };
    let v: serde_json::Value = match serde_json::from_str(&json) {
        Ok(v) => v,
        Err(_) => return (None, None),
    };
    let first = match v.as_array().and_then(|a| a.first()) {
        Some(f) => f,
        None => return (None, None),
    };
    let number = first
        .get("number")
        .and_then(|n| n.as_u64())
        .map(|n| n as u32);
    let url = first.get("url").and_then(|u| u.as_str()).map(String::from);
    (number, url)
}

pub fn print_cli(status: &StatusResult) {
    // ── mv cherry-pick / rebase conflict banner ──────────────────────────────
    if let Some(ref info) = status.data.mv_rebase_conflict {
        let short_sha = &info.conflicting_commit[..12.min(info.conflicting_commit.len())];
        if info.phase == "cp" {
            println!(
                "⚠  Cherry-pick conflict copying {} to '{}' (prio cp):",
                short_sha, info.dest_branch
            );
            println!(
                "   Resolve the conflict, then continue the cherry-pick on '{}'.",
                info.dest_branch
            );
        } else if info.phase == "dest" {
            println!(
                "⚠  Cherry-pick conflict adding {} to '{}' (prio mv):",
                short_sha, info.dest_branch
            );
            println!(
                "   Resolve the conflict, then commit to apply '{}' onto '{}'.",
                short_sha, info.dest_branch
            );
        } else {
            println!(
                "⚠  Cherry-pick conflict rebasing '{}' for prio mv:",
                info.source_branch
            );
            println!(
                "   Conflicting commit: {}  (dest '{}' already updated ✓)",
                short_sha, info.dest_branch
            );
        }
        let active_branch = if info.phase == "dest" || info.phase == "cp" {
            &info.dest_branch
        } else {
            &info.source_branch
        };
        println!();
        println!("   Resolve conflicts in: {}", info.mc_path);
        println!("   Branch: {active_branch}");
        println!();
        println!("   To accept and continue:");
        println!("     git -C \"{}\" cherry-pick --continue", info.mc_path);
        println!("   Prio will automatically complete the operation after you commit.");
        println!();
        println!("   To abort:");
        println!("     prio abort");
        println!();
    }

    // ── Merge conflict banner ─────────────────────────────────────────────────
    if let Some(ref info) = status.data.merge_conflict {
        println!("⚠  Merge conflict in prio-mc");
        println!(
            "   Incoming: {}  →  merging into: ({})",
            info.incoming_branch, info.base_desc
        );
        if !info.merge_branch.is_empty() {
            println!("   Branch:   {}", info.merge_branch);
        }
        println!();
        println!("   Resolve conflicts in:  {}", info.mc_path);
        println!("   When done, run:");
        println!("     git -C \"{}\" commit --no-edit", info.mc_path);
        println!("   Prio will automatically finish applying the remaining branches.");
        println!(
            "   (Or run `prio unapply {}` to discard and cancel.)",
            info.incoming_branch
        );
        println!();
    }

    // ── Applied branches ──────────────────────────────────────────────────────
    println!("Applied Branches:");
    if status.data.applied_branches.is_empty() {
        println!("  (none)");
    } else {
        for b in &status.data.applied_branches {
            let (icon, note) = match b.apply_status.as_deref() {
                Some("merged") => ("↻", " (merged in prio-mc — awaiting conflict resolution)"),
                Some("conflict") => ("✗", " ← merge conflict here"),
                Some("pending") => ("·", " (pending)"),
                _ => ("✓", ""),
            };
            let pr_tag = match (&b.pr_url, b.pr_number) {
                (Some(url), _) => format!("  {url}"),
                (None, Some(num)) => format!("  (PR #{num})"),
                _ => String::new(),
            };
            let stack_label = if b.stacked_after.is_empty() {
                String::new()
            } else {
                format!("  (stacked after: {})", b.stacked_after.join(", "))
            };
            println!("  {icon}  {}{pr_tag}{note}{stack_label}", b.name);
            for c in &b.commits {
                let pending = if c.on_branch { "" } else { "  (pending apply)" };
                println!(
                    "       {}  {}{pending}",
                    &c.sha[..7.min(c.sha.len())],
                    c.message
                );
            }
        }
    }

    // ── Unassigned commits ────────────────────────────────────────────────────
    println!("\nUnassigned Commits:");
    if status.data.unassigned_commits.is_empty() {
        println!("  (none)");
    } else {
        for c in &status.data.unassigned_commits {
            println!("  {}  {}", &c.sha[..7.min(c.sha.len())], c.message);
        }
    }

    // ── Recovery advisory (work area out of sync — not normal after `prio mv`) ─
    let pending_count: usize = status
        .data
        .applied_branches
        .iter()
        .flat_map(|b| &b.commits)
        .filter(|c| !c.on_branch)
        .count();

    if pending_count > 0 {
        println!();
        println!(
            "⚠  {pending_count} commit(s) are assigned to a branch in prio metadata but \
             not yet in the merged work area (see \"pending apply\" above)."
        );
        println!(
            "   This usually means `prio apply` did not run after a move, or was interrupted."
        );
        println!();
        if !status.data.unassigned_commits.is_empty() {
            println!(
                "   Recovery: assign unassigned commits (`prio mv <sha> <branch>`), then run \
                 `prio apply`."
            );
        } else {
            println!("   Recovery: run `prio apply` to rebuild the work area.");
        }
        println!("   Warning: `prio apply` discards any above-baseline commits still unassigned.");
    }
}
