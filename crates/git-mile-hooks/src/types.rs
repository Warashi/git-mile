//! Hook types and context

use git_mile_core::event::Event;
use serde::{Deserialize, Serialize};

/// Hook types that can be executed
///
/// Hooks are executed at specific points in the task lifecycle, allowing
/// custom validation, notifications, and integrations.
///
/// # Hook Execution Order
///
/// For any operation, hooks are executed in this order:
/// 1. `PreEvent` (global, applies to all operations)
/// 2. Specific pre-hook (e.g., `PreTaskUpdate`)
/// 3. Event is persisted to store
/// 4. Specific post-hook (e.g., `PostTaskUpdate`)
/// 5. `PostEvent` (global, applies to all operations)
///
/// # Pre-hooks vs Post-hooks
///
/// **Pre-hooks** can reject operations by returning a non-zero exit code.
/// If a pre-hook fails, the operation is aborted and no events are persisted.
///
/// **Post-hooks** are fire-and-forget. They execute after events are persisted,
/// and their success or failure does not affect the operation outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookKind {
    /// Executed before task creation
    ///
    /// # Use Cases
    /// - Validate task title format
    /// - Require specific labels for new tasks
    /// - Enforce naming conventions
    PreTaskCreate,

    /// Executed after task creation
    ///
    /// # Use Cases
    /// - Send notifications to team channels
    /// - Create related tasks automatically
    /// - Update external tracking systems
    PostTaskCreate,

    /// Executed before task updates (title, description, labels, assignees)
    ///
    /// # Use Cases
    /// - Validate field constraints
    /// - Prevent removal of required labels
    /// - Enforce assignee policies
    PreTaskUpdate,

    /// Executed after task updates
    ///
    /// # Use Cases
    /// - Notify assignees of changes
    /// - Sync updates to external systems
    /// - Trigger automated workflows
    PostTaskUpdate,

    /// Executed before workflow state changes
    ///
    /// # Use Cases
    /// - Enforce state transition rules (e.g., Todo → Done requires review)
    /// - Validate prerequisites for state changes
    /// - Check required fields for specific states
    PreStateChange,

    /// Executed after workflow state changes
    ///
    /// # Use Cases
    /// - Send status notifications
    /// - Update dashboards and metrics
    /// - Trigger deployment or CI/CD pipelines
    PostStateChange,

    /// Executed before comment addition
    ///
    /// # Use Cases
    /// - Content moderation and spam filtering
    /// - Require comment approval
    /// - Validate comment format
    PreCommentAdd,

    /// Executed after comment addition
    ///
    /// # Use Cases
    /// - Notify mentioned users
    /// - Index comments for search
    /// - Send email notifications
    PostCommentAdd,

    /// Executed before parent/child relationship changes
    ///
    /// # Use Cases
    /// - Detect circular dependencies
    /// - Enforce relationship constraints
    /// - Validate task hierarchy rules
    PreRelationChange,

    /// Executed after relationship changes
    ///
    /// # Use Cases
    /// - Update dependency graphs
    /// - Recalculate task estimates
    /// - Notify affected task owners
    PostRelationChange,

    /// Executed before any event (universal pre-hook)
    ///
    /// # Use Cases
    /// - Global audit logging
    /// - Rate limiting all operations
    /// - Maintenance mode enforcement
    /// - Cross-cutting validation rules
    PreEvent,

    /// Executed after any event (universal post-hook)
    ///
    /// # Use Cases
    /// - Backup all changes
    /// - Real-time replication
    /// - Global metrics collection
    /// - Event stream publishing
    PostEvent,
}

impl HookKind {
    /// Returns the script name for this hook kind
    #[must_use]
    pub const fn script_name(self) -> &'static str {
        match self {
            Self::PreTaskCreate => "pre-task-create",
            Self::PostTaskCreate => "post-task-create",
            Self::PreTaskUpdate => "pre-task-update",
            Self::PostTaskUpdate => "post-task-update",
            Self::PreStateChange => "pre-state-change",
            Self::PostStateChange => "post-state-change",
            Self::PreCommentAdd => "pre-comment-add",
            Self::PostCommentAdd => "post-comment-add",
            Self::PreRelationChange => "pre-relation-change",
            Self::PostRelationChange => "post-relation-change",
            Self::PreEvent => "pre-event",
            Self::PostEvent => "post-event",
        }
    }

    /// Returns true if this is a pre-hook (can reject operations)
    #[must_use]
    pub const fn is_pre_hook(self) -> bool {
        matches!(
            self,
            Self::PreTaskCreate
                | Self::PreTaskUpdate
                | Self::PreStateChange
                | Self::PreCommentAdd
                | Self::PreRelationChange
                | Self::PreEvent
        )
    }
}

/// Context passed to hook scripts
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookContext {
    /// The event being processed
    pub event: Event,
    /// Additional hook-specific data
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

impl HookContext {
    /// Create a new hook context from an event
    #[must_use]
    pub fn new(event: &Event) -> Self {
        Self {
            event: event.clone(),
            data: None,
        }
    }

    /// Create a new hook context with additional data
    #[must_use]
    pub fn with_data(event: &Event, data: serde_json::Value) -> Self {
        Self {
            event: event.clone(),
            data: Some(data),
        }
    }
}

/// Result from hook execution
#[derive(Debug, Clone)]
pub struct HookResult {
    /// Exit code from the hook script
    pub exit_code: i32,
    /// Standard output
    pub stdout: String,
    /// Standard error
    pub stderr: String,
    /// Modified event (if returned by hook)
    pub modified_event: Option<Event>,
}

impl HookResult {
    /// Returns true if the hook execution was successful
    #[must_use]
    pub const fn is_success(&self) -> bool {
        self.exit_code == 0
    }
}
