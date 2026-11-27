//! Todos view (spec 9.3): the todo list, optional grouping by repo, and the todo details.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::Frame;

use super::theme;
use super::widgets::{
    self, clean, field, fit, fmt_when, group_header, push_section, render_list, BarItem, Cut,
    ListBlock,
};
use crate::app::{App, BarHit, GroupHeader, ListRow, Tab, TodoFilter};
use crate::todos::{Todo, TodoStatus};

const MARK_W: usize = 3;

fn date_ts(app: &App, d: chrono::NaiveDate) -> Option<i64> {
    use chrono::TimeZone;
    app.now
        .offset()
        .from_local_datetime(&d.and_hms_opt(12, 0, 0)?)
        .single()
        .map(|t| t.timestamp())
}

/// A list row: the status box and the todo's name, nothing else. Priority, link, dates and
/// notes are in the details pane.
fn todo_line(t: &Todo, width: usize, indent: usize) -> Line<'static> {
    let fixed = 1 + indent + MARK_W + 1;
    let title_w = width.saturating_sub(fixed + 1);
    let title_style = match t.status {
        TodoStatus::Todo => Style::new(),
        other => theme::todo_status(other),
    };
    Line::from(vec![
        Span::raw(" ".repeat(1 + indent)),
        Span::styled(t.status.marker(), theme::todo_status(t.status)),
        Span::raw(" "),
        Span::styled(fit(&clean(&t.title), title_w), title_style),
    ])
}

pub fn draw_list(frame: &mut Frame, app: &App, area: Rect) {
    // The tab names the pane; the title only carries a todo file error. The grouping sits
    // on the right of the filter bar.
    let mut title = Vec::new();
    if app.todo_error.is_some() {
        title.push(Span::styled(
            " ⚠ parse error ",
            Style::new().fg(theme::DANGER).add_modifier(Modifier::BOLD),
        ));
    }
    let visible = app.visible_todos();
    let bar = TodoFilter::ALL
        .into_iter()
        .map(|f| BarItem {
            label: f.label(),
            short: f.short(),
            count: app.todo_filter_count(f),
            active: f == app.todo_filter,
            hit: BarHit::Todo(f),
        })
        .collect();
    let rows_area = super::list_pane(
        frame,
        app,
        area,
        Line::from(title),
        format!("{} todos", visible.len()),
        Some(super::PaneBar {
            items: bar,
            aside: vec![
                Span::styled("group: ", theme::muted()),
                Span::styled(
                    if app.todo_grouped { "repo" } else { "none" },
                    theme::soft(),
                ),
                Span::raw(" "),
            ],
        }),
    );
    if visible.is_empty() {
        super::render_empty(
            frame,
            rows_area,
            if app.todos.is_empty() {
                "No todos yet. Press a to add one, or O to open the todo file."
            } else if !app.search[1].trim().is_empty() {
                "No todo matches the search. Esc clears it."
            } else {
                "Nothing matches this filter. h/l switch filters."
            },
        );
        return;
    }
    let width = rows_area.width as usize;
    let indent = if app.todo_grouped { 2 } else { 0 };
    let rows = app.todo_rows();
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
                lines: vec![todo_line(&app.todos[*i], width, indent)],
            },
        })
        .collect();
    let cursor = (!rows.is_empty()).then(|| app.cursor[Tab::Todos.index()].min(rows.len() - 1));
    render_list(
        frame,
        rows_area,
        area,
        blocks,
        cursor,
        &app.feedback.list_offset[Tab::Todos.index()],
    );
}

pub fn details(app: &App) -> Vec<Line<'static>> {
    if let Some(ListRow::Group(g)) = app.selected_row(Tab::Todos) {
        return group_details(app, &g);
    }
    let Some(t) = app.selected_todo() else {
        return vec![Line::from(Span::styled("no todo selected", theme::muted()))];
    };
    todo_details(app, t)
}

