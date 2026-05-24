use std::path::PathBuf;

use serde::Deserialize;

use crate::error::PrioError;
use crate::git::runner;
use crate::logger::Logger;
use crate::services::common::resolve_repo_path;
use crate::storage::repo_state;

#[derive(Deserialize)]
struct GhPr {
    number: u64,
    title: String,
    #[serde(rename = "headRefName")]
    head_ref: String,
}

pub fn run(repo_path: Option<PathBuf>, logger: &mut Logger) -> Result<(), PrioError> {
    let repo_path = resolve_repo_path(repo_path)?;
    let state = repo_state::load_state(&repo_path)?;

    let json = runner::run_gh(
        &[
            "pr", "list",
            "--json", "number,title,headRefName",
            "--limit", "50",
        ],
        &repo_path,
        logger,
    )?;

    let prs: Vec<GhPr> = serde_json::from_str(&json).unwrap_or_default();

    if prs.is_empty() {
        println!("No open pull requests.");
        return Ok(());
    }

    // Column widths for alignment.
    let num_width = prs.iter()
        .map(|p| format!("#{}", p.number).len())
        .max()
        .unwrap_or(3);
    let branch_width = prs.iter()
        .map(|p| p.head_ref.len())
        .max()
        .unwrap_or(10)
        .min(40);

    // Track the first unapplied PR number for the usage hint.
    let mut example_num: Option<u64> = None;

    for pr in &prs {
        let is_applied = state.applied_branches.contains(&pr.head_ref);
        let (emoji, label) = if is_applied {
            ("✅", "applied  ")
        } else {
            ("⬜", "unapplied")
        };
        if !is_applied && example_num.is_none() {
            example_num = Some(pr.number);
        }
        println!(
            " {} {}  {:<num_width$}  {:<branch_width$}  {}",
            emoji,
            label,
            format!("#{}", pr.number),
            pr.head_ref,
            pr.title,
        );
    }

    let example = example_num
        .or_else(|| prs.first().map(|p| p.number))
        .unwrap_or(1);
    println!();
    println!("To apply a PR:  prio apply pr-{example}");

    Ok(())
}
