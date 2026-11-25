//! Views, one file per view + `widgets/` (spec 9). OWNED BY: UI agent. MUST NOT perform IO.
//!
//! Entry point: [`draw`]`(frame, &app)`. Every size decision is made from `frame.area()` on
//! each frame, so a resized terminal is laid out afresh and never panics. The renderer
//! reports pane areas and scroll limits back through `App::feedback` (for the mouse wheel
//! and key handling).
//!
//! Frame (spec 9.1): header row, tab bar row, content (list pane + details pane), footer of
//! two rows (status line, context hints). Breakpoints: ≥ 120 columns side by side (list
//! 60 %), 80–119 stacked (60/40), < 80 list only with `Enter`/`Esc` for details, below
//! 40×10 only `terminal too small`. `v` forces an arrangement (falls back to list only when
//! it does not fit).

mod dialogs;
mod help;
mod inbox;
#[cfg(test)]
pub(crate) mod sample;
mod sessions;
mod summary;
#[cfg(test)]
mod tests;
mod theme;
mod todos;
pub mod widgets;
mod work;

use ratatui::layout::{Alignment, Constraint, Flex, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};
use ratatui::Frame;

use crate::app::{App, InputPurpose, Overlay, StatusKind, Tab};
use crate::lgbc::responsive::{self, str_width, truncate_end, Arrangement, LayoutMode};
use crate::refresh::PrStatus;
use widgets::{clean, fmt_age, hint_line, key_hint, pane_block};

/// Auto layout: side by side from this terminal width (spec 9.1).
const SIDE_BY_SIDE_WIDTH: u16 = 120;
/// Auto layout: stacked from this terminal width.
const STACKED_WIDTH: u16 = 80;
/// Stacked needs this many content rows, else list only.
const STACKED_MIN_HEIGHT: u16 = 13;

/// Render the whole UI.
pub fn draw(frame: &mut Frame, app: &App) {
    let area = frame.area();
    let fb = &app.feedback;
    let too_small = responsive::is_too_small(area);
    fb.too_small.set(too_small);
    if too_small {
        fb.list_area.set(Rect::default());
        fb.details_area.set(None);
        render_too_small(frame, area);
        finish(frame, app);
        return;
    }

    let [header, tabs, content, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(2),
    ])
    .areas(area);
    render_header(frame, app, header);
    render_tabs(frame, app, tabs);
    render_footer(frame, app, footer);

    let arrangement = choose_arrangement(app.layout_mode, area, content);
    fb.arrangement.set(Some(arrangement));

    if app.tab == Tab::Summary {
        fb.list_area.set(content);
        fb.details_area.set(None);
        summary::draw(frame, app, content);
    } else if arrangement == Arrangement::ListOnly && app.details_open {
        fb.list_area.set(Rect::default());
        fb.details_area.set(Some(content));
        draw_details(frame, app, content);
    } else {
        let (list, details) = split(arrangement, content);
        fb.list_area.set(list);
        fb.details_area.set(details);
        match app.tab {
            Tab::Work => work::draw_list(frame, app, list),
            Tab::Todos => todos::draw_list(frame, app, list),
            Tab::Sessions => sessions::draw_list(frame, app, list),
            Tab::Inbox => inbox::draw_list(frame, app, list),
            Tab::Summary => {}
        }
        if let Some(details) = details {
            draw_details(frame, app, details);
        }
    }

    match &app.overlay {
        Some(Overlay::Help) => help::draw_help(frame, app),
        Some(Overlay::Diagnostics) => help::draw_diagnostics(frame, app),
        Some(Overlay::Confirm(c)) => dialogs::draw_confirm(frame, app, c),
        Some(Overlay::LinkPicker(p)) => dialogs::draw_link_picker(frame, app, p),
        None => {}
    }
    finish(frame, app);
}

fn finish(frame: &mut Frame, app: &App) {
    if app.no_color {
        strip_colors(frame);
    }
}

/// `NO_COLOR`: drop every foreground/background color, keep modifiers (bold, reverse...).
fn strip_colors(frame: &mut Frame) {
    let buf = frame.buffer_mut();
    for cell in buf.content.iter_mut() {
        cell.set_fg(Color::Reset);
        cell.set_bg(Color::Reset);
    }
}

