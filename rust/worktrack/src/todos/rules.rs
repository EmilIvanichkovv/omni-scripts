//! Auto-update rules engine (spec 7.5). OWNED BY: todos/rules agent.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SuggestionStatus {
    Pending,
    Accepted,
    Rejected,
}

/// What accepting a suggestion does.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SuggestionPayload {
    /// `+`: add a todo.
    AddTodo {
        title: String,
        link: Option<String>,
        source: String,
    },
    /// `?`: keep or abandon a WorkItem (pr_declined, worktree_removed_unfinished).
    KeepOrAbandon {
        repo_id: String,
        branch: String,
        link: String,
        message: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Suggestion {
    pub id: i64,
    pub rule: String,
    pub subject_key: String,
    pub payload: SuggestionPayload,
    pub status: SuggestionStatus,
    pub created_at: i64,
    pub decided_at: Option<i64>,
}
