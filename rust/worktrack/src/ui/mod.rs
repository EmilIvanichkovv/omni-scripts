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

use crate::app::{App, InputPurpose, Overlay, StatusKind, Tab, WorkRow};
use crate::lgbc::responsive::{self, str_width, truncate_end, Arrangement, LayoutMode};
use crate::refresh::PrStatus;
use widgets::{clean, fmt_age, hint_line_pinned, key_hint, pane_block, BarItem};

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
    fb.bar_hits.borrow_mut().clear();
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

    if arrangement == Arrangement::ListOnly && app.details_open {
        fb.list_area.set(Rect::default());
        fb.details_area.set(Some(content));
        draw_details(frame, app, content);
    } else {
        let (list, details) = split(arrangement, content, list_rows_wanted(app));
        fb.list_area.set(list);
        fb.details_area.set(details);
        match app.tab {
            Tab::Work => work::draw_list(frame, app, list),
            Tab::Todos => todos::draw_list(frame, app, list),
            Tab::Sessions => sessions::draw_list(frame, app, list),
            Tab::Summary => summary::draw_list(frame, app, list),
            Tab::Inbox => inbox::draw_list(frame, app, list),
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
    let inner_w = area.width.saturating_sub(2);
    let (lines, title) = match app.tab {
        Tab::Work => (work::details(app, inner_w), " Details "),
        Tab::Todos => (todos::details(app), " Todo "),
        Tab::Sessions => (sessions::details(app), " Session "),
        Tab::Inbox => (inbox::details(app), " Suggestion "),
        Tab::Summary => summary::details(app, inner_w),
    };
    // One column of air on the right of the text, the scrollbar sits on the border.
    let lines = widgets::tidy_details(lines, inner_w.saturating_sub(1) as usize);
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

/// Rows the list pane of the current tab needs to show everything (borders included).
fn list_rows_wanted(app: &App) -> u16 {
    let rows = match app.tab {
        Tab::Work => app.work_rows().len(),
        Tab::Todos => app.todo_rows().len() + 1,
        Tab::Sessions => sessions::line_count(app),
        Tab::Summary => summary::line_count(app) + 1,
        Tab::Inbox => app.pending_suggestions().len(),
    };
    let searching = !app.search[app.tab.index()].is_empty()
        || matches!(&app.input, Some(a) if a.purpose == InputPurpose::Search);
    let extra = usize::from(searching) + usize::from(app.input.is_some());
    // Empty lists show a message of up to three rows.
    u16::try_from(rows.max(3) + extra + 2).unwrap_or(u16::MAX)
}

/// Stacked: at most 60 % for the list, less when its rows need less (so a short list does
/// not leave a block of empty rows), at least 40 %. Side by side: list 60 %, details 40 %.
fn split(arrangement: Arrangement, content: Rect, list_wanted: u16) -> (Rect, Option<Rect>) {
    match arrangement {
        Arrangement::SideBySide => {
            let [l, d] =
                Layout::horizontal([Constraint::Percentage(60), Constraint::Percentage(40)])
                    .areas(content);
            (l, Some(d))
        }
        Arrangement::Stacked => {
            let max = content.height * 3 / 5;
            let min = (content.height * 2 / 5).min(max);
            let list_h = list_wanted.clamp(min, max);
            let [l, d] =
                Layout::vertical([Constraint::Length(list_h), Constraint::Min(0)]).areas(content);
            (l, Some(d))
        }
        Arrangement::ListOnly => (content, None),
    }
}

/// Draw the bordered list pane chrome: `title` left, `count` right (if any), the filter /
/// section `bar` as the first row, the search line under it when a query is set, the inline
/// input at the bottom when one is open. Returns the area left for rows and records the
/// viewport height.
pub(crate) fn list_pane(
    frame: &mut Frame,
    app: &App,
    area: Rect,
    title: Line<'static>,
    count: String,
    bar: Option<Vec<BarItem>>,
) -> Rect {
    let mut block = pane_block(title);
    if !count.is_empty() {
        block = block
            .title(Line::from(Span::styled(format!(" {count} "), theme::muted())).right_aligned());
    }
    if app.feedback.arrangement.get() == Some(Arrangement::ListOnly) {
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

    if let Some(items) = bar {
        if inner.height > 1 {
            let [top, rest] =
                Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(inner);
            let (line, hits) = widgets::filter_bar(&items, top);
            frame.render_widget(Paragraph::new(line), top);
            app.feedback.bar_hits.borrow_mut().extend(hits);
            inner = rest;
        }
    }

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

/// Empty-list message: one friendly sentence saying what to do, under one blank row when
/// there is room.
pub(crate) fn render_empty(frame: &mut Frame, area: Rect, text: &str) {
    let mut lines = Vec::new();
    if area.height > 2 {
        lines.push(Line::from(""));
    }
    lines.push(Line::from(Span::styled(text.to_string(), theme::soft())));
    let inner = Rect {
        x: area.x.saturating_add(2).min(area.right()),
        width: area.width.saturating_sub(3),
        ..area
    };
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), inner);
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
        // Colour only when something is wrong.
        Some(PrStatus::Ok) => ("PR ✓".into(), Style::new()),
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
        Span::styled("  ", theme::muted()),
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
    kept.push(Span::styled(right, theme::accent()));
    frame.render_widget(Paragraph::new(Line::from(kept)), area);
}

/// ` label ` of the tab bar: the active one accent, bold and underlined (no block, so
/// `NO_COLOR` still shows it through bold + underline), the others soft.
fn tab_label(label: String, active: bool) -> [Span<'static>; 3] {
    let style = if active {
        theme::active_label()
    } else {
        theme::soft()
    };
    [Span::raw(" "), Span::styled(label, style), Span::raw(" ")]
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
        spans.extend(tab_label(label, tab == app.tab));
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
            spans.extend(tab_label(label, tab == app.tab));
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
        let mut g = vec![key_hint("esc", "back"), key_hint("j/k", "scroll")];
        match app.tab {
            Tab::Sessions => g.push(key_hint("enter", "work item")),
            Tab::Summary => g.extend([
                key_hint("enter", "open"),
                key_hint("o", "PR"),
                key_hint("space", "done"),
            ]),
            _ => {}
        }
        return g;
    }
    let list_only = app.feedback.arrangement.get() == Some(Arrangement::ListOnly);
    // The most useful keys of the view; `?` (pinned by the footer) lists the rest.
    match app.tab {
        Tab::Work => {
            let enter = match app.selected_work_row() {
                _ if list_only => Some("details"),
                Some(WorkRow::Group { collapsed, .. }) => {
                    Some(if collapsed { "expand" } else { "collapse" })
                }
                _ => None,
            };
            let mut g: Vec<_> = enter.map(|e| key_hint("enter", e)).into_iter().collect();
            g.extend([
                key_hint("o", "PR"),
                key_hint("c", "resume"),
                key_hint("t", "todo"),
                key_hint("f", "filter"),
                key_hint("/", "search"),
            ]);
            g
        }
        Tab::Todos => vec![
            key_hint("a", "add"),
            key_hint("space", "done"),
            key_hint("s", "status"),
            key_hint("e", "edit"),
            key_hint("L", "link"),
            key_hint("h/l", "filter"),
        ],
        Tab::Sessions => vec![
            key_hint("c", "resume"),
            key_hint("enter", if list_only { "details" } else { "work item" }),
            key_hint("s", "resummarize"),
            key_hint("/", "search"),
        ],
        Tab::Summary => vec![
            key_hint("h/l", "section"),
            // List only: Enter opens the details, Enter there jumps.
            key_hint("enter", if list_only { "details" } else { "open" }),
            key_hint("o", "PR"),
            key_hint("space", "done"),
            key_hint("y", "copy"),
            key_hint("[ ]", "window"),
        ],
        Tab::Inbox => vec![
            key_hint("enter", "accept"),
            key_hint("x", "reject"),
            key_hint("e", "edit"),
            key_hint("k/b", "keep/abandon"),
            key_hint("A/X", "all"),
        ],
    }
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
        Paragraph::new(hint_line_pinned(
            footer_hints(app),
            // While typing, `?` and `q` are text.
            if app.input.is_some() {
                Vec::new()
            } else {
                vec![key_hint("?", "help"), key_hint("q", "quit")]
            },
            hints_row.width,
        )),
        hints_row,
    );
}
