use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::error::PrioError;
use crate::git::runner;
use crate::logger::Logger;
use crate::result::PrioResult;
use crate::services::common::{assert_on_work_branch, mc_path_for_repo, resolve_repo_path};
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommitInfo {
    pub sha: String,
    pub message: String,
    pub branch: String,
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusResult {
    pub data: StatusData,
    pub prio_result: PrioResult,
}

pub fn run(repo_path: Option<PathBuf>, logger: &mut Logger) -> Result<StatusResult, PrioError> {
    let repo_path = resolve_repo_path(repo_path)?;
    assert_on_work_branch(&repo_path, logger)?;

    let state = repo_state::load_state(&repo_path)?;

    // ── Conflict state ────────────────────────────────────────────────────────
    // Load mc_state to get accurate pending_merge_index (it advances with each
    // continued conflict resolution, whereas the work-clone state is only
    // updated when the whole apply finishes).
    let mc_conflict: Option<MergeConflictInfo> = if state.merge_in_progress {
        mc_path_for_repo(&repo_path, None).ok().and_then(|mc_path| {
            repo_state::load_mc_state(&mc_path).ok().map(|mc_state| {
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
                MergeConflictInfo {
                    mc_path: mc_path.to_string_lossy().into_owned(),
                    merge_branch: mc_state.merge_branch.clone(),
                    incoming_branch: incoming,
                    base_desc,
                    branches_merged: merged,
                    branches_pending: pending,
                }
            })
        })
    } else {
        None
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

    // ── Commits ───────────────────────────────────────────────────────────────
    let log_out = runner::run_git(
        &[
            "log",
            &format!("{}..HEAD", state.baseline_commit),
            "--format=%H|%s",
        ],
        &repo_path,
        logger,
    )?;

    let mut by_branch: HashMap<String, Vec<CommitInfo>> = HashMap::new();
    let mut unassigned_commits = Vec::new();

    for line in log_out.lines().filter(|l| !l.is_empty()) {
        let mut parts = line.splitn(2, '|');
        let sha = parts.next().unwrap_or("").to_string();
        let message = parts.next().unwrap_or("").to_string();
        let assignment = state
            .commit_assignments
            .get(&sha)
            .map(String::as_str)
            .unwrap_or(".");
        let info = CommitInfo {
            sha,
            message,
            branch: assignment.to_string(),
        };
        if assignment == "." {
            unassigned_commits.push(info);
        } else {
            by_branch
                .entry(assignment.to_string())
                .or_default()
                .push(info);
        }
    }

    // ── Applied branches ──────────────────────────────────────────────────────
    let mut applied_branches = Vec::new();
    for branch in &state.applied_branches {
        let commits = by_branch.remove(branch).unwrap_or_default();
        let (pr_number, pr_url) = resolve_pr_info(branch, &repo_path, logger);
        applied_branches.push(BranchInfo {
            name: branch.clone(),
            pr_number,
            pr_url,
            commits,
            apply_status: branch_status.get(branch.as_str()).map(|s| s.to_string()),
        });
    }

    // Commits assigned to a branch not in applied_branches (shouldn't normally
    // happen, but surface them rather than silently dropping).
    let mut extra: Vec<_> = by_branch.into_iter().collect();
    extra.sort_by(|a, b| a.0.cmp(&b.0));
    for (name, commits) in extra {
        let (pr_number, pr_url) = resolve_pr_info(&name, &repo_path, logger);
        applied_branches.push(BranchInfo {
            name: name.clone(),
            pr_number,
            pr_url,
            commits,
            apply_status: None,
        });
    }

    let logs = logger.drain();
    Ok(StatusResult {
        data: StatusData {
            applied_branches,
            unassigned_commits,
            merge_conflict: mc_conflict,
        },
        prio_result: PrioResult::success("Status loaded.", logs),
    })
}

fn resolve_pr_info(branch: &str, repo_path: &std::path::Path, logger: &mut Logger) -> (Option<u32>, Option<String>) {
    let json = match runner::run_gh(
        &["pr", "list", "--head", branch, "--json", "number,url", "--limit", "1"],
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
    let number = first.get("number").and_then(|n| n.as_u64()).map(|n| n as u32);
    let url = first.get("url").and_then(|u| u.as_str()).map(String::from);
    (number, url)
}

pub fn print_cli(status: &StatusResult) {
    // ── Merge conflict banner ─────────────────────────────────────────────────
    if let Some(ref info) = status.data.merge_conflict {
        println!("⚠  Merge conflict in prio-mc");
        println!("   Incoming: {}  →  merging into: ({})", info.incoming_branch, info.base_desc);
        if !info.merge_branch.is_empty() {
            println!("   Branch:   {}", info.merge_branch);
        }
        println!();
        println!("   Resolve conflicts in:  {}", info.mc_path);
        println!("   When done, run:");
        println!("     git -C \"{}\" commit --no-edit", info.mc_path);
        println!("   Prio will automatically finish applying the remaining branches.");
        println!("   (Or run `prio unapply {}` to discard and cancel.)", info.incoming_branch);
        println!();
    }

    // ── Applied branches ──────────────────────────────────────────────────────
    println!("Applied Branches:");
    if status.data.applied_branches.is_empty() {
        println!("  (none)");
    } else {
        for b in &status.data.applied_branches {
            let (icon, note) = match b.apply_status.as_deref() {
                Some("merged")   => ("↻", " (merged in prio-mc — awaiting conflict resolution)"),
                Some("conflict") => ("✗", " ← merge conflict here"),
                Some("pending")  => ("·", " (pending)"),
                _                => ("✓", ""),
            };
            let pr_tag = match (&b.pr_url, b.pr_number) {
                (Some(url), _)     => format!("  {url}"),
                (None, Some(num))  => format!("  (PR #{num})"),
                _                  => String::new(),
            };
            println!("  {icon}  {}{pr_tag}{note}", b.name);
            for c in &b.commits {
                println!(
                    "       {}  {}",
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
}
