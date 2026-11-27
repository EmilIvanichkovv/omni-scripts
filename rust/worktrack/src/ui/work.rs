//! Work view (spec 9.2): WorkItems grouped by repo, and the WorkItem details pane.

use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::Frame;

use super::theme;
use super::widgets::{
    self, clean, field, field_cont, fit, fit_right, fmt_age, fmt_when, group_header, push_section,
    render_list, Cut, ListBlock, LABEL_W,
};
use crate::app::{App, GroupHeader, ListRow, Tab};
use crate::lgbc::responsive::{str_width, truncate_middle};
use crate::model::{ReviewerStatus, WorkItem, WorkState};

const INDENT: usize = 3;
const ICON: usize = 2;
const BRANCH_MIN: usize = 14;
/// `#12345`; `—` without a PR.
const PR_W: usize = 6;
const CI_W: usize = 3;
/// Widest the flags column gets; it shrinks to the widest flags on screen.
const FLAGS_MAX: usize = 11;
const AGE_W: usize = 4;
const TS_W: usize = 7;
/// The branch is kept at least this wide (or its full width) before CI goes.
const BRANCH_BEFORE_CI: usize = 24;
/// ... and before the flags go.
const BRANCH_BEFORE_FLAGS: usize = 32;

/// What the visible rows need, so empty or narrow columns take no space.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ColumnNeeds {
    /// Widest branch name on screen.
    pub branch: usize,
    /// Widest open-tasks cell (`☐2` = 2); 0 when no row has tasks.
    pub tasks: usize,
    /// Widest flags cell; 0 when no row has flags.
    pub flags: usize,
}

impl ColumnNeeds {
    pub fn of<'a>(items: impl IntoIterator<Item = &'a WorkItem>) -> Self {
        let mut n = ColumnNeeds::default();
        for it in items {
            n.branch = n.branch.max(str_width(&clean(&it.branch)));
            n.tasks = n.tasks.max(tasks_text(it).chars().count());
            let flags: usize = flag_spans(it).iter().map(|s| s.width()).sum();
            n.flags = n.flags.max(flags.min(FLAGS_MAX));
        }
        n
    }
}

/// Which optional columns fit (spec 9.2) and how wide the branch and the sized columns are.
/// The branch gets priority: T/S, then flags, then CI are dropped before a branch on screen
/// is cut below its full width (or below `BRANCH_BEFORE_FLAGS` / `BRANCH_BEFORE_CI`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkColumns {
    pub ci: bool,
    pub flags: bool,
    pub ts: bool,
    pub branch: usize,
    pub tasks_w: usize,
    pub flags_w: usize,
}

pub fn columns(width: usize, needs: ColumnNeeds) -> WorkColumns {
    let tasks_w = needs.tasks;
    let flags_w = needs.flags;
    let opt = |w: usize| if w == 0 { 0 } else { 1 + w };
    let base = INDENT + ICON + (1 + PR_W) + opt(tasks_w) + (1 + AGE_W);
    let mut c = WorkColumns {
        ci: true,
        flags: flags_w > 0,
        ts: true,
        branch: 0,
        tasks_w,
        flags_w,
    };
    let need = |c: &WorkColumns| {
        base + if c.ci { 1 + CI_W } else { 0 }
            + if c.flags { opt(c.flags_w) } else { 0 }
            + if c.ts { 1 + TS_W } else { 0 }
    };
    let room = |c: &WorkColumns| width.saturating_sub(need(c));
    let wanted = needs.branch.max(BRANCH_MIN);
    if room(&c) < wanted {
        c.ts = false;
    }
    if c.flags && room(&c) < wanted.min(BRANCH_BEFORE_FLAGS) {
        c.flags = false;
    }
    if room(&c) < wanted.min(BRANCH_BEFORE_CI) {
        c.ci = false;
    }
    c.branch = room(&c).max(BRANCH_MIN);
    c
}

