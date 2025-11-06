use serde::{Deserialize, Serialize};

/// Classification of workflow states.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StateKind {
    /// Task is completed.
    Done,
    /// Task is actively being worked on.
    InProgress,
    /// Task is blocked or waiting.
    Blocked,
    /// Task is ready to be worked on next.
    Todo,
    /// Task resides in the backlog.
    Backlog,
}

impl StateKind {
    /// String representation used in configuration files.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Done => "done",
            Self::InProgress => "in_progress",
            Self::Blocked => "blocked",
            Self::Todo => "todo",
            Self::Backlog => "backlog",
        }
    }
}
