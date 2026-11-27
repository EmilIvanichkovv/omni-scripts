//! Claude Code session model and linking (spec 6). CONTRACT types; behaviour owned by the
//! sessions agent.

pub mod hook;
pub mod ingest;
pub mod link;
pub mod resume;
pub mod summarize;
pub mod workdirs;

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
    /// The `sessions.summary_status` value; the SQL writes these as literals.
    #[allow(dead_code)]
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

/// A worktree a session worked in besides its own (edits, `cd`, `git -C`; see
/// [`link`]): the reason for a link that no `gitBranch` in the transcript explains.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkedIn {
    /// Worktree root.
    pub path: String,
    pub repo_id: String,
    /// The worktree's branch when worktrack linked the session to it.
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
    /// Every link: the session's own branches and the worktrees it worked in.
    pub links: Vec<SessionLink>,
    /// The links that come from work in another worktree, with that worktree's path.
    #[serde(default)]
    pub worked_in: Vec<WorkedIn>,
}

impl Session {
    /// Links that come from the session's own `cwd` and `gitBranch` values, not from work in
    /// another worktree.
    pub fn own_links(&self) -> impl Iterator<Item = &SessionLink> {
        self.links.iter().filter(|l| {
            !self
                .worked_in
                .iter()
                .any(|w| w.repo_id == l.repo_id && w.branch == l.branch)
        })
    }

    /// Activity within the last 5 minutes and not ended (spec 4.4 `session-live`).
    pub fn is_live(&self, now: i64) -> bool {
        self.ended_at.is_none() && self.last_activity_at.is_some_and(|t| now - t <= 300)
    }
}
