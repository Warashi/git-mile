//! Error types for hook execution

use std::io;

/// Result type for hook operations
pub type Result<T> = std::result::Result<T, HookError>;

/// Errors that can occur during hook execution
#[derive(Debug, thiserror::Error)]
pub enum HookError {
    /// Hook script execution failed
    #[error("Hook execution failed: {0}")]
    ExecutionFailed(String),

    /// Hook script timed out
    #[error("Hook timed out after {0} seconds")]
    Timeout(u64),

    /// Hook script rejected the operation (non-zero exit code)
    #[error("Hook rejected operation with exit code {code}: {stderr}")]
    Rejected {
        /// Exit code from the hook script
        code: i32,
        /// Standard error output
        stderr: String,
    },

    /// I/O error occurred
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),

    /// JSON serialization/deserialization error
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    /// Hook script not found
    #[error("Hook script not found: {0}")]
    NotFound(String),

    /// Hook configuration error
    #[error("Configuration error: {0}")]
    Config(String),
}
