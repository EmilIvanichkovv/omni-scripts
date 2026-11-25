//! Todos view (spec 9.3): the todo list, optional grouping by repo, and the todo details.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::Frame;

use super::theme;
use super::widgets::{clean, field, fit, fmt_when, render_rows, section, short_link, BarItem};
use crate::app::{App, BarHit, TodoFilter, TodoRow};
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
                "no todos yet (a adds one, O opens the file)"
            } else {
                "nothing matches this filter (h/l switch filters)"
            },
        );
        return;
    }
    let width = rows_area.width as usize;
    let indent = if app.todo_grouped { 2 } else { 0 };
    let mut lines = Vec::new();
    let mut cursor_row = None;
    let mut todo_idx = 0;
    for row in app.todo_rows() {
        match row {
            TodoRow::Header(name) => lines.push(Line::from(Span::styled(
                format!(" ▾ {}", clean(&name)),
                theme::bold(),
            ))),
            TodoRow::Todo(i) => {
                if todo_idx == app.cursor[1] {
                    cursor_row = Some(lines.len());
                }
                todo_idx += 1;
                lines.push(todo_line(app, &app.todos[i], width, indent));
            }
        }
    }
    render_rows(
        frame,
        rows_area,
        area,
        lines,
        cursor_row,
        &app.feedback.list_offset[1],
    );
}

pub fn details(app: &App) -> Vec<Line<'static>> {
    let Some(t) = app.selected_todo() else {
        return vec![Line::from(Span::styled("no todo selected", theme::muted()))];
    };
    todo_details(app, t)
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
    lines.push(Line::from(""));
    lines.push(section("Notes"));
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
