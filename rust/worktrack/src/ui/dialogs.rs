//! Confirmation dialog and the todo link picker.

use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::Frame;

use super::theme;
use super::widgets::{clean, fit, key_hint, render_modal};
use crate::app::{App, Confirm, LinkPicker, PickerEntry};

fn hint(groups: Vec<Vec<Span<'static>>>) -> Line<'static> {
    let mut spans = Vec::new();
    for (i, g) in groups.into_iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled(" · ", theme::muted()));
        }
        spans.extend(g);
    }
    Line::from(spans)
}

/// Lines of a confirmation dialog.
pub fn confirm_lines(c: &Confirm) -> (&'static str, Vec<Line<'static>>) {
    match c {
        Confirm::Abandon {
            branch, repo_name, ..
        } => (
            "Abandon branch",
            vec![
                Line::from(vec![
                    Span::raw("Mark "),
                    Span::styled(clean(branch), theme::bold()),
                    Span::raw(format!(" in {} as abandoned?", clean(repo_name))),
                ]),
                Line::from(Span::styled(
                    "Nothing is deleted. U unmarks it later.",
                    theme::muted(),
                )),
            ],
        ),
        Confirm::RemoveWorktree {
            branch, commands, ..
        } => {
            let mut lines = vec![
                Line::from(vec![
                    Span::raw("Clean up "),
                    Span::styled(clean(branch), theme::bold()),
                    Span::raw("? These commands run:"),
                ]),
                Line::from(""),
            ];
            for cmd in commands {
                lines.push(Line::from(Span::styled(
                    format!("  {}", clean(cmd)),
                    Style::new().fg(theme::WARN),
                )));
            }
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "No --force: git refuses a worktree with changes, and -d refuses unmerged branches.",
                theme::muted(),
            )));
            ("Remove worktree", lines)
        }
        Confirm::DeleteTodo { title, .. } => (
            "Delete todo",
            vec![
                Line::from(vec![
                    Span::raw("Delete "),
                    Span::styled(format!("“{}”", clean(title)), theme::bold()),
                    Span::raw(" from the todo file?"),
                ]),
                Line::from(Span::styled(
                    "D drops it instead and keeps it in Done.",
                    theme::muted(),
                )),
            ],
        ),
        Confirm::AcceptAll { count } => (
            "Accept all",
            vec![Line::from(format!(
                "Accept all {count} pending suggestions? Choice suggestions are kept."
            ))],
        ),
        Confirm::RejectAll { count } => (
            "Reject all",
            vec![
                Line::from(format!("Reject all {count} pending suggestions?")),
                Line::from(Span::styled(
                    "Rejected suggestions are never suggested again.",
                    theme::muted(),
                )),
            ],
        ),
    }
}

pub fn draw_confirm(frame: &mut Frame, app: &App, c: &Confirm) {
    let (title, lines) = confirm_lines(c);
    let footer = hint(vec![key_hint("y", "confirm"), key_hint("n/esc", "cancel")]);
    let scroll = render_modal(
        frame,
        title,
        theme::WARN,
        72,
        lines,
        Some(footer),
        app.overlay_scroll,
    );
    app.feedback
        .overlay_max_scroll
        .set(scroll.total.saturating_sub(scroll.viewport));
}

pub fn draw_link_picker(frame: &mut Frame, app: &App, p: &LinkPicker) {
    let entries = app.picker_entries(p);
    let room = (frame.area().height as usize)
        .saturating_sub(10)
        .clamp(3, 14);
    let cursor = p.cursor.min(entries.len().saturating_sub(1));
    let start = cursor.saturating_sub(room.saturating_sub(1));
    let width = 60usize.min(frame.area().width.saturating_sub(8) as usize);
    let mut lines = vec![Line::from(vec![
        Span::styled("Link to › ", theme::key()),
        Span::raw(clean(&p.query.text)),
        Span::styled("▏", theme::accent()),
    ])];
    lines.push(Line::from(""));
    if entries.is_empty() {
        lines.push(Line::from(Span::styled(
            "no matching work item",
            theme::muted(),
        )));
    }
    for (i, e) in entries.iter().enumerate().skip(start).take(room) {
        let mut line = match e {
            PickerEntry::RemoveLink => Line::from(Span::styled(
                fit("  — remove link —", width),
                theme::muted(),
            )),
            PickerEntry::Item(idx) => {
                let w = &app.work_items[*idx];
                Line::from(vec![
                    Span::styled(format!("{} ", w.state.icon()), theme::state(w.state)),
                    Span::raw(fit(&clean(&w.link_ref()), width.saturating_sub(2))),
                ])
            }
        };
        if i == cursor {
            line = line.patch_style(theme::selected());
        }
        lines.push(line);
    }
    let footer = hint(vec![
        key_hint("↑↓", "move"),
        key_hint("enter", "link"),
        key_hint("esc", "cancel"),
    ]);
    render_modal(
        frame,
        "Link todo",
        theme::ACCENT,
        66,
        lines,
        Some(footer),
        0,
    );
}
