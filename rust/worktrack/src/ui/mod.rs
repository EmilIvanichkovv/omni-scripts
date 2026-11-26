//! Views, one file per view + `widgets/` (spec 9). OWNED BY: UI agent. MUST NOT perform IO.
//!
//! Entry point: [`draw`]`(frame, &app)`. Every size decision is made from `frame.area()` on
//! each frame, so a resized terminal is laid out afresh and never panics. The renderer
//! reports pane areas and scroll limits back through `App::feedback` (for the mouse wheel
//! and key handling).
//!
//! Frame (spec 9.1): header row, tab bar (three rows of boxed tabs plus a blank row when
//! the terminal is at least [`ROOMY_MIN_HEIGHT`] rows tall, else one row), content (list
//! pane + details pane), footer of two rows (status line, context hints). Breakpoints: ≥ 120 columns side by side (list
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

use crate::app::{App, BarHit, InputPurpose, ListRow, Overlay, StatusKind, Tab};
use crate::lgbc::responsive::{self, truncate_end, Arrangement, LayoutMode};
use crate::refresh::PrStatus;
use widgets::{clean, fmt_age, hint_line_pinned, key_hint, pane_block, BarItem};

/// Auto layout: side by side from this terminal width (spec 9.1).
const SIDE_BY_SIDE_WIDTH: u16 = 120;
/// Auto layout: stacked from this terminal width.
const STACKED_WIDTH: u16 = 80;
/// Stacked needs this many content rows, else list only.
const STACKED_MIN_HEIGHT: u16 = 13;
/// From this terminal height the frame is roomy: boxed tabs, a blank row between them and
/// the content, and a blank row under the filter / section bars. Shorter terminals get a
/// one-row tab bar and keep every other row for content.
pub(crate) const ROOMY_MIN_HEIGHT: u16 = 24;

/// Whether the frame for a terminal of this size has room for boxed tabs and air rows.
pub(crate) fn is_roomy(area: Rect) -> bool {
    area.height >= ROOMY_MIN_HEIGHT
}

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

    let roomy = is_roomy(area);
    let tab_bar = tab_bar_kind(app, area.width, roomy);
    let [header, tabs, _air, content, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(tab_bar.height()),
        Constraint::Length(u16::from(roomy)),
        Constraint::Min(0),
        Constraint::Length(2),
    ])
    .areas(area);
    render_header(frame, app, header);
    render_tabs(frame, app, tabs, tab_bar);
    render_footer(frame, app, footer);

    let arrangement = choose_arrangement(app.layout_mode, area, content);
    fb.arrangement.set(Some(arrangement));

    if arrangement == Arrangement::ListOnly && app.details_open {
        fb.list_area.set(Rect::default());
        fb.details_area.set(Some(content));
        draw_details(frame, app, content);
    } else {
        let (list, details) = split(arrangement, content, list_rows_wanted(app, roomy));
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
        Some(Overlay::NewTodo(d)) => dialogs::draw_new_todo(frame, app, d),
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
    let header = app.tab != Tab::Inbox && app.on_group_header(app.tab);
    let (lines, title) = match app.tab {
        Tab::Work if header => (work::details(app, inner_w), " Repo "),
        Tab::Work => (work::details(app, inner_w), " Details "),
        Tab::Todos if header => (todos::details(app), " Repo "),
        Tab::Todos => (todos::details(app), " Todo "),
        Tab::Sessions if header => (sessions::details(app), " Day "),
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

/// What the list pane of the current tab needs to show everything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ListWanted {
    /// List rows without the blank rows between groups.
    rows: usize,
    /// Blank rows between groups (drawn only when the viewport is tall enough).
    gaps: usize,
    /// Borders, filter bar (and the blank row under it), search line and input row.
    chrome: usize,
}

impl ListWanted {
    /// Pane height for `rows` list rows; empty lists show a message of up to three rows.
    fn height(self, rows: usize) -> u16 {
        u16::try_from(rows.max(3) + self.chrome).unwrap_or(u16::MAX)
    }
}

fn list_rows_wanted(app: &App, roomy: bool) -> ListWanted {
    // The filter / section bar and, when roomy, the blank row under it.
    let bar_rows = 1 + usize::from(roomy);
    let (rows, bar) = match app.tab {
        Tab::Work => (app.work_rows().len(), 0),
        Tab::Todos => (app.todo_rows().len(), bar_rows),
        Tab::Sessions => (sessions::line_count(app), 0),
        Tab::Summary => (summary::line_count(app), bar_rows),
        Tab::Inbox => (app.pending_suggestions().len(), 0),
    };
    let gaps = match app.tab {
        Tab::Inbox => 0,
        tab => widgets::gap_rows(&app.list_rows(tab)),
    };
    let searching = !app.search[app.tab.index()].is_empty()
        || matches!(&app.input, Some(a) if a.purpose == InputPurpose::Search);
    let extra = usize::from(searching) + usize::from(app.input.is_some());
    ListWanted {
        rows,
        gaps,
        chrome: bar + extra + 2,
    }
}

/// Stacked: at most 60 % for the list, less when its rows need less (so a short list does
/// not leave a block of empty rows), at least 40 %. The blank rows between groups count only
/// when the resulting viewport is tall enough to draw them (`widgets::group_gap`). Side by
/// side: list 60 %, details 40 %.
fn split(arrangement: Arrangement, content: Rect, wanted: ListWanted) -> (Rect, Option<Rect>) {
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
            let mut list_h = wanted.height(wanted.rows + wanted.gaps).clamp(min, max);
            let viewport = (list_h as usize).saturating_sub(wanted.chrome);
            if !widgets::group_gap(viewport) {
                list_h = wanted.height(wanted.rows).clamp(min, max);
            }
            let [l, d] =
                Layout::vertical([Constraint::Length(list_h), Constraint::Min(0)]).areas(content);
            (l, Some(d))
        }
        Arrangement::ListOnly => (content, None),
    }
}

