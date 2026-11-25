//! Sessions view (spec 9.4): Claude Code sessions, newest first, under day headers, and
//! their details.

use chrono::{Datelike, NaiveDate, TimeZone};
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::Frame;

use super::theme;
use super::widgets::{
    clean, field, fit, fit_right, header_row, push_section, render_rows, section,
};
use crate::app::{App, Now};
use crate::lgbc::responsive::str_width;
use crate::sessions::{Session, SummaryStatus};

const TIME_W: usize = 5;
const TITLE_MIN: usize = 12;
const BRANCH_MIN: usize = 10;
const BRANCH_MAX: usize = 32;
const REPO_MAX: usize = 16;
const FAILED_SUFFIX: &str = " · summary failed";
const NO_BRANCH: &str = "(no branch)";

/// Column widths of the session rows, sized from the visible content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Columns {
    /// 0 when the repo column is dropped.
    repo: usize,
    branch: usize,
    /// 0 when the turns column is dropped.
    turns: usize,
    title: usize,
}

fn repo_short(app: &App, s: &Session) -> String {
    app.session_repo_name(s)
        .map(|r| r.rsplit_once('/').map_or(r.clone(), |(_, x)| x.to_string()))
        .map(|r| clean(&r))
        .unwrap_or_default()
}

fn branch_of(app: &App, s: &Session) -> String {
    app.session_branch(s)
        .map(|b| clean(&b))
        .unwrap_or_else(|| NO_BRANCH.to_string())
}

fn turns_text(s: &Session) -> String {
    format!("{} turns", s.user_turns)
}

/// ` HH:MM ● ` before the columns.
const FIXED: usize = 1 + TIME_W + 1 + 1 + 1;

fn columns(app: &App, sessions: &[&Session], width: usize) -> Columns {
    let max = |f: &dyn Fn(&Session) -> usize| sessions.iter().map(|s| f(s)).max().unwrap_or(0);
    let repo_w = max(&|s| str_width(&repo_short(app, s))).min(REPO_MAX);
    let branch_w = max(&|s| str_width(&branch_of(app, s))).clamp(BRANCH_MIN, BRANCH_MAX);
    let turns_w = max(&|s| str_width(&turns_text(s)));
    let need = |repo: usize, turns: usize| {
        FIXED
            + if repo > 0 { repo + 2 } else { 0 }
            + branch_w
            + 2
            + TITLE_MIN
            + if turns > 0 { turns + 1 } else { 0 }
    };
    let mut c = Columns {
        repo: repo_w,
        branch: branch_w,
        turns: turns_w,
        title: 0,
    };
    if need(c.repo, c.turns) > width {
        c.turns = 0;
    }
    if need(c.repo, c.turns) > width {
        c.repo = 0;
    }
    let used = |c: &Columns| {
        FIXED
            + if c.repo > 0 { c.repo + 2 } else { 0 }
            + c.branch
            + 2
            + if c.turns > 0 { c.turns + 1 } else { 0 }
    };
    if used(&c) + TITLE_MIN > width {
        c.branch = width
            .saturating_sub(used(&c) - c.branch + TITLE_MIN)
            .max(BRANCH_MIN.min(width / 3));
    }
    c.title = width.saturating_sub(used(&c));
    c
}

