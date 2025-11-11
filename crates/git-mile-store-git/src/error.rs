//! Error types for git-mile store operations.

use thiserror::Error;

/// Errors that can occur during `GitStore` operations.
#[derive(Error, Debug)]
pub enum GitStoreError {
    /// Task was not found in the repository.
    #[error("Task not found: {0}")]
    TaskNotFound(String),

    /// Invalid task ID format.
    #[error("Invalid task ID: {0}")]
    InvalidTaskId(String),

    /// Git repository error.
    #[error("Git repository error: {0}")]
    GitError(#[from] git2::Error),

    /// Failed to parse event JSON.
    #[error("Failed to parse event: {0}")]
    EventParseError(String),

    /// Failed to serialize event to JSON.
    #[error("Failed to serialize event: {0}")]
    EventSerializeError(String),

    /// Failed to acquire repository lock.
    #[error("Repository lock error")]
    LockError,

    /// I/O operation failed.
    #[error("I/O error: {0}")]
    IoError(#[from] std::io::Error),

    /// Other unclassified error.
    #[error("Other error: {0}")]
    Other(String),
}

impl From<anyhow::Error> for GitStoreError {
    fn from(err: anyhow::Error) -> Self {
        Self::Other(err.to_string())
    }
}
