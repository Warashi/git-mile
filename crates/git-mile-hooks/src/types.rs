//! Hook types and context

use git_mile_core::event::Event;
use serde::{Deserialize, Serialize};

/// Hook types that can be executed
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookKind {
    /// Executed before task creation
    PreTaskCreate,
    /// Executed after task creation
    PostTaskCreate,
    /// Executed before task update
    PreTaskUpdate,
    /// Executed after task update
    PostTaskUpdate,
    /// Executed before state change
    PreStateChange,
    /// Executed after state change
    PostStateChange,
    /// Executed before comment addition
    PreCommentAdd,
    /// Executed after comment addition
    PostCommentAdd,
    /// Executed before relation change
    PreRelationChange,
    /// Executed after relation change
    PostRelationChange,
    /// Executed before any event
    PreEvent,
    /// Executed after any event
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
