//! Errors with stable codes, shared by the CLI (exit codes) and JSON-RPC.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Stable error categories. The CLI maps them to exit codes (see AGENTS.md).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ErrorCode {
    Internal,
    Usage,
    NotFound,
    InvalidEdit,
    Conflict,
    NoInstance,
    Media,
    Export,
    Cancelled,
    Dependency,
}

impl ErrorCode {
    pub fn exit_code(self) -> i32 {
        match self {
            Self::Internal => 1,
            Self::Usage => 2,
            Self::NotFound => 3,
            Self::InvalidEdit => 4,
            Self::Conflict => 5,
            Self::NoInstance => 6,
            Self::Media => 7,
            Self::Export => 8,
            Self::Cancelled => 9,
            Self::Dependency => 10,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, thiserror::Error)]
#[error("{message}")]
pub struct Error {
    pub code: ErrorCode,
    pub message: String,
}

impl Error {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    pub fn usage(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Usage, message)
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::NotFound, message)
    }

    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::InvalidEdit, message)
    }

    pub fn conflict(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Conflict, message)
    }

    pub fn media(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Media, message)
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Internal, message)
    }
}

impl From<crate::time::TimeError> for Error {
    fn from(e: crate::time::TimeError) -> Self {
        Self::usage(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, Error>;
