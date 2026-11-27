//! Daily summary builder + Markdown export (spec 8).
//!
//! Flow for callers:
//! 1. `Window::for_since(since, today)` (or `Window::default_for(today, &workdays)`).
//! 2. Gather inputs: `load_events_since(conn, window.start_ts)`, `load_unresolved_choices(conn)`,
//!    the current WorkItems, todos and sessions.
//! 3. `build(&input, &window)` → `Summary`, then `to_markdown(&summary)`.
//! 4. `write_export(dir, window.today, &md)` for `w`, or print for `worktrack summary`.
//!
//! Event times come from `state_events`, todo `done` dates and session timestamps, never from
//! live state (spec 8.1).

use crate::model::{CiState, WorkItem, WorkState};
use crate::sanitize::clean_line;
use crate::sessions::Session;
use crate::todos::{Priority, Todo, TodoStatus};
use chrono::{Datelike, Duration, Local, NaiveDate, TimeZone, Weekday};
use color_eyre::eyre::Result;
use rusqlite::Connection;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Number of Active todos listed in the Markdown before `+N more`.
pub const TODO_LIMIT: usize = 10;

/// The latest workday in `workdays` strictly before `today` (Monday → Friday).
pub fn previous_workday(today: NaiveDate, workdays: &[Weekday]) -> NaiveDate {
    shift_workday(today, workdays, -1)
}

/// Move `date` by `delta` workdays (negative = back). With no workdays configured, moves by
/// calendar days.
pub fn shift_workday(date: NaiveDate, workdays: &[Weekday], delta: i32) -> NaiveDate {
    let step = if delta < 0 { -1 } else { 1 };
    let mut d = date;
    for _ in 0..delta.unsigned_abs() {
        d += Duration::days(step);
        if !workdays.is_empty() {
            let mut guard = 0;
            while !workdays.contains(&d.weekday()) && guard < 7 {
                d += Duration::days(step);
                guard += 1;
            }
        }
    }
    d
}

pub fn is_workday(date: NaiveDate, workdays: &[Weekday]) -> bool {
    workdays.is_empty() || workdays.contains(&date.weekday())
}

/// Summary window: from 00:00 local time on `since` until now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Window {
    pub since: NaiveDate,
    pub today: NaiveDate,
    /// Unix seconds of 00:00 local time on `since`.
    pub start_ts: i64,
}

impl Window {
    pub fn for_since(since: NaiveDate, today: NaiveDate) -> Self {
        Self {
            since,
            today,
            start_ts: local_midnight(since),
        }
    }

    /// Default window: since the previous workday.
    pub fn default_for(today: NaiveDate, workdays: &[Weekday]) -> Self {
        Self::for_since(previous_workday(today, workdays), today)
    }
}

/// Unix seconds of 00:00 local time on `date`.
pub fn local_midnight(date: NaiveDate) -> i64 {
    let naive = date.and_hms_opt(0, 0, 0).expect("midnight is valid");
    Local
        .from_local_datetime(&naive)
        .earliest()
        .map(|dt| dt.timestamp())
        .unwrap_or_else(|| naive.and_utc().timestamp())
}

/// One `state_events` row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateEvent {
    pub repo_id: String,
    pub branch: String,
    pub from_state: Option<WorkState>,
    pub to_state: WorkState,
    pub at: i64,
}

pub fn load_events_since(conn: &Connection, start_ts: i64) -> Result<Vec<StateEvent>> {
    let mut stmt = conn.prepare(
        "SELECT repo_id, branch, from_state, to_state, at FROM state_events
         WHERE at >= ?1 ORDER BY at, id",
    )?;
    let rows = stmt.query_map([start_ts], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, Option<String>>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, i64>(4)?,
        ))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (repo_id, branch, from, to, at) = row?;
        let Some(to_state) = WorkState::parse(&to) else {
            continue;
        };
        out.push(StateEvent {
            repo_id,
            branch,
            from_state: from.as_deref().and_then(WorkState::parse),
            to_state,
            at,
        });
    }
    Ok(out)
}

