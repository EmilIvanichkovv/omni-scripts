//! Confirmation dialog, the todo link picker and the New todo dialog.

use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::Frame;

use super::theme;
use super::widgets::{
    clean, cursor_line, field, fit, fit_right, hint_line, key_hint, render_input, render_modal,
    render_modal_body, LABEL_W,
};
use crate::app::{App, BarHit, Confirm, LinkPicker, NewTodo, PickerEntry};
use crate::lgbc::responsive::str_width;
use crate::todos::Priority;

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
                fit("   — remove link —", width),
                theme::muted(),
            )),
            PickerEntry::Item(idx) => {
                let w = &app.work_items[*idx];
                // Column 0 is the cursor gutter.
                Line::from(vec![
                    Span::raw(" "),
                    Span::styled(format!("{} ", w.state.icon()), theme::state(w.state)),
                    Span::raw(fit(&clean(&w.link_ref()), width.saturating_sub(3))),
                ])
            }
        };
        if i == cursor {
            line = cursor_line(line, width);
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

/// Widest the New todo dialog gets.
const NEW_TODO_WIDTH: u16 = 76;

/// Key hints of the New todo dialog.
pub fn new_todo_hints() -> Vec<Vec<Span<'static>>> {
    vec![
        key_hint("enter", "add"),
        key_hint("↑↓", "suggestion"),
        key_hint("tab", "priority"),
        key_hint("esc", "cancel"),
    ]
}

/// A row of the dialog, after the one-column gutter every row keeps for the cursor mark.
fn gutter(line: Line<'static>) -> Line<'static> {
    let mut spans = vec![Span::raw(" ")];
    spans.extend(line.spans);
    Line::from(spans)
}

/// `none · A · B · C` with the chosen one as the active label.
fn priority_choices(chosen: Option<Priority>) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    for (i, (p, label)) in [
        (None, "none"),
        (Some(Priority::A), "A"),
        (Some(Priority::B), "B"),
        (Some(Priority::C), "C"),
    ]
    .into_iter()
    .enumerate()
    {
        if i > 0 {
            spans.push(Span::styled(" · ", theme::muted()));
        }
        let style = if p == chosen {
            theme::active_label()
        } else {
            theme::soft()
        };
        spans.push(Span::styled(label, style));
    }
    spans
}

/// The New todo dialog: link, priority, the title input and the suggestions from the
/// linked sessions. Every row fits the width (nothing wraps), and the suggestion list gets
/// the rows left on screen, scrolled to keep the highlighted one visible, so the body never
/// scrolls and its rows can be click targets.
pub fn draw_new_todo(frame: &mut Frame, app: &App, d: &NewTodo) {
    let area = frame.area();
    let width = NEW_TODO_WIDTH.min(area.width.saturating_sub(4));
    let inner_w = width.saturating_sub(2) as usize;
    // Screen rows minus the margins, borders and the two footer rows of `render_modal`.
    let max_rows = (area.height as usize).saturating_sub(6);
    let roomy = max_rows >= 8;
    let value_w = inner_w.saturating_sub(1 + LABEL_W);

    let mut lines = Vec::new();
    let link = match &d.link {
        Some(l) => Span::raw(fit(&clean(l), value_w).trim_end().to_string()),
        None => Span::styled("no link", theme::muted()),
    };
    lines.push(gutter(field("Link", vec![link])));
    lines.push(gutter(field("Priority", priority_choices(d.priority))));
    if roomy {
        lines.push(Line::from(""));
    }
    let input_row = lines.len();
    lines.push(Line::from(""));
    if roomy {
        lines.push(Line::from(""));
    }

    let room = max_rows.saturating_sub(lines.len() + 1);
    let mut seed_rows: Vec<(usize, usize)> = Vec::new();
    if room > 0 {
        let mut heading = vec![Span::styled("Suggestions", theme::heading())];
        if d.seeds.len() > room {
            heading.push(Span::styled(format!("  {}", d.seeds.len()), theme::muted()));
        }
        lines.push(gutter(Line::from(heading)));
        if d.seeds.is_empty() {
            lines.push(gutter(Line::from(Span::styled(
                "no suggestions from sessions",
                theme::muted(),
            ))));
        }
        // One blank column after the tag, like the gutter before the text.
        let tag = |exists: bool, kind: &str| {
            if exists {
                format!("  ✓ exists  {kind} ")
            } else {
                format!("  {kind} ")
            }
        };
        let tag_w = d
            .seeds
            .iter()
            .map(|s| str_width(&tag(s.exists, s.kind.label())))
            .max()
            .unwrap_or(0)
            .min(inner_w / 2);
        let text_w = inner_w.saturating_sub(1 + tag_w);
        let start = d
            .selected
            .map_or(0, |sel| sel.saturating_sub(room.saturating_sub(1)));
        for (i, s) in d.seeds.iter().enumerate().skip(start).take(room) {
            let text_style = if s.exists {
                theme::muted()
            } else {
                Style::new()
            };
            let mut line = Line::from(vec![
                Span::raw(" "),
                Span::styled(fit(&s.text, text_w), text_style),
                Span::styled(
                    fit_right(&tag(s.exists, s.kind.label()), tag_w),
                    theme::muted(),
                ),
            ]);
            if d.selected == Some(i) {
                line = cursor_line(line, inner_w);
            }
            seed_rows.push((lines.len(), i));
            lines.push(line);
        }
    }

    let footer = hint_line(new_todo_hints(), width.saturating_sub(2));
    let (_, body) = render_modal_body(
        frame,
        "New todo",
        theme::ACCENT,
        NEW_TODO_WIDTH,
        lines,
        Some(footer),
        0,
    );
    let row = |n: usize| -> Option<Rect> {
        let y = body.y.checked_add(u16::try_from(n).ok()?)?;
        (y < body.bottom()).then_some(Rect {
            y,
            height: 1,
            ..body
        })
    };
    let mut hits = app.feedback.bar_hits.borrow_mut();
    for (n, i) in seed_rows {
        if let Some(r) = row(n) {
            hits.push((r, BarHit::Seed(i)));
        }
    }
    if let Some(r) = row(input_row) {
        render_input(frame, r, &format!(" {}", fit("Title", LABEL_W)), &d.input);
    }
}
