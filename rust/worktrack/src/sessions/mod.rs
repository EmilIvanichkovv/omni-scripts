//! Claude Code session model and linking (spec 6). CONTRACT types; behaviour owned by the
//! sessions agent.

pub mod hook;
pub mod ingest;
pub mod link;
pub mod resume;
pub mod summarize;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SummaryStatus {
    /// Below turn threshold or summaries disabled.
    None,
    Queued,
    Done,
    Failed,
}

impl SummaryStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Queued => "queued",
            Self::Done => "done",
            Self::Failed => "failed",
        }
    }
    pub fn parse(s: &str) -> Self {
        match s {
            "queued" => Self::Queued,
            "done" => Self::Done,
            "failed" => Self::Failed,
            _ => Self::None,
        }
    }
}

/// Validated output of the summarizer (spec 6.4).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionSummary {
    pub title: String,
    pub summary: String,
    #[serde(default)]
    pub done: Vec<String>,
    #[serde(default)]
    pub unfinished: Vec<String>,
    #[serde(default)]
    pub todos: Vec<String>,
    #[serde(default)]
    pub blockers: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionLink {
    pub repo_id: String,
    pub branch: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub cwd: Option<String>,
    pub transcript_path: Option<String>,
    pub title: Option<String>,
    pub started_at: Option<i64>,
    pub last_activity_at: Option<i64>,
    pub ended_at: Option<i64>,
    pub end_reason: Option<String>,
    pub user_turns: u32,
    pub branches: Vec<String>,
    pub files_touched: Vec<String>,
    pub explicit_todos: Vec<String>,
    pub summary_status: SummaryStatus,
    pub summary: Option<SessionSummary>,
    pub summarized_at: Option<i64>,
    pub links: Vec<SessionLink>,
}

impl Session {
    /// Activity within the last 5 minutes and not ended (spec 4.4 `session-live`).
    pub fn is_live(&self, now: i64) -> bool {
        self.ended_at.is_none() && self.last_activity_at.is_some_and(|t| now - t <= 300)
    }
}