/// A pending keep/abandon suggestion (`pr_declined`, `worktree_removed_unfinished`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnresolvedChoice {
    pub rule: String,
    pub message: String,
}

pub fn load_unresolved_choices(conn: &Connection) -> Result<Vec<UnresolvedChoice>> {
    let mut stmt = conn.prepare(
        "SELECT rule, payload FROM suggestions
         WHERE status = 'pending' AND rule IN ('pr_declined', 'worktree_removed_unfinished')
         ORDER BY created_at, id",
    )?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
    let mut out = Vec::new();
    for row in rows {
        let (rule, payload) = row?;
        let message = serde_json::from_str::<serde_json::Value>(&payload)
            .ok()
            .and_then(|v| {
                v.get("message")
                    .and_then(|m| m.as_str())
                    .map(str::to_string)
            })
            .unwrap_or_else(|| rule.clone());
        out.push(UnresolvedChoice { rule, message });
    }
    Ok(out)
}

pub struct SummaryInput<'a> {
    pub items: &'a [WorkItem],
    pub todos: &'a [Todo],
    pub sessions: &'a [Session],
    pub events: &'a [StateEvent],
    pub unresolved: &'a [UnresolvedChoice],
}

/// What a summary entry points at, so the TUI can show its details and jump to it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub enum Target {
    #[default]
    None,
    /// A WorkItem.
    Item {
        repo_id: String,
        branch: String,
    },
    /// A todo; `id` is `None` for todos not written back with an id yet (match by title).
    Todo {
        id: Option<String>,
        title: String,
    },
    Session {
        id: String,
    },
    /// A pending keep/abandon choice in the Inbox.
    Inbox,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Entry {
    pub line: String,
    pub children: Vec<String>,
    pub target: Target,
}

