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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn as_str_matches_serde_representation() {
        let cases = [
            (StateKind::Done, "done"),
            (StateKind::InProgress, "in_progress"),
            (StateKind::Blocked, "blocked"),
            (StateKind::Todo, "todo"),
            (StateKind::Backlog, "backlog"),
        ];

        for (state, expected) in cases {
            assert_eq!(state.as_str(), expected);

            let serialized = serde_json::to_string(&state)
                .unwrap_or_else(|err| panic!("must serialize state kind: {err}"));
            assert_eq!(serialized, format!("\"{expected}\""));

            let decoded: StateKind = serde_json::from_str(&format!("\"{expected}\""))
                .unwrap_or_else(|err| panic!("must deserialize state kind: {err}"));
            assert_eq!(decoded, state);
        }
    }
}
