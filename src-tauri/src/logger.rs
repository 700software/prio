use serde::{Deserialize, Serialize};

use crate::util;

use crate::error::PrioError;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Info,
    Warning,
    Error,
}

impl std::fmt::Display for LogLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LogLevel::Info => write!(f, "info"),
            LogLevel::Warning => write!(f, "warn"),
            LogLevel::Error => write!(f, "error"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CliCommandLog {
    pub cwd: String,
    pub command: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogEntry {
    pub level: LogLevel,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub message: String,
    pub timestamp_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cli: Option<CliCommandLog>,
}

impl LogEntry {
    pub fn new(level: LogLevel, message: impl Into<String>, cli: Option<CliCommandLog>) -> Self {
        Self {
            level,
            message: message.into(),
            timestamp_ms: util::now_ms(),
            cli,
        }
    }

    pub fn cli_command(
        level: LogLevel,
        cwd: impl Into<String>,
        command: impl Into<String>,
        comment: Option<impl Into<String>>,
    ) -> Self {
        Self {
            level,
            message: String::new(),
            timestamp_ms: util::now_ms(),
            cli: Some(CliCommandLog {
                cwd: cwd.into(),
                command: command.into(),
                comment: comment.map(Into::into),
            }),
        }
    }

    pub fn info(message: impl Into<String>) -> Self {
        Self::new(LogLevel::Info, message, None)
    }

    pub fn warning(message: impl Into<String>) -> Self {
        Self::new(LogLevel::Warning, message, None)
    }

    pub fn error(message: impl Into<String>) -> Self {
        Self::new(LogLevel::Error, message, None)
    }
}

fn format_log_entry_terminal(entry: &LogEntry) -> String {
    if let Some(cli) = &entry.cli {
        let mut line = format!("{} $ {}", cli.cwd, cli.command);
        if let Some(comment) = &cli.comment {
            line.push_str(&format!(" \x1b[90m# {comment}\x1b[0m"));
        }
        format!("[{}] {line}", entry.level)
    } else {
        format!("[{}] {}", entry.level, entry.message)
    }
}

/// Brown `prio:` prefix for hook output (256-color palette, widely supported).
const PRIO_HOOK_PREFIX: &str = "\x1b[38;5;130mprio:\x1b[0m";

fn prefix_hook_line(line: &str) -> String {
    format!("{PRIO_HOOK_PREFIX} {line}")
}

pub enum Logger {
    Cli,
    /// Git hook callbacks — prefix terminal output with brown `prio:`.
    Hook,
    Ui(Vec<LogEntry>),
}

impl Logger {
    pub fn ui() -> Self {
        Logger::Ui(Vec::new())
    }

    pub fn log(&mut self, entry: LogEntry) {
        match self {
            Logger::Cli => {
                eprintln!("{}", format_log_entry_terminal(&entry));
            }
            Logger::Hook => {
                eprintln!("{}", prefix_hook_line(&format_log_entry_terminal(&entry)));
            }
            Logger::Ui(v) => v.push(entry),
        }
    }

    pub fn info(&mut self, message: impl Into<String>) {
        self.log(LogEntry::info(message));
    }

    pub fn warning(&mut self, message: impl Into<String>) {
        self.log(LogEntry::warning(message));
    }

    pub fn drain(&mut self) -> Vec<LogEntry> {
        match self {
            Logger::Ui(v) => std::mem::take(v),
            Logger::Cli | Logger::Hook => Vec::new(),
        }
    }

    pub fn entries(&self) -> &[LogEntry] {
        match self {
            Logger::Ui(v) => v,
            Logger::Cli | Logger::Hook => &[],
        }
    }
}

pub fn print_cli_result(result: &crate::result::PrioResult) {
    print_cli_result_inner(result, false);
}

pub fn print_hook_result(result: &crate::result::PrioResult) {
    print_cli_result_inner(result, true);
}

fn print_cli_result_inner(result: &crate::result::PrioResult, hook: bool) {
    use crate::result::PrioStatus;

    let (prefix, color) = match result.status {
        PrioStatus::Success => ("SUCCESS", "\x1b[32m"),
        PrioStatus::Warning => ("WARNING", "\x1b[33m"),
        PrioStatus::Failure => ("FAILURE", "\x1b[31m"),
    };
    let status_line = format!("{color}{prefix}\x1b[0m: {}", result.message);
    if hook {
        eprintln!("{}", prefix_hook_line(&status_line));
    } else {
        eprintln!("{status_line}");
    }
    for entry in &result.logs {
        let line = format_log_entry_terminal(entry);
        if hook {
            eprintln!("{}", prefix_hook_line(&line));
        } else {
            eprintln!("  {line}");
        }
    }
}

pub fn repo_path_from_cwd() -> Result<std::path::PathBuf, PrioError> {
    std::env::current_dir().map_err(PrioError::from)
}