impl Entry {
    fn leaf(line: String, target: Target) -> Self {
        Self {
            line,
            children: Vec::new(),
            target,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Summary {
    pub today: Option<NaiveDate>,
    pub since: Option<NaiveDate>,
    pub done: Vec<Entry>,
    pub in_progress: Vec<Entry>,
    /// Every Active `todo` item, priority A first. The Markdown lists the first
    /// [`TODO_LIMIT`] and `+N more`.
    pub todo: Vec<Entry>,
    pub blockers: Vec<Entry>,
    pub cleanup: Vec<Entry>,
}

fn key(repo_id: &str, branch: &str) -> (String, String) {
    (repo_id.to_string(), branch.to_string())
}

fn item_target(item: &WorkItem) -> Target {
    Target::Item {
        repo_id: item.repo_id.clone(),
        branch: item.branch.clone(),
    }
}

fn todo_target(t: &Todo) -> Target {
    Target::Todo {
        id: t.id.clone(),
        title: t.title.clone(),
    }
}

fn session_target(s: &Session) -> Target {
    Target::Session { id: s.id.clone() }
}

fn sessions_for<'a>(sessions: &'a [Session], item: &WorkItem) -> Vec<&'a Session> {
    sessions
        .iter()
        .filter(|s| {
            s.links
                .iter()
                .any(|l| l.repo_id == item.repo_id && l.branch == item.branch)
        })
        .collect()
}

fn session_in_window(s: &Session, start: i64) -> bool {
    s.last_activity_at
        .or(s.started_at)
        .is_some_and(|t| t >= start)
}

fn session_text(s: &Session) -> String {
    let text = s
        .summary
        .as_ref()
        .map(|sum| {
            if sum.summary.is_empty() {
                sum.title.clone()
            } else {
                sum.summary.clone()
            }
        })
        .or_else(|| s.title.clone())
        .unwrap_or_else(|| s.id.clone());
    clean_line(&text)
}

fn session_title(s: &Session) -> String {
    let t = s
        .summary
        .as_ref()
        .map(|sum| sum.title.clone())
        .filter(|t| !t.is_empty())
        .or_else(|| s.title.clone())
        .unwrap_or_else(|| s.id.clone());
    clean_line(&t)
}

fn pr_phrase(item: &WorkItem) -> String {
    let id = item
        .pr
        .as_ref()
        .map(|p| format!("PR #{} ", p.id))
        .unwrap_or_default();
    match item.state {
        WorkState::Merged => format!("{id}merged"),
        WorkState::MergedNoPr => "merged·git".to_string(),
        WorkState::PrNeedsWork => format!("{id}needs work"),
        WorkState::PrApproved => format!("{id}approved"),
        WorkState::PrOpen => format!("{id}open"),
        WorkState::PrDeclined => format!("{id}declined"),
        WorkState::Pushed => "pushed, no PR".to_string(),
        WorkState::LocalOnly => "local only".to_string(),
        WorkState::Abandoned => "abandoned".to_string(),
        WorkState::Protected => "protected".to_string(),
    }
}

fn flag_phrases(item: &WorkItem) -> Vec<String> {
    let f = &item.flags;
    let mut out = Vec::new();
    match f.ci {
        Some(CiState::Failed) => out.push("ci failed".to_string()),
        Some(CiState::InProgress) => out.push("ci running".to_string()),
        Some(CiState::Successful) => out.push("ci ok".to_string()),
        None => {}
    }
    if f.open_tasks > 0 {
        let s = if f.open_tasks == 1 { "" } else { "s" };
        out.push(format!("{} task{s}", f.open_tasks));
    }
    if f.dirty {
        out.push("dirty".to_string());
    }
    if f.ahead > 0 {
        out.push(format!("ahead {}", f.ahead));
    }
    if f.behind > 0 {
        out.push(format!("behind {}", f.behind));
    }
    if f.stale {
        out.push("stale".to_string());
    }
    out
}

fn item_head(item: &WorkItem) -> String {
    format!(
        "**{}** `{}`",
        clean_line(&item.repo_name),
        clean_line(&item.branch)
    )
}

fn todo_line(t: &Todo) -> String {
    let title = clean_line(&t.title);
    match t.priority {
        Some(p) => format!("({}) {title}", p.as_char()),
        None => title,
    }
}

fn priority_rank(p: Option<Priority>) -> u8 {
    match p {
        Some(Priority::A) => 0,
        Some(Priority::B) => 1,
        Some(Priority::C) => 2,
        None => 3,
    }
}

/// Build the summary for `window` (spec 8.2).
pub fn build(input: &SummaryInput, window: &Window) -> Summary {
    let start = window.start_ts;
    let since_date = window.since;
    let by_key: HashMap<(String, String), &WorkItem> = input
        .items
        .iter()
        .map(|i| (key(&i.repo_id, &i.branch), i))
        .collect();

    // Done: items that became Merged / MergedNoPr in the window, by merge time.
    let mut done_events: Vec<&StateEvent> = Vec::new();
    for ev in input
        .events
        .iter()
        .filter(|e| e.at >= start && e.to_state.is_done())
    {
        // Keep the latest transition per item.
        done_events.retain(|d| !(d.repo_id == ev.repo_id && d.branch == ev.branch));
        done_events.push(ev);
    }
    done_events.sort_by_key(|e| e.at);
    let mut done = Vec::new();
    for ev in &done_events {
        let Some(item) = by_key.get(&key(&ev.repo_id, &ev.branch)) else {
            continue;
        };
        if !item.state.is_done() {
            continue; // reused after merge, no longer done
        }
        let mut children: Vec<String> = input
            .todos
            .iter()
            .filter(|t| t.link.as_deref() == Some(item.link_ref().as_str()))
            .map(todo_line)
            .collect();
        children.extend(
            sessions_for(input.sessions, item)
                .into_iter()
                .filter(|s| session_in_window(s, start))
                .map(|s| format!("Session: {}", session_title(s))),
        );
        done.push(Entry {
            line: format!("{} — {}", item_head(item), pr_phrase(item)),
            children,
            target: item_target(item),
        });
    }
    for t in input.todos.iter().filter(|t| {
        t.status == TodoStatus::Done && t.link.is_none() && t.done.is_some_and(|d| d >= since_date)
    }) {
        done.push(Entry::leaf(todo_line(t), todo_target(t)));
    }

    // In progress: unfinished items with activity in the window.
    let mut active_events: HashMap<(String, String), i64> = HashMap::new();
    for ev in input.events.iter().filter(|e| e.at >= start) {
        let k = key(&ev.repo_id, &ev.branch);
        let v = active_events.entry(k).or_insert(ev.at);
        *v = (*v).max(ev.at);
    }
    let mut progress: Vec<(i64, Entry)> = Vec::new();
    for item in input.items.iter().filter(|i| i.state.is_unfinished()) {
        let sessions: Vec<&Session> = sessions_for(input.sessions, item)
            .into_iter()
            .filter(|s| session_in_window(s, start))
            .collect();
        let commit = item.local.last_commit_at.filter(|t| *t >= start);
        let pr = item
            .pr
            .as_ref()
            .and_then(|p| p.updated_at)
            .filter(|t| *t >= start);
        let ev = active_events
            .get(&key(&item.repo_id, &item.branch))
            .copied();
        let sess = sessions.iter().filter_map(|s| s.last_activity_at).max();
        let Some(last) = [commit, pr, ev, sess].into_iter().flatten().max() else {
            continue;
        };
        let mut parts = vec![pr_phrase(item)];
        parts.extend(flag_phrases(item));
        let mut children: Vec<String> = sessions
            .iter()
            .map(|s| format!("Session: {}", session_text(s)))
            .collect();
        children.extend(
            input
                .todos
                .iter()
                .filter(|t| {
                    t.status == TodoStatus::Doing
                        && t.link.as_deref() == Some(item.link_ref().as_str())
                })
                .map(|t| format!("Doing: {}", todo_line(t))),
        );
        progress.push((
            last,
            Entry {
                line: format!("{} — {}", item_head(item), parts.join(" · ")),
                children,
                target: item_target(item),
            },
        ));
    }
    // Unlinked sessions in the window.
    for s in input
        .sessions
        .iter()
        .filter(|s| s.links.is_empty() && session_in_window(s, start))
    {
        let last = s.last_activity_at.or(s.started_at).unwrap_or(start);
        progress.push((
            last,
            Entry {
                line: format!("(no branch) — {}", session_title(s)),
                children: vec![format!("Session: {}", session_text(s))],
                target: session_target(s),
            },
        ));
    }
    progress.sort_by_key(|(t, _)| std::cmp::Reverse(*t));
    let mut in_progress: Vec<Entry> = progress.into_iter().map(|(_, e)| e).collect();
    // Doing todos not already shown under an item.
    let shown_links: Vec<String> = input
        .items
        .iter()
        .filter(|i| i.state.is_unfinished())
        .map(|i| i.link_ref())
        .collect();
    for t in input.todos.iter().filter(|t| t.status == TodoStatus::Doing) {
        let under_item = t.link.as_ref().is_some_and(|l| {
            shown_links.contains(l)
                && in_progress
                    .iter()
                    .any(|e| e.children.iter().any(|c| c.ends_with(&todo_line(t))))
        });
        if !under_item {
            in_progress.push(Entry::leaf(
                format!("Doing: {}", todo_line(t)),
                todo_target(t),
            ));
        }
    }

    // Todo: Active `todo` items, priority A → none, then file order.
    let mut todo_items: Vec<(usize, &Todo)> = input
        .todos
        .iter()
        .filter(|t| t.status == TodoStatus::Todo)
        .enumerate()
        .collect();
    todo_items.sort_by_key(|(idx, t)| (priority_rank(t.priority), *idx));
    let todo: Vec<Entry> = todo_items
        .iter()
        .map(|(_, t)| Entry::leaf(todo_line(t), todo_target(t)))
        .collect();

    // Blockers: needs work, ci failed, unresolved choices, session blockers, stale.
    let mut blockers = Vec::new();
    let pr_label = |i: &WorkItem| match &i.pr {
        Some(p) => format!("PR #{}", p.id),
        None => format!("`{}`", clean_line(&i.branch)),
    };
    for i in input
        .items
        .iter()
        .filter(|i| i.state == WorkState::PrNeedsWork)
    {
        blockers.push(Entry::leaf(
            format!("{}: reviewer requested changes", pr_label(i)),
            item_target(i),
        ));
    }
    for i in input
        .items
        .iter()
        .filter(|i| i.state.is_unfinished() && i.flags.ci == Some(CiState::Failed))
    {
        blockers.push(Entry::leaf(
            format!("{}: CI failed", pr_label(i)),
            item_target(i),
        ));
    }
    for u in input.unresolved {
        blockers.push(Entry::leaf(clean_line(&u.message), Target::Inbox));
    }
    for s in input
        .sessions
        .iter()
        .filter(|s| session_in_window(s, start))
    {
        if let Some(sum) = &s.summary {
            for b in &sum.blockers {
                blockers.push(Entry::leaf(clean_line(b), session_target(s)));
            }
        }
    }
    for i in input
        .items
        .iter()
        .filter(|i| i.state.is_unfinished() && i.flags.stale)
    {
        blockers.push(Entry::leaf(
            format!(
                "{} `{}`: stale",
                clean_line(&i.repo_name),
                clean_line(&i.branch)
            ),
            item_target(i),
        ));
    }

    // Cleanup, by repo.
    let mut cleanup_items: Vec<&WorkItem> =
        input.items.iter().filter(|i| i.flags.cleanup).collect();
    cleanup_items.sort_by(|a, b| (&a.repo_name, &a.branch).cmp(&(&b.repo_name, &b.branch)));
    let cleanup = cleanup_items
        .iter()
        .map(|i| {
            Entry::leaf(
                format!("{} — {}", item_head(i), pr_phrase(i)),
                item_target(i),
            )
        })
        .collect();

    Summary {
        today: Some(window.today),
        since: Some(window.since),
        done,
        in_progress,
        todo,
        blockers,
        cleanup,
    }
}

fn fmt_day(d: NaiveDate) -> String {
    d.format("%a %Y-%m-%d").to_string()
}

fn push_entries(out: &mut String, title: &str, entries: &[Entry]) {
    if entries.is_empty() {
        return;
    }
    out.push_str(&format!("\n## {title}\n"));
    for e in entries {
        out.push_str(&format!("- {}\n", e.line));
        for c in &e.children {
            out.push_str(&format!("  - {c}\n"));
        }
    }
}

/// Render the spec 8.3 Markdown shape. Empty sections are omitted.
pub fn to_markdown(s: &Summary) -> String {
    let mut out = String::new();
    match (s.today, s.since) {
        (Some(t), Some(since)) => out.push_str(&format!(
            "# Summary — {} (since {})\n",
            fmt_day(t),
            fmt_day(since)
        )),
        _ => out.push_str("# Summary\n"),
    }
    push_entries(&mut out, "Done", &s.done);
    push_entries(&mut out, "In progress", &s.in_progress);
    if !s.todo.is_empty() {
        out.push_str("\n## Todo\n");
        for t in s.todo.iter().take(TODO_LIMIT) {
            out.push_str(&format!("- {}\n", t.line));
        }
        let more = s.todo.len().saturating_sub(TODO_LIMIT);
        if more > 0 {
            out.push_str(&format!("- +{more} more\n"));
        }
    }
    push_entries(&mut out, "Blockers", &s.blockers);
    push_entries(&mut out, "Cleanup", &s.cleanup);
    if s.done.is_empty()
        && s.in_progress.is_empty()
        && s.todo.is_empty()
        && s.blockers.is_empty()
        && s.cleanup.is_empty()
    {
        out.push_str("\nNothing to report.\n");
    }
    out
}

/// Write `<dir>/<YYYY-MM-DD>.md`, overwriting the same day's file (temp file + rename).
pub fn write_export(dir: &Path, today: NaiveDate, markdown: &str) -> Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join(format!("{}.md", today.format("%Y-%m-%d")));
    let tmp = dir.join(format!(".{}.md.tmp", today.format("%Y-%m-%d")));
    std::fs::write(&tmp, markdown)?;
    std::fs::rename(&tmp, &path)?;
    Ok(path)
}