fn session_line(app: &App, s: &Session, cols: Columns) -> Line<'static> {
    let live = s.is_live(app.now_ts());
    let (mark, mark_style) = theme::session_mark(live, s.summary_status);
    let mut spans = vec![
        Span::raw(" "),
        Span::styled(
            fit(
                &clock(&app.now, s.last_activity_at.or(s.started_at)),
                TIME_W,
            ),
            theme::muted(),
        ),
        Span::raw(" "),
        Span::styled(mark, mark_style),
        Span::raw(" "),
    ];
    if cols.repo > 0 {
        let repo = repo_short(app, s);
        if repo.is_empty() {
            spans.push(Span::styled(fit("—", cols.repo), theme::muted()));
        } else {
            spans.push(Span::styled(fit(&repo, cols.repo), theme::accent()));
        }
        spans.push(Span::raw("  "));
    }
    let branch = branch_of(app, s);
    let branch_style = if branch == NO_BRANCH {
        theme::muted()
    } else {
        Style::new()
    };
    spans.push(Span::styled(fit(&branch, cols.branch), branch_style));
    spans.push(Span::raw("  "));
    let title = clean(&app.session_title(s));
    let failed = s.summary_status == SummaryStatus::Failed && !live;
    if failed && str_width(&title) + str_width(FAILED_SUFFIX) <= cols.title {
        let pad = cols.title - str_width(&title) - str_width(FAILED_SUFFIX);
        spans.push(Span::raw(title));
        spans.push(Span::styled(
            format!("{FAILED_SUFFIX}{}", " ".repeat(pad)),
            theme::muted(),
        ));
    } else {
        spans.push(Span::raw(fit(&title, cols.title)));
    }
    if cols.turns > 0 {
        spans.push(Span::raw(" "));
        spans.push(Span::styled(
            fit_right(&turns_text(s), cols.turns),
            theme::muted(),
        ));
    }
    Line::from(spans)
}

/// `HH:MM` of `t` in the app's time zone.
fn clock(now: &Now, t: Option<i64>) -> String {
    t.and_then(|t| now.offset().timestamp_opt(t, 0).single())
        .map_or_else(|| "—".to_string(), |dt| dt.format("%H:%M").to_string())
}

fn day_of(now: &Now, t: Option<i64>) -> Option<NaiveDate> {
    t.and_then(|t| now.offset().timestamp_opt(t, 0).single())
        .map(|dt| dt.date_naive())
}

/// `Today`, `Yesterday`, `Tue 24 Nov` (with the year when it is not this year).
fn day_label(now: &Now, day: Option<NaiveDate>) -> String {
    let Some(d) = day else {
        return "No date".to_string();
    };
    let today = now.date_naive();
    match (today - d).num_days() {
        0 => "Today".to_string(),
        1 => "Yesterday".to_string(),
        _ if d.year() == today.year() => d.format("%a %-d %b").to_string(),
        _ => d.format("%a %-d %b %Y").to_string(),
    }
}

fn session_day(app: &App, s: &Session) -> Option<NaiveDate> {
    day_of(&app.now, s.last_activity_at.or(s.started_at))
}

/// Rows of the list: day headers plus one row per visible session.
pub fn line_count(app: &App) -> usize {
    let visible = app.visible_sessions();
    let mut days: Vec<Option<NaiveDate>> = visible
        .iter()
        .map(|&i| session_day(app, &app.sessions[i]))
        .collect();
    days.dedup();
    visible.len() + days.len()
}

pub fn draw_list(frame: &mut Frame, app: &App, area: Rect) {
    let live = app
        .sessions
        .iter()
        .filter(|s| s.is_live(app.now_ts()))
        .count();
    let mut title = vec![Span::styled(" Sessions ", theme::accent())];
    if live > 0 {
        title.push(Span::styled(
            format!("· {live} live "),
            Style::new().fg(theme::LIVE),
        ));
    }
    let visible = app.visible_sessions();
    let rows_area = super::list_pane(
        frame,
        app,
        area,
        Line::from(title),
        format!("{} sessions", visible.len()),
        None,
    );
    if visible.is_empty() {
        let msg = if app.sessions.is_empty() {
            "No Claude Code sessions yet. Run `worktrack hook install --apply`, or start \
             `claude` in a tracked repo."
        } else {
            "No session matches the search. Esc clears it."
        };
        super::render_empty(frame, rows_area, msg);
        return;
    }
    let width = rows_area.width as usize;
    let sessions: Vec<&Session> = visible.iter().map(|&i| &app.sessions[i]).collect();
    // One column of air before the border.
    let cols = columns(app, &sessions, width.saturating_sub(1));
    let cursor = app.cursor[2].min(sessions.len() - 1);
    let mut lines = Vec::new();
    let mut cursor_row = None;
    let mut last_day = None;
    for (n, s) in sessions.iter().enumerate() {
        let day = session_day(app, s);
        if last_day != Some(day) {
            let count = sessions[n..]
                .iter()
                .take_while(|x| session_day(app, x) == day)
                .count();
            lines.push(header_row(
                &day_label(&app.now, day),
                &count.to_string(),
                width,
            ));
            last_day = Some(day);
        }
        if n == cursor {
            cursor_row = Some(lines.len());
        }
        lines.push(session_line(app, s, cols));
    }
    render_rows(
        frame,
        rows_area,
        area,
        lines,
        cursor_row,
        &app.feedback.list_offset[2],
    );
}