/// The filter / section bar of a list pane: its labels and an optional muted `aside`
/// drawn on the right of the same row when it fits (the Todos grouping).
pub(crate) struct PaneBar {
    pub items: Vec<BarItem>,
    pub aside: Vec<Span<'static>>,
}

/// Draw the bordered list pane chrome: `title` left, `count` right (if any), the filter /
/// section `bar` as the first row (plus one blank row under it when the frame is roomy), the
/// search line under it when a query is set, the inline input at the bottom when one is
/// open. Returns the area left for rows and records the viewport height.
pub(crate) fn list_pane(
    frame: &mut Frame,
    app: &App,
    area: Rect,
    title: Line<'static>,
    count: String,
    bar: Option<PaneBar>,
) -> Rect {
    let roomy = is_roomy(frame.area());
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

    if let Some(bar) = bar {
        if inner.height > 1 {
            let [top, rest] =
                Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(inner);
            let (line, hits) = widgets::filter_bar(&bar.items, top);
            let aside = Line::from(bar.aside);
            let fits = line.width() + 3 + aside.width() <= top.width as usize;
            frame.render_widget(Paragraph::new(line), top);
            if fits && aside.width() > 0 {
                frame.render_widget(Paragraph::new(aside.right_aligned()), top);
            }
            app.feedback.bar_hits.borrow_mut().extend(hits);
            inner = rest;
            if roomy && inner.height > 1 {
                inner = Rect {
                    y: inner.y + 1,
                    height: inner.height - 1,
                    ..inner
                };
            }
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
                InputPurpose::AddTodo => " new todo › ",
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
        Some(PrStatus::Ok) => ("PR ✓".into(), theme::soft()),
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

/// One part of the header's status (right side), dropped in `keep` order (lowest first)
/// when the row is too narrow.
struct HeaderPart {
    span: Span<'static>,
    keep: u8,
    /// The progress part: set apart from the counts by a wider gap.
    progress: bool,
}

fn header_status(parts: &[HeaderPart]) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    for (i, p) in parts.iter().enumerate() {
        if i > 0 {
            if parts[i - 1].progress {
                spans.push(Span::raw("   "));
            } else {
                spans.push(Span::styled(" · ", theme::muted()));
            }
        }
        spans.push(p.span.clone());
    }
    spans.push(Span::raw(" "));
    spans
}

/// The app name on the left in accent bold; the status on the right: sync progress in the
/// accent colour, then counts, PR status (coloured only when something is wrong) and the
/// last sync, muted. Parts that do not fit are dropped (last sync first, PR status last).
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
    let mut parts = Vec::new();
    if let Some((task, done, total)) = &app.progress {
        parts.push(HeaderPart {
            span: Span::styled(format!("⟳ {} {done}/{total}", clean(task)), theme::accent()),
            keep: 3,
            progress: true,
        });
    }
    for (text, style, keep) in [
        (format!("{repos} repos"), theme::soft(), 1),
        (format!("{items} items"), theme::soft(), 2),
        (pr, pr_style, 4),
        (synced, theme::muted(), 0),
    ] {
        parts.push(HeaderPart {
            span: Span::styled(text, style),
            keep,
            progress: false,
        });
    }

    let name = Span::styled(" worktrack", theme::heading());
    let budget = (area.width as usize).saturating_sub(name.width() + 2);
    let mut status = header_status(&parts);
    while parts.len() > 1 && status.iter().map(Span::width).sum::<usize>() > budget {
        if let Some(i) = (0..parts.len()).min_by_key(|&i| parts[i].keep) {
            parts.remove(i);
        }
        status = header_status(&parts);
    }
    let status = widgets::cut_spans(status, budget);
    frame.render_widget(Paragraph::new(Line::from(name)), area);
    frame.render_widget(Paragraph::new(Line::from(status).right_aligned()), area);
}

/// How the tab bar is drawn (see the UI conventions in `theme`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TabBar {
    /// Three rows of boxes: `short` labels (`1 W`) and `pad` spaces inside the borders.
    Boxed { short: bool, pad: u16 },
    /// One row of labels (short terminals, or too narrow for boxes).
    Row,
}

