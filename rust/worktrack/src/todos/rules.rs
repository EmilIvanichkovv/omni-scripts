//! Auto-update rules engine and Inbox suggestion storage (spec 7.5, 4.5, 10.1).
//!
//! The engine never touches the todo file. It reads `state_events`, `meta` and `suggestions`,
//! writes `suggestions` and `meta`, and returns the todo operations and WorkItem abandon
//! decisions for the caller to apply.
//!
//! # Integration (local reload, after states are derived and `state_events` written)
//!
//! 1. Build a [`RulesContext`] from the current WorkItems, the sessions (newly summarized or
//!    ended ones; passing all sessions is fine, a cursor filters old ones), the current todos,
//!    `config.todos.rules`, the repo display names, `first_run` (meta `first_run_done` unset)
//!    and `now`.
//! 2. Call [`run`] on the writer connection. It loads new `state_events` past the cursor in meta
//!    `rules_last_event_id`, compares per-item snapshots (open task count, worktree path, meta
//!    `rules_item_snapshot`) to spot `openTaskCount` increases and removed worktrees, evaluates
//!    the six rules, inserts suggestions with `INSERT OR IGNORE` on the subject key, records
//!    auto-applied actions as `accepted`, and advances the cursors, all in one transaction.
//! 3. Apply `RulesOutput::todo_ops` through the todo store, mark every `RulesOutput::abandon`
//!    item as `abandoned_manual`, and emit `AppEvent::SuggestionsUpdated(list_pending(..))`.
//!
//! # Inbox actions (refresh.rs `Command::*Suggestion`)
//!
//! [`accept`], [`decide`], [`accept_all`] return an [`Accepted`] (todo ops + optional abandon)
//! for the caller to apply the same way; [`reject`] and [`reject_all`] only record the decision.
//! Decided subject keys are never suggested again.
//!
//! # Subject keys
//!
//! | Rule                          | Key                                                         |
//! | ----------------------------- | ----------------------------------------------------------- |
//! | `new_unfinished_branch`       | `new_unfinished_branch:<link>`                              |
//! | `pr_merged`                   | `pr_merged:<link>:<pr id or transition time>`               |
//! | `pr_declined`                 | `pr_declined:<link>:<pr id or transition time>`             |
//! | `pr_needs_work`               | `pr_needs_work:<link>:<pr id>:<transition time>`            |
//! | `session_unfinished`          | `session_unfinished:<session id>:<hash of normalized text>` |
//! | `worktree_removed_unfinished` | `worktree_removed_unfinished:<link>:<path>:<time>`          |
//!
//! Times are UTC `YYYY-MM-DDTHH:MMZ`. For an `openTaskCount` increase the time is `now`.
//!
//! # Mode semantics
//!
//! `auto` applies directly; for the keep/abandon rules (`pr_declined`,
//! `worktree_removed_unfinished`) `auto` means abandon. The first-run batch always creates
//! suggestions (unless the rule is `off`), even when `new_unfinished_branch` is `auto`.
//!
//! # `state_events.detail`
//!
//! Optional JSON object. The engine reads `pr_id` (number) and `closed_at` (unix seconds) from
//! it when the WorkItem no longer carries the PR. Anything else is ignored.

use std::collections::{BTreeMap, HashMap, HashSet};

use chrono::{DateTime, Local, NaiveDate, TimeZone, Utc};
use color_eyre::eyre::Result;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::config::{RuleMode, Rules};
use crate::model::{PrStateKind, RepoId, WorkItem, WorkState};
use crate::sanitize::clean_line;
use crate::sessions::Session;
use crate::store::{meta_get, meta_set};
use crate::todos::{new_todo_id, Todo, TodoOp, TodoPatch, TodoStatus};

pub const RULE_NEW_UNFINISHED_BRANCH: &str = "new_unfinished_branch";
pub const RULE_PR_MERGED: &str = "pr_merged";
pub const RULE_PR_DECLINED: &str = "pr_declined";
pub const RULE_PR_NEEDS_WORK: &str = "pr_needs_work";
pub const RULE_SESSION_UNFINISHED: &str = "session_unfinished";
pub const RULE_WORKTREE_REMOVED_UNFINISHED: &str = "worktree_removed_unfinished";

/// Meta key: id of the last `state_events` row the engine processed.
pub const META_LAST_EVENT_ID: &str = "rules_last_event_id";
/// Meta key: newest session timestamp (`summarized_at` or `ended_at`) processed.
pub const META_LAST_SESSION_AT: &str = "rules_last_session_at";
/// Meta key: JSON [`Snapshot`] of per-item open task counts and worktree paths.
pub const META_ITEM_SNAPSHOT: &str = "rules_item_snapshot";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SuggestionStatus {
    Pending,
    Accepted,
    Rejected,
}

impl SuggestionStatus {
    /// Stored in `suggestions.status`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Accepted => "accepted",
            Self::Rejected => "rejected",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "pending" => Some(Self::Pending),
            "accepted" => Some(Self::Accepted),
            "rejected" => Some(Self::Rejected),
            _ => None,
        }
    }
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
    /// `+`: mark the linked active todos done (`pr_merged` in `suggest` mode).
    CompleteTodos {
        link: String,
        message: String,
        /// Merge date, used as the todos' `done` date.
        done: NaiveDate,
    },
}

impl SuggestionPayload {
    /// One-line text for the Inbox (already sanitized when the suggestion was created).
    pub fn text(&self) -> &str {
        match self {
            Self::AddTodo { title, .. } => title,
            Self::KeepOrAbandon { message, .. } | Self::CompleteTodos { message, .. } => message,
        }
    }

    pub fn link(&self) -> Option<&str> {
        match self {
            Self::AddTodo { link, .. } => link.as_deref(),
            Self::KeepOrAbandon { link, .. } | Self::CompleteTodos { link, .. } => Some(link),
        }
    }

    /// `?` items need a keep/abandon choice; the rest are `+`.
    pub fn needs_choice(&self) -> bool {
        matches!(self, Self::KeepOrAbandon { .. })
    }
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

/// A suggestion produced by [`evaluate`], before it gets a row id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewSuggestion {
    pub rule: String,
    pub subject_key: String,
    pub payload: SuggestionPayload,
}

/// One `state_events` row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateEvent {
    pub id: i64,
    pub repo_id: RepoId,
    pub branch: String,
    pub from_state: Option<WorkState>,
    pub to_state: WorkState,
    /// Unix seconds.
    pub at: i64,
    pub detail: Option<String>,
}

/// Per-item facts remembered between runs, keyed by `<repo_id>:<branch>`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemSnapshot {
    /// `openTaskCount` while the PR is open.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub open_tasks: Option<u32>,
    /// Worktree path the branch was checked out in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree: Option<String>,
}

pub type Snapshot = BTreeMap<String, ItemSnapshot>;

/// Everything the caller provides for one run.
#[derive(Debug, Clone, Copy)]
pub struct RulesContext<'a> {
    pub work_items: &'a [WorkItem],
    /// Sessions to consider; ones not newer than the session cursor are skipped.
    pub sessions: &'a [Session],
    pub todos: &'a [Todo],
    pub rules: &'a Rules,
    /// `repo_id` → `PROJECT/repo`.
    pub repo_names: &'a HashMap<RepoId, String>,
    /// First launch: suggest every existing unfinished branch in one batch.
    pub first_run: bool,
    /// Unix seconds.
    pub now: i64,
}

