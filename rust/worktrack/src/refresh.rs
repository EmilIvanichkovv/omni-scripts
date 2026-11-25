//! Scheduler, background tasks, channels to UI (spec 10.2, 10.3). OWNED BY: core/refresh agent.
//!
//! CONTRACT: `AppEvent` flows from background tasks to the UI thread; `Command` flows from the
//! UI thread to the background. `ui/` and `app.rs` never perform IO themselves.

use crate::model::{Repo, WorkItem, Worktree};
use crate::sessions::Session;
use crate::todos::rules::Suggestion;
use crate::todos::{Todo, TodoOp};

/// PR integration status shown in the header (spec 9.1, 5.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrStatus {
    Ok,
    NoToken,
    Offline,
    /// 401/403 etc: message and affected repos.
    Error {
        message: String,
        repos: Vec<String>,
    },
    /// Host unreachable: showing cached data of this age (secs).
    Cached {
        age_secs: i64,
    },
}

#[derive(Debug, Clone)]
pub enum AppEvent {
    ReposUpdated {
        repos: Vec<Repo>,
        worktrees: Vec<Worktree>,
    },
    WorkItemsUpdated(Vec<WorkItem>),
    PrUpdated {
        status: PrStatus,
        synced_at: Option<i64>,
    },
    SessionsUpdated(Vec<Session>),
    /// `Err` carries the parse error line; the UI keeps its last good state.
    TodosReloaded(Result<Vec<Todo>, String>),
    SuggestionsUpdated(Vec<Suggestion>),
    /// Rendered daily summary Markdown for the current window.
    SummaryUpdated {
        since: chrono::NaiveDate,
        markdown: String,
    },
    Diagnostics(Vec<String>),
    Progress {
        task: String,
        done: usize,
        total: usize,
    },
    Status(String),
    Error {
        scope: String,
        message: String,
    },
}

/// Requests from the UI to the background.
#[derive(Debug, Clone)]
pub enum Command {
    LocalReload,
    FullSync,
    Todo(TodoOp),
    AcceptSuggestion {
        id: i64,
        edited_title: Option<String>,
    },
    RejectSuggestion {
        id: i64,
    },
    /// For KeepOrAbandon suggestions: `abandon = false` keeps.
    DecideSuggestion {
        id: i64,
        abandon: bool,
    },
    AcceptAll,
    RejectAll,
    SetAbandoned {
        repo_id: String,
        branch: String,
        abandoned: bool,
    },
    RemoveWorktree {
        repo_id: String,
        branch: String,
    },
    Resummarize {
        session_id: String,
    },
    /// Shift/recompute the summary window.
    Summary {
        since: chrono::NaiveDate,
    },
    WriteSummary {
        since: chrono::NaiveDate,
    },
    OpenUrl(String),
    Quit,
}