/// Whether the Summary should open automatically: first launch on a workday (spec 8). Records
/// today as `last_launch_date` in `meta`.
pub fn should_auto_open(conn: &Connection, today: NaiveDate, workdays: &[Weekday]) -> Result<bool> {
    let last = crate::store::meta_get(conn, "last_launch_date")?;
    let today_s = today.format("%Y-%m-%d").to_string();
    crate::store::meta_set(conn, "last_launch_date", &today_s)?;
    Ok(is_workday(today, workdays) && last.as_deref() != Some(today_s.as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Flags, LocalBranchState, PrData, PrStateKind};
    use crate::sessions::{SessionLink, SessionSummary, SummaryStatus};
    use Weekday::*;

    const WD: [Weekday; 5] = [Mon, Tue, Wed, Thu, Fri];

    fn d(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    #[test]
    fn previous_workday_monday_is_friday() {
        assert_eq!(previous_workday(d("2026-11-23"), &WD), d("2026-11-20")); // Mon → Fri
        assert_eq!(previous_workday(d("2026-11-25"), &WD), d("2026-11-24")); // Wed → Tue
        assert_eq!(previous_workday(d("2026-11-22"), &WD), d("2026-11-20")); // Sun → Fri
        assert_eq!(shift_workday(d("2026-11-20"), &WD, 1), d("2026-11-23"));
        assert_eq!(shift_workday(d("2026-11-25"), &[], -1), d("2026-11-24"));
    }

    #[test]
    fn since_override_window() {
        let w = Window::for_since(d("2026-11-01"), d("2026-11-25"));
        assert_eq!(w.since, d("2026-11-01"));
        assert_eq!(w.start_ts, local_midnight(d("2026-11-01")));
    }

    fn item(branch: &str, state: WorkState, pr: Option<u64>) -> WorkItem {
        WorkItem {
            repo_id: "r1".into(),
            repo_name: "API/auth".into(),
            branch: branch.into(),
            state,
            flags: Flags::default(),
            local: LocalBranchState {
                branch: branch.into(),
                ..Default::default()
            },
            pr: pr.map(|id| PrData {
                id,
                state: PrStateKind::Open,
                title: "t".into(),
                url: "u".into(),
                author_slug: "me".into(),
                reviewers: vec![],
                open_tasks: 0,
                comment_count: 0,
                from_latest_commit: None,
                updated_at: None,
                closed_at: None,
            }),
            abandoned_manual: false,
            last_activity: None,
            state_since: None,
            linked_todos: 0,
            session_count: 0,
        }
    }

    fn todo(title: &str, status: TodoStatus, link: Option<&str>) -> Todo {
        let mut t = Todo::new(title);
        t.status = status;
        t.link = link.map(str::to_string);
        t
    }

    #[test]
    fn sections_from_seeded_events() {
        let w = Window::for_since(d("2026-11-24"), d("2026-11-25"));
        let t0 = w.start_ts + 3600;
        let mut needs = item("feature/retry", WorkState::PrNeedsWork, Some(44));
        needs.flags.ci = Some(CiState::Failed);
        needs.flags.open_tasks = 2;
        let items = vec![
            item("bugfix/flaky", WorkState::Merged, Some(41)),
            needs,
            item("old/thing", WorkState::Pushed, None),
        ];
        let events = vec![
            StateEvent {
                repo_id: "r1".into(),
                branch: "bugfix/flaky".into(),
                from_state: Some(WorkState::PrOpen),
                to_state: WorkState::Merged,
                at: t0,
            },
            StateEvent {
                repo_id: "r1".into(),
                branch: "feature/retry".into(),
                from_state: Some(WorkState::PrOpen),
                to_state: WorkState::PrNeedsWork,
                at: t0 + 10,
            },
            // Before the window: ignored.
            StateEvent {
                repo_id: "r1".into(),
                branch: "old/thing".into(),
                from_state: None,
                to_state: WorkState::Pushed,
                at: w.start_ts - 10,
            },
        ];
        let mut a = todo("Finish upload retry logic", TodoStatus::Todo, None);
        a.priority = Some(Priority::A);
        let todos = vec![
            todo(
                "Fix flaky auth test",
                TodoStatus::Done,
                Some("API/auth:bugfix/flaky"),
            ),
            todo("Write migration doc", TodoStatus::Todo, None),
            a,
        ];
        let sessions = vec![Session {
            id: "s1".into(),
            cwd: None,
            transcript_path: None,
            title: Some("Add backoff".into()),
            started_at: Some(t0),
            last_activity_at: Some(t0 + 20),
            ended_at: Some(t0 + 30),
            end_reason: None,
            user_turns: 4,
            branches: vec!["feature/retry".into()],
            files_touched: vec![],
            explicit_todos: vec![],
            summary_status: SummaryStatus::Done,
            summary: Some(SessionSummary {
                title: "Backoff".into(),
                summary: "Added exponential backoff; integration test still failing".into(),
                blockers: vec!["Staging access missing".into()],
                ..Default::default()
            }),
            summarized_at: None,
            links: vec![SessionLink {
                repo_id: "r1".into(),
                branch: "feature/retry".into(),
            }],
            worked_in: vec![],
        }];
        let input = SummaryInput {
            items: &items,
            todos: &todos,
            sessions: &sessions,
            events: &events,
            unresolved: &[],
        };
        let md = to_markdown(&build(&input, &w));
        let expected = "# Summary — Wed 2026-11-25 (since Tue 2026-11-24)

## Done
- **API/auth** `bugfix/flaky` — PR #41 merged
  - Fix flaky auth test

## In progress
- **API/auth** `feature/retry` — PR #44 needs work · ci failed · 2 tasks
  - Session: Added exponential backoff; integration test still failing

## Todo
- (A) Finish upload retry logic
- Write migration doc

## Blockers
- PR #44: reviewer requested changes
- PR #44: CI failed
- Staging access missing
";
        assert_eq!(md, expected);
    }

    #[test]
    fn entries_carry_targets() {
        let w = Window::for_since(d("2026-11-24"), d("2026-11-25"));
        let t0 = w.start_ts + 60;
        let mut needs = item("feature/retry", WorkState::PrNeedsWork, Some(44));
        needs.flags.ci = Some(CiState::Failed);
        needs.local.last_commit_at = Some(t0);
        let mut stale = item("old/thing", WorkState::Pushed, None);
        stale.flags.stale = true;
        let mut merged = item("bugfix/flaky", WorkState::Merged, Some(41));
        merged.flags.cleanup = true;
        let items = vec![needs, stale, merged];
        let mut doing = todo("Unlinked doing", TodoStatus::Doing, None);
        doing.id = Some("t_d".into());
        let mut done = todo("Unlinked done", TodoStatus::Done, None);
        done.done = Some(d("2026-11-25"));
        let todos = vec![doing, done, todo("Plain", TodoStatus::Todo, None)];
        let sessions = vec![Session {
            id: "s1".into(),
            cwd: None,
            transcript_path: None,
            title: Some("Loose".into()),
            started_at: Some(t0),
            last_activity_at: Some(t0),
            ended_at: None,
            end_reason: None,
            user_turns: 1,
            branches: vec![],
            files_touched: vec![],
            explicit_todos: vec![],
            summary_status: SummaryStatus::Done,
            summary: Some(SessionSummary {
                blockers: vec!["No access".into()],
                ..Default::default()
            }),
            summarized_at: None,
            links: vec![],
            worked_in: vec![],
        }];
        let unresolved = vec![UnresolvedChoice {
            rule: "pr_declined".into(),
            message: "PR #31 declined".into(),
        }];
        let input = SummaryInput {
            items: &items,
            todos: &todos,
            sessions: &sessions,
            events: &[],
            unresolved: &unresolved,
        };
        let s = build(&input, &w);
        let it = |b: &str| Target::Item {
            repo_id: "r1".into(),
            branch: b.into(),
        };
        let targets = |v: &[Entry]| v.iter().map(|e| e.target.clone()).collect::<Vec<_>>();
        assert_eq!(
            targets(&s.done),
            vec![Target::Todo {
                id: None,
                title: "Unlinked done".into()
            }]
        );
        assert_eq!(
            targets(&s.in_progress),
            vec![
                it("feature/retry"),
                Target::Session { id: "s1".into() },
                Target::Todo {
                    id: Some("t_d".into()),
                    title: "Unlinked doing".into()
                }
            ]
        );
        assert_eq!(
            targets(&s.blockers),
            vec![
                it("feature/retry"),
                it("feature/retry"),
                Target::Inbox,
                Target::Session { id: "s1".into() },
                it("old/thing"),
            ]
        );
        assert_eq!(targets(&s.cleanup), vec![it("bugfix/flaky")]);
        assert_eq!(s.todo.len(), 1);
    }

    #[test]
    fn todo_holds_every_entry_but_markdown_stops_at_ten() {
        let w = Window::for_since(d("2026-11-24"), d("2026-11-25"));
        let todos: Vec<Todo> = (0..12)
            .map(|i| todo(&format!("t{i}"), TodoStatus::Todo, None))
            .collect();
        let input = SummaryInput {
            items: &[],
            todos: &todos,
            sessions: &[],
            events: &[],
            unresolved: &[],
        };
        let s = build(&input, &w);
        assert_eq!(s.todo.len(), 12);
        assert_eq!(s.todo[11].line, "t11");
    }

    #[test]
    fn empty_sections_omitted_and_more_counter() {
        let w = Window::for_since(d("2026-11-24"), d("2026-11-25"));
        let todos: Vec<Todo> = (0..12)
            .map(|i| todo(&format!("t{i}"), TodoStatus::Todo, None))
            .collect();
        let input = SummaryInput {
            items: &[],
            todos: &todos,
            sessions: &[],
            events: &[],
            unresolved: &[],
        };
        let md = to_markdown(&build(&input, &w));
        assert!(!md.contains("## Done"));
        assert!(md.contains("- +2 more"));
        assert!(!md.contains("- t10"));
    }

    #[test]
    fn unresolved_choices_and_auto_open() {
        let conn = crate::store::open_in_memory().unwrap();
        conn.execute(
            "INSERT INTO suggestions(rule, subject_key, payload, created_at) VALUES
             ('pr_declined', 'k1', '{\"kind\":\"keep_or_abandon\",\"message\":\"PR #31 declined: keep or abandon\"}', 1)",
            [],
        )
        .unwrap();
        let u = load_unresolved_choices(&conn).unwrap();
        assert_eq!(u[0].message, "PR #31 declined: keep or abandon");
        assert!(should_auto_open(&conn, d("2026-11-25"), &WD).unwrap());
        assert!(!should_auto_open(&conn, d("2026-11-25"), &WD).unwrap());
        assert!(!should_auto_open(&conn, d("2026-11-28"), &WD).unwrap()); // Saturday
    }

    #[test]
    fn export_overwrites_same_day() {
        let dir = tempfile::tempdir().unwrap();
        let p = write_export(dir.path(), d("2026-11-25"), "a").unwrap();
        write_export(dir.path(), d("2026-11-25"), "b").unwrap();
        assert_eq!(std::fs::read_to_string(p).unwrap(), "b");
    }
}
