//! Summary view (spec 9.5): a section bar (Done, In progress, Todo, Blockers, Cleanup), the
//! entries of the current section grouped under their working directory (`App::summary_rows`)
//! as a selectable list, and the details of what the selected entry points at (a WorkItem,
//! todo, session or Inbox choice).

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::Frame;

use super::theme;
use super::widgets::{
    self, clean, cut_spans, field, fmt_when, group_header, render_list, BarItem, Cut, ListBlock,
};
use crate::app::{App, BarHit, GroupHeader, ListRow, SummarySection, Tab};
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

/// Child lines shown under an entry before `+N more`.
const CHILDREN_MAX: usize = 2;

/// Main text and status phrase of an entry shown under its directory header, so the repo
/// and path are not repeated: an item shows its branch and the phrase after ` — ` (or the
/// blocker text), a session its title, todos and the rest their own line.
fn entry_parts(e: &Entry) -> (Vec<Span<'static>>, Option<String>) {
    let line = clean(&e.line);
    let (head, tail) = match line.split_once(" — ") {
        Some((h, t)) => (h.to_string(), Some(t.to_string())),
        None => (line.clone(), None),
    };
    match &e.target {
        Target::Item { branch, .. } => {
            let branch = clean(branch);
            let phrase = tail.unwrap_or_else(|| {
                // `repo `branch`: stale` → `stale`; `PR #3: CI failed` stays.
                let tick = format!("`{branch}`: ");
                match line.split_once(&tick) {
                    Some((_, rest)) => rest.to_string(),
                    None => line.replace(&format!("`{branch}`"), &branch),
                }
            });
            (vec![Span::raw(branch)], Some(phrase))
        }
        Target::Session { .. } => match tail {
            Some(t) => (vec![Span::raw(t)], Some(head)),
            None => (inline(&line, Style::new()), None),
        },
        _ => (inline(&head, Style::new()), tail),
    }
}

/// List lines of one entry: ` icon main  phrase`, then at most [`CHILDREN_MAX`] dimmed
/// child lines and `+N more`.
fn entry_lines(app: &App, e: &Entry, width: usize) -> Vec<Line<'static>> {
    let (icon, icon_style) = target_icon(app, &e.target);
    let (main, phrase) = entry_parts(e);
    let children = shown_children(e);
    let mut spans = vec![
        Span::raw("   "),
        Span::styled(icon, icon_style),
        Span::raw(" "),
    ];
    spans.extend(main);
    if let Some(p) = phrase.filter(|p| !p.is_empty()) {
        spans.push(Span::styled(format!("  {p}"), theme::soft()));
    }
    let mut out = vec![Line::from(cut_spans(spans, width))];
    for c in children.iter().take(CHILDREN_MAX) {
        out.push(Line::from(cut_spans(
            vec![Span::styled(format!("       {}", clean(c)), theme::muted())],
            width,
        )));
    }
    if children.len() > CHILDREN_MAX {
        out.push(Line::from(cut_spans(
            vec![Span::styled(
                format!("       +{} more", children.len() - CHILDREN_MAX),
                theme::muted(),
            )],
            width,
        )));
    }
    out
}

/// Children worth a list row: not a `Session: …` line repeating the entry's own text.
fn shown_children(e: &Entry) -> Vec<&String> {
    let (main, _) = entry_parts(e);
    let main: String = main.iter().map(|s| s.content.as_ref()).collect();
    e.children
        .iter()
        .filter(|c| c.strip_prefix("Session: ") != Some(main.as_str()))
        .collect()
}

/// Rows of an entry in the list (main line, children, `+N more`).
fn entry_height(e: &Entry) -> usize {
    let n = shown_children(e).len();
    1 + n.min(CHILDREN_MAX) + usize::from(n > CHILDREN_MAX)
}

/// Rows of the list below the section bar.
pub fn line_count(app: &App) -> usize {
    let Some(sum) = &app.summary else {
        return 0;
    };
    let entries = app.summary_section.entries(sum);
    app.summary_rows()
        .iter()
        .map(|r| match r {
            ListRow::Group(_) => 1,
            ListRow::Item(i) => entries.get(*i).map_or(0, entry_height),
        })
        .sum()
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
    ]);
    let rows_area = super::list_pane(frame, app, area, title, String::new(), Some(bar(app)));
    if app.summary.is_none() {
        super::render_empty(frame, rows_area, "Building the summary…");
        return;
    }
    let Some(sum) = &app.summary else {
        return;
    };
    let entries = app.summary_section.entries(sum);
    if entries.is_empty() {
        let msg = format!(
            "Nothing in {} for this window. h/l switch sections, [ and ] move the window start.",
            app.summary_section.label()
        );
        super::render_empty(frame, rows_area, &msg);
        return;
    }
    let width = rows_area.width as usize;
    let rows = app.summary_rows();
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
                    Cut::Middle,
                )],
            },
            ListRow::Item(i) => ListBlock {
                header: false,
                lines: entry_lines(app, &entries[*i], width),
            },
        })
        .collect();
    let cursor = Some(app.cursor[Tab::Summary.index()].min(rows.len() - 1));
    render_list(
        frame,
        rows_area,
        area,
        blocks,
        cursor,
        &app.feedback.list_offset[Tab::Summary.index()],
    );
}

