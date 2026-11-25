//! WorkItem, WorkState, flags and state derivation (spec section 4).
//!
//! CONTRACT FILE: types here are shared by every module. `derive_state` and the
//! flag helpers are pure functions with no IO (spec 10.2).

use serde::{Deserialize, Serialize};

/// `repo_id` = first 16 hex chars of SHA-256 of the canonical git common dir (spec 4.1).
pub type RepoId = String;

/// One discovered repository (a group of worktrees sharing a git common dir).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Repo {
    pub id: RepoId,
    pub common_dir: std::path::PathBuf,
    /// Path used to run git commands (main worktree, or the bare dir).
    pub git_dir_for_commands: std::path::PathBuf,
    pub origin_url: Option<String>,
    /// Bitbucket identity; `None` when skipped.
    pub bb: Option<BitbucketIdentity>,
    pub trunk: Option<String>,
    /// Why the repo is skipped (no origin, not Bitbucket DC, ...). Skipped repos
    /// are listed in diagnostics only.
    pub skipped_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct BitbucketIdentity {
    /// Normalized base URL without trailing slash, e.g. `https://bitbucket.company.com`.
    pub base_url: String,
    pub project: String,
    pub repo: String,
}

impl Repo {
    /// Display name `PROJECT/repo`, falling back to the directory name.
    pub fn display_name(&self) -> String {
        match &self.bb {
            Some(bb) => format!("{}/{}", bb.project, bb.repo),
            None => self
                .common_dir
                .parent()
                .and_then(|p| p.file_name())
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| self.id.clone()),
        }
    }
}

/// One entry of `git worktree list --porcelain`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Worktree {
    pub path: std::path::PathBuf,
    pub repo_id: RepoId,
    /// Short branch name (`feature/x`), `None` when detached or bare.
    pub branch: Option<String>,
    pub head: Option<String>,
    pub is_main: bool,
    pub is_bare: bool,
    pub locked: bool,
    pub prunable: bool,
    /// Filled by the local reload from `git status --porcelain=v2 --branch`.
    pub status: Option<WorktreeStatus>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorktreeStatus {
    pub dirty: bool,
    pub ahead: u32,
    pub behind: u32,
}

/// Local git facts about one branch of one repo, gathered by the local reload.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalBranchState {
    pub branch: String,
    pub local_exists: bool,
    pub remote_exists: bool,
    pub tip: Option<String>,
    /// Tip of `refs/remotes/origin/<branch>` if any.
    pub remote_tip: Option<String>,
    pub is_protected: bool,
    /// Branch tip is an ancestor of trunk (`git merge-base --is-ancestor`).
    pub tip_in_trunk: bool,
    /// Branch reflog shows at least one commit made on it (spec 4.3 row 3).
    pub has_own_commits: bool,
    /// Ahead/behind upstream.
    pub ahead: u32,
    pub behind: u32,
    /// Trunk commits not in branch.
    pub behind_trunk: u32,
    /// Commits in trunk..branch (count).
    pub ahead_of_trunk: u32,
    /// Worktree path where checked out, if any.
    pub worktree: Option<std::path::PathBuf>,
    pub dirty: bool,
    /// Unix seconds of the tip commit's committer date.
    pub last_commit_at: Option<i64>,
    /// Author emails of commits in `trunk..branch` (for ownership rule 3).
    pub author_emails: Vec<String>,
}

/// PR state as reported by Bitbucket DC.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PrStateKind {
    Open,
    Merged,
    Declined,
}

impl PrStateKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "OPEN",
            Self::Merged => "MERGED",
            Self::Declined => "DECLINED",
        }
    }
    /// Unknown state = error (spec 2).
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "OPEN" => Some(Self::Open),
            "MERGED" => Some(Self::Merged),
            "DECLINED" => Some(Self::Declined),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReviewerStatus {
    Approved,
    NeedsWork,
    Unapproved,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reviewer {
    /// Display name.
    pub name: String,
    pub slug: String,
    pub status: ReviewerStatus,
}

/// The latest PR for a branch (spec 4.3 "Latest PR").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrData {
    pub id: u64,
    pub state: PrStateKind,
    pub title: String,
    pub url: String,
    pub author_slug: String,
    pub reviewers: Vec<Reviewer>,
    pub open_tasks: u32,
    pub comment_count: u32,
    /// `fromRef.latestCommit`.
    pub from_latest_commit: Option<String>,
    /// Unix seconds.
    pub updated_at: Option<i64>,
    /// Unix seconds; for merged PRs (`closedDate`).
    pub closed_at: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CiState {
    Successful,
    Failed,
    InProgress,
}

impl CiState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Successful => "SUCCESSFUL",
            Self::Failed => "FAILED",
            Self::InProgress => "INPROGRESS",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "SUCCESSFUL" => Some(Self::Successful),
            "FAILED" => Some(Self::Failed),
            "INPROGRESS" => Some(Self::InProgress),
            _ => None,
        }
    }
}

/// Exactly one state per WorkItem (spec 4.3). Order = derivation priority.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum WorkState {
    Protected,
    Merged,
    MergedNoPr,
    Abandoned,
    PrDeclined,
    PrNeedsWork,
    PrApproved,
    PrOpen,
    Pushed,
    LocalOnly,
}

impl WorkState {
    pub const ALL: [WorkState; 10] = [
        Self::Protected,
        Self::Merged,
        Self::MergedNoPr,
        Self::Abandoned,
        Self::PrDeclined,
        Self::PrNeedsWork,
        Self::PrApproved,
        Self::PrOpen,
        Self::Pushed,
        Self::LocalOnly,
    ];

