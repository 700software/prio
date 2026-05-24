use std::path::Path;

use crate::error::PrioError;
use crate::logger::Logger;
use crate::services::apply::score_merge_order;
use crate::storage::repo_state;

pub fn run_and_log(repo_path: &Path, mc_path: &Path, logger: &mut Logger) -> Result<Vec<String>, PrioError> {
    let suggestions = run_suggestions(repo_path, mc_path, logger)?;
    for s in &suggestions {
        logger.info(s.clone());
    }
    Ok(suggestions)
}

pub fn run_suggestions(
    repo_path: &Path,
    mc_path: &Path,
    logger: &mut Logger,
) -> Result<Vec<String>, PrioError> {
    let state = repo_state::load_state(repo_path)?;
    let mut out = Vec::new();

    if state.applied_branches.len() < 2 {
        return Ok(out);
    }

    for i in 0..state.applied_branches.len() {
        for j in (i + 1)..state.applied_branches.len() {
            let a = &state.applied_branches[i];
            let b = &state.applied_branches[j];

            let stacked = state.stacks.iter().any(|s| {
                s.branch == *b && s.dependency.contains(a.as_str())
                    || s.branch == *a && s.dependency.contains(b.as_str())
            });

            let pair = vec![a.clone(), b.clone()];
            let conflicts = score_merge_order(&pair, mc_path, &state.default_branch, logger)?;

            if conflicts == 0 && !stacked {
                out.push(format!(
                    "{b} has no conflicts with {a} — consider: prio stack {a} {b}"
                ));
            } else if conflicts > 0 && !stacked {
                out.push(format!(
                    "{a} and {b} may conflict when merged — consider: prio stack {a} {b}"
                ));
            } else if stacked && conflicts > 2 {
                out.push(format!(
                    "Stacking {a} and {b} may be unnecessary given conflict history"
                ));
            }
        }
    }

    Ok(out)
}