/// Pure input to [`evaluate`]: the context plus what [`run`] loaded from SQLite.
#[derive(Debug, Clone, Copy)]
pub struct RulesInput<'a> {
    pub ctx: RulesContext<'a>,
    /// New `state_events`, ascending by id.
    pub events: &'a [StateEvent],
    /// Snapshot from the previous run; `None` = no previous run (baseline only).
    pub snapshot: Option<&'a Snapshot>,
    /// Every subject key ever recorded, with its status.
    pub known: &'a HashMap<String, SuggestionStatus>,
    /// Pending suggestions (for cross-session title dedupe).
    pub pending: &'a [Suggestion],
    /// Sessions with a timestamp below this were processed already.
    pub session_cursor: Option<i64>,
}

/// Result of [`evaluate`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Evaluation {
    pub todo_ops: Vec<TodoOp>,
    pub new_suggestions: Vec<NewSuggestion>,
    pub abandon: Vec<(RepoId, String)>,
    /// Actions applied in `auto` mode; recorded as `accepted` so they never repeat.
    pub auto_applied: Vec<NewSuggestion>,
    pub snapshot: Snapshot,
    pub session_cursor: Option<i64>,
}

/// Result of [`run`]: what the caller applies.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RulesOutput {
    pub todo_ops: Vec<TodoOp>,
    /// Suggestions inserted in this run (with ids).
    pub new_suggestions: Vec<Suggestion>,
    /// WorkItems to mark abandoned.
    pub abandon: Vec<(RepoId, String)>,
}

/// What a decision on a suggestion requires the caller to apply.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Accepted {
    pub todo_ops: Vec<TodoOp>,
    /// WorkItem to mark abandoned.
    pub abandon: Option<(RepoId, String)>,
}

/// Lowercase with whitespace runs collapsed to one space (spec 7.5 dedupe).
pub fn normalize_title(s: &str) -> String {
    s.split_whitespace()
        .map(str::to_lowercase)
        .collect::<Vec<_>>()
        .join(" ")
}

fn snapshot_key(repo_id: &str, branch: &str) -> String {
    format!("{repo_id}:{branch}")
}

fn minute_utc(ts: i64) -> String {
    DateTime::<Utc>::from_timestamp(ts, 0)
        .map(|d| d.format("%Y-%m-%dT%H:%MZ").to_string())
        .unwrap_or_else(|| ts.to_string())
}

/// Local calendar date of a unix timestamp (todo dates are local).
pub fn local_date(ts: i64) -> NaiveDate {
    match Local.timestamp_opt(ts, 0).single() {
        Some(d) => d.date_naive(),
        None => DateTime::<Utc>::from_timestamp(ts, 0)
            .unwrap_or_default()
            .date_naive(),
    }
}

#[derive(Debug, Default, Deserialize)]
struct EventDetail {
    pr_id: Option<u64>,
    closed_at: Option<i64>,
}

fn parse_detail(detail: Option<&str>) -> EventDetail {
    detail
        .and_then(|d| serde_json::from_str(d).ok())
        .unwrap_or_default()
}

fn active_linked<'t>(todos: &'t [Todo], link: &str) -> impl Iterator<Item = &'t Todo> + 't {
    let link = link.to_string();
    todos
        .iter()
        .filter(move |t| t.status.is_active() && t.link.as_deref() == Some(link.as_str()))
}

fn has_linked_todo(todos: &[Todo], link: &str) -> bool {
    todos.iter().any(|t| t.link.as_deref() == Some(link))
}

/// `Update` ops setting every active todo linked to `link` to `status` with `done` = `date`.
fn close_linked(todos: &[Todo], link: &str, status: TodoStatus, date: NaiveDate) -> Vec<TodoOp> {
    active_linked(todos, link)
        .filter_map(|t| t.id.clone())
        .map(|id| TodoOp::Update {
            id,
            patch: TodoPatch {
                status: Some(status),
                done: Some(Some(date)),
                ..TodoPatch::default()
            },
        })
        .collect()
}

fn add_todo_op(title: &str, link: Option<&str>, source: &str, today: NaiveDate) -> TodoOp {
    let mut t = Todo::new(title);
    t.id = Some(new_todo_id());
    t.link = link.map(str::to_string);
    t.source = Some(source.to_string());
    t.created = Some(today);
    TodoOp::Add(t)
}

/// Collects actions and applies dedupe and mode handling.
struct Collector<'a> {
    known: &'a HashMap<String, SuggestionStatus>,
    seen: HashSet<String>,
    todos: &'a [Todo],
    today: NaiveDate,
    out: Evaluation,
}

impl Collector<'_> {
    /// True when the key was already recorded or emitted in this run.
    fn is_dup(&self, key: &str) -> bool {
        self.known.contains_key(key) || self.seen.contains(key)
    }

    /// Emit one action for `key` under `mode`, unless `off` or a duplicate.
    fn emit(&mut self, mode: RuleMode, rule: &str, key: String, payload: SuggestionPayload) {
        if mode == RuleMode::Off || self.is_dup(&key) {
            return;
        }
        self.seen.insert(key.clone());
        let s = NewSuggestion {
            rule: rule.to_string(),
            subject_key: key,
            payload,
        };
        match mode {
            RuleMode::Suggest => self.out.new_suggestions.push(s),
            RuleMode::Auto => {
                let applied = apply_payload(&s.payload, None, true, self.todos, self.today);
                self.out.todo_ops.extend(applied.todo_ops);
                self.out.abandon.extend(applied.abandon);
                self.out.auto_applied.push(s);
            }
            RuleMode::Off => {}
        }
    }
}

/// What a payload does when accepted (`abandon` selects the `?` choice).
fn apply_payload(
    payload: &SuggestionPayload,
    edited_title: Option<&str>,
    abandon: bool,
    todos: &[Todo],
    today: NaiveDate,
) -> Accepted {
    match payload {
        SuggestionPayload::AddTodo {
            title,
            link,
            source,
        } => {
            let title = edited_title
                .map(clean_line)
                .map(|t| t.trim().to_string())
                .filter(|t| !t.is_empty())
                .unwrap_or_else(|| title.clone());
            Accepted {
                todo_ops: vec![add_todo_op(&title, link.as_deref(), source, today)],
                abandon: None,
            }
        }
        SuggestionPayload::KeepOrAbandon {
            repo_id,
            branch,
            link,
            ..
        } => {
            if abandon {
                Accepted {
                    todo_ops: close_linked(todos, link, TodoStatus::Dropped, today),
                    abandon: Some((repo_id.clone(), branch.clone())),
                }
            } else {
                Accepted::default()
            }
        }
        SuggestionPayload::CompleteTodos { link, done, .. } => Accepted {
            todo_ops: close_linked(todos, link, TodoStatus::Done, *done),
            abandon: None,
        },
    }
}