/// Details of a repo group of todos: how many, their statuses, the latest activity, and the
/// work items they link to.
fn group_details(app: &App, g: &GroupHeader) -> Vec<Line<'static>> {
    let todos: Vec<&Todo> = g.members.iter().map(|&i| &app.todos[i]).collect();
    let mut fields = vec![field(
        "Todos",
        vec![Span::raw(format!(
            "{} shown{}",
            todos.len(),
            if g.collapsed { " · folded" } else { "" }
        ))],
    )];
    fields.extend(widgets::breakdown(
        "Status",
        [
            TodoStatus::Doing,
            TodoStatus::Todo,
            TodoStatus::Done,
            TodoStatus::Dropped,
        ]
        .into_iter()
        .map(|st| {
            let n = todos.iter().filter(|t| t.status == st).count();
            (
                vec![
                    Span::styled(st.marker(), theme::todo_status(st)),
                    Span::raw(format!(" {}", st.as_str())),
                ],
                n,
            )
        })
        .collect(),
    ));
    let latest = todos
        .iter()
        .filter_map(|t| match (t.done, t.created) {
            (Some(d), _) => Some((d, "done")),
            (None, Some(c)) => Some((c, "added")),
            (None, None) => None,
        })
        .max_by_key(|(d, _)| *d);
    if let Some((d, what)) = latest {
        fields.push(field(
            "Latest",
            vec![Span::raw(format!("{what} {}", d.format("%a %Y-%m-%d")))],
        ));
    }
    let mut links: Vec<String> = todos.iter().filter_map(|t| t.link.clone()).collect();
    links.sort();
    links.dedup();
    let tracked = links
        .iter()
        .filter(|l| app.item_for_link(l).is_some())
        .count();
    if !links.is_empty() {
        fields.push(field(
            "Links",
            vec![Span::raw(format!(
                "{} {} · {tracked} tracked in Work",
                links.len(),
                if links.len() == 1 {
                    "branch"
                } else {
                    "branches"
                }
            ))],
        ));
    }
    widgets::group_details(&clean(&g.label), g.collapsed, fields)
}