/// A branch name cut to `width` columns, keeping what identifies it: first the type prefix
/// shrinks (`feature/` → `f/`), then the middle goes, keeping everything up to the ticket key
/// (`jira.key_pattern`) and as much of the end as fits.
pub fn fit_branch(app: &App, branch: &str, width: usize) -> String {
    let b = clean(branch);
    if str_width(&b) <= width {
        return b;
    }
    let short = match b.split_once('/') {
        Some((kind, rest)) if !kind.is_empty() && !rest.is_empty() && !kind.contains(' ') => {
            format!("{}/{rest}", kind.chars().next().unwrap_or('?'))
        }
        _ => b.clone(),
    };
    if str_width(&short) <= width {
        return short;
    }
    if let Some(key) = app.jira_key(&short) {
        if let Some(pos) = short.find(&key) {
            let head: String = short[..pos + key.len()].to_string();
            let head_w = str_width(&head);
            // Head, the ellipsis, and at least 6 columns of the ending.
            if head_w + 1 + 6 <= width {
                let tail_budget = width - head_w - 1;
                let rest = &short[pos + key.len()..];
                let mut tail: Vec<char> = Vec::new();
                let mut w = 0;
                for ch in rest.chars().rev() {
                    let cw = str_width(&ch.to_string());
                    if w + cw > tail_budget {
                        break;
                    }
                    w += cw;
                    tail.push(ch);
                }
                let tail: String = tail.into_iter().rev().collect();
                return format!("{head}…{tail}");
            }
        }
    }
    truncate_middle(&short, width)
}

fn tasks_text(item: &WorkItem) -> String {
    if item.flags.open_tasks > 0 {
        format!("☐{}", item.flags.open_tasks)
    } else {
        String::new()
    }
}

/// Flag icons of the list (spec 9.8) as styled spans.
fn flag_spans(item: &WorkItem) -> Vec<Span<'static>> {
    let f = &item.flags;
    let mut out: Vec<Span<'static>> = Vec::new();
    let mut push = |s: String, style: Style| {
        if !out.is_empty() {
            out.push(Span::raw(" "));
        }
        out.push(Span::styled(s, style));
    };
    if f.dirty || item.local.dirty {
        push("*".into(), Style::new().fg(theme::WARN));
    }
    if f.ahead > 0 {
        push(format!("↑{}", f.ahead), Style::new());
    }
    if f.behind > 0 {
        push(format!("↓{}", f.behind), Style::new());
    }
    if f.no_worktree {
        push("∅".into(), theme::muted());
    }
    if f.stale {
        push("z".into(), theme::muted());
    }
    if f.cleanup {
        push("⌫".into(), Style::new().fg(theme::WARN));
    }
    if f.session_live {
        push("●".into(), Style::new().fg(theme::LIVE));
    }
    out
}

fn fit_spans(spans: Vec<Span<'static>>, width: usize) -> Vec<Span<'static>> {
    let mut used = 0;
    let mut out = Vec::new();
    for s in spans {
        let w = s.width();
        if used + w > width {
            break;
        }
        used += w;
        out.push(s);
    }
    out.push(Span::raw(" ".repeat(width - used)));
    out
}

fn item_line(app: &App, item: &WorkItem, cols: WorkColumns) -> Line<'static> {
    let st = theme::state(item.state);
    let mut spans = vec![
        Span::raw(" ".repeat(INDENT)),
        Span::styled(format!("{} ", item.state.icon()), st),
        Span::styled(
            fit(&fit_branch(app, &item.branch, cols.branch), cols.branch),
            Style::new(),
        ),
        Span::raw(" "),
    ];
    let pr = item
        .pr
        .as_ref()
        .map_or("—".to_string(), |p| format!("#{}", p.id));
    spans.push(Span::styled(fit(&pr, PR_W), st));
    if cols.ci {
        spans.push(Span::raw(" "));
        spans.push(match item.flags.ci {
            Some(c) => Span::styled(theme::ci_icon(c), theme::ci(c)),
            None => Span::raw(" ".repeat(CI_W)),
        });
    }
    if cols.tasks_w > 0 {
        spans.push(Span::raw(" "));
        spans.push(Span::styled(
            fit(&tasks_text(item), cols.tasks_w),
            Style::new().fg(theme::WARN),
        ));
    }
    if cols.flags && cols.flags_w > 0 {
        spans.push(Span::raw(" "));
        spans.extend(fit_spans(flag_spans(item), cols.flags_w));
    }
    spans.push(Span::raw(" "));
    spans.push(Span::styled(
        fit_right(&fmt_age(app.now_ts(), item.last_activity), AGE_W),
        theme::muted(),
    ));
    if cols.ts {
        spans.push(Span::raw(" "));
        spans.push(Span::styled(
            fit(
                &format!("T{} S{}", item.linked_todos, item.session_count),
                TS_W,
            ),
            theme::muted(),
        ));
    }
    Line::from(spans)
}