fn draw_details(frame: &mut Frame, app: &App, area: Rect) {
    let (lines, title) = match app.tab {
        Tab::Work => (
            work::details(app, area.width.saturating_sub(2)),
            " Details ",
        ),
        Tab::Todos => (todos::details(app), " Todo "),
        Tab::Sessions => (sessions::details(app), " Session "),
        Tab::Inbox => (inbox::details(app), " Suggestion "),
        Tab::Summary => (Vec::new(), ""),
    };
    let mut block = pane_block(Line::from(Span::styled(title, theme::accent())));
    if app.details_open {
        block = block.title(
            Line::from(vec![
                Span::styled(" esc", theme::key()),
                Span::styled(" back ", theme::muted()),
            ])
            .right_aligned(),
        );
    }
    let scroll = widgets::render_scrolled(
        frame,
        block,
        lines,
        area,
        app.details_scroll,
        Some(widgets::scroll_hint(if app.details_open {
            "j/k"
        } else {
            "J/K"
        })),
    );
    app.feedback
        .details_max_scroll
        .set(scroll.total.saturating_sub(scroll.viewport));
}

/// The arrangement for the terminal `area` (spec 9.1 breakpoints), or the forced one.
pub fn choose_arrangement(mode: LayoutMode, area: Rect, content: Rect) -> Arrangement {
    match mode {
        LayoutMode::Auto if area.width >= SIDE_BY_SIDE_WIDTH => Arrangement::SideBySide,
        LayoutMode::Auto if area.width >= STACKED_WIDTH && content.height >= STACKED_MIN_HEIGHT => {
            Arrangement::Stacked
        }
        LayoutMode::Auto => Arrangement::ListOnly,
        forced => responsive::arrange(forced, content),
    }
}

/// List 60 %, details 40 %.
fn split(arrangement: Arrangement, content: Rect) -> (Rect, Option<Rect>) {
    match arrangement {
        Arrangement::SideBySide => {
            let [l, d] =
                Layout::horizontal([Constraint::Percentage(60), Constraint::Percentage(40)])
                    .areas(content);
            (l, Some(d))
        }
        Arrangement::Stacked => {
            let [l, d] = Layout::vertical([Constraint::Percentage(60), Constraint::Percentage(40)])
                .areas(content);
            (l, Some(d))
        }
        Arrangement::ListOnly => (content, None),
    }
}

/// Draw the bordered list pane chrome: `title` left, `count` right, the search line on top
/// when a query is set, the inline input at the bottom when one is open. Returns the area
/// left for rows and records the viewport height.
pub(crate) fn list_pane(
    frame: &mut Frame,
    app: &App,
    area: Rect,
    title: Line<'static>,
    count: String,
) -> Rect {
    let mut block = pane_block(title)
        .title(Line::from(Span::styled(format!(" {count} "), theme::muted())).right_aligned());
    if app.tab.has_details() && app.feedback.arrangement.get() == Some(Arrangement::ListOnly) {
        block = block.title_bottom(
            Line::from(vec![
                Span::styled(" enter", theme::key()),
                Span::styled(" details ", theme::muted()),
            ])
            .right_aligned(),
        );
    }
    let mut inner = block.inner(area);
    frame.render_widget(block, area);

    let query = &app.search[app.tab.index()];
    let searching = matches!(&app.input, Some(a) if a.purpose == InputPurpose::Search);
    if (searching || !query.is_empty()) && inner.height > 1 {
        let [top, rest] =
            Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(inner);
        match &app.input {
            Some(a) if searching => widgets::render_input(frame, top, " / ", &a.input),
            _ => frame.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled(" / ", theme::key()),
                    Span::raw(clean(query)),
                    Span::styled("  (esc clears)", theme::muted()),
                ])),
                top,
            ),
        }
        inner = rest;
    }
    if let Some(active) = &app.input {
        if active.purpose != InputPurpose::Search && inner.height > 1 {
            let [rest, bottom] =
                Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(inner);
            let prompt = match &active.purpose {
                InputPurpose::AddTodo { link: Some(_) } => " new linked todo › ",
                InputPurpose::AddTodo { link: None } => " new todo › ",
                InputPurpose::EditTodo { .. } => " title › ",
                InputPurpose::EditSuggestion { .. } => " accept as › ",
                InputPurpose::Search => " / ",
            };
            widgets::render_input(frame, bottom, prompt, &active.input);
            inner = rest;
        }
    }
    app.feedback.list_viewport.set(inner.height as usize);
    inner
}

