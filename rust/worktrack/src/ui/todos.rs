//! Todos view (spec 9.3): the todo list, optional grouping by repo, and the todo details.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::Frame;

use super::theme;
use super::widgets::{
    self, clean, field, fit, fmt_when, group_header, push_section, render_list, short_link,
    BarItem, Cut, ListBlock,
};
use crate::app::{App, BarHit, GroupHeader, ListRow, Tab, TodoFilter};
use crate::todos::{Todo, TodoStatus};

const MARK_W: usize = 3;
const PRIO_W: usize = 1;
const DATE_W: usize = 5;
const TITLE_MIN: usize = 16;

fn date_ts(app: &App, d: chrono::NaiveDate) -> Option<i64> {
    use chrono::TimeZone;
    app.now
        .offset()
        .from_local_datetime(&d.and_hms_opt(12, 0, 0)?)
        .single()
        .map(|t| t.timestamp())
}

fn todo_line(app: &App, t: &Todo, width: usize, indent: usize) -> Line<'static> {
    // " [~] A  title  link  date"
    let fixed = 1 + indent + MARK_W + 1 + PRIO_W + 2;
    let date_w = DATE_W + 1;
    let rest = width.saturating_sub(fixed + date_w);
    let link_w = if rest >= TITLE_MIN + 12 {
        (rest - TITLE_MIN).min(36).min(rest / 2)
    } else {
        0
    };
    let title_w = rest.saturating_sub(if link_w > 0 { link_w + 1 } else { 0 });
    let mut title_style = theme::todo_status(t.status);
    if t.status == TodoStatus::Todo {
        title_style = Style::new();
    }
    let mut spans = vec![
        Span::raw(" ".repeat(1 + indent)),
        Span::styled(t.status.marker(), theme::todo_status(t.status)),
        Span::raw(" "),
        Span::styled(
            t.priority
                .map_or(" ".to_string(), |p| p.as_char().to_string()),
            Style::new().fg(theme::WARN).add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(fit(&clean(&t.title), title_w), title_style),
    ];
    if link_w > 0 {
        spans.push(Span::raw(" "));
        spans.push(Span::styled(
            fit(
                &clean(&t.link.as_deref().map(short_link).unwrap_or_default()),
                link_w,
            ),
            theme::accent(),
        ));
    }
    let date = match (t.status, t.done) {
        (TodoStatus::Done | TodoStatus::Dropped, Some(d)) => fmt_when(&app.now, date_ts(app, d)),
        _ => String::new(),
    };
    spans.push(Span::raw(" "));
    spans.push(Span::styled(fit(&date, DATE_W), theme::muted()));
    Line::from(spans)
}

pub fn draw_list(frame: &mut Frame, app: &App, area: Rect) {
    let mut title = vec![
        Span::styled(" Todos ", theme::accent()),
        Span::styled(" Group: ", theme::muted()),
        Span::styled(
            if app.todo_grouped { "repo" } else { "none" },
            theme::accent(),
        ),
        Span::raw(" "),
    ];
    if app.todo_error.is_some() {
        title.push(Span::styled(
            "⚠ parse error ",
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
        Some(bar),
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
                lines: vec![todo_line(app, &app.todos[*i], width, indent)],
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
    let mut lines = vec![Line::from(Span::styled(
        clean(&t.title),
        theme::bold().patch(theme::todo_status(t.status)),
    ))];
    let mut status = vec![Span::styled(
        format!("{} {}", t.status.marker(), t.status.as_str()),
        theme::todo_status(t.status),
    )];
    if let Some(p) = t.priority {
        status.push(Span::raw(format!(" · priority {}", p.as_char())));
    }
    lines.push(field("Status", status));
    match &t.link {
        Some(link) => {
            let mut v = vec![Span::raw(clean(link))];
            match app.item_for_link(link) {
                Some(item) => v.push(Span::styled(
                    format!("  {} {}", item.state.icon(), item.state.label()),
                    theme::state(item.state),
                )),
                None => v.push(Span::styled("  (not tracked)", theme::muted())),
            }
            lines.push(field("Link", v));
        }
        None => lines.push(field(
            "Link",
            vec![Span::styled("none (L links)", theme::muted())],
        )),
    }
    let jira = t.jira.clone().or_else(|| {
        t.link
            .as_deref()
            .and_then(|l| app.jira_key(l))
            .or_else(|| app.jira_key(&t.title))
    });
    if let Some(j) = jira {
        lines.push(field(
            "Jira",
            vec![Span::styled(clean(&j), theme::accent())],
        ));
    }
    if let Some(s) = &t.source {
        lines.push(field("Source", vec![Span::raw(clean(s))]));
    }
    if let Some(d) = t.created {
        lines.push(field(
            "Created",
            vec![Span::raw(d.format("%a %Y-%m-%d").to_string())],
        ));
    }
    if let Some(d) = t.done {
        lines.push(field(
            "Done",
            vec![Span::raw(d.format("%a %Y-%m-%d").to_string())],
        ));
    }
    if t.id.is_none() {
        lines.push(field(
            "Id",
            vec![Span::styled(
                "none yet (assigned on the next write)",
                theme::muted(),
            )],
        ));
    }
    push_section(&mut lines, "Notes");
    let notes = crate::sanitize::clean(&t.notes);
    if notes.trim().is_empty() {
        lines.push(Line::from(Span::styled("  none (E edits)", theme::muted())));
    } else {
        for l in notes.lines() {
            lines.push(Line::from(format!("  {l}")));
        }
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