/// Pure rule evaluation (no IO). See the module docs for the rules and keys.
pub fn evaluate(input: &RulesInput) -> Evaluation {
    let ctx = &input.ctx;
    let rules = ctx.rules;
    let mut c = Collector {
        known: input.known,
        seen: HashSet::new(),
        todos: ctx.todos,
        today: local_date(ctx.now),
        out: Evaluation::default(),
    };
    let items: HashMap<(&str, &str), &WorkItem> = ctx
        .work_items
        .iter()
        .map(|w| ((w.repo_id.as_str(), w.branch.as_str()), w))
        .collect();
    let link_for = |repo_id: &str, branch: &str| -> Option<String> {
        let name = ctx.repo_names.get(repo_id).cloned().or_else(|| {
            items
                .get(&(repo_id, branch))
                .map(|w| w.repo_name.clone())
                .filter(|n| !n.is_empty())
        })?;
        Some(clean_line(&format!("{name}:{branch}")))
    };
    // PR ids that got a pr_needs_work action in this run (one per PR per run).
    let mut needs_work_prs: HashSet<(String, u64)> = HashSet::new();

    // 1. State transitions.
    for ev in input.events {
        let Some(link) = link_for(&ev.repo_id, &ev.branch) else {
            continue;
        };
        let item = items
            .get(&(ev.repo_id.as_str(), ev.branch.as_str()))
            .copied();
        let detail = parse_detail(ev.detail.as_deref());
        let pr_id = item
            .and_then(|w| w.pr.as_ref())
            .map(|p| p.id)
            .or(detail.pr_id);
        let still_in = |s: WorkState| item.is_some_and(|w| w.state == s);
        let entered_unfinished =
            ev.to_state.is_unfinished() && !ev.from_state.is_some_and(|f| f.is_unfinished());

        if entered_unfinished
            && item.is_some_and(|w| w.state.is_unfinished())
            && !has_linked_todo(ctx.todos, &link)
        {
            c.emit(
                rules.new_unfinished_branch,
                RULE_NEW_UNFINISHED_BRANCH,
                format!("{RULE_NEW_UNFINISHED_BRANCH}:{link}"),
                SuggestionPayload::AddTodo {
                    title: clean_line(&ev.branch),
                    link: Some(link.clone()),
                    source: format!("rule:{RULE_NEW_UNFINISHED_BRANCH}"),
                },
            );
        }

        match ev.to_state {
            WorkState::Merged => {
                if active_linked(ctx.todos, &link).next().is_none() {
                    continue;
                }
                let merged_at = item
                    .and_then(|w| w.pr.as_ref())
                    .filter(|p| p.state == PrStateKind::Merged)
                    .and_then(|p| p.closed_at)
                    .or(detail.closed_at)
                    .unwrap_or(ev.at);
                let subject = pr_id
                    .map(|id| id.to_string())
                    .unwrap_or_else(|| minute_utc(ev.at));
                let message = match pr_id {
                    Some(id) => format!("PR #{id} merged: complete linked todos"),
                    None => "PR merged: complete linked todos".to_string(),
                };
                c.emit(
                    rules.pr_merged,
                    RULE_PR_MERGED,
                    format!("{RULE_PR_MERGED}:{link}:{subject}"),
                    SuggestionPayload::CompleteTodos {
                        link: link.clone(),
                        message,
                        done: local_date(merged_at),
                    },
                );
            }
            WorkState::PrDeclined if still_in(WorkState::PrDeclined) => {
                let subject = pr_id
                    .map(|id| id.to_string())
                    .unwrap_or_else(|| minute_utc(ev.at));
                let message = match pr_id {
                    Some(id) => format!("PR #{id} declined: keep or abandon"),
                    None => "PR declined: keep or abandon".to_string(),
                };
                c.emit(
                    rules.pr_declined,
                    RULE_PR_DECLINED,
                    format!("{RULE_PR_DECLINED}:{link}:{subject}"),
                    SuggestionPayload::KeepOrAbandon {
                        repo_id: ev.repo_id.clone(),
                        branch: ev.branch.clone(),
                        link: link.clone(),
                        message,
                    },
                );
            }
            WorkState::PrNeedsWork if still_in(WorkState::PrNeedsWork) => {
                let Some(id) = pr_id else { continue };
                needs_work_prs.insert((link.clone(), id));
                c.emit(
                    rules.pr_needs_work,
                    RULE_PR_NEEDS_WORK,
                    format!("{RULE_PR_NEEDS_WORK}:{link}:{id}:{}", minute_utc(ev.at)),
                    needs_work_payload(id, &link),
                );
            }
            _ => {}
        }
    }

    // 2. Snapshot comparisons: openTaskCount increases and removed worktrees.
    let mut snapshot = Snapshot::new();
    for w in ctx.work_items {
        let key = snapshot_key(&w.repo_id, &w.branch);
        let open_pr = w.pr.as_ref().filter(|p| p.state == PrStateKind::Open);
        let cur = ItemSnapshot {
            open_tasks: open_pr.map(|p| p.open_tasks),
            worktree: w
                .local
                .worktree
                .as_ref()
                .map(|p| p.to_string_lossy().into_owned()),
        };
        let prev = input.snapshot.and_then(|s| s.get(&key));
        if let (Some(prev), Some(link)) = (prev, link_for(&w.repo_id, &w.branch)) {
            if let (Some(pr), Some(before), Some(after)) =
                (open_pr, prev.open_tasks, cur.open_tasks)
            {
                let title = needs_work_title(pr.id);
                let prefix = format!("{RULE_PR_NEEDS_WORK}:{link}:{}:", pr.id);
                let already = needs_work_prs.contains(&(link.clone(), pr.id))
                    || input
                        .pending
                        .iter()
                        .any(|s| s.subject_key.starts_with(&prefix))
                    || active_linked(ctx.todos, &link)
                        .any(|t| normalize_title(&t.title) == normalize_title(&title));
                if after > before && !already {
                    c.emit(
                        rules.pr_needs_work,
                        RULE_PR_NEEDS_WORK,
                        format!("{prefix}{}", minute_utc(ctx.now)),
                        needs_work_payload(pr.id, &link),
                    );
                }
            }
            if let (Some(path), None) = (&prev.worktree, &cur.worktree) {
                if w.state.is_unfinished() {
                    c.emit(
                        rules.worktree_removed_unfinished,
                        RULE_WORKTREE_REMOVED_UNFINISHED,
                        format!(
                            "{RULE_WORKTREE_REMOVED_UNFINISHED}:{link}:{path}:{}",
                            minute_utc(ctx.now)
                        ),
                        SuggestionPayload::KeepOrAbandon {
                            repo_id: w.repo_id.clone(),
                            branch: w.branch.clone(),
                            link: link.clone(),
                            message: clean_line(&format!(
                                "Worktree {path} removed: keep or abandon"
                            )),
                        },
                    );
                }
            }
        }
        snapshot.insert(key, cur);
    }
    c.out.snapshot = snapshot;

    // 3. Session items.
    let mut titles: HashSet<String> = ctx
        .todos
        .iter()
        .map(|t| normalize_title(&t.title))
        .collect();
    titles.extend(input.pending.iter().filter_map(|s| match &s.payload {
        SuggestionPayload::AddTodo { title, .. } => Some(normalize_title(title)),
        _ => None,
    }));
    let mut session_cursor = input.session_cursor;
    for s in ctx.sessions {
        let Some(ts) = s.summarized_at.or(s.ended_at) else {
            continue;
        };
        if input.session_cursor.is_some_and(|cur| ts < cur) {
            continue;
        }
        session_cursor = Some(session_cursor.map_or(ts, |c| c.max(ts)));
        let link = s
            .links
            .first()
            .and_then(|l| link_for(&l.repo_id, &l.branch));
        let summary_items = s
            .summary
            .iter()
            .flat_map(|sum| sum.unfinished.iter().chain(sum.todos.iter()));
        for raw in summary_items.chain(s.explicit_todos.iter()) {
            let title = clean_line(raw).trim().to_string();
            let norm = normalize_title(&title);
            if norm.is_empty() || titles.contains(&norm) {
                continue;
            }
            let hash = Sha256::digest(norm.as_bytes());
            let hex: String = hash.iter().take(8).map(|b| format!("{b:02x}")).collect();
            let key = format!("{RULE_SESSION_UNFINISHED}:{}:{hex}", s.id);
            if rules.session_unfinished == RuleMode::Off || c.is_dup(&key) {
                continue;
            }
            titles.insert(norm);
            c.emit(
                rules.session_unfinished,
                RULE_SESSION_UNFINISHED,
                key,
                SuggestionPayload::AddTodo {
                    title,
                    link: link.clone(),
                    source: format!("session:{}", s.id),
                },
            );
        }
    }
    c.out.session_cursor = session_cursor;

    // 4. First run: every existing unfinished branch without a linked todo, as suggestions.
    if ctx.first_run && rules.new_unfinished_branch != RuleMode::Off {
        for w in ctx.work_items.iter().filter(|w| w.state.is_unfinished()) {
            let Some(link) = link_for(&w.repo_id, &w.branch) else {
                continue;
            };
            if has_linked_todo(ctx.todos, &link) {
                continue;
            }
            c.emit(
                RuleMode::Suggest,
                RULE_NEW_UNFINISHED_BRANCH,
                format!("{RULE_NEW_UNFINISHED_BRANCH}:{link}"),
                SuggestionPayload::AddTodo {
                    title: clean_line(&w.branch),
                    link: Some(link),
                    source: format!("rule:{RULE_NEW_UNFINISHED_BRANCH}"),
                },
            );
        }
    }

    c.out
}

