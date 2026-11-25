//! Work view (spec 9.2): WorkItems grouped by repo, and the WorkItem details pane.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::Frame;

use super::theme;
use super::widgets::{clean, field, fit, fit_right, fmt_age, fmt_when, render_rows, section};
use crate::app::{App, WorkRow};
use crate::lgbc::responsive::{str_width, truncate_middle};
use crate::model::{ReviewerStatus, WorkItem};

const INDENT: usize = 3;
const ICON: usize = 2;
const BRANCH_MIN: usize = 14;
const PR_W: usize = 7;
const CI_W: usize = 3;
const TASKS_W: usize = 3;
const FLAGS_W: usize = 11;
const AGE_W: usize = 4;
const TS_W: usize = 7;

/// Which optional columns fit; dropped right to left: S/T, then flags, then CI (spec 9.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkColumns {
    pub ci: bool,
    pub flags: bool,
    pub ts: bool,
    pub branch: usize,
}

pub fn columns(width: usize) -> WorkColumns {
    let base = INDENT + ICON + BRANCH_MIN + (1 + PR_W) + (1 + TASKS_W) + (1 + AGE_W);
    let mut c = WorkColumns {
        ci: true,
        flags: true,
        ts: true,
        branch: 0,
    };
    let need = |c: &WorkColumns| {
        base + if c.ci { 1 + CI_W } else { 0 }
            + if c.flags { 1 + FLAGS_W } else { 0 }
            + if c.ts { 1 + TS_W } else { 0 }
    };
    if need(&c) > width {
        c.ts = false;
    }
    if need(&c) > width {
        c.flags = false;
    }
    if need(&c) > width {
        c.ci = false;
    }
    c.branch = BRANCH_MIN + width.saturating_sub(need(&c));
    c
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
        Span::styled(fit(&clean(&item.branch), cols.branch), Style::new()),
        Span::raw(" "),
    ];
    let pr = item
        .pr
        .as_ref()
        .map_or("—".to_string(), |p| format!("PR #{}", p.id));
    spans.push(Span::styled(fit(&pr, PR_W), st));
    if cols.ci {
        spans.push(Span::raw(" "));
        spans.push(match item.flags.ci {
            Some(c) => Span::styled(theme::ci_icon(c), theme::ci(c)),
            None => Span::raw(" ".repeat(CI_W)),
        });
    }
    spans.push(Span::raw(" "));
    let tasks = if item.flags.open_tasks > 0 {
        format!("☐{}", item.flags.open_tasks)
    } else {
        String::new()
    };
    spans.push(Span::styled(
        fit(&tasks, TASKS_W),
        Style::new().fg(theme::WARN),
    ));
    if cols.flags {
        spans.push(Span::raw(" "));
        spans.extend(fit_spans(flag_spans(item), FLAGS_W));
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

fn group_line(name: &str, count: usize, collapsed: bool, width: usize) -> Line<'static> {
    let left = format!(" {} {}", if collapsed { "▸" } else { "▾" }, clean(name));
    let right = format!("{count} ");
    let pad = width.saturating_sub(str_width(&left) + str_width(&right));
    Line::from(vec![
        Span::styled(left, theme::bold()),
        Span::raw(" ".repeat(pad)),
        Span::styled(right, theme::muted()),
    ])
}