impl TabBar {
    fn height(self) -> u16 {
        match self {
            TabBar::Boxed { .. } => 3,
            TabBar::Row => 1,
        }
    }
}

/// Box levels, roomiest first.
const TAB_BOXES: [TabBar; 4] = [
    TabBar::Boxed {
        short: false,
        pad: 1,
    },
    TabBar::Boxed {
        short: false,
        pad: 0,
    },
    TabBar::Boxed {
        short: true,
        pad: 1,
    },
    TabBar::Boxed {
        short: true,
        pad: 0,
    },
];

/// The label of `tab`: `1 Work` (`1 W` when `short`) in `style`, then its badge (pending
/// Inbox count, live `●` for Sessions, `!` for a todo file error).
fn tab_label(app: &App, tab: Tab, short: bool, style: Style) -> Vec<Span<'static>> {
    let n = tab.index() + 1;
    let name = if short {
        format!("{n} {}", &tab.title()[..1])
    } else {
        format!("{n} {}", tab.title())
    };
    let mut spans = vec![Span::styled(name, style)];
    let badge = match tab {
        Tab::Inbox if app.pending_count() > 0 => Some(Span::styled(
            app.pending_count().to_string(),
            style.fg(theme::ACCENT),
        )),
        Tab::Sessions if app.any_session_live() => Some(Span::styled("●", style.fg(theme::LIVE))),
        Tab::Todos if app.todo_error.is_some() => Some(Span::styled(
            "!",
            style.fg(theme::DANGER).add_modifier(Modifier::BOLD),
        )),
        _ => None,
    };
    if let Some(badge) = badge {
        spans.push(Span::styled(" ", style));
        spans.push(badge);
    }
    spans
}

fn spans_width(spans: &[Span]) -> usize {
    spans.iter().map(Span::width).sum()
}

/// Width of the boxed tab bar at a box level: a margin column, then each box (borders, pad,
/// label) with one column between boxes.
fn boxed_width(app: &App, short: bool, pad: u16) -> usize {
    let boxes: usize = Tab::ALL
        .into_iter()
        .map(|t| spans_width(&tab_label(app, t, short, Style::new())) + 2 + 2 * pad as usize)
        .sum();
    1 + boxes + Tab::ALL.len() - 1
}

/// Boxed tabs when the frame is roomy and a box level fits `width`, else one row.
pub(crate) fn tab_bar_kind(app: &App, width: u16, roomy: bool) -> TabBar {
    if !roomy {
        return TabBar::Row;
    }
    TAB_BOXES
        .into_iter()
        .find(|level| match *level {
            TabBar::Boxed { short, pad } => boxed_width(app, short, pad) <= width as usize,
            TabBar::Row => true,
        })
        .unwrap_or(TabBar::Row)
}

fn render_tabs(frame: &mut Frame, app: &App, area: Rect, kind: TabBar) {
    match kind {
        TabBar::Boxed { short, pad } => render_tab_boxes(frame, app, area, short, pad),
        TabBar::Row => render_tab_row(frame, app, area),
    }
}