fn needs_work_title(pr_id: u64) -> String {
    format!("Address review on PR #{pr_id}")
}

fn needs_work_payload(pr_id: u64, link: &str) -> SuggestionPayload {
    SuggestionPayload::AddTodo {
        title: needs_work_title(pr_id),
        link: Some(link.to_string()),
        source: format!("rule:{RULE_PR_NEEDS_WORK}"),
    }
}

// ---------------------------------------------------------------------------------------------
// SQL
// ---------------------------------------------------------------------------------------------

/// `state_events` rows with `id > after_id`, ascending. Rows with unknown states are skipped.
pub fn load_events_since(conn: &Connection, after_id: i64) -> Result<Vec<StateEvent>> {
    let mut stmt = conn.prepare(
        "SELECT id, repo_id, branch, from_state, to_state, at, detail
         FROM state_events WHERE id > ?1 ORDER BY id",
    )?;
    let rows = stmt.query_map([after_id], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, Option<String>>(3)?,
            r.get::<_, String>(4)?,
            r.get::<_, i64>(5)?,
            r.get::<_, Option<String>>(6)?,
        ))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (id, repo_id, branch, from, to, at, detail) = row?;
        let Some(to_state) = WorkState::parse(&to) else {
            tracing::warn!(id, to, "state_events row with unknown state");
            continue;
        };
        out.push(StateEvent {
            id,
            repo_id,
            branch,
            from_state: from.as_deref().and_then(WorkState::parse),
            to_state,
            at,
            detail,
        });
    }
    Ok(out)
}

fn max_event_id(conn: &Connection) -> Result<i64> {
    Ok(
        conn.query_row("SELECT COALESCE(MAX(id), 0) FROM state_events", [], |r| {
            r.get(0)
        })?,
    )
}

fn known_subjects(conn: &Connection) -> Result<HashMap<String, SuggestionStatus>> {
    let mut stmt = conn.prepare("SELECT subject_key, status FROM suggestions")?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
    let mut out = HashMap::new();
    for row in rows {
        let (k, s) = row?;
        out.insert(
            k,
            SuggestionStatus::parse(&s).unwrap_or(SuggestionStatus::Pending),
        );
    }
    Ok(out)
}

/// Insert a suggestion unless its subject key exists. Returns the new row id.
pub fn insert_suggestion(
    conn: &Connection,
    s: &NewSuggestion,
    status: SuggestionStatus,
    now: i64,
) -> Result<Option<i64>> {
    let payload = serde_json::to_string(&s.payload)?;
    let decided = (status != SuggestionStatus::Pending).then_some(now);
    let n = conn.execute(
        "INSERT OR IGNORE INTO suggestions(rule, subject_key, payload, status, created_at, decided_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![s.rule, s.subject_key, payload, status.as_str(), now, decided],
    )?;
    Ok((n == 1).then(|| conn.last_insert_rowid()))
}

/// Run the rules once (see module docs). Everything happens in one transaction.
pub fn run(conn: &Connection, ctx: &RulesContext) -> Result<RulesOutput> {
    let tx = conn.unchecked_transaction()?;
    let cursor: i64 = meta_get(&tx, META_LAST_EVENT_ID)?
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let events = load_events_since(&tx, cursor)?;
    let snapshot: Option<Snapshot> =
        meta_get(&tx, META_ITEM_SNAPSHOT)?.and_then(|v| serde_json::from_str(&v).ok());
    let session_cursor: Option<i64> =
        meta_get(&tx, META_LAST_SESSION_AT)?.and_then(|v| v.parse().ok());
    let known = known_subjects(&tx)?;
    let pending = list_pending(&tx)?;

    let eval = evaluate(&RulesInput {
        ctx: *ctx,
        events: &events,
        snapshot: snapshot.as_ref(),
        known: &known,
        pending: &pending,
        session_cursor,
    });

    let mut new_suggestions = Vec::new();
    for s in &eval.new_suggestions {
        if let Some(id) = insert_suggestion(&tx, s, SuggestionStatus::Pending, ctx.now)? {
            new_suggestions.push(Suggestion {
                id,
                rule: s.rule.clone(),
                subject_key: s.subject_key.clone(),
                payload: s.payload.clone(),
                status: SuggestionStatus::Pending,
                created_at: ctx.now,
                decided_at: None,
            });
        }
    }
    for s in &eval.auto_applied {
        insert_suggestion(&tx, s, SuggestionStatus::Accepted, ctx.now)?;
    }

    // Every row up to the current max was loaded (rows with unknown states are skipped for
    // good), so the cursor moves to it.
    let new_cursor = cursor.max(max_event_id(&tx)?);
    meta_set(&tx, META_LAST_EVENT_ID, &new_cursor.to_string())?;
    meta_set(
        &tx,
        META_ITEM_SNAPSHOT,
        &serde_json::to_string(&eval.snapshot)?,
    )?;
    if let Some(sc) = eval.session_cursor {
        meta_set(&tx, META_LAST_SESSION_AT, &sc.to_string())?;
    }
    tx.commit()?;

    Ok(RulesOutput {
        todo_ops: eval.todo_ops,
        new_suggestions,
        abandon: eval.abandon,
    })
}

const SELECT_SUGGESTION: &str =
    "SELECT id, rule, subject_key, payload, status, created_at, decided_at FROM suggestions";

fn suggestion_from_row(r: &rusqlite::Row) -> rusqlite::Result<Option<Suggestion>> {
    let payload: String = r.get(3)?;
    let status: String = r.get(4)?;
    let Ok(payload) = serde_json::from_str::<SuggestionPayload>(&payload) else {
        return Ok(None);
    };
    let Some(status) = SuggestionStatus::parse(&status) else {
        return Ok(None);
    };
    Ok(Some(Suggestion {
        id: r.get(0)?,
        rule: r.get(1)?,
        subject_key: r.get(2)?,
        payload,
        status,
        created_at: r.get(5)?,
        decided_at: r.get(6)?,
    }))
}

fn query_suggestions(
    conn: &Connection,
    sql_tail: &str,
    p: impl rusqlite::Params,
) -> Result<Vec<Suggestion>> {
    let mut stmt = conn.prepare(&format!("{SELECT_SUGGESTION} {sql_tail}"))?;
    let rows = stmt.query_map(p, suggestion_from_row)?;
    let mut out = Vec::new();
    for row in rows {
        if let Some(s) = row? {
            out.push(s);
        }
    }
    Ok(out)
}