fn bullets(lines: &mut Vec<Line<'static>>, title: &str, items: &[String]) {
    if items.is_empty() {
        return;
    }
    push_section(lines, title);
    for i in items {
        lines.push(Line::from(format!("  • {}", clean(i))));
    }
}

pub fn details(app: &App) -> Vec<Line<'static>> {
    let Some(s) = app.selected_session() else {
        return vec![Line::from(Span::styled(
            "no session selected",
            theme::muted(),
        ))];
    };
    session_details(app, s)
}

/// `Tue 24 Nov 08:20 – 09:00` (no day for today, the end day when it differs).
fn when_range(app: &App, start: Option<i64>, end: Option<i64>) -> String {
    let start = start.or(end);
    let day = |t: Option<i64>| {
        let d = day_of(&app.now, t)?;
        (d != app.now.date_naive()).then(|| day_label(&app.now, Some(d)))
    };
    let mut out = String::new();
    if let Some(d) = day(start) {
        out.push_str(&d);
        out.push(' ');
    }
    out.push_str(&clock(&app.now, start));
    if end.is_some() && end != start {
        out.push_str(" – ");
        if day_of(&app.now, end) != day_of(&app.now, start) {
            if let Some(d) = day(end) {
                out.push_str(&d);
                out.push(' ');
            }
        }
        out.push_str(&clock(&app.now, end));
    }
    out
}

