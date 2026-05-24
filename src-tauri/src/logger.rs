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
pub struct LogEntry {
    pub level: LogLevel,
    pub message: String,
    pub timestamp_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cli_command: Option<String>,
}

impl LogEntry {
    pub fn new(level: LogLevel, message: impl Into<String>, cli_command: Option<String>) -> Self {
        Self {
            level,
            message: message.into(),
            timestamp_ms: util::now_ms(),
            cli_command,
        }
    }

    pub fn info(message: impl Into<String>) -> Self {
        Self::new(LogLevel::Info, message, None)
    }

    pub fn info_cmd(message: impl Into<String>, cmd: impl Into<String>) -> Self {
        Self::new(LogLevel::Info, message, Some(cmd.into()))
    }

    pub fn warning(message: impl Into<String>) -> Self {
        Self::new(LogLevel::Warning, message, None)
    }

    pub fn error(message: impl Into<String>) -> Self {
        Self::new(LogLevel::Error, message, None)
    }
}

pub enum Logger {
    Cli,
    Ui(Vec<LogEntry>),
}

impl Logger {
    pub fn ui() -> Self {
        Logger::Ui(Vec::new())
    }

    pub fn log(&mut self, entry: LogEntry) {
        match self {
            Logger::Cli => {
                eprintln!("[{}] {}", entry.level, entry.message);
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
            Logger::Cli => Vec::new(),
        }
    }

    pub fn entries(&self) -> &[LogEntry] {
        match self {
            Logger::Ui(v) => v,
            Logger::Cli => &[],
        }
    }
}

pub fn print_cli_result(result: &crate::result::PrioResult) {
    use crate::result::PrioStatus;

    let (prefix, color) = match result.status {
        PrioStatus::Success => ("SUCCESS", "\x1b[32m"),
        PrioStatus::Warning => ("WARNING", "\x1b[33m"),
        PrioStatus::Failure => ("FAILURE", "\x1b[31m"),
    };
    eprintln!("{color}{prefix}\x1b[0m: {}", result.message);
    for entry in &result.logs {
        eprintln!("  [{}] {}", entry.level, entry.message);
    }
}

pub fn repo_path_from_cwd() -> Result<std::path::PathBuf, PrioError> {
    std::env::current_dir().map_err(PrioError::from)
}
