//! Inbox view (spec 9.5): pending rule suggestions, newest first. `+` adds a todo on
//! accept; `?` needs a keep/abandon choice.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::Frame;

use super::theme;
use super::widgets::{
    clean, field, fit, fmt_when_long, key_hint, push_section, render_rows, short_link,
};
use crate::app::{needs_choice, suggestion_text, App};
use crate::todos::rules::{Suggestion, SuggestionPayload};

const RULE_W: usize = 21;
const LINK_MAX: usize = 36;
const TEXT_MIN: usize = 16;

fn marker(s: &Suggestion) -> (&'static str, Style) {
    if needs_choice(s) {
        (
            "?",
            Style::new().fg(theme::WARN).add_modifier(Modifier::BOLD),
        )
    } else {
        ("+", Style::new().fg(theme::OK).add_modifier(Modifier::BOLD))
    }
}

fn suggestion_line(s: &Suggestion, width: usize) -> Line<'static> {
    let show_rule = width >= 1 + RULE_W + 1 + 2 + TEXT_MIN + 20;
    let fixed = 1 + if show_rule { RULE_W + 1 } else { 0 } + 2;
    let rest = width.saturating_sub(fixed);
    let link_w = if rest >= TEXT_MIN + 12 {
        (rest - TEXT_MIN).min(LINK_MAX).min(rest / 2)
    } else {
        0
    };
    let text_w = rest.saturating_sub(if link_w > 0 { link_w + 1 } else { 0 });
    let (text, link) = suggestion_text(s);
    let (m, mstyle) = marker(s);
    let mut spans = vec![Span::raw(" ")];
    if show_rule {
        spans.push(Span::styled(fit(&clean(&s.rule), RULE_W), theme::muted()));
        spans.push(Span::raw(" "));
    }
    spans.push(Span::styled(m, mstyle));
    spans.push(Span::raw(" "));
    spans.push(Span::raw(fit(&clean(text), text_w)));
    if link_w > 0 {
        spans.push(Span::raw(" "));
        spans.push(Span::styled(
            fit(&clean(&link.map(short_link).unwrap_or_default()), link_w),
            theme::accent(),
        ));
    }
    Line::from(spans)
}

pub fn draw_list(frame: &mut Frame, app: &App, area: Rect) {
    // The tab names the pane; the count on the right says the rest.
    let title = Line::default();
    let visible = app.pending_suggestions();
    let rows_area = super::list_pane(
        frame,
        app,
        area,
        title,
        format!("{} pending", visible.len()),
        None,
    );
    if visible.is_empty() {
        super::render_empty(
            frame,
            rows_area,
            if app.pending_count() == 0 {
                "Inbox is empty. Todo suggestions from branch, PR and session changes show up here."
            } else {
                "No suggestion matches the search. Esc clears it."
            },
        );
        return;
    }
    let width = rows_area.width as usize;
    let lines = visible
        .iter()
        .map(|&i| suggestion_line(&app.suggestions[i], width))
        .collect::<Vec<_>>();
    let cursor = Some(app.cursor[4].min(lines.len() - 1));
    render_rows(
        frame,
        rows_area,
        area,
        lines,
        cursor,
        &app.feedback.list_offset[4],
    );
}

pub fn details(app: &App) -> Vec<Line<'static>> {
    let Some(s) = app.selected_suggestion() else {
        return vec![Line::from(Span::styled(
            "no pending suggestions",
            theme::muted(),
        ))];
    };
    suggestion_details(app, s)
}

/// Details pane lines for one suggestion (Inbox tab and Summary targets).
pub fn suggestion_details(app: &App, s: &Suggestion) -> Vec<Line<'static>> {
    let (text, link) = suggestion_text(s);
    let (m, mstyle) = marker(s);
    let mut lines = vec![Line::from(vec![
        Span::styled(format!("{m} "), mstyle),
        Span::styled(clean(text), theme::bold()),
    ])];
    lines.push(field("Rule", vec![Span::raw(clean(&s.rule))]));
    lines.push(field(
        "Created",
        vec![Span::raw(fmt_when_long(&app.now, Some(s.created_at)))],
    ));
    if let Some(link) = link {
        let mut v = vec![Span::raw(clean(link))];
        if let Some(item) = app.item_for_link(link) {
            v.push(Span::styled(
                format!("  {} {}", item.state.icon(), item.state.label()),
                theme::state(item.state),
            ));
        }
        lines.push(field("Link", v));
    }
    if let SuggestionPayload::AddTodo { source, .. } = &s.payload {
        lines.push(field("Source", vec![Span::raw(clean(source))]));
    }
    lines.push(field(
        "Key",
        vec![Span::styled(clean(&s.subject_key), theme::muted())],
    ));
    push_section(&mut lines, "Actions");
    let actions = if needs_choice(s) {
        vec![
            key_hint("k", "keep"),
            key_hint("b", "abandon branch, drop todos"),
            key_hint("x", "reject"),
        ]
    } else if matches!(s.payload, SuggestionPayload::AddTodo { .. }) {
        vec![
            key_hint("enter", "accept"),
            key_hint("e", "edit, then accept"),
            key_hint("x", "reject"),
        ]
    } else {
        vec![key_hint("enter", "accept"), key_hint("x", "reject")]
    };
    for a in actions {
        let mut spans = vec![Span::raw("  ")];
        spans.extend(a);
        lines.push(Line::from(spans));
    }
    lines
}