/// Details pane lines for one session (Sessions tab and Summary targets).
pub fn session_details(app: &App, s: &Session) -> Vec<Line<'static>> {
    let live = s.is_live(app.now_ts());
    let (mark, mark_style) = theme::session_mark(live, s.summary_status);
    let mut title = Vec::new();
    if mark.trim().is_empty() {
        title.push(Span::styled("◆ ", theme::muted()));
    } else {
        title.push(Span::styled(format!("{mark} "), mark_style));
    }
    title.push(Span::styled(clean(&app.session_title(s)), theme::bold()));
    let mut lines = vec![Line::from(title)];

    let mut when = vec![Span::raw(when_range(app, s.started_at, s.last_activity_at))];
    when.push(Span::styled(
        format!(" · {} turns", s.user_turns),
        theme::muted(),
    ));
    if live {
        when.push(Span::styled(" · live", Style::new().fg(theme::LIVE)));
    } else if s.ended_at.is_some() {
        when.push(Span::styled(" · ended", theme::muted()));
    }
    lines.push(field("When", when));
    if s.links.is_empty() {
        lines.push(field(
            "Branch",
            vec![Span::styled("(no branch) · unlinked", theme::muted())],
        ));
    }
    for l in &s.links {
        let repo = app
            .repo_name(&l.repo_id)
            .unwrap_or_else(|| l.repo_id.clone());
        lines.push(field(
            "Branch",
            vec![
                Span::raw(clean(&l.branch)),
                Span::styled(format!("  {}", clean(&repo)), theme::accent()),
            ],
        ));
    }
    if let Some(cwd) = &s.cwd {
        lines.push(field(
            "Cwd",
            vec![Span::raw(clean(
                &app.display_path(std::path::Path::new(cwd)),
            ))],
        ));
    }
    // Only the statuses that need attention get a row; the mark shows the rest.
    match s.summary_status {
        SummaryStatus::Queued => lines.push(field(
            "Summary",
            vec![Span::styled("queued", Style::new().fg(theme::WARN))],
        )),
        SummaryStatus::Failed => lines.push(field(
            "Summary",
            vec![
                Span::styled("failed", Style::new().fg(theme::DANGER)),
                Span::styled(" · s retries", theme::muted()),
            ],
        )),
        SummaryStatus::Done | SummaryStatus::None => {}
    }
    if let Some(sum) = &s.summary {
        if !sum.summary.trim().is_empty() {
            push_section(&mut lines, "Summary");
            lines.push(Line::from(format!("  {}", clean(&sum.summary))));
        }
        bullets(&mut lines, "Done", &sum.done);
        bullets(&mut lines, "Unfinished", &sum.unfinished);
    }
    let mut todos: Vec<String> = s
        .summary
        .as_ref()
        .map(|x| x.todos.clone())
        .unwrap_or_default();
    todos.extend(s.explicit_todos.iter().cloned());
    bullets(&mut lines, "Todos", &todos);
    if let Some(sum) = &s.summary {
        bullets(&mut lines, "Blockers", &sum.blockers);
    }
    if !s.files_touched.is_empty() {
        push_section(
            &mut lines,
            &format!("Files touched ({})", s.files_touched.len()),
        );
        for f in &s.files_touched {
            lines.push(Line::from(format!("  {}", clean(f))));
        }
    }
    lines.push(Line::from(""));
    let mut resume = section("Resume");
    resume
        .spans
        .push(Span::styled("  c copies", theme::muted()));
    lines.push(resume);
    match app.session_resume(s) {
        Some(cmd) => lines.push(Line::from(Span::styled(
            format!("  {}", clean(&cmd)),
            Style::new().fg(theme::WARN),
        ))),
        None => lines.push(Line::from(Span::styled(
            "  no worktree known for this session",
            theme::muted(),
        ))),
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn day_labels_read_like_a_timeline() {
        let now = crate::ui::sample::now();
        let d = |y, m, day| NaiveDate::from_ymd_opt(y, m, day);
        assert_eq!(day_label(&now, d(2026, 11, 25)), "Today");
        assert_eq!(day_label(&now, d(2026, 11, 24)), "Yesterday");
        assert_eq!(day_label(&now, d(2026, 11, 23)), "Mon 23 Nov");
        assert_eq!(day_label(&now, d(2025, 11, 23)), "Sun 23 Nov 2025");
        assert_eq!(day_label(&now, None), "No date");
    }

    #[test]
    fn columns_are_sized_from_the_content() {
        let app = crate::ui::sample::app();
        let sessions: Vec<&Session> = app.sessions.iter().collect();
        let longest = sessions
            .iter()
            .map(|s| str_width(&branch_of(&app, s)))
            .max()
            .unwrap();
        let wide = columns(&app, &sessions, 120);
        assert_eq!(wide.branch, longest.clamp(BRANCH_MIN, BRANCH_MAX));
        assert!(wide.repo > 0 && wide.turns > 0);
        let narrow = columns(&app, &sessions, 40);
        assert_eq!((narrow.repo, narrow.turns), (0, 0));
        assert!(narrow.title >= TITLE_MIN - 2, "{narrow:?}");
        for w in [20, 40, 58, 98, 200] {
            let c = columns(&app, &sessions, w);
            for s in &sessions {
                assert!(session_line(&app, s, c).width() <= w.max(FIXED + 2 + c.branch));
            }
        }
    }
}
