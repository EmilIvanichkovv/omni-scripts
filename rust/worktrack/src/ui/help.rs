//! Help overlay (`?`, full key list of spec 9.6) and diagnostics overlay (`i`, spec 9.7).

use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::Frame;

use super::theme;
use super::widgets::{clean, fit, render_modal};
use crate::app::App;

/// `(scope, [(keys, action)])` — the full key list.
pub const KEYS: &[(&str, &[(&str, &str)])] = &[
    (
        "Global",
        &[
            ("1–5  Tab  Shift-Tab", "switch tab"),
            ("j k ↓ ↑", "move"),
            ("g G  Home End", "top / bottom (G groups in Todos)"),
            ("Ctrl-d Ctrl-u", "half page down / up"),
            ("J K", "scroll details"),
            ("/", "search (@repo: @state: in Work); esc clears"),
            ("f", "cycle filter (Summary: section)"),
            ("v", "cycle layout: auto / side / stacked / list"),
            (
                "Enter / Esc",
                "details full screen on narrow layouts / back",
            ),
            ("r / R", "local reload / full sync"),
            ("i", "diagnostics"),
            ("? / q", "help / quit"),
        ],
    ),
    (
        "Groups (repos, days, directories)",
        &[
            ("Enter / Space", "on a group header: fold / unfold it"),
            ("z", "fold / unfold the group of the selected row"),
            ("Z", "fold all groups (unfold all when all are folded)"),
            ("/", "searching shows every group open"),
        ],
    ),
    (
        "Work",
        &[
            ("o", "open PR in browser (Jira if no PR)"),
            ("y / Y", "copy branch name / worktree path"),
            ("t", "add todo linked to this item"),
            ("c", "copy resume command of latest session"),
            ("A / U", "mark abandoned (confirm) / unmark"),
            ("X", "remove worktree, cleanup items only (confirm)"),
        ],
    ),
    (
        "Todos",
        &[
            ("a / e / E", "add / edit title / edit notes in editor"),
            ("Space", "toggle done"),
            ("s / p", "cycle status / cycle priority"),
            ("L", "link to a work item"),
            ("h l  ← →", "previous / next status filter (f cycles)"),
            ("+ / -", "move up / down"),
            ("D / d", "drop / delete (confirm)"),
            ("G / O", "group by repo / open file in editor"),
        ],
    ),
    (
        "Sessions",
        &[
            ("c", "copy resume command"),
            ("s", "re-summarize"),
            ("Enter", "jump to linked work item"),
        ],
    ),
    (
        "Summary",
        &[
            ("h l  ← →  f", "previous / next section"),
            ("j k  g G", "move in the section"),
            (
                "Enter",
                "open the entry in its tab (list-only: details, Enter again)",
            ),
            ("o", "open the item's PR in browser (Jira if no PR)"),
            ("Space", "toggle done on a todo entry"),
            ("[ / ]", "shift window start by one workday"),
            ("y / w", "copy Markdown / write file"),
        ],
    ),
    (
        "Inbox",
        &[
            ("Enter / x", "accept / reject"),
            ("e", "edit text, then accept"),
            ("k / b", "keep / abandon (? suggestions)"),
            ("A / X", "accept all / reject all (confirm)"),
            ("↑ ↓", "move (k is keep here)"),
        ],
    ),
];

const LEGEND: &[(&str, &str)] = &[
    (
        "○ ↑ ◐ ✓ ✎ ✕ ✔ –",
        "local, pushed, PR open/approved/needs work/declined, merged, abandoned",
    ),
    ("* ↑N ↓N", "dirty, ahead, behind"),
    ("✓ci ✗ci …ci ☐N", "CI passed/failed/running, open PR tasks"),
    ("∅ z ⌫ ●", "no worktree, stale, cleanup, live session"),
];

/// The full key list for a modal `width` columns wide inside its borders. Narrow modals
/// put each action under its keys instead of in a column.
pub fn help_lines(width: usize) -> Vec<Line<'static>> {
    let narrow = width < 64;
    let row = |k: &str, what: String, key_style: Style, what_style: Style| -> Vec<Line<'static>> {
        if narrow {
            vec![
                Line::from(Span::styled(format!("  {k}"), key_style)),
                Line::from(Span::styled(format!("      {what}"), what_style)),
            ]
        } else {
            vec![Line::from(vec![
                Span::styled(format!("  {}", fit(k, 22)), key_style),
                Span::styled(what, what_style),
            ])]
        }
    };
    let mut lines = Vec::new();
    for (i, (scope, keys)) in KEYS.iter().enumerate() {
        if i > 0 {
            lines.push(Line::from(""));
        }
        lines.push(Line::from(Span::styled(
            scope.to_string(),
            theme::heading(),
        )));
        for (k, action) in keys.iter() {
            lines.extend(row(k, action.to_string(), theme::key(), Style::new()));
        }
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled("Legend", theme::heading())));
    for (k, what) in LEGEND {
        lines.extend(row(k, what.to_string(), theme::bold(), theme::muted()));
    }
    lines
}

pub fn draw_help(frame: &mut Frame, app: &App) {
    let scroll = render_modal(
        frame,
        "Keys",
        theme::ACCENT,
        76,
        help_lines(
            76.min(frame.area().width.saturating_sub(4))
                .saturating_sub(2) as usize,
        ),
        None,
        app.overlay_scroll,
    );
    app.feedback
        .overlay_max_scroll
        .set(scroll.total.saturating_sub(scroll.viewport));
}

pub fn diagnostics_lines(app: &App) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    if app.diagnostics.is_empty() {
        lines.push(Line::from(Span::styled(
            "no diagnostics reported yet",
            theme::muted(),
        )));
    }
    for d in &app.diagnostics {
        let d = clean(d);
        // `Section:` lines (ending in a colon) act as headings.
        if d.ends_with(':') && !d.starts_with(' ') {
            lines.push(Line::from(Span::styled(d, theme::heading())));
        } else {
            lines.push(Line::from(d));
        }
    }
    let skipped: Vec<_> = app
        .repos
        .iter()
        .filter_map(|r| r.skipped_reason.as_ref().map(|why| (r, why)))
        .collect();
    if !skipped.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled("Skipped repos", theme::heading())));
        for (r, why) in skipped {
            lines.push(Line::from(format!(
                "  {}  {}",
                clean(&super::work::display_path(app, &r.common_dir)),
                clean(why)
            )));
        }
    }
    if let Some(e) = &app.todo_error {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled("Todo file", theme::heading())));
        lines.push(Line::from(Span::styled(
            format!("  parse error: {}", clean(e)),
            Style::new().fg(theme::DANGER),
        )));
    }
    if !app.errors.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled("Recent errors", theme::heading())));
        for e in app.errors.iter().rev() {
            lines.push(Line::from(Span::styled(
                format!("  {}", clean(e)),
                Style::new().fg(theme::DANGER),
            )));
        }
    }
    lines
}

pub fn draw_diagnostics(frame: &mut Frame, app: &App) {
    let scroll = render_modal(
        frame,
        "Diagnostics",
        theme::ACCENT,
        96,
        diagnostics_lines(app),
        None,
        app.overlay_scroll,
    );
    app.feedback
        .overlay_max_scroll
        .set(scroll.total.saturating_sub(scroll.viewport));
}