/// Details pane lines for one todo (Todos tab and Summary targets).
pub fn todo_details(app: &App, t: &Todo) -> Vec<Line<'static>> {
    // Title, then one line of status chips.
    let mut lines = vec![Line::from(Span::styled(
        clean(&t.title),
        theme::bold().patch(theme::todo_status(t.status)),
    ))];
    let sep = || Span::styled("  ·  ", theme::muted());
    let mut chips = vec![Span::styled(
        format!("{} {}", t.status.marker(), t.status.as_str()),
        theme::todo_status(t.status).add_modifier(Modifier::BOLD),
    )];
    if let Some(p) = t.priority {
        chips.push(sep());
        chips.push(Span::styled(
            format!("priority {}", p.as_char()),
            Style::new().fg(theme::WARN).add_modifier(Modifier::BOLD),
        ));
    }
    match (t.status, t.done, t.created) {
        (TodoStatus::Done | TodoStatus::Dropped, Some(d), _) => {
            chips.push(sep());
            let verb = if t.status == TodoStatus::Done {
                "done"
            } else {
                "dropped"
            };
            chips.push(Span::styled(
                format!("{verb} {}", fmt_when(&app.now, date_ts(app, d))),
                theme::muted(),
            ));
        }
        (_, _, Some(c)) => {
            chips.push(sep());
            chips.push(Span::styled(
                format!("added {}", fmt_when(&app.now, date_ts(app, c))),
                theme::muted(),
            ));
        }
        _ => {}
    }
    lines.push(Line::from(chips));

    // Work: the linked branch with its state, PR, CI and Jira key.
    push_section(&mut lines, "Work");
    let jira = t.jira.clone().or_else(|| {
        t.link
            .as_deref()
            .and_then(|l| app.jira_key(l))
            .or_else(|| app.jira_key(&t.title))
    });
    match t.link.as_deref() {
        None => lines.push(Line::from(Span::styled(
            "  not linked to a branch · L links one",
            theme::muted(),
        ))),
        Some(link) => match app.item_for_link(link) {
            Some(item) => {
                let st = theme::state(item.state);
                let mut row = vec![
                    Span::styled(format!("  {} ", item.state.icon()), st),
                    Span::styled(clean(&item.branch), theme::bold()),
                ];
                // "PR needs work" + PR 44 → "PR #44 needs work"; no PR → "pushed", "local only".
                let label = item.state.label();
                let status = match &item.pr {
                    Some(pr) => match label.strip_prefix("PR ") {
                        Some(rest) => format!("PR #{} {rest}", pr.id),
                        None => format!("PR #{} · {label}", pr.id),
                    },
                    None => label.to_string(),
                };
                row.push(Span::styled(format!("  {status}"), st));
                if let Some(ci) = item.flags.ci {
                    row.push(Span::styled(
                        format!(" · {}", theme::ci_label(ci)),
                        theme::ci(ci),
                    ));
                }
                lines.push(Line::from(row));
                let mut sub = vec![Span::styled(
                    format!("    {}", clean(&item.repo_name)),
                    theme::accent(),
                )];
                if let Some(path) = &item.local.worktree {
                    sub.push(Span::styled(
                        format!("  ·  {}", clean(&app.display_path(path))),
                        theme::muted(),
                    ));
                }
                lines.push(Line::from(sub));
            }
            None => {
                lines.push(Line::from(Span::raw(format!("  {}", clean(link)))));
                lines.push(Line::from(Span::styled(
                    "    not a tracked branch (gone, or in a repo worktrack does not scan)",
                    theme::muted(),
                )));
            }
        },
    }
    if let Some(j) = jira {
        lines.push(Line::from(vec![
            Span::styled("    Jira ", theme::muted()),
            Span::styled(clean(&j), theme::accent()),
            Span::styled(
                if app.jira_browse_url.is_empty() {
                    ""
                } else {
                    " ↗"
                },
                theme::accent(),
            ),
        ]));
    }

    // Session: where Enter goes.
    if let Some((s, kind)) = app.todo_session(t) {
        push_section(&mut lines, "Session");
        lines.push(Line::from(Span::raw(format!(
            "  {}",
            clean(&app.session_title(s))
        ))));
        let how = match kind {
            crate::app::TodoSessionKind::CreatedFrom => "this todo came from it",
            crate::app::TodoSessionKind::LatestOnBranch => "latest on the branch",
        };
        let when = fmt_when(&app.now, s.last_activity_at.or(s.started_at));
        lines.push(Line::from(vec![
            Span::styled(format!("  {how} · {when} · "), theme::muted()),
            Span::styled("enter", theme::key()),
            Span::styled(" opens it", theme::muted()),
        ]));
    }

    // Notes.
    push_section(&mut lines, "Notes");
    let notes = crate::sanitize::clean(&t.notes);
    if notes.trim().is_empty() {
        lines.push(Line::from(vec![
            Span::styled("  none · ", theme::muted()),
            Span::styled("E", theme::key()),
            Span::styled(" writes some", theme::muted()),
        ]));
    } else {
        for l in notes.lines() {
            lines.push(Line::from(format!("  {l}")));
        }
    }

    // Record: dates and where the todo came from, indented like the other sections.
    let field = |label: &str, value: Vec<Span<'static>>| {
        let mut l = field(label, value);
        l.spans.insert(0, Span::raw("  "));
        l
    };
    push_section(&mut lines, "Record");
    if let Some(d) = t.created {
        lines.push(field(
            "Added",
            vec![Span::raw(d.format("%a %d %b %Y").to_string())],
        ));
    }
    if let Some(d) = t.done {
        let label = if t.status == TodoStatus::Dropped {
            "Dropped"
        } else {
            "Done"
        };
        lines.push(field(
            label,
            vec![Span::raw(d.format("%a %d %b %Y").to_string())],
        ));
    }
    lines.push(field(
        "Source",
        vec![Span::raw(source_text(t.source.as_deref()))],
    ));
    if t.id.is_none() {
        lines.push(field(
            "Id",
            vec![Span::styled(
                "none yet (assigned on the next write)",
                theme::muted(),
            )],
        ));
    }
    if let Some(err) = &app.todo_error {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            format!("todo file parse error: {}", clean(err)),
            Style::new().fg(theme::DANGER),
        )));
    }
    lines
}

/// `manual` / `rule:<name>` / `session:<id>` in words.
fn source_text(source: Option<&str>) -> String {
    match source {
        None | Some("manual") => "added by hand".to_string(),
        Some(s) => match s.split_once(':') {
            Some(("rule", r)) => format!("rule {}", clean(&r.replace('_', " "))),
            Some(("session", id)) => {
                format!("session {}", clean(&id.chars().take(8).collect::<String>()))
            }
            _ => clean(s),
        },
    }
}