/// Pending suggestions, newest first (Inbox order, spec 9.5).
pub fn list_pending(conn: &Connection) -> Result<Vec<Suggestion>> {
    query_suggestions(
        conn,
        "WHERE status = 'pending' ORDER BY created_at DESC, id DESC",
        [],
    )
}

#[cfg(test)]
pub fn pending_count(conn: &Connection) -> Result<usize> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM suggestions WHERE status = 'pending'",
        [],
        |r| r.get(0),
    )?;
    Ok(usize::try_from(n).unwrap_or(0))
}

pub fn get(conn: &Connection, id: i64) -> Result<Option<Suggestion>> {
    Ok(query_suggestions(conn, "WHERE id = ?1", [id])?
        .into_iter()
        .next())
}

/// Pending `pr_declined` and `worktree_removed_unfinished` suggestions, oldest first
/// (Blockers in the daily summary).
#[cfg(test)]
pub fn list_unresolved_keep_or_abandon(conn: &Connection) -> Result<Vec<Suggestion>> {
    query_suggestions(
        conn,
        "WHERE status = 'pending' AND rule IN (?1, ?2) ORDER BY created_at, id",
        [RULE_PR_DECLINED, RULE_WORKTREE_REMOVED_UNFINISHED],
    )
}

fn set_status(conn: &Connection, id: i64, status: SuggestionStatus, now: i64) -> Result<bool> {
    let n = conn.execute(
        "UPDATE suggestions SET status = ?1, decided_at = ?2 WHERE id = ?3 AND status = 'pending'",
        params![status.as_str(), now, id],
    )?;
    Ok(n == 1)
}

fn pending_by_id(conn: &Connection, id: i64) -> Result<Option<Suggestion>> {
    Ok(get(conn, id)?.filter(|s| s.status == SuggestionStatus::Pending))
}

/// Accept a pending suggestion. `edited_title` replaces an `AddTodo` title when non-empty.
/// Accepting a keep/abandon suggestion keeps the WorkItem. `None` if the id is not pending.
pub fn accept(
    conn: &Connection,
    id: i64,
    edited_title: Option<&str>,
    todos: &[Todo],
    now: i64,
) -> Result<Option<Accepted>> {
    let Some(s) = pending_by_id(conn, id)? else {
        return Ok(None);
    };
    let out = apply_payload(&s.payload, edited_title, false, todos, local_date(now));
    set_status(conn, id, SuggestionStatus::Accepted, now)?;
    Ok(Some(out))
}

/// Keep (`abandon = false`) or abandon a keep/abandon suggestion. Abandon returns the WorkItem
/// to mark abandoned and `Dropped` updates for its active linked todos. Other payloads are
/// accepted as by [`accept`]. `None` if the id is not pending.
pub fn decide(
    conn: &Connection,
    id: i64,
    abandon: bool,
    todos: &[Todo],
    now: i64,
) -> Result<Option<Accepted>> {
    let Some(s) = pending_by_id(conn, id)? else {
        return Ok(None);
    };
    let out = apply_payload(&s.payload, None, abandon, todos, local_date(now));
    set_status(conn, id, SuggestionStatus::Accepted, now)?;
    Ok(Some(out))
}

/// Reject permanently. Returns false if the id was not pending.
pub fn reject(conn: &Connection, id: i64, now: i64) -> Result<bool> {
    set_status(conn, id, SuggestionStatus::Rejected, now)
}

/// Accept every pending suggestion (keep/abandon ones are kept). `AddTodo`s with the same
/// normalized title as an existing todo or an earlier one in the batch add nothing.
pub fn accept_all(conn: &Connection, todos: &[Todo], now: i64) -> Result<Accepted> {
    let tx = conn.unchecked_transaction()?;
    let today = local_date(now);
    let mut titles: HashSet<String> = todos.iter().map(|t| normalize_title(&t.title)).collect();
    let mut out = Accepted::default();
    let mut pending = list_pending(&tx)?;
    pending.reverse(); // oldest first, so todos keep creation order
    for s in pending {
        if let SuggestionPayload::AddTodo { title, .. } = &s.payload {
            if !titles.insert(normalize_title(title)) {
                set_status(&tx, s.id, SuggestionStatus::Accepted, now)?;
                continue;
            }
        }
        let a = apply_payload(&s.payload, None, false, todos, today);
        out.todo_ops.extend(a.todo_ops);
        set_status(&tx, s.id, SuggestionStatus::Accepted, now)?;
    }
    tx.commit()?;
    Ok(out)
}