    /// Stored in SQLite `work_items.state` / `state_events`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Protected => "Protected",
            Self::Merged => "Merged",
            Self::MergedNoPr => "MergedNoPr",
            Self::Abandoned => "Abandoned",
            Self::PrDeclined => "PrDeclined",
            Self::PrNeedsWork => "PrNeedsWork",
            Self::PrApproved => "PrApproved",
            Self::PrOpen => "PrOpen",
            Self::Pushed => "Pushed",
            Self::LocalOnly => "LocalOnly",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|st| st.as_str() == s)
    }

    /// Human label used in UI and summary.
    pub fn label(self) -> &'static str {
        match self {
            Self::Protected => "protected",
            Self::Merged => "merged",
            Self::MergedNoPr => "merged·git",
            Self::Abandoned => "abandoned",
            Self::PrDeclined => "PR declined",
            Self::PrNeedsWork => "PR needs work",
            Self::PrApproved => "PR approved",
            Self::PrOpen => "PR open",
            Self::Pushed => "pushed",
            Self::LocalOnly => "local only",
        }
    }

    /// Spec 9.8 icons.
    pub fn icon(self) -> &'static str {
        match self {
            Self::Protected => "■",
            Self::LocalOnly => "○",
            Self::Pushed => "↑",
            Self::PrOpen => "◐",
            Self::PrApproved => "✓",
            Self::PrNeedsWork => "✎",
            Self::PrDeclined => "✕",
            Self::Merged | Self::MergedNoPr => "✔",
            Self::Abandoned => "–",
        }
    }

    pub fn is_unfinished(self) -> bool {
        matches!(
            self,
            Self::PrDeclined
                | Self::PrNeedsWork
                | Self::PrApproved
                | Self::PrOpen
                | Self::Pushed
                | Self::LocalOnly
        )
    }

    pub fn is_done(self) -> bool {
        matches!(self, Self::Merged | Self::MergedNoPr)
    }
}

/// Orthogonal flags (spec 4.4).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Flags {
    pub dirty: bool,
    pub ahead: u32,
    pub behind: u32,
    pub behind_trunk: u32,
    pub no_worktree: bool,
    pub cleanup: bool,
    pub stale: bool,
    pub ci: Option<CiState>,
    /// `openTaskCount` of the open PR.
    pub open_tasks: u32,
    pub session_live: bool,
}

/// The unit of tracking, keyed by `(repo_id, branch)`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkItem {
    pub repo_id: RepoId,
    /// `PROJECT/repo` display name of the repo.
    pub repo_name: String,
    pub branch: String,
    pub state: WorkState,
    pub flags: Flags,
    pub local: LocalBranchState,
    pub pr: Option<PrData>,
    pub abandoned_manual: bool,
    /// Newest of commit, session, PR update (unix seconds).
    pub last_activity: Option<i64>,
    /// Unix seconds when the current state was entered (from state_events).
    pub state_since: Option<i64>,
    pub linked_todos: u32,
    pub session_count: u32,
}

impl WorkItem {
    /// `<PROJECT>/<repo>:<branch>` link reference used by todos (spec 7.1).
    pub fn link_ref(&self) -> String {
        format!("{}:{}", self.repo_name, self.branch)
    }
}

/// Whether PR data could be evaluated (token present and request ok).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrAvailability {
    Available,
    Unavailable,
}

/// Inputs to `derive_state` besides local git state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ManualMarks {
    pub abandoned: bool,
}

/// Pure state derivation, spec 4.3. First matching row wins.
///
/// `pr` is the latest PR for the branch (already filtered by the caller to the
/// newest PR whose source branch is this branch). The branch-reuse rule (a
/// merged PR whose head is not the branch tip and `merged_head..tip` is
/// non-empty) is signalled by `pr_reused_after_merge`, computed by the caller
/// via git.
pub fn derive_state(
    local: &LocalBranchState,
    pr: Option<&PrData>,
    pr_availability: PrAvailability,
    pr_reused_after_merge: bool,
    manual: ManualMarks,
) -> WorkState {
    // 1. Protected
    if local.is_protected {
        return WorkState::Protected;
    }
    let pr_ok = pr_availability == PrAvailability::Available;
    let pr = if pr_ok { pr } else { None };
    // A merged PR whose branch got new commits afterwards is ignored (spec 4.3 reuse rule).
    let pr = match pr {
        Some(p) if p.state == PrStateKind::Merged && pr_reused_after_merge => None,
        other => other,
    };
    // 2. Merged
    if let Some(p) = pr {
        if p.state == PrStateKind::Merged {
            return WorkState::Merged;
        }
    }
    // 3. MergedNoPr: no PR, tip in trunk, branch had own commits.
    if pr.is_none() && local.tip_in_trunk && local.has_own_commits && local.tip.is_some() {
        return WorkState::MergedNoPr;
    }
    // 4. Abandoned
    if manual.abandoned || (!local.local_exists && !local.remote_exists) {
        return WorkState::Abandoned;
    }
    if let Some(p) = pr {
        match p.state {
            // 5.
            PrStateKind::Declined => return WorkState::PrDeclined,
            PrStateKind::Open => {
                let needs_work = p
                    .reviewers
                    .iter()
                    .any(|r| r.status == ReviewerStatus::NeedsWork);
                let approved = p
                    .reviewers
                    .iter()
                    .any(|r| r.status == ReviewerStatus::Approved);
                // 6, 7, 8
                return if needs_work {
                    WorkState::PrNeedsWork
                } else if approved {
                    WorkState::PrApproved
                } else {
                    WorkState::PrOpen
                };
            }
            PrStateKind::Merged => unreachable!("handled above"),
        }
    }
    // 9, 10
    if local.remote_exists {
        WorkState::Pushed
    } else {
        WorkState::LocalOnly
    }
}
