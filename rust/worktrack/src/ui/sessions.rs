//! Sessions view (spec 9.4): Claude Code sessions, newest first, under day headers, and
//! their details.

use chrono::{NaiveDate, TimeZone};
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::Frame;

use super::theme;
use super::widgets::{
    self, clean, field, fit, fit_right, group_header, push_section, render_list, section, Cut,
    ListBlock,
};
use crate::app::{day_label, App, GroupHeader, ListRow, Now, Tab};
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

/// Rows of the list: day headers plus the sessions of open days.
pub fn line_count(app: &App) -> usize {
    app.session_rows().len()
}

pub fn draw_list(frame: &mut Frame, app: &App, area: Rect) {
    let live = app
        .sessions
        .iter()
        .filter(|s| s.is_live(app.now_ts()))
        .count();
    // The tab names the pane; the title only counts live sessions.
    let mut title = Vec::new();
    if live > 0 {
        title.push(Span::styled(
            format!(" ● {live} live "),
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
    // One column of air before the border; sized from every visible session so folding
    // a day does not shift the columns.
    let cols = columns(app, &sessions, width.saturating_sub(1));
    let rows = app.session_rows();
    let blocks = rows
        .iter()
        .map(|r| match r {
            ListRow::Group(g) => ListBlock {
                header: true,
                lines: vec![group_header(
                    &g.label,
                    g.count(),
                    g.collapsed,
                    width,
                    Cut::End,
                )],
            },
            ListRow::Item(i) => ListBlock {
                header: false,
                lines: vec![session_line(app, &app.sessions[*i], cols)],
            },
        })
        .collect();
    let cursor = Some(app.cursor[Tab::Sessions.index()].min(rows.len() - 1));
    render_list(
        frame,
        rows_area,
        area,
        blocks,
        cursor,
        &app.feedback.list_offset[Tab::Sessions.index()],
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

/// `start → … → working  REPO`: every branch the session was on, in order, with the working
/// (last) one in normal text and the earlier ones muted. Repos only when they differ.
fn branch_move(app: &App, s: &Session) -> Vec<Span<'static>> {
    let mut branches: Vec<String> = s.branches.clone();
    for l in &s.links {
        if !branches.contains(&l.branch) {
            branches.push(l.branch.clone());
        }
    }
    if branches.is_empty() {
        return vec![Span::styled("(no branch) · unlinked", theme::muted())];
    }
    let last = branches.len() - 1;
    let mut spans = Vec::new();
    for (i, b) in branches.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled(" → ", theme::muted()));
        }
        let style = if i == last {
            theme::bold()
        } else {
            theme::muted()
        };
        spans.push(Span::styled(clean(b), style));
    }
    let mut repos: Vec<String> = Vec::new();
    for l in &s.links {
        let r = app
            .repo_name(&l.repo_id)
            .unwrap_or_else(|| l.repo_id.clone());
        if !repos.contains(&r) {
            repos.push(r);
        }
    }
    if repos.is_empty() {
        spans.push(Span::styled("  · unlinked", theme::muted()));
    } else {
        spans.push(Span::styled(
            format!("  {}", clean(&repos.join(", "))),
            theme::accent(),
        ));
    }
    spans
}
pub fn details(app: &App) -> Vec<Line<'static>> {
    if let Some(ListRow::Group(g)) = app.selected_row(Tab::Sessions) {
        return group_details(app, &g);
    }
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

/// Details of a day: how many sessions (live ones), the time span, turns, summary
/// statuses, the repos worked in and the latest session.
fn group_details(app: &App, g: &GroupHeader) -> Vec<Line<'static>> {
    let sessions: Vec<&Session> = g.members.iter().map(|&i| &app.sessions[i]).collect();
    let now = app.now_ts();
    let live = sessions.iter().filter(|s| s.is_live(now)).count();
    let mut count = vec![Span::raw(match sessions.len() {
        1 => "1 session".to_string(),
        n => format!("{n} sessions"),
    })];
    if live > 0 {
        count.push(Span::styled(
            format!(" · {live} live"),
            Style::new().fg(theme::LIVE),
        ));
    }
    if g.collapsed {
        count.push(Span::styled(" · folded", theme::muted()));
    }
    let mut fields = vec![field("Sessions", count)];
    let start = sessions.iter().filter_map(|s| s.started_at).min();
    let end = sessions
        .iter()
        .filter_map(|s| s.last_activity_at.or(s.started_at))
        .max();
    if start.is_some() || end.is_some() {
        let turns: u64 = sessions.iter().map(|s| s.user_turns as u64).sum();
        fields.push(field(
            "When",
            vec![
                Span::raw(format!(
                    "{} – {}",
                    clock(&app.now, start),
                    clock(&app.now, end)
                )),
                Span::styled(format!(" · {turns} turns"), theme::muted()),
            ],
        ));
    }
    fields.extend(widgets::breakdown(
        "Summary",
        [
            (SummaryStatus::Done, "summarized"),
            (SummaryStatus::Queued, "queued"),
            (SummaryStatus::Failed, "failed"),
        ]
        .into_iter()
        .map(|(st, label)| {
            let n = sessions
                .iter()
                .filter(|s| s.summary_status == st && !s.is_live(now))
                .count();
            let (mark, style) = theme::session_mark(false, st);
            (
                vec![Span::styled(mark, style), Span::raw(format!(" {label}"))],
                n,
            )
        })
        .collect(),
    ));
    let mut repos: Vec<String> = Vec::new();
    for s in &sessions {
        let r = repo_short(app, s);
        if !r.is_empty() && !repos.contains(&r) {
            repos.push(r);
        }
    }
    if !repos.is_empty() {
        fields.push(field("Repos", vec![Span::raw(repos.join(" · "))]));
    }
    if let Some(latest) = sessions.first() {
        fields.push(field(
            "Latest",
            vec![
                Span::raw(clock(
                    &app.now,
                    latest.last_activity_at.or(latest.started_at),
                )),
                Span::styled(
                    format!(" · {}", clean(&app.session_title(latest))),
                    theme::muted(),
                ),
            ],
        ));
    }
    let title = match sessions.first().and_then(|s| app.session_day(s)) {
        Some(d) if matches!(g.label.as_str(), "Today" | "Yesterday") => {
            format!("{} · {}", g.label, d.format("%a %-d %b"))
        }
        _ => g.label.clone(),
    };
    widgets::group_details(&title, g.collapsed, fields)
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
    lines.push(field("Branch", branch_move(app, s)));
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
            "  no recorded directory for this session",
            theme::muted(),
        ))),
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sessions::SessionLink;
    use ratatui::style::Modifier;

    fn text(spans: &[Span]) -> String {
        spans.iter().map(|s| s.content.as_ref()).collect()
    }

    /// A session that started on `pre-release` and switched to a feature branch.
    fn moved_session(app: &mut App) -> Session {
        let mut s = app
            .sessions
            .iter()
            .find(|s| s.id == "s-live")
            .unwrap()
            .clone();
        let repo_id = s.links[0].repo_id.clone();
        s.branches = vec!["pre-release".into(), "feature/strategies-engine".into()];
        s.links = vec![
            SessionLink {
                repo_id: repo_id.clone(),
                branch: "pre-release".into(),
            },
            SessionLink {
                repo_id,
                branch: "feature/strategies-engine".into(),
            },
        ];
        s
    }

    #[test]
    fn list_shows_the_working_branch_and_details_the_move() {
        let mut app = crate::ui::sample::app();
        let s = moved_session(&mut app);
        assert_eq!(
            app.session_branch(&s).as_deref(),
            Some("feature/strategies-engine")
        );
        assert_eq!(
            crate::app::working_link(&s).unwrap().branch,
            "feature/strategies-engine"
        );
        let spans = branch_move(&app, &s);
        let line = text(&spans);
        assert!(
            line.starts_with("pre-release → feature/strategies-engine  "),
            "{line}"
        );
        // The working branch is bold, the earlier one muted.
        let working = spans
            .iter()
            .find(|sp| sp.content == "feature/strategies-engine")
            .unwrap();
        assert!(working.style.add_modifier.contains(Modifier::BOLD));
        let start = spans.iter().find(|sp| sp.content == "pre-release").unwrap();
        assert_eq!(start.style, theme::muted());
        // One Branch row in the details, not one per branch.
        let details = session_details(&app, &s);
        let rows = details
            .iter()
            .filter(|l| text(&l.spans).starts_with("Branch"))
            .count();
        assert_eq!(rows, 1);
    }

    #[test]
    fn single_branch_and_unlinked_sessions() {
        let mut app = crate::ui::sample::app();
        let mut s = moved_session(&mut app);
        s.branches = vec!["feature/strategies-engine".into()];
        s.links.remove(0);
        assert!(!text(&branch_move(&app, &s)).contains('→'));
        s.branches.clear();
        s.links.clear();
        assert_eq!(text(&branch_move(&app, &s)), "(no branch) · unlinked");
    }

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