/// Reject every pending suggestion. Returns how many were rejected.
pub fn reject_all(conn: &Connection, now: i64) -> Result<usize> {
    Ok(conn.execute(
        "UPDATE suggestions SET status = 'rejected', decided_at = ?1 WHERE status = 'pending'",
        [now],
    )?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Flags, LocalBranchState, PrData};
    use crate::sessions::{SessionLink, SessionSummary, SummaryStatus};
    use crate::store::open_in_memory;

    const T0: i64 = 1_795_000_000; // 2026-11-18 (UTC), mid-day-ish
    const REPO: &str = "abcdef0123456789";

    fn names() -> HashMap<RepoId, String> {
        HashMap::from([(REPO.to_string(), "API/auth".to_string())])
    }

    fn item(branch: &str, state: WorkState) -> WorkItem {
        WorkItem {
            repo_id: REPO.into(),
            repo_name: "API/auth".into(),
            branch: branch.into(),
            state,
            flags: Flags::default(),
            local: LocalBranchState {
                branch: branch.into(),
                local_exists: true,
                ..LocalBranchState::default()
            },
            pr: None,
            abandoned_manual: false,
            last_activity: None,
            state_since: None,
            linked_todos: 0,
            session_count: 0,
        }
    }

    fn with_pr(mut w: WorkItem, id: u64, state: PrStateKind, tasks: u32) -> WorkItem {
        w.pr = Some(PrData {
            id,
            state,
            title: "t".into(),
            url: "u".into(),
            author_slug: "me".into(),
            reviewers: vec![],
            open_tasks: tasks,
            comment_count: 0,
            from_latest_commit: None,
            updated_at: None,
            closed_at: (state == PrStateKind::Merged).then_some(T0 - 86_400),
        });
        w
    }

    fn todo(id: &str, title: &str, link: Option<&str>, status: TodoStatus) -> Todo {
        let mut t = Todo::new(title);
        t.id = Some(id.into());
        t.link = link.map(str::to_string);
        t.status = status;
        t
    }

    fn event(conn: &Connection, branch: &str, from: Option<WorkState>, to: WorkState, at: i64) {
        conn.execute(
            "INSERT INTO state_events(repo_id, branch, from_state, to_state, at) VALUES (?1,?2,?3,?4,?5)",
            params![REPO, branch, from.map(|s| s.as_str()), to.as_str(), at],
        )
        .unwrap();
    }

    struct Fixture {
        items: Vec<WorkItem>,
        sessions: Vec<Session>,
        todos: Vec<Todo>,
        rules: Rules,
        names: HashMap<RepoId, String>,
        first_run: bool,
        now: i64,
    }

    impl Fixture {
        fn new() -> Self {
            Self {
                items: vec![],
                sessions: vec![],
                todos: vec![],
                rules: Rules::default(),
                names: names(),
                first_run: false,
                now: T0,
            }
        }
        fn run(&self, conn: &Connection) -> RulesOutput {
            run(
                conn,
                &RulesContext {
                    work_items: &self.items,
                    sessions: &self.sessions,
                    todos: &self.todos,
                    rules: &self.rules,
                    repo_names: &self.names,
                    first_run: self.first_run,
                    now: self.now,
                },
            )
            .unwrap()
        }
    }

    fn keys(out: &RulesOutput) -> Vec<String> {
        out.new_suggestions
            .iter()
            .map(|s| s.subject_key.clone())
            .collect()
    }

    #[test]
    fn normalize_collapses_and_lowercases() {
        assert_eq!(normalize_title("  Fix   THE\tTest \n"), "fix the test");
    }

    #[test]
    fn new_unfinished_branch_suggested_once() {
        let conn = open_in_memory().unwrap();
        let mut f = Fixture::new();
        f.items = vec![item("feature/x", WorkState::LocalOnly)];
        event(&conn, "feature/x", None, WorkState::LocalOnly, T0);
        let out = f.run(&conn);
        assert_eq!(keys(&out), ["new_unfinished_branch:API/auth:feature/x"]);
        assert_eq!(
            out.new_suggestions[0].payload,
            SuggestionPayload::AddTodo {
                title: "feature/x".into(),
                link: Some("API/auth:feature/x".into()),
                source: "rule:new_unfinished_branch".into(),
            }
        );
        // Same transition again (e.g. after abandon → unfinished): deduped by subject key.
        event(
            &conn,
            "feature/x",
            Some(WorkState::Abandoned),
            WorkState::Pushed,
            T0 + 60,
        );
        assert!(f.run(&conn).new_suggestions.is_empty());
        assert_eq!(pending_count(&conn).unwrap(), 1);
    }

    #[test]
    fn cursor_prevents_reprocessing() {
        let conn = open_in_memory().unwrap();
        let mut f = Fixture::new();
        f.items = vec![with_pr(
            item("b", WorkState::PrNeedsWork),
            42,
            PrStateKind::Open,
            0,
        )];
        event(&conn, "b", None, WorkState::PrNeedsWork, T0);
        assert_eq!(f.run(&conn).new_suggestions.len(), 2);
        let pending = list_pending(&conn).unwrap();
        for s in &pending {
            assert!(reject(&conn, s.id, T0).unwrap());
        }
        // Clearing the decided rows would re-suggest if the cursor did not advance.
        conn.execute("DELETE FROM suggestions", []).unwrap();
        assert!(f.run(&conn).new_suggestions.is_empty());
    }

    #[test]
    fn existing_linked_todo_suppresses_new_branch() {
        let conn = open_in_memory().unwrap();
        let mut f = Fixture::new();
        f.items = vec![item("b", WorkState::Pushed)];
        f.todos = vec![todo("t_1", "x", Some("API/auth:b"), TodoStatus::Done)];
        event(&conn, "b", None, WorkState::Pushed, T0);
        assert!(f.run(&conn).new_suggestions.is_empty());
    }

    #[test]
    fn reject_is_permanent() {
        let conn = open_in_memory().unwrap();
        let mut f = Fixture::new();
        f.items = vec![item("b", WorkState::LocalOnly)];
        event(&conn, "b", None, WorkState::LocalOnly, T0);
        let out = f.run(&conn);
        let id = out.new_suggestions[0].id;
        assert!(reject(&conn, id, T0 + 1).unwrap());
        assert!(!reject(&conn, id, T0 + 2).unwrap(), "already decided");
        let s = get(&conn, id).unwrap().unwrap();
        assert_eq!(s.status, SuggestionStatus::Rejected);
        assert_eq!(s.decided_at, Some(T0 + 1));
        // Trigger again, also via first run: still not re-suggested.
        event(
            &conn,
            "b",
            Some(WorkState::Abandoned),
            WorkState::LocalOnly,
            T0 + 100,
        );
        f.first_run = true;
        assert!(f.run(&conn).new_suggestions.is_empty());
        assert_eq!(pending_count(&conn).unwrap(), 0);
    }

    #[test]
    fn second_review_round_retriggers() {
        let conn = open_in_memory().unwrap();
        let mut f = Fixture::new();
        f.rules.new_unfinished_branch = RuleMode::Off;
        f.items = vec![with_pr(
            item("b", WorkState::PrNeedsWork),
            42,
            PrStateKind::Open,
            0,
        )];
        event(
            &conn,
            "b",
            Some(WorkState::PrOpen),
            WorkState::PrNeedsWork,
            T0,
        );
        let out = f.run(&conn);
        assert_eq!(
            keys(&out),
            [format!("pr_needs_work:API/auth:b:42:{}", minute_utc(T0))]
        );
        assert_eq!(
            out.new_suggestions[0].payload.text(),
            "Address review on PR #42"
        );
        reject(&conn, out.new_suggestions[0].id, T0).unwrap();
        // Round two after the reviewer approved, then asked for work again.
        event(
            &conn,
            "b",
            Some(WorkState::PrNeedsWork),
            WorkState::PrApproved,
            T0 + 3600,
        );
        event(
            &conn,
            "b",
            Some(WorkState::PrApproved),
            WorkState::PrNeedsWork,
            T0 + 7200,
        );
        let out = f.run(&conn);
        assert_eq!(
            keys(&out),
            [format!(
                "pr_needs_work:API/auth:b:42:{}",
                minute_utc(T0 + 7200)
            )]
        );
    }

    #[test]
    fn needs_work_example_key_format() {
        // 2026-11-25T10:02Z from the spec.
        let ts = DateTime::parse_from_rfc3339("2026-11-25T10:02:30Z")
            .unwrap()
            .timestamp();
        assert_eq!(minute_utc(ts), "2026-11-25T10:02Z");
    }

    #[test]
    fn pr_merged_auto_completes_only_linked_active_todos() {
        let conn = open_in_memory().unwrap();
        let mut f = Fixture::new();
        f.items = vec![with_pr(
            item("b", WorkState::Merged),
            7,
            PrStateKind::Merged,
            0,
        )];
        f.todos = vec![
            todo("t_todo", "a", Some("API/auth:b"), TodoStatus::Todo),
            todo("t_doing", "b", Some("API/auth:b"), TodoStatus::Doing),
            todo("t_done", "c", Some("API/auth:b"), TodoStatus::Done),
            todo("t_dropped", "d", Some("API/auth:b"), TodoStatus::Dropped),
            todo("t_other", "e", Some("API/auth:other"), TodoStatus::Todo),
            todo("t_none", "f", None, TodoStatus::Todo),
        ];
        event(
            &conn,
            "b",
            Some(WorkState::PrApproved),
            WorkState::Merged,
            T0,
        );
        let out = f.run(&conn);
        let merge_date = local_date(T0 - 86_400);
        let expected: Vec<TodoOp> = ["t_todo", "t_doing"]
            .iter()
            .map(|id| TodoOp::Update {
                id: id.to_string(),
                patch: TodoPatch {
                    status: Some(TodoStatus::Done),
                    done: Some(Some(merge_date)),
                    ..TodoPatch::default()
                },
            })
            .collect();
        assert_eq!(out.todo_ops, expected);
        assert!(out.new_suggestions.is_empty());
        assert_eq!(pending_count(&conn).unwrap(), 0);
        // Recorded as accepted.
        let status: String = conn
            .query_row(
                "SELECT status FROM suggestions WHERE subject_key = 'pr_merged:API/auth:b:7'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(status, "accepted");
    }

    #[test]
    fn pr_merged_suggest_mode_creates_complete_suggestion() {
        let conn = open_in_memory().unwrap();
        let mut f = Fixture::new();
        f.rules.pr_merged = RuleMode::Suggest;
        f.items = vec![with_pr(
            item("b", WorkState::Merged),
            7,
            PrStateKind::Merged,
            0,
        )];
        f.todos = vec![todo("t_1", "a", Some("API/auth:b"), TodoStatus::Todo)];
        event(&conn, "b", Some(WorkState::PrOpen), WorkState::Merged, T0);
        let out = f.run(&conn);
        assert!(out.todo_ops.is_empty());
        let id = out.new_suggestions[0].id;
        let acc = accept(&conn, id, None, &f.todos, T0).unwrap().unwrap();
        assert_eq!(acc.todo_ops.len(), 1);
    }

    #[test]
    fn mode_off_does_nothing() {
        let conn = open_in_memory().unwrap();
        let mut f = Fixture::new();
        f.rules = Rules {
            new_unfinished_branch: RuleMode::Off,
            pr_merged: RuleMode::Off,
            pr_declined: RuleMode::Off,
            pr_needs_work: RuleMode::Off,
            session_unfinished: RuleMode::Off,
            worktree_removed_unfinished: RuleMode::Off,
        };
        f.first_run = true;
        f.items = vec![
            with_pr(item("m", WorkState::Merged), 1, PrStateKind::Merged, 0),
            with_pr(
                item("d", WorkState::PrDeclined),
                2,
                PrStateKind::Declined,
                0,
            ),
            with_pr(item("n", WorkState::PrNeedsWork), 3, PrStateKind::Open, 0),
            item("l", WorkState::LocalOnly),
        ];
        f.todos = vec![todo("t_1", "a", Some("API/auth:m"), TodoStatus::Todo)];
        f.sessions = vec![session("s1", &["finish it"])];
        event(&conn, "m", None, WorkState::Merged, T0);
        event(&conn, "d", None, WorkState::PrDeclined, T0);
        event(&conn, "n", None, WorkState::PrNeedsWork, T0);
        event(&conn, "l", None, WorkState::LocalOnly, T0);
        let out = f.run(&conn);
        assert_eq!(out, RulesOutput::default());
        let rows: i64 = conn
            .query_row("SELECT COUNT(*) FROM suggestions", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, 0);
    }

    #[test]
    fn first_run_batch() {
        let conn = open_in_memory().unwrap();
        let mut f = Fixture::new();
        f.first_run = true;
        f.rules.new_unfinished_branch = RuleMode::Auto; // first run still only suggests
        f.items = vec![
            item("a", WorkState::LocalOnly),
            item("b", WorkState::Pushed),
            with_pr(item("c", WorkState::PrOpen), 5, PrStateKind::Open, 0),
            item("main", WorkState::Protected),
            item("old", WorkState::MergedNoPr),
            item("linked", WorkState::Pushed),
        ];
        f.todos = vec![todo("t_1", "x", Some("API/auth:linked"), TodoStatus::Todo)];
        let out = f.run(&conn);
        assert!(out.todo_ops.is_empty());
        let mut k = keys(&out);
        k.sort();
        assert_eq!(
            k,
            [
                "new_unfinished_branch:API/auth:a",
                "new_unfinished_branch:API/auth:b",
                "new_unfinished_branch:API/auth:c",
            ]
        );
        // A later normal run does not repeat them.
        f.first_run = false;
        event(&conn, "a", None, WorkState::LocalOnly, T0);
        assert!(f.run(&conn).new_suggestions.is_empty());
    }

    #[test]
    fn open_task_count_increase_triggers() {
        let conn = open_in_memory().unwrap();
        let mut f = Fixture::new();
        f.rules.new_unfinished_branch = RuleMode::Off;
        f.items = vec![with_pr(
            item("b", WorkState::PrOpen),
            9,
            PrStateKind::Open,
            1,
        )];
        // Baseline.
        assert!(f.run(&conn).new_suggestions.is_empty());
        // Unchanged.
        f.now += 10;
        assert!(f.run(&conn).new_suggestions.is_empty());
        // Decrease.
        f.items = vec![with_pr(
            item("b", WorkState::PrOpen),
            9,
            PrStateKind::Open,
            0,
        )];
        f.now += 10;
        assert!(f.run(&conn).new_suggestions.is_empty());
        // Increase.
        f.items = vec![with_pr(
            item("b", WorkState::PrOpen),
            9,
            PrStateKind::Open,
            2,
        )];
        f.now += 120;
        let out = f.run(&conn);
        assert_eq!(
            keys(&out),
            [format!("pr_needs_work:API/auth:b:9:{}", minute_utc(f.now))]
        );
        // Another increase while the first is still pending: no duplicate.
        f.items = vec![with_pr(
            item("b", WorkState::PrOpen),
            9,
            PrStateKind::Open,
            3,
        )];
        f.now += 120;
        assert!(f.run(&conn).new_suggestions.is_empty());
    }

    fn session(id: &str, unfinished: &[&str]) -> Session {
        Session {
            id: id.into(),
            cwd: None,
            transcript_path: None,
            title: None,
            started_at: Some(T0 - 100),
            last_activity_at: Some(T0 - 10),
            ended_at: Some(T0 - 10),
            end_reason: None,
            user_turns: 3,
            branches: vec![],
            files_touched: vec![],
            explicit_todos: vec![],
            summary_status: SummaryStatus::Done,
            summary: Some(SessionSummary {
                title: "t".into(),
                summary: "s".into(),
                unfinished: unfinished.iter().map(|s| s.to_string()).collect(),
                ..SessionSummary::default()
            }),
            summarized_at: Some(T0),
            links: vec![SessionLink {
                repo_id: REPO.into(),
                branch: "feature/y".into(),
            }],
        }
    }

    #[test]
    fn session_unfinished_dedupes_by_normalized_title() {
        let conn = open_in_memory().unwrap();
        let mut f = Fixture::new();
        f.todos = vec![todo("t_1", "Fix  the Flaky test", None, TodoStatus::Done)];
        let mut s = session(
            "s1",
            &[
                "fix the flaky   TEST",
                "Write \u{1b}[31mdocs",
                "write docs",
                "",
            ],
        );
        s.explicit_todos = vec!["Add retries".into()];
        f.sessions = vec![s];
        let out = f.run(&conn);
        let payloads: Vec<_> = out.new_suggestions.iter().map(|s| &s.payload).collect();
        assert_eq!(
            payloads,
            [
                &SuggestionPayload::AddTodo {
                    title: "Write docs".into(),
                    link: Some("API/auth:feature/y".into()),
                    source: "session:s1".into(),
                },
                &SuggestionPayload::AddTodo {
                    title: "Add retries".into(),
                    link: Some("API/auth:feature/y".into()),
                    source: "session:s1".into(),
                },
            ]
        );
        // Another session with the same item while it is pending: skipped.
        let mut s2 = session("s2", &["WRITE DOCS"]);
        s2.summarized_at = Some(T0 + 50);
        f.sessions.push(s2);
        assert!(f.run(&conn).new_suggestions.is_empty());
    }

    #[test]
    fn session_cursor_skips_old_sessions() {
        let conn = open_in_memory().unwrap();
        let mut f = Fixture::new();
        f.sessions = vec![session("s1", &["one"])];
        assert_eq!(f.run(&conn).new_suggestions.len(), 1);
        reject_all(&conn, T0).unwrap();
        let mut old = session("s0", &["two"]);
        old.summarized_at = Some(T0 - 1000);
        f.sessions.push(old);
        assert!(f.run(&conn).new_suggestions.is_empty());
    }

    #[test]
    fn pr_declined_decide_abandon_drops_linked_todos() {
        let conn = open_in_memory().unwrap();
        let mut f = Fixture::new();
        f.items = vec![with_pr(
            item("b", WorkState::PrDeclined),
            31,
            PrStateKind::Declined,
            0,
        )];
        f.todos = vec![
            todo("t_1", "a", Some("API/auth:b"), TodoStatus::Doing),
            todo("t_2", "b", Some("API/auth:b"), TodoStatus::Done),
        ];
        event(
            &conn,
            "b",
            Some(WorkState::PrOpen),
            WorkState::PrDeclined,
            T0,
        );
        let out = f.run(&conn);
        assert_eq!(keys(&out), ["pr_declined:API/auth:b:31"]);
        assert_eq!(
            out.new_suggestions[0].payload.text(),
            "PR #31 declined: keep or abandon"
        );
        assert!(out.new_suggestions[0].payload.needs_choice());
        assert_eq!(list_unresolved_keep_or_abandon(&conn).unwrap().len(), 1);
        let acc = decide(&conn, out.new_suggestions[0].id, true, &f.todos, T0)
            .unwrap()
            .unwrap();
        assert_eq!(acc.abandon, Some((REPO.to_string(), "b".to_string())));
        assert_eq!(
            acc.todo_ops,
            [TodoOp::Update {
                id: "t_1".into(),
                patch: TodoPatch {
                    status: Some(TodoStatus::Dropped),
                    done: Some(Some(local_date(T0))),
                    ..TodoPatch::default()
                },
            }]
        );
        assert!(list_unresolved_keep_or_abandon(&conn).unwrap().is_empty());
        assert!(decide(&conn, out.new_suggestions[0].id, true, &f.todos, T0)
            .unwrap()
            .is_none());
    }

    #[test]
    fn pr_declined_auto_abandons() {
        let conn = open_in_memory().unwrap();
        let mut f = Fixture::new();
        f.rules.pr_declined = RuleMode::Auto;
        f.items = vec![with_pr(
            item("b", WorkState::PrDeclined),
            31,
            PrStateKind::Declined,
            0,
        )];
        f.todos = vec![todo("t_1", "a", Some("API/auth:b"), TodoStatus::Todo)];
        event(
            &conn,
            "b",
            Some(WorkState::PrOpen),
            WorkState::PrDeclined,
            T0,
        );
        let out = f.run(&conn);
        assert_eq!(out.abandon, [(REPO.to_string(), "b".to_string())]);
        assert_eq!(out.todo_ops.len(), 1);
    }

    #[test]
    fn stale_events_are_ignored_for_suggestions() {
        let conn = open_in_memory().unwrap();
        let mut f = Fixture::new();
        f.items = vec![with_pr(
            item("b", WorkState::PrApproved),
            4,
            PrStateKind::Open,
            0,
        )];
        f.todos = vec![todo("t_1", "a", Some("API/auth:b"), TodoStatus::Todo)];
        event(
            &conn,
            "b",
            Some(WorkState::PrOpen),
            WorkState::PrNeedsWork,
            T0,
        );
        event(
            &conn,
            "b",
            Some(WorkState::PrNeedsWork),
            WorkState::PrApproved,
            T0 + 5,
        );
        assert!(f.run(&conn).new_suggestions.is_empty());
    }

    #[test]
    fn worktree_removed_while_unfinished() {
        let conn = open_in_memory().unwrap();
        let mut f = Fixture::new();
        f.rules.new_unfinished_branch = RuleMode::Off;
        let mut w = item("b", WorkState::Pushed);
        w.local.worktree = Some("/wt/b".into());
        let mut done = item("m", WorkState::MergedNoPr);
        done.local.worktree = Some("/wt/m".into());
        f.items = vec![w.clone(), done.clone()];
        assert!(f.run(&conn).new_suggestions.is_empty());
        w.local.worktree = None;
        done.local.worktree = None;
        f.items = vec![w, done];
        f.now += 60;
        let out = f.run(&conn);
        assert_eq!(
            keys(&out),
            [format!(
                "worktree_removed_unfinished:API/auth:b:/wt/b:{}",
                minute_utc(f.now)
            )]
        );
        assert_eq!(list_unresolved_keep_or_abandon(&conn).unwrap().len(), 1);
    }

    #[test]
    fn accept_with_edited_title() {
        let conn = open_in_memory().unwrap();
        let mut f = Fixture::new();
        f.items = vec![item("b", WorkState::LocalOnly)];
        event(&conn, "b", None, WorkState::LocalOnly, T0);
        let id = f.run(&conn).new_suggestions[0].id;
        let acc = accept(&conn, id, Some("  Ship it "), &[], T0)
            .unwrap()
            .unwrap();
        let [TodoOp::Add(t)] = acc.todo_ops.as_slice() else {
            panic!("expected one add: {acc:?}");
        };
        assert_eq!(t.title, "Ship it");
        assert_eq!(t.link.as_deref(), Some("API/auth:b"));
        assert_eq!(t.source.as_deref(), Some("rule:new_unfinished_branch"));
        assert!(t.id.as_deref().is_some_and(|i| i.starts_with("t_")));
        assert_eq!(t.created, Some(local_date(T0)));
        assert!(accept(&conn, id, None, &[], T0).unwrap().is_none());
        assert_eq!(
            get(&conn, id).unwrap().unwrap().status,
            SuggestionStatus::Accepted
        );
    }

    #[test]
    fn accept_all_and_reject_all() {
        let conn = open_in_memory().unwrap();
        let mut f = Fixture::new();
        f.first_run = true;
        f.items = vec![
            item("a", WorkState::LocalOnly),
            item("b", WorkState::LocalOnly),
            with_pr(
                item("d", WorkState::PrDeclined),
                3,
                PrStateKind::Declined,
                0,
            ),
        ];
        event(&conn, "d", None, WorkState::PrDeclined, T0);
        f.run(&conn);
        assert_eq!(pending_count(&conn).unwrap(), 4);
        let acc = accept_all(&conn, &[], T0 + 1).unwrap();
        // Three adds (a, b, d branches); the declined PR is kept, not abandoned.
        assert_eq!(acc.todo_ops.len(), 3);
        assert!(acc.abandon.is_none());
        assert_eq!(pending_count(&conn).unwrap(), 0);

        let conn = open_in_memory().unwrap();
        event(&conn, "d", None, WorkState::PrDeclined, T0);
        f.run(&conn);
        assert_eq!(pending_count(&conn).unwrap(), 4);
        assert_eq!(reject_all(&conn, T0 + 1).unwrap(), 4);
        assert_eq!(pending_count(&conn).unwrap(), 0);
        assert!(f.run(&conn).new_suggestions.is_empty());
    }

    #[test]
    fn list_pending_newest_first() {
        let conn = open_in_memory().unwrap();
        for (i, key) in ["a", "b", "c"].iter().enumerate() {
            insert_suggestion(
                &conn,
                &NewSuggestion {
                    rule: RULE_NEW_UNFINISHED_BRANCH.into(),
                    subject_key: key.to_string(),
                    payload: SuggestionPayload::AddTodo {
                        title: key.to_string(),
                        link: None,
                        source: "rule:x".into(),
                    },
                },
                SuggestionStatus::Pending,
                T0 + i as i64,
            )
            .unwrap();
        }
        let dup = insert_suggestion(
            &conn,
            &NewSuggestion {
                rule: "r".into(),
                subject_key: "a".into(),
                payload: SuggestionPayload::AddTodo {
                    title: "z".into(),
                    link: None,
                    source: "s".into(),
                },
            },
            SuggestionStatus::Pending,
            T0,
        )
        .unwrap();
        assert!(dup.is_none());
        let order: Vec<_> = list_pending(&conn)
            .unwrap()
            .into_iter()
            .map(|s| s.subject_key)
            .collect();
        assert_eq!(order, ["c", "b", "a"]);
    }
}