pub fn draw_list(frame: &mut Frame, app: &App, area: Rect) {
    let title = Line::from(vec![
        Span::styled(" Filter: ", theme::muted()),
        Span::styled(format!("{} ▾", app.work_filter.label()), theme::accent()),
        Span::styled("  Sort: activity ", theme::muted()),
    ]);
    let count = app.work_item_count();
    let rows_area = super::list_pane(frame, app, area, title, format!("{count} items"), None);
    let rows = app.work_rows();
    if rows.is_empty() {
        let msg = if app.work_items.is_empty() {
            "No work items yet. Press r to reload, or i to see which repos were scanned."
        } else if !app.search[0].trim().is_empty() {
            "Nothing matches the search. Esc clears it."
        } else {
            "Nothing matches this filter. Press f for the next filter."
        };
        super::render_empty(frame, rows_area, msg);
        return;
    }
    let width = rows_area.width as usize;
    // One column of air before the border.
    let needs = ColumnNeeds::of(rows.iter().filter_map(|r| match r {
        ListRow::Item(i) => app.work_items.get(*i),
        ListRow::Group(_) => None,
    }));
    let cols = columns(width.saturating_sub(1), needs);
    let blocks = rows
        .iter()
        .map(|r| match r {
            ListRow::Group(g) => ListBlock {
                header: true,
                lines: vec![group_header(
                    &clean(&g.label),
                    g.count(),
                    g.collapsed,
                    width,
                    Cut::End,
                )],
            },
            ListRow::Item(i) => ListBlock {
                header: false,
                lines: vec![item_line(app, &app.work_items[*i], cols)],
            },
        })
        .collect();
    let cursor = Some(app.cursor[Tab::Work.index()].min(rows.len() - 1));
    render_list(
        frame,
        rows_area,
        area,
        blocks,
        cursor,
        &app.feedback.list_offset[Tab::Work.index()],
    );
}

fn reviewer_mark(s: ReviewerStatus) -> (&'static str, &'static str, Style) {
    match s {
        ReviewerStatus::Approved => ("✓", "approved", Style::new().fg(theme::OK)),
        ReviewerStatus::NeedsWork => ("✗", "needs work", Style::new().fg(theme::DANGER)),
        ReviewerStatus::Unapproved => ("○", "unapproved", theme::muted()),
    }
}

/// Details pane lines for the selected row. `width` is the inner width (for the Jira key
/// aligned right on the first line).
pub fn details(app: &App, width: u16) -> Vec<Line<'static>> {
    let Some(row) = app.selected_work_row() else {
        return vec![Line::from(Span::styled("nothing selected", theme::muted()))];
    };
    let item = match row {
        ListRow::Group(g) => return group_details(app, &g),
        ListRow::Item(i) => &app.work_items[i],
    };
    item_details(app, item, width)
}

