//! Sessions view (spec 9.4): Claude Code sessions, newest first, and their details.

use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::Frame;

use super::theme;
use super::widgets::{clean, field, fit, fit_right, fmt_when, fmt_when_long, render_rows, section};
use crate::app::App;
use crate::sessions::{Session, SummaryStatus};

const WHEN_W: usize = 5;
const REPO_W: usize = 12;
const TITLE_MIN: usize = 12;
const TURNS_W: usize = 8;
const TAIL_W: usize = 14;

fn session_line(app: &App, s: &Session, width: usize) -> Line<'static> {
    let live = s.is_live(app.now_ts());
    let (mark, mark_style) = theme::session_mark(live, s.summary_status);
    let tail = if live {
        "live"
    } else if s.summary_status == SummaryStatus::Failed {
        "summary failed"
    } else {
        ""
    };
    // when, mark, [repo], branch, title, [turns], [tail]
    let fixed = 1 + WHEN_W + 1 + 1 + 1;
    let mut show_repo = true;
    let mut show_turns = true;
    let mut show_tail = true;
    let need = |repo: bool, turns: bool, tail: bool| {
        fixed
            + if repo { REPO_W + 1 } else { 0 }
            + 14
            + 1
            + TITLE_MIN
            + if turns { TURNS_W + 1 } else { 0 }
            + if tail { TAIL_W + 1 } else { 0 }
    };
    if need(show_repo, show_turns, show_tail) > width {
        show_tail = false;
    }
    if need(show_repo, show_turns, show_tail) > width {
        show_turns = false;
    }
    if need(show_repo, show_turns, show_tail) > width {
        show_repo = false;
    }
    let rest = width.saturating_sub(need(show_repo, show_turns, show_tail) - 14 - TITLE_MIN);
    let branch_w = (rest * 2 / 5).clamp(14, 28);
    let title_w = rest.saturating_sub(branch_w + 1);

    let repo = app
        .session_repo_name(s)
        .map(|r| r.rsplit_once('/').map_or(r.clone(), |(_, x)| x.to_string()))
        .unwrap_or_else(|| "—".into());
    let branch = app
        .session_branch(s)
        .unwrap_or_else(|| "(no branch)".to_string());

    let mut spans = vec![
        Span::raw(" "),
        Span::styled(
            fit(
                &fmt_when(&app.now, s.last_activity_at.or(s.started_at)),
                WHEN_W,
            ),
            theme::muted(),
        ),
        Span::raw(" "),
        Span::styled(mark, mark_style),
        Span::raw(" "),
    ];
    if show_repo {
        spans.push(Span::styled(fit(&clean(&repo), REPO_W), theme::accent()));
        spans.push(Span::raw(" "));
    }
    spans.push(Span::raw(fit(&clean(&branch), branch_w)));
    spans.push(Span::raw(" "));
    spans.push(Span::raw(fit(&clean(&app.session_title(s)), title_w)));
    if show_turns {
        spans.push(Span::raw(" "));
        spans.push(Span::styled(
            fit_right(&format!("{} turns", s.user_turns), TURNS_W),
            theme::muted(),
        ));
    }
    if show_tail {
        spans.push(Span::raw(" "));
        let style = if live {
            Style::new().fg(theme::LIVE)
        } else {
            Style::new().fg(theme::DANGER)
        };
        spans.push(Span::styled(fit(tail, TAIL_W), style));
    }
    Line::from(spans)
}

pub fn draw_list(frame: &mut Frame, app: &App, area: Rect) {
    let live = app
        .sessions
        .iter()
        .filter(|s| s.is_live(app.now_ts()))
        .count();
    let title = Line::from(vec![
        Span::styled(" Sessions ", theme::accent()),
        Span::styled(
            if live > 0 {
                format!("· {live} live ")
            } else {
                String::new()
            },
            Style::new().fg(theme::LIVE),
        ),
    ]);
    let visible = app.visible_sessions();
    let rows_area = super::list_pane(
        frame,
        app,
        area,
        title,
        format!("{} sessions", visible.len()),
        None,
    );
    if visible.is_empty() {
        super::render_empty(
            frame,
            rows_area,
            "no sessions yet (install the hook: worktrack hook install)",
        );
        return;
    }
    let width = rows_area.width as usize;
    let lines = visible
        .iter()
        .map(|&i| session_line(app, &app.sessions[i], width))
        .collect::<Vec<_>>();
    let cursor = Some(app.cursor[2].min(lines.len() - 1));
    render_rows(
        frame,
        rows_area,
        area,
        lines,
        cursor,
        &app.feedback.list_offset[2],
    );
}

fn bullets(lines: &mut Vec<Line<'static>>, title: &str, items: &[String]) {
    if items.is_empty() {
        return;
    }
    lines.push(Line::from(""));
    lines.push(section(title));
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

/// Details pane lines for one session (Sessions tab and Summary targets).
pub fn session_details(app: &App, s: &Session) -> Vec<Line<'static>> {
    let live = s.is_live(app.now_ts());
    let mut lines = vec![Line::from(Span::styled(
        clean(&app.session_title(s)),
        theme::bold(),
    ))];
    let mut when = format!(
        "{} – {}",
        fmt_when_long(&app.now, s.started_at),
        fmt_when(&app.now, s.last_activity_at)
    );
    when.push_str(&format!(" · {} turns", s.user_turns));
    let mut when_spans = vec![Span::raw(when)];
    if live {
        when_spans.push(Span::styled(" · ● live", Style::new().fg(theme::LIVE)));
    } else if s.ended_at.is_some() {
        when_spans.push(Span::styled(" · ended", theme::muted()));
    }
    lines.push(field("When", when_spans));
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
                Span::styled(clean(&repo), theme::accent()),
                Span::raw(format!("  {}", clean(&l.branch))),
            ],
        ));
    }
    if let Some(cwd) = &s.cwd {
        lines.push(field(
            "Cwd",
            vec![Span::raw(clean(&super::work::display_path(
                app,
                std::path::Path::new(cwd),
            )))],
        ));
    }
    let status = match s.summary_status {
        SummaryStatus::None => "not summarized (below turn threshold)",
        SummaryStatus::Queued => "summary queued",
        SummaryStatus::Done => "summarized",
        SummaryStatus::Failed => "summary failed (s retries)",
    };
    let status_style = match s.summary_status {
        SummaryStatus::Failed => Style::new().fg(theme::DANGER),
        _ => theme::muted(),
    };
    lines.push(field("Summary", vec![Span::styled(status, status_style)]));
    if let Some(sum) = &s.summary {
        if !sum.summary.trim().is_empty() {
            lines.push(Line::from(""));
            lines.push(Line::from(clean(&sum.summary)));
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
        lines.push(Line::from(""));
        lines.push(section(&format!(
            "Files touched ({})",
            s.files_touched.len()
        )));
        for f in &s.files_touched {
            lines.push(Line::from(format!("  {}", clean(f))));
        }
    }
    lines.push(Line::from(""));
    lines.push(section("Resume (c copies)"));
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