/// Boxed tabs (notebook style): rounded boxes in a row over a muted line that runs across
/// the full width; the active box is open at the bottom into that line, its border accent
/// and its label accent bold. Inactive boxes have muted borders and soft labels. Each box
/// (all three rows) is a click target.
fn render_tab_boxes(frame: &mut Frame, app: &App, area: Rect, short: bool, pad: u16) {
    let pad_s = " ".repeat(pad as usize);
    let rule = theme::muted();
    let mut top = vec![Span::raw(" ")];
    let mut mid = vec![Span::raw(" ")];
    let mut base = vec![Span::styled("─", rule)];
    let mut hits = Vec::new();
    let mut x = area.x + 1;
    for (i, tab) in Tab::ALL.into_iter().enumerate() {
        let active = tab == app.tab;
        let (border, label_style) = if active {
            (theme::accent(), theme::active_tab())
        } else {
            (theme::muted(), theme::soft())
        };
        if i > 0 {
            top.push(Span::raw(" "));
            mid.push(Span::raw(" "));
            base.push(Span::styled("─", rule));
            x += 1;
        }
        let label = tab_label(app, tab, short, label_style);
        let inner = spans_width(&label) + 2 * pad as usize;
        top.push(Span::styled(format!("╭{}╮", "─".repeat(inner)), border));
        mid.push(Span::styled("│", border));
        mid.push(Span::raw(pad_s.clone()));
        mid.extend(label);
        mid.push(Span::raw(pad_s.clone()));
        mid.push(Span::styled("│", border));
        if active {
            base.push(Span::styled("╯", border));
            base.push(Span::raw(" ".repeat(inner)));
            base.push(Span::styled("╰", border));
        } else {
            base.push(Span::styled(format!("┴{}┴", "─".repeat(inner)), rule));
        }
        let w = inner as u16 + 2;
        hits.push((Rect::new(x, area.y, w, 3), BarHit::Tab(tab)));
        x += w;
    }
    let used = spans_width(&base);
    base.push(Span::styled(
        "─".repeat((area.width as usize).saturating_sub(used)),
        rule,
    ));
    let lines = [top, mid, base]
        .into_iter()
        .map(|spans| Line::from(widgets::cut_spans(spans, area.width as usize)))
        .collect::<Vec<_>>();
    frame.render_widget(Paragraph::new(lines), area);
    hits.retain(|(r, _)| r.right() <= area.right());
    app.feedback.bar_hits.borrow_mut().extend(hits);
}

/// One-row tab bar (short terminals): ` 1 Work   2 Todos ` with the active label in the
/// active-label style (accent, bold, underlined); short labels (`1 W`) when narrow.
fn render_tab_row(frame: &mut Frame, app: &App, area: Rect) {
    let build = |short: bool| {
        let mut spans = vec![Span::raw(" ")];
        let mut hits = Vec::new();
        let mut x = area.x + 1;
        for tab in Tab::ALL {
            let style = if tab == app.tab {
                theme::active_label()
            } else {
                theme::soft()
            };
            let label = tab_label(app, tab, short, style);
            let w = spans_width(&label) as u16 + 2;
            hits.push((Rect::new(x, area.y, w, 1), BarHit::Tab(tab)));
            spans.push(Span::raw(" "));
            spans.extend(label);
            spans.push(Span::raw("  "));
            x += w + 1;
        }
        (spans, hits)
    };
    let (mut spans, mut hits) = build(false);
    if spans_width(&spans) > area.width as usize {
        (spans, hits) = build(true);
    }
    let spans = widgets::cut_spans(spans, area.width as usize);
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
    hits.retain(|(r, _)| r.right() <= area.right());
    app.feedback.bar_hits.borrow_mut().extend(hits);
}

fn footer_hints(app: &App) -> Vec<Vec<Span<'static>>> {
    // The New todo dialog shows its own keys.
    if matches!(app.overlay, Some(Overlay::NewTodo(_))) {
        return Vec::new();
    }
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
    if app.tab != Tab::Inbox && matches!(app.selected_row(app.tab), Some(ListRow::Group(_))) {
        let mut g = vec![
            key_hint("enter/space", "toggle"),
            key_hint("z", "fold"),
            key_hint("Z", "fold all"),
        ];
        g.extend(match app.tab {
            Tab::Work => vec![key_hint("f", "filter"), key_hint("/", "search")],
            Tab::Todos => vec![key_hint("h/l", "filter"), key_hint("a", "add")],
            Tab::Summary => vec![key_hint("h/l", "section"), key_hint("[ ]", "window")],
            _ => vec![key_hint("/", "search")],
        });
        return g;
    }
    match app.tab {
        Tab::Work => {
            let mut g: Vec<_> = list_only
                .then(|| key_hint("enter", "details"))
                .into_iter()
                .collect();
            g.extend([
                key_hint("o", "PR"),
                key_hint("c", "resume"),
                key_hint("t", "new todo"),
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
            key_hint("t", "new todo"),
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
            if app.input.is_some() || matches!(app.overlay, Some(Overlay::NewTodo(_))) {
                Vec::new()
            } else {
                vec![key_hint("?", "help"), key_hint("q", "quit")]
            },
            hints_row.width,
        )),
        hints_row,
    );
}