/// Details pane lines for one WorkItem (Work tab and Summary targets).
pub fn item_details(app: &App, item: &WorkItem, width: u16) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let branch = clean(&item.branch);
    // Title row: branch, Jira key (↗ when it links somewhere) on the right.
    let jira = app.jira_key(&item.branch).map(|k| {
        let arrow = if app.jira_url(&k).is_some() {
            " ↗"
        } else {
            ""
        };
        format!("{}{arrow}", clean(&k))
    });
    let mut title = vec![Span::styled(branch.clone(), theme::bold())];
    if let Some(j) = jira {
        let pad = (width as usize).saturating_sub(str_width(&branch) + str_width(&j) + 1);
        title.push(Span::raw(" ".repeat(pad.max(2))));
        title.push(Span::styled(j, theme::accent()));
    }
    lines.push(Line::from(title));

    let mut state = vec![Span::styled(
        format!("{} {}", item.state.icon(), item.state.label()),
        theme::state(item.state),
    )];
    if let Some(t) = item.state_since {
        state.push(Span::styled(
            format!(" (since {})", fmt_when(&app.now, Some(t))),
            theme::muted(),
        ));
    }
    if item.abandoned_manual {
        state.push(Span::styled(" · marked by you (U unmarks)", theme::muted()));
    }
    lines.push(field("State", state));
    lines.push(field("Repo", vec![Span::raw(clean(&item.repo_name))]));
    let path = match &item.local.worktree {
        Some(p) => Span::raw(truncate_middle(
            &clean(&display_path(app, p)),
            (width as usize).saturating_sub(LABEL_W + 1).max(10),
        )),
        None => Span::styled("no worktree", theme::muted()),
    };
    lines.push(field("Path", vec![path]));

    let f = &item.flags;
    let mut flags: Vec<String> = Vec::new();
    if f.dirty || item.local.dirty {
        flags.push("dirty".into());
    }
    if f.ahead > 0 {
        flags.push(format!("ahead {}", f.ahead));
    }
    if f.behind > 0 {
        flags.push(format!("behind {}", f.behind));
    }
    if f.behind_trunk > 0 {
        flags.push(format!("behind trunk {}", f.behind_trunk));
    }
    if let Some(c) = f.ci {
        flags.push(theme::ci_label(c).into());
    }
    if f.open_tasks > 0 {
        flags.push(format!("{} open tasks", f.open_tasks));
    }
    if f.no_worktree {
        flags.push("no worktree".into());
    }
    if f.stale {
        flags.push("stale".into());
    }
    if f.cleanup {
        flags.push("cleanup (X removes)".into());
    }
    if f.session_live {
        flags.push("session live".into());
    }
    if !flags.is_empty() {
        lines.push(field("Flags", vec![Span::raw(flags.join(" · "))]));
    }

    match &item.pr {
        Some(pr) => {
            lines.push(field(
                &format!("PR #{}", pr.id),
                vec![Span::raw(clean(&pr.title))],
            ));
            let mut rev: Vec<Span<'static>> = Vec::new();
            for (i, r) in pr.reviewers.iter().enumerate() {
                if i > 0 {
                    rev.push(Span::styled(" · ", theme::muted()));
                }
                let (icon, label, style) = reviewer_mark(r.status);
                rev.push(Span::raw(format!("{} ", clean(&r.name))));
                rev.push(Span::styled(format!("{icon} {label}"), style));
            }
            if pr.reviewers.is_empty() {
                rev.push(Span::styled("no reviewers", theme::muted()));
            }
            lines.push(field_cont(rev));
            if pr.comment_count > 0 {
                lines.push(field_cont(vec![Span::styled(
                    format!("{} comments", pr.comment_count),
                    theme::muted(),
                )]));
            }
        }
        None => lines.push(field("PR", vec![Span::styled("none", theme::muted())])),
    }
    lines.push(field(
        "Commits",
        vec![Span::raw(format!(
            "{} ahead of trunk{}",
            item.local.ahead_of_trunk,
            item.local
                .last_commit_at
                .map(|t| format!(" · last {}", fmt_age(app.now_ts(), Some(t))))
                .unwrap_or_default()
        ))],
    ));

    let todos = app.item_todos(item);
    push_section(&mut lines, &format!("Todos ({})", todos.len()));
    if todos.is_empty() {
        lines.push(Line::from(Span::styled(
            "  none (t adds one)",
            theme::muted(),
        )));
    }
    for t in todos {
        let prio = t
            .priority
            .map(|p| format!("({}) ", p.as_char()))
            .unwrap_or_default();
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(t.status.marker(), theme::todo_status(t.status)),
            Span::raw(format!(" {prio}{}", clean(&t.title))),
        ]));
    }
    let sessions = app.item_sessions(item);
    push_section(&mut lines, &format!("Sessions ({})", sessions.len()));
    if sessions.is_empty() {
        lines.push(Line::from(Span::styled("  none", theme::muted())));
    }
    for s in sessions {
        let text = s
            .summary
            .as_ref()
            .map(|x| x.summary.clone())
            .filter(|x| !x.trim().is_empty())
            .unwrap_or_else(|| app.session_title(s));
        // `  13:59  text`: the time acts as a label, so wrapped text hangs under the text.
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(
                fit(
                    &fmt_when(&app.now, s.last_activity_at.or(s.started_at)),
                    LABEL_W - 2,
                ),
                theme::muted(),
            ),
            Span::raw(clean(&text)),
        ]));
    }
    lines
}

