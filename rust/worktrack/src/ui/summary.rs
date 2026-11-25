//! Summary view (spec 9.5): a section bar (Done, In progress, Todo, Blockers, Cleanup), the
//! entries of the current section as a selectable list, and the details of what the selected
//! entry points at (a WorkItem, todo, session or Inbox choice).

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::Frame;

use super::theme;
use super::widgets::{clean, cut_spans, render_rows, BarItem};
use crate::app::{App, BarHit, SummarySection, Tab};
use crate::summary::{Entry, Target};

/// Style inline `**bold**` and `` `code` `` in one Markdown line.
fn inline(text: &str, base: Style) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut buf = String::new();
    let mut bold = false;
    let mut code = false;
    let mut chars = text.chars().peekable();
    let style = |bold: bool, code: bool| {
        let mut s = base;
        if bold {
            s = s.add_modifier(Modifier::BOLD);
        }
        if code {
            s = s.fg(theme::WARN);
        }
        s
    };
    while let Some(c) = chars.next() {
        if c == '`' {
            spans.push(Span::styled(std::mem::take(&mut buf), style(bold, code)));
            code = !code;
        } else if c == '*' && !code && chars.peek() == Some(&'*') {
            chars.next();
            spans.push(Span::styled(std::mem::take(&mut buf), style(bold, code)));
            bold = !bold;
        } else {
            buf.push(c);
        }
    }
    spans.push(Span::styled(buf, style(bold, code)));
    spans.retain(|s| !s.content.is_empty());
    spans
}

/// Icon of an entry: the WorkItem state, the todo status, a session mark, `?` for an Inbox
/// choice.
fn target_icon(app: &App, target: &Target) -> (String, Style) {
    match target {
        Target::Item { repo_id, branch } => match app.work_item(repo_id, branch) {
            Some(item) => (item.state.icon().into(), theme::state(item.state)),
            None => ("·".into(), theme::muted()),
        },
        Target::Todo { id, title } => match app.find_todo(id.as_deref(), title) {
            Some(t) => (t.status.marker().into(), theme::todo_status(t.status)),
            None => ("[?]".into(), theme::muted()),
        },
        Target::Session { id } => match app.find_session(id) {
            Some(s) if s.is_live(app.now_ts()) => ("●".into(), Style::new().fg(theme::LIVE)),
            Some(_) => ("◆".into(), theme::accent()),
            None => ("◆".into(), theme::muted()),
        },
        Target::Inbox => (
            "?".into(),
            Style::new().fg(theme::WARN).add_modifier(Modifier::BOLD),
        ),
        Target::None => ("•".into(), theme::muted()),
    }
}

/// List lines of one entry: `▶ icon head`, then the part after ` — ` and the children
/// indented under it.
fn entry_lines(app: &App, e: &Entry, width: usize, selected: bool) -> Vec<Line<'static>> {
    let line = clean(&e.line);
    let (head, tail) = match line.split_once(" — ") {
        Some((h, t)) => (h.to_string(), Some(t.to_string())),
        None => (line.clone(), None),
    };
    let (icon, icon_style) = target_icon(app, &e.target);
    let mut spans = vec![
        Span::styled(if selected { "▶ " } else { "  " }, theme::accent()),
        Span::styled(icon, icon_style),
        Span::raw(" "),
    ];
    spans.extend(inline(&head, Style::new()));
    let mut out = vec![Line::from(cut_spans(spans, width))];
    if let Some(tail) = tail {
        out.push(Line::from(cut_spans(
            vec![Span::raw("    "), Span::raw(tail)],
            width,
        )));
    }
    for c in &e.children {
        out.push(Line::from(cut_spans(
            vec![Span::styled(format!("    {}", clean(c)), theme::muted())],
            width,
        )));
    }
    out
}

fn bar(app: &App) -> Vec<BarItem> {
    SummarySection::ALL
        .into_iter()
        .map(|sec| BarItem {
            label: sec.label(),
            short: sec.short(),
            count: app.summary_section_count(sec),
            active: sec == app.summary_section,
            hit: BarHit::Summary(sec),
        })
        .collect()
}