/// Empty-list message centered-ish in `area`.
pub(crate) fn render_empty(frame: &mut Frame, area: Rect, text: &str) {
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(format!(" {text}"), theme::muted())))
            .wrap(Wrap { trim: true }),
        area,
    );
}

fn render_too_small(frame: &mut Frame, area: Rect) {
    let lines = vec![
        Line::from(Span::styled(
            "terminal too small",
            Style::new().fg(theme::WARN).add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            format!(
                "{}×{}, need {}×{}",
                area.width,
                area.height,
                responsive::MIN_WIDTH,
                responsive::MIN_HEIGHT
            ),
            theme::muted(),
        )),
        Line::from(Span::styled("q quits", theme::muted())),
    ];
    let height = (lines.len() as u16).min(area.height);
    let [middle] = Layout::vertical([Constraint::Length(height)])
        .flex(Flex::Center)
        .areas(area);
    frame.render_widget(
        Paragraph::new(lines)
            .alignment(Alignment::Center)
            .wrap(Wrap { trim: true }),
        middle,
    );
}

fn pr_status_text(app: &App) -> (String, Style) {
    match &app.pr_status {
        None => ("PR …".into(), theme::muted()),
        Some(PrStatus::Ok) => ("PR ✓".into(), Style::new().fg(theme::OK)),
        Some(PrStatus::NoToken) => ("PR: no token".into(), Style::new().fg(theme::WARN)),
        Some(PrStatus::Offline) => ("PR: offline".into(), Style::new().fg(theme::WARN)),
        Some(PrStatus::Error { message, .. }) => (
            format!("PR: {}", clean(message)),
            Style::new().fg(theme::DANGER),
        ),
        Some(PrStatus::Cached { age_secs }) => (
            format!("PR: cached {}", fmt_age(*age_secs, Some(0))),
            Style::new().fg(theme::WARN),
        ),
    }
}

fn render_header(frame: &mut Frame, app: &App, area: Rect) {
    let repos = app
        .repos
        .iter()
        .filter(|r| r.skipped_reason.is_none())
        .count();
    let items = app
        .work_items
        .iter()
        .filter(|w| w.state != crate::model::WorkState::Protected)
        .count();
    let (pr, pr_style) = pr_status_text(app);
    let synced = match app.synced_at {
        Some(t) => format!("synced {} ago", fmt_age(app.now_ts(), Some(t))),
        None => "not synced yet".into(),
    };
    let right = app
        .progress
        .as_ref()
        .map(|(task, done, total)| format!("⟳ {} {done}/{total} ", clean(task)))
        .unwrap_or_default();
    let right_w = str_width(&right);
    let left_budget = (area.width as usize).saturating_sub(right_w + 1);

    let sep = Span::styled(" · ", theme::muted());
    let mut spans = vec![
        Span::styled(" worktrack", theme::heading()),
        Span::styled(" ── ", theme::muted()),
        Span::raw(format!("{repos} repos")),
        sep.clone(),
        Span::raw(format!("{items} items")),
        sep.clone(),
        Span::styled(pr, pr_style),
        sep,
        Span::styled(synced, theme::muted()),
    ];
    // Cut from the right so the title and counts stay.
    let mut used = 0;
    let mut kept = Vec::new();
    for s in spans.drain(..) {
        let w = s.width();
        if used + w > left_budget {
            let rest = left_budget.saturating_sub(used);
            if rest > 0 {
                let cut = truncate_end(&s.content, rest);
                used += str_width(&cut);
                kept.push(Span::styled(cut, s.style));
            }
            break;
        }
        used += w;
        kept.push(s);
    }
    let fill = (area.width as usize).saturating_sub(used + right_w);
    kept.push(Span::raw(" ".repeat(fill)));
    kept.push(Span::styled(right, Style::new().fg(theme::WARN)));
    frame.render_widget(Paragraph::new(Line::from(kept)), area);
}

