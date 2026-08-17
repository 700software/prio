use std::io::{self, Write};
use std::path::Path;

use crate::error::PrioError;
use crate::git::runner;
use crate::logger::Logger;
use crate::storage::user_config;

/// Resolve the default work branch name (`prio/<slug>`) for setup and suggestions.
pub fn resolve(
    repo_path: &Path,
    override_name: Option<String>,
    interactive: bool,
    logger: &mut Logger,
) -> Result<String, PrioError> {
    if let Some(name) = override_name {
        return Ok(name);
    }
    let user_cfg = user_config::load_config()?;
    if let Some(prefix) = user_cfg.work_branch_prefix {
        return Ok(prefix);
    }

    // 1. Authenticated GitHub CLI user (gh has no `whoami`; use `gh api user`).
    if let Some(login) = runner::gh_user_login(repo_path, logger) {
        return Ok(format!("prio/{login}"));
    }

    let email = git_user_email(repo_path, logger);

    // 2. git user.email cross-referenced with the unauthenticated GitHub search API.
    if let Some(login) = github_login_for_email(&email) {
        return Ok(format!("prio/{login}"));
    }

    if interactive {
        if !runner::is_gh_installed() {
            if prompt_yes_no(
                "GitHub CLI is not installed. Install it for the best prio experience? [y/n] ",
            )? {
                return Err(PrioError::Message(format!(
                    "GitHub CLI is not installed.\n\n{}\n\nRe-run `prio setup` after installing.",
                    runner::gh_install_hint()
                )));
            }
        } else if !runner::is_gh_authenticated(repo_path) {
            if prompt_yes_no(
                "GitHub CLI is not authenticated. Log in with gh for the best prio experience? [y/n] ",
            )? {
                return Err(PrioError::Message(format!(
                    "{}\n\nRe-run `prio setup` after logging in.",
                    runner::gh_auth_hint()
                )));
            }
        }
    }

    // 5. Local part of user.email when gh lookup and API search did not succeed.
    if let Some(local) = email_local_part(&email) {
        return Ok(format!("prio/{local}"));
    }

    // 6. OS username when email is unavailable.
    Ok(format!("prio/{}", os_username()))
}

fn git_user_email(repo_path: &Path, logger: &mut Logger) -> String {
    runner::run_git(&["config", "user.email"], repo_path, logger)
        .unwrap_or_default()
        .trim()
        .to_string()
}

fn github_login_for_email(email: &str) -> Option<String> {
    if email.is_empty() {
        return None;
    }
    let client = reqwest::blocking::Client::builder()
        .user_agent("prio")
        .build()
        .ok()?;
    let url = format!(
        "https://api.github.com/search/users?q={}",
        percent_encode_query(email)
    );
    let resp: serde_json::Value = client.get(&url).send().ok()?.json().ok()?;
    let items = resp.get("items")?.as_array()?;
    let first = items.first()?;
    first.get("login")?.as_str().map(str::to_string)
}

fn email_local_part(email: &str) -> Option<String> {
    let local = email.split('@').next()?.trim();
    if local.is_empty() {
        None
    } else {
        Some(local.to_string())
    }
}

fn os_username() -> String {
    #[cfg(windows)]
    {
        return std::env::var("USERNAME").unwrap_or_else(|_| "user".into());
    }
    #[cfg(not(windows))]
    {
        std::env::var("USER")
            .or_else(|_| std::env::var("LOGNAME"))
            .unwrap_or_else(|_| "user".into())
    }
}

fn prompt_yes_no(question: &str) -> Result<bool, PrioError> {
    print!("{question}");
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let trimmed = input.trim().to_lowercase();
    Ok(trimmed == "y" || trimmed == "yes")
}

fn percent_encode_query(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn email_local_part_strips_domain() {
        assert_eq!(
            email_local_part("bfield@example.com").as_deref(),
            Some("bfield")
        );
    }

    #[test]
    fn percent_encode_encodes_at_sign() {
        assert_eq!(percent_encode_query("a@b.com"), "a%40b.com");
    }
}