pub fn draw_list(frame: &mut Frame, app: &App, area: Rect) {
    let day = |d: chrono::NaiveDate| d.format("%a %-d %b").to_string();
    let today = app
        .summary
        .as_ref()
        .and_then(|s| s.today)
        .unwrap_or_else(|| app.today());
    let since = app.summary_since.map(day).unwrap_or_else(|| "—".into());
    let title = Line::from(vec![
        Span::styled(" Summary ", theme::accent()),
        Span::styled(format!("· {} · since {since} ", day(today)), theme::bold()),
        Span::styled(" [ ] shift ", theme::muted()),
    ]);
    let rows_area = super::list_pane(frame, app, area, title, String::new(), Some(bar(app)));
    if app.summary.is_none() {
        super::render_empty(frame, rows_area, "building the summary…");
        return;
    }
    let entries = app.summary_entries();
    if entries.is_empty() {
        let msg = format!(
            "nothing in {} for this window (h/l switch sections, [ ] shifts the window)",
            app.summary_section.label()
        );
        super::render_empty(frame, rows_area, &msg);
        return;
    }
    let width = rows_area.width as usize;
    let cursor = app.cursor[Tab::Summary.index()].min(entries.len() - 1);
    let mut lines = Vec::new();
    let mut cursor_row = None;
    for (i, e) in entries.iter().enumerate() {
        if i == cursor {
            cursor_row = Some(lines.len());
        }
        lines.extend(entry_lines(app, e, width, i == cursor));
    }
    render_rows(
        frame,
        rows_area,
        area,
        lines,
        cursor_row,
        &app.feedback.list_offset[Tab::Summary.index()],
    );
}

fn gone(what: &str) -> Vec<Line<'static>> {
    vec![Line::from(Span::styled(
        format!("this {what} is no longer present"),
        theme::muted(),
    ))]
}

/// Details of the selected entry's target, and the pane title.
pub fn details(app: &App, width: u16) -> (Vec<Line<'static>>, &'static str) {
    let Some(e) = app.selected_summary_entry() else {
        return (
            vec![Line::from(Span::styled("nothing selected", theme::muted()))],
            " Details ",
        );
    };
    match &e.target {
        Target::Item { repo_id, branch } => (
            match app.work_item(repo_id, branch) {
                Some(item) => super::work::item_details(app, item, width),
                None => gone("work item"),
            },
            " Details ",
        ),
        Target::Todo { id, title } => (
            match app.find_todo(id.as_deref(), title) {
                Some(t) => super::todos::todo_details(app, t),
                None => gone("todo"),
            },
            " Todo ",
        ),
        Target::Session { id } => (
            match app.find_session(id) {
                Some(s) => super::sessions::session_details(app, s),
                None => gone("session"),
            },
            " Session ",
        ),
        Target::Inbox => (
            match app.suggestion_for_entry(e) {
                Some(s) => super::inbox::suggestion_details(app, s),
                None => vec![
                    Line::from(Span::styled(clean(&e.line), theme::bold())),
                    Line::from(Span::styled(
                        "an unresolved keep-or-abandon choice; enter opens the Inbox",
                        theme::muted(),
                    )),
                ],
            },
            " Suggestion ",
        ),
        Target::None => {
            let mut lines = vec![Line::from(inline(&clean(&e.line), theme::bold()))];
            lines.extend(
                e.children
                    .iter()
                    .map(|c| Line::from(format!("  {}", clean(c)))),
            );
            (lines, " Details ")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(line: &Line) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn inline_styles_bold_and_code() {
        let spans = inline("**API/x** `b` ok", Style::new());
        let line = Line::from(spans);
        assert_eq!(text(&line), "API/x b ok");
        assert!(line.spans[0].style.add_modifier.contains(Modifier::BOLD));
        assert_eq!(line.spans[2].style.fg, Some(theme::WARN));
    }

    #[test]
    fn entry_lines_split_head_tail_and_children_and_are_sanitized() {
        let app = crate::ui::sample::app();
        let e = Entry {
            line: "**API/x** `b\u{1b}[31m` — PR #1 open\u{1b}]0;t\u{7}".into(),
            children: vec!["Session: evil\u{1b}[2J".into()],
            target: Target::None,
        };
        let lines = entry_lines(&app, &e, 80, true);
        assert_eq!(text(&lines[0]), "▶ • API/x b");
        assert_eq!(text(&lines[1]), "    PR #1 open");
        assert_eq!(text(&lines[2]), "    Session: evil");
        let narrow = entry_lines(&app, &e, 8, false);
        assert!(narrow.iter().all(|l| l.width() <= 8));
    }
}
