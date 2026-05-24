use serde::{Deserialize, Serialize};

use crate::error::PrioError;
use crate::logger::{LogEntry, LogLevel};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PrioStatus {
    Success,
    Warning,
    Failure,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrioResult {
    pub status: PrioStatus,
    pub message: String,
    pub logs: Vec<LogEntry>,
}

impl PrioResult {
    pub fn success(message: impl Into<String>, logs: Vec<LogEntry>) -> Self {
        Self {
            status: PrioStatus::Success,
            message: message.into(),
            logs,
        }
    }

    pub fn warning(message: impl Into<String>, logs: Vec<LogEntry>) -> Self {
        Self {
            status: PrioStatus::Warning,
            message: message.into(),
            logs,
        }
    }

    pub fn failure(message: impl Into<String>, logs: Vec<LogEntry>) -> Self {
        Self {
            status: PrioStatus::Failure,
            message: message.into(),
            logs,
        }
    }

    pub fn from_error(err: PrioError, mut logs: Vec<LogEntry>) -> Self {
        logs.push(LogEntry::new(LogLevel::Error, err.to_string(), None));
        Self {
            status: PrioStatus::Failure,
            message: err.to_string(),
            logs,
        }
    }
}