fn render_tabs(frame: &mut Frame, app: &App, area: Rect) {
    let mut spans = vec![Span::raw(" ")];
    for tab in Tab::ALL {
        let mut label = format!("{} {}", tab.index() + 1, tab.title());
        match tab {
            Tab::Inbox => {
                let n = app.pending_count();
                if n > 0 {
                    label.push_str(&format!("·{n}"));
                }
            }
            Tab::Sessions if app.any_session_live() => label.push_str("·live"),
            Tab::Todos if app.todo_error.is_some() => label.push_str("·!"),
            _ => {}
        }
        let style = if tab == app.tab {
            Style::new()
                .fg(theme::ACCENT)
                .add_modifier(Modifier::BOLD | Modifier::REVERSED)
        } else {
            theme::muted()
        };
        spans.push(Span::styled(format!(" {label} "), style));
        spans.push(Span::raw(" "));
    }
    let line = Line::from(spans);
    let line = if line.width() > area.width as usize {
        // Narrow: drop the titles, keep digits and badges.
        let mut spans = vec![Span::raw(" ")];
        for tab in Tab::ALL {
            let mut label = format!("{}{}", tab.index() + 1, &tab.title()[..1]);
            if tab == Tab::Inbox && app.pending_count() > 0 {
                label.push_str(&format!("·{}", app.pending_count()));
            }
            if tab == Tab::Sessions && app.any_session_live() {
                label.push('●');
            }
            let style = if tab == app.tab {
                Style::new()
                    .fg(theme::ACCENT)
                    .add_modifier(Modifier::BOLD | Modifier::REVERSED)
            } else {
                theme::muted()
            };
            spans.push(Span::styled(format!(" {label} "), style));
        }
        Line::from(spans)
    } else {
        line
    };
    frame.render_widget(Paragraph::new(line), area);
}

fn footer_hints(app: &App) -> Vec<Vec<Span<'static>>> {
    if let Some(input) = &app.input {
        return match input.purpose {
            InputPurpose::Search => vec![
                key_hint("enter", "keep"),
                key_hint("esc", "clear"),
                key_hint("↑↓", "move"),
                key_hint("@repo: @state:", "prefixes"),
            ],
            _ => vec![key_hint("enter", "save"), key_hint("esc", "cancel")],
        };
    }
    if app.details_open {
        return vec![
            key_hint("esc", "back"),
            key_hint("j/k", "scroll"),
            key_hint("?", "help"),
        ];
    }
    let mut g = match app.tab {
        Tab::Work => vec![
            key_hint("j/k", "move"),
            key_hint("enter", "open"),
            key_hint("o", "PR"),
            key_hint("t", "todo"),
            key_hint("c", "resume"),
            key_hint("y/Y", "copy"),
            key_hint("f", "filter"),
            key_hint("/", "search"),
            key_hint("A", "abandon"),
            key_hint("X", "remove"),
        ],
        Tab::Todos => vec![
            key_hint("a", "add"),
            key_hint("e", "edit"),
            key_hint("space", "done"),
            key_hint("s", "status"),
            key_hint("p", "prio"),
            key_hint("L", "link"),
            key_hint("+/-", "move"),
            key_hint("d", "delete"),
            key_hint("f", "filter"),
            key_hint("G", "group"),
            key_hint("E/O", "editor"),
        ],
        Tab::Sessions => vec![
            key_hint("j/k", "move"),
            key_hint("c", "resume"),
            key_hint("s", "resummarize"),
            key_hint("enter", "work item"),
            key_hint("/", "search"),
        ],
        Tab::Summary => vec![
            key_hint("[ ]", "shift"),
            key_hint("y", "copy"),
            key_hint("w", "write"),
            key_hint("j/k", "scroll"),
        ],
        Tab::Inbox => vec![
            key_hint("enter", "accept"),
            key_hint("x", "reject"),
            key_hint("e", "edit"),
            key_hint("k/b", "keep/abandon"),
            key_hint("A/X", "all"),
            key_hint("↑↓", "move"),
        ],
    };
    g.push(key_hint("?", "help"));
    g.push(key_hint("q", "quit"));
    g
}

fn render_footer(frame: &mut Frame, app: &App, area: Rect) {
    let [status_row, hints_row] =
        Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).areas(area);
    let width = status_row.width as usize;
    let status = match (&app.status, &app.todo_error) {
        (Some(s), _) => Some((clean(&s.text), s.kind)),
        (None, Some(e)) => Some((format!("todo file: {}", clean(e)), StatusKind::Error)),
        _ => None,
    };
    if let Some((text, kind)) = status {
        let (prefix, style) = match kind {
            StatusKind::Info => (" ", Style::new()),
            StatusKind::Error => (" ✗ ", Style::new().fg(theme::DANGER)),
        };
        let text = truncate_end(&format!("{prefix}{text}"), width);
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(text, style))),
            status_row,
        );
    }
    frame.render_widget(
        Paragraph::new(hint_line(footer_hints(app), hints_row.width)),
        hints_row,
    );
}
