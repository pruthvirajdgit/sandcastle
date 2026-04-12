use std::path::PathBuf;
use thiserror::Error;

/// Errors that can occur in sandbox operations.
#[derive(Debug, Error)]
pub enum SandcastleError {
    // Session errors
    #[error("session not found: {0}")]
    SessionNotFound(String),

    #[error("session expired: {0}")]
    SessionExpired(String),

    #[error("maximum sessions reached (limit: {0})")]
    MaxSessionsReached(usize),

    // File errors
    #[error("path not allowed: {0}")]
    PathNotAllowed(PathBuf),

    #[error("path traversal detected: {0}")]
    PathTraversal(String),

    #[error("file not found: {0}")]
    FileNotFound(PathBuf),

    #[error("file too large: {size} bytes (max: {max} bytes)")]
    FileTooLarge { size: u64, max: u64 },

    // Execution errors
    #[error("execution failed: {0}")]
    ExecutionFailed(String),

    #[error("execution timed out")]
    Timeout,

    #[error("process killed by OOM")]
    OomKilled,

    // Runtime errors
    #[error("sandbox creation failed: {0}")]
    SandboxCreationFailed(String),

    #[error("runtime error: {0}")]
    RuntimeError(String),

    // Protocol errors
    #[error("invalid parameters: {0}")]
    InvalidParams(String),

    #[error("unknown tool: {0}")]
    UnknownTool(String),

    // Language errors
    #[error("unsupported language: {0}")]
    UnsupportedLanguage(String),
}

impl SandcastleError {
    /// Map to MCP JSON-RPC error code.
    pub fn error_code(&self) -> i32 {
        match self {
            Self::InvalidParams(_) | Self::UnknownTool(_) => -32602,
            Self::SessionNotFound(_) => -1,
            Self::MaxSessionsReached(_) | Self::FileTooLarge { .. } => -2,
            Self::SessionExpired(_) => -3,
            _ => -32603,
        }
    }
}

pub type Result<T> = std::result::Result<T, SandcastleError>;