/// Details of a directory group: how many entries of the section, what kinds they are,
/// and the latest activity behind them.
fn group_details(app: &App, g: &GroupHeader) -> Vec<Line<'static>> {
    let entries = app.summary_section_entries();
    let members: Vec<&Entry> = g.members.iter().filter_map(|&i| entries.get(i)).collect();
    let mut fields = vec![field(
        "Entries",
        vec![Span::raw(format!(
            "{} in {}{}",
            members.len(),
            app.summary_section.label(),
            if g.collapsed { " · folded" } else { "" }
        ))],
    )];
    let kind = |e: &Entry| match e.target {
        Target::Item { .. } => 0,
        Target::Todo { .. } => 1,
        Target::Session { .. } => 2,
        Target::Inbox => 3,
        Target::None => 4,
    };
    fields.extend(widgets::breakdown(
        "Kinds",
        [
            ("work item", "work items"),
            ("todo", "todos"),
            ("session", "sessions"),
            ("inbox choice", "inbox choices"),
            ("other", "other"),
        ]
        .into_iter()
        .enumerate()
        .map(|(k, (one, many))| {
            let n = members.iter().filter(|e| kind(e) == k).count();
            (vec![Span::raw(if n == 1 { one } else { many })], n)
        })
        .collect(),
    ));
    let latest = members
        .iter()
        .filter_map(|e| app.summary_activity(&e.target))
        .max();
    if latest.is_some() {
        fields.push(field("Latest", vec![Span::raw(fmt_when(&app.now, latest))]));
    }
    widgets::group_details(&clean(&g.label), g.collapsed, fields)
}

fn gone(what: &str) -> Vec<Line<'static>> {
    vec![Line::from(Span::styled(
        format!("this {what} is no longer present"),
        theme::muted(),
    ))]
}

/// Details of the selected entry's target, and the pane title.
pub fn details(app: &App, width: u16) -> (Vec<Line<'static>>, &'static str) {
    if let Some(ListRow::Group(g)) = app.selected_row(Tab::Summary) {
        return (group_details(app, &g), " Directory ");
    }
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
    fn entry_lines_show_main_phrase_and_capped_children_sanitized() {
        let app = crate::ui::sample::app();
        let e = Entry {
            line: "**API/x** `b\u{1b}[31m` — PR #1 open\u{1b}]0;t\u{7}".into(),
            children: vec![
                "Session: evil\u{1b}[2J".into(),
                "Session: two".into(),
                "Doing: three".into(),
                "Doing: four".into(),
            ],
            target: Target::None,
        };
        let lines = entry_lines(&app, &e, 80);
        assert_eq!(text(&lines[0]), "   • API/x b  PR #1 open");
        assert_eq!(text(&lines[1]), "       Session: evil");
        assert_eq!(text(&lines[2]), "       Session: two");
        assert_eq!(text(&lines[3]), "       +2 more");
        assert_eq!(lines.len(), entry_height(&e));
        let narrow = entry_lines(&app, &e, 8);
        assert!(narrow.iter().all(|l| l.width() <= 8));
    }

    #[test]
    fn item_entries_show_the_branch_not_the_repo() {
        let app = crate::ui::sample::app();
        let item = &app.work_items[0];
        let e = Entry {
            line: format!(
                "**{}** `{}` — PR #44 needs work",
                item.repo_name, item.branch
            ),
            children: Vec::new(),
            target: Target::Item {
                repo_id: item.repo_id.clone(),
                branch: item.branch.clone(),
            },
        };
        let first = text(&entry_lines(&app, &e, 120)[0]);
        assert!(
            first.contains(&format!("{}  PR #44 needs work", item.branch)),
            "{first}"
        );
        assert!(!first.contains(&item.repo_name), "{first}");
        let stale = Entry {
            line: format!("{} `{}`: stale", item.repo_name, item.branch),
            ..e
        };
        let first = text(&entry_lines(&app, &stale, 120)[0]);
        assert!(
            first.ends_with(&format!("{}  stale", item.branch)),
            "{first}"
        );
    }
}