/// Details of a repo group: how many items, their states, the latest activity, and the
/// repo's path, trunk and remote.
fn group_details(app: &App, g: &GroupHeader) -> Vec<Line<'static>> {
    let items: Vec<&WorkItem> = g.members.iter().map(|&i| &app.work_items[i]).collect();
    let mut fields = vec![field(
        "Items",
        vec![Span::raw(format!(
            "{} shown{}",
            items.len(),
            if g.collapsed { " · folded" } else { "" }
        ))],
    )];
    fields.extend(widgets::breakdown(
        "States",
        WorkState::ALL
            .into_iter()
            .map(|st| {
                let n = items.iter().filter(|w| w.state == st).count();
                (
                    vec![Span::styled(
                        format!("{} {}", st.icon(), st.label()),
                        theme::state(st),
                    )],
                    n,
                )
            })
            .collect(),
    ));
    if let Some(latest) = items.iter().max_by_key(|w| w.last_activity) {
        if latest.last_activity.is_some() {
            fields.push(field(
                "Latest",
                vec![
                    Span::raw(fmt_when(&app.now, latest.last_activity)),
                    Span::styled(format!(" · {}", clean(&latest.branch)), theme::muted()),
                ],
            ));
        }
    }
    if let Some(repo) = app.repos.iter().find(|r| r.id == g.key) {
        fields.push(field(
            "Path",
            vec![Span::raw(clean(&display_path(
                app,
                &repo.git_dir_for_commands,
            )))],
        ));
        if let Some(t) = &repo.trunk {
            fields.push(field("Trunk", vec![Span::raw(clean(t))]));
        }
        if let Some(bb) = &repo.bb {
            fields.push(field("Remote", vec![Span::raw(clean(&bb.base_url))]));
        }
    }
    widgets::group_details(&clean(&g.label), g.collapsed, fields)
}

/// Path with the home directory shown as `~`.
pub fn display_path(app: &App, p: &std::path::Path) -> String {
    app.display_path(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn columns_drop_right_to_left_only_when_the_branch_needs_the_room() {
        let needs = ColumnNeeds {
            branch: 48,
            tasks: 2,
            flags: 7,
        };
        let wide = columns(160, needs);
        assert!(wide.ci && wide.flags && wide.ts);
        assert!(wide.branch >= 48);
        // T/S goes first so the full branch fits.
        let c = columns(80, needs);
        assert!(c.ci && c.flags && !c.ts, "{c:?}");
        assert!(c.branch >= 48, "{c:?}");
        // Then flags, before the branch drops below 32.
        let c = columns(60, needs);
        assert!(c.ci && !c.flags && !c.ts, "{c:?}");
        let c = columns(36, needs);
        assert!(!c.ci && !c.flags && !c.ts, "{c:?}");
        assert!(c.branch >= BRANCH_MIN);
        // Short branches keep every column on a narrow pane.
        let short = ColumnNeeds {
            branch: 12,
            ..needs
        };
        let c = columns(70, short);
        assert!(c.ci && c.flags && c.ts, "{c:?}");
        // Empty columns take no space.
        let none = ColumnNeeds {
            branch: 20,
            tasks: 0,
            flags: 0,
        };
        let c = columns(80, none);
        assert_eq!((c.tasks_w, c.flags), (0, false));
    }

    #[test]
    fn long_branches_keep_their_ticket_and_ending() {
        let app = crate::ui::sample::app();
        let b = "feature/PROJ-123456-upload-retry-with-backoff-ok";
        assert_eq!(fit_branch(&app, b, 60), b);
        assert_eq!(
            fit_branch(&app, b, 42),
            "f/PROJ-123456-upload-retry-with-backoff-ok"
        );
        let cut = fit_branch(&app, b, 30);
        assert_eq!(str_width(&cut), 30);
        assert!(cut.starts_with("f/PROJ-123456…"), "{cut}");
        assert!(cut.ends_with("with-backoff-ok"), "{cut}");
        // Without a ticket key: plain middle cut.
        let cut = fit_branch(&app, "chore/bump-all-the-dependencies-everywhere", 20);
        assert_eq!(str_width(&cut), 20);
        assert!(cut.contains('…'));
    }
}