pub fn draw_list(frame: &mut Frame, app: &App, area: Rect) {
    let title = Line::from(vec![
        Span::styled(" Filter: ", theme::muted()),
        Span::styled(format!("{} ▾", app.work_filter.label()), theme::accent()),
        Span::styled("  Sort: activity ", theme::muted()),
    ]);
    let count = app.work_item_count();
    let rows_area = super::list_pane(frame, app, area, title, format!("{count} items"));
    let rows = app.work_rows();
    if rows.is_empty() {
        let msg = if app.work_items.is_empty() {
            "no work items yet (r reloads, i shows diagnostics)"
        } else {
            "nothing matches this filter (f cycles filters)"
        };
        super::render_empty(frame, rows_area, msg);
        return;
    }
    let width = rows_area.width as usize;
    let cols = columns(width);
    let lines: Vec<Line<'static>> = rows
        .iter()
        .map(|r| match r {
            WorkRow::Group {
                name,
                count,
                collapsed,
                ..
            } => group_line(name, *count, *collapsed, width),
            WorkRow::Item(i) => item_line(app, &app.work_items[*i], cols),
        })
        .collect();
    let cursor = Some(app.cursor[0].min(lines.len() - 1));
    render_rows(
        frame,
        rows_area,
        area,
        lines,
        cursor,
        &app.feedback.list_offset[0],
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
        WorkRow::Group {
            repo_id,
            name,
            count,
            collapsed,
        } => return group_details(app, &repo_id, &name, count, collapsed),
        WorkRow::Item(i) => &app.work_items[i],
    };
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
            (width as usize).saturating_sub(8).max(10),
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
            lines.push(Line::from(vec![
                Span::styled(fit(&format!("PR #{}", pr.id), 8), theme::muted()),
                Span::raw(clean(&pr.title)),
            ]));
            let mut rev: Vec<Span<'static>> = vec![Span::raw(" ".repeat(8))];
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
            lines.push(Line::from(rev));
            if pr.comment_count > 0 {
                lines.push(Line::from(Span::styled(
                    format!("        {} comments", pr.comment_count),
                    theme::muted(),
                )));
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
    lines.push(Line::from(""));
    lines.push(section(&format!("Todos ({})", todos.len())));
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
    lines.push(Line::from(""));
    lines.push(section(&format!("Sessions ({})", sessions.len())));
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
        lines.push(Line::from(vec![
            Span::styled(
                format!(
                    "  {} ",
                    fit(&fmt_when(&app.now, s.last_activity_at.or(s.started_at)), 5)
                ),
                theme::muted(),
            ),
            Span::raw(clean(&text)),
        ]));
    }
    lines
}

fn group_details(
    app: &App,
    repo_id: &str,
    name: &str,
    count: usize,
    collapsed: bool,
) -> Vec<Line<'static>> {
    let mut lines = vec![Line::from(Span::styled(clean(name), theme::bold()))];
    lines.push(field("Items", vec![Span::raw(format!("{count} shown"))]));
    if let Some(repo) = app.repos.iter().find(|r| r.id == repo_id) {
        lines.push(field(
            "Path",
            vec![Span::raw(clean(&display_path(
                app,
                &repo.git_dir_for_commands,
            )))],
        ));
        if let Some(t) = &repo.trunk {
            lines.push(field("Trunk", vec![Span::raw(clean(t))]));
        }
        if let Some(bb) = &repo.bb {
            lines.push(field("Remote", vec![Span::raw(clean(&bb.base_url))]));
        }
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        if collapsed {
            "enter expands this group"
        } else {
            "enter collapses this group"
        },
        Style::new().add_modifier(Modifier::ITALIC).fg(theme::MUTED),
    )));
    lines
}

/// Path with the home directory shown as `~`.
pub fn display_path(app: &App, p: &std::path::Path) -> String {
    if let Some(home) = &app.home {
        if let Ok(rest) = p.strip_prefix(home) {
            return if rest.as_os_str().is_empty() {
                "~".into()
            } else {
                format!("~/{}", rest.display())
            };
        }
    }
    p.display().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn columns_drop_right_to_left() {
        let wide = columns(120);
        assert!(wide.ci && wide.flags && wide.ts);
        let c = columns(58);
        assert!(c.ci && c.flags && !c.ts);
        let c = columns(48);
        assert!(c.ci && !c.flags && !c.ts);
        let c = columns(38);
        assert!(!c.ci && !c.flags && !c.ts);
        assert!(c.branch >= BRANCH_MIN);
    }
}
