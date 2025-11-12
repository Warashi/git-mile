//! Application layer logic for git-mile.
//!
//! This crate provides high-level services, caching, configuration, and utilities
//! shared across CLI, TUI, and MCP interfaces.

pub mod async_store;
pub mod config;
pub mod filter_util;
pub mod service;
pub mod task_cache;
pub mod task_patch;
pub mod task_repository;
pub mod task_writer;

// Re-exports for convenience
pub use async_store::{AsyncTaskRepository, AsyncTaskStore};
pub use config::{ProjectConfig, StateKind, WorkflowConfig, WorkflowState};
pub use filter_util::{
    normalize_timestamp, parse_timestamp, FilterBuildError, TaskFilterBuilder,
};
pub use service::{
    CommentInput, CommentOutput, CreateTaskInput, CreateTaskOutput, ParentLink, TaskService,
};
pub use task_cache::{TaskCache, TaskComment, TaskView};
pub use task_patch::{
    DescriptionPatch, SetDiff, StatePatch, TaskEditData, TaskPatch, TaskUpdate, diff_sets,
};
pub use task_repository::TaskRepository;
pub use task_writer::{
    CommentRequest, CreateTaskRequest, CreateTaskResult, ParentLinkResult, TaskStore, TaskWriteError,
    TaskWriteResult, TaskWriter,
};
