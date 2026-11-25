//! Todo model (spec 7.1). CONTRACT types; behaviour owned by the todos agent.

pub mod markdown;
pub mod rules;
pub mod store;
pub mod watch;

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TodoStatus {
    Todo,
    Doing,
    Done,
    Dropped,
}

impl TodoStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Todo => "todo",
            Self::Doing => "doing",
            Self::Done => "done",
            Self::Dropped => "dropped",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "todo" => Some(Self::Todo),
            "doing" => Some(Self::Doing),
            "done" => Some(Self::Done),
            "dropped" => Some(Self::Dropped),
            _ => None,
        }
    }
    pub fn is_active(self) -> bool {
        matches!(self, Self::Todo | Self::Doing)
    }
    /// `[ ]` `[~]` `[x]` `[-]` as shown in the TUI (spec 9.3).
    pub fn marker(self) -> &'static str {
        match self {
            Self::Todo => "[ ]",
            Self::Doing => "[~]",
            Self::Done => "[x]",
            Self::Dropped => "[-]",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Priority {
    A,
    B,
    C,
}

impl Priority {
    pub fn as_char(self) -> char {
        match self {
            Self::A => 'A',
            Self::B => 'B',
            Self::C => 'C',
        }
    }
}

/// Which `##` section of the file an item currently sits in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Section {
    Active,
    Done,
    Archive,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Todo {
    /// `t_` + ULID. `None` for a hand-written item not yet written back by the app.
    pub id: Option<String>,
    pub title: String,
    /// Multi-line notes (de-indented).
    pub notes: String,
    pub status: TodoStatus,
    pub priority: Option<Priority>,
    /// `<PROJECT>/<repo>:<branch>`.
    pub link: Option<String>,
    pub jira: Option<String>,
    pub created: Option<NaiveDate>,
    pub done: Option<NaiveDate>,
    /// `manual`, `rule:<name>`, `session:<id>`.
    pub source: Option<String>,
    pub section: Section,
    /// Unknown metadata keys, preserved in order.
    pub extra: Vec<(String, String)>,
}

impl Todo {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            id: None,
            title: title.into(),
            notes: String::new(),
            status: TodoStatus::Todo,
            priority: None,
            link: None,
            jira: None,
            created: None,
            done: None,
            source: None,
            section: Section::Active,
            extra: Vec::new(),
        }
    }
}

/// Field updates for `TodoOp::Update`. `None` = unchanged; `Some(None)` = clear.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TodoPatch {
    pub title: Option<String>,
    pub notes: Option<String>,
    pub status: Option<TodoStatus>,
    pub priority: Option<Option<Priority>>,
    pub link: Option<Option<String>>,
    pub jira: Option<Option<String>>,
    pub done: Option<Option<NaiveDate>>,
}

/// Id-based operations (spec 7.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TodoOp {
    /// Adds to the end of Active (or Done for done/dropped). The app assigns id/created if absent.
    Add(Todo),
    Update {
        id: String,
        patch: TodoPatch,
    },
    /// Move within Active: place after `after` (None = to the top).
    Move {
        id: String,
        after: Option<String>,
    },
    Delete {
        id: String,
    },
}

pub fn new_todo_id() -> String {
    format!("t_{}", ulid::Ulid::new())
}
