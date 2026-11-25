//! Reusable drawing helpers: scrolled lists and panes, scrollbars, modals, the one-line
//! input, key hints, and text/time formatting. Patterns adapted from LGBC's `ui.rs`.

use std::cell::Cell;

use chrono::{Datelike, TimeZone};
use ratatui::layout::{Margin, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Borders, Clear, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, Wrap,
};
use ratatui::Frame;

use super::theme;
use crate::app::{BarHit, LineInput, ListRow, Now};
use crate::lgbc::responsive::{str_width, truncate_end, truncate_middle, Scroll};

/// Sanitized single-line external text (spec 11.3).
pub fn clean(s: &str) -> String {
    crate::sanitize::clean_line(s)
}

/// `s` cut to `width` columns (with `…`) and padded with spaces to exactly `width`.
pub fn fit(s: &str, width: usize) -> String {
    let cut = truncate_end(s, width);
    let pad = width.saturating_sub(str_width(&cut));
    format!("{cut}{}", " ".repeat(pad))
}

/// `s` right-aligned in `width` columns.
pub fn fit_right(s: &str, width: usize) -> String {
    let cut = truncate_end(s, width);
    let pad = width.saturating_sub(str_width(&cut));
    format!("{}{cut}", " ".repeat(pad))
}

/// Compact age: `now`, `5m`, `2h`, `16d`.
pub fn fmt_age(now: i64, t: Option<i64>) -> String {
    let Some(t) = t else {
        return "—".to_string();
    };
    let d = (now - t).max(0);
    if d < 60 {
        "now".into()
    } else if d < 3600 {
        format!("{}m", d / 60)
    } else if d < 86_400 {
        format!("{}h", d / 3600)
    } else {
        format!("{}d", d / 86_400)
    }
}

/// `HH:MM` today, weekday within the last 6 days, `MM-DD` this year, else `YYYY-MM-DD`.
pub fn fmt_when(now: &Now, t: Option<i64>) -> String {
    let Some(t) = t else {
        return "—".to_string();
    };
    let Some(dt) = now.offset().timestamp_opt(t, 0).single() else {
        return "—".to_string();
    };
    let days = (now.date_naive() - dt.date_naive()).num_days();
    if days == 0 {
        dt.format("%H:%M").to_string()
    } else if (1..7).contains(&days) {
        dt.format("%a").to_string()
    } else if dt.year() == now.year() {
        dt.format("%m-%d").to_string()
    } else {
        dt.format("%Y-%m-%d").to_string()
    }
}

/// Like `fmt_when` but with the time added for past days: `Tue 09:12`.
pub fn fmt_when_long(now: &Now, t: Option<i64>) -> String {
    let Some(t) = t else {
        return "—".to_string();
    };
    let Some(dt) = now.offset().timestamp_opt(t, 0).single() else {
        return "—".to_string();
    };
    if dt.date_naive() == now.date_naive() {
        dt.format("%H:%M").to_string()
    } else {
        dt.format("%a %Y-%m-%d %H:%M").to_string()
    }
}

/// `key label` group of a hint line.
pub fn key_hint(key: &str, label: &str) -> Vec<Span<'static>> {
    vec![
        Span::styled(key.to_string(), theme::key()),
        Span::styled(format!(" {label}"), theme::muted()),
    ]
}

/// Hint groups on one line, cut with `…` at the first group that does not fit.
pub fn hint_line(groups: Vec<Vec<Span<'static>>>, width: u16) -> Line<'static> {
    let width = width as usize;
    let sep = " · ";
    let mut spans: Vec<Span<'static>> = vec![Span::raw(" ")];
    let mut used = 1;
    let total = groups.len();
    for (i, group) in groups.into_iter().enumerate() {
        let w: usize = group.iter().map(Span::width).sum();
        let sep_w = if i == 0 { 0 } else { str_width(sep) };
        let room_for_more = if i + 1 < total { 2 } else { 0 };
        if used + sep_w + w + room_for_more > width {
            spans.push(Span::styled(
                if i == 0 { "…" } else { " …" },
                theme::muted(),
            ));
            break;
        }
        if i > 0 {
            spans.push(Span::styled(sep, theme::muted()));
        }
        spans.extend(group);
        used += sep_w + w;
    }
    Line::from(spans)
}

/// Like [`hint_line`], but the `pinned` groups (`? help`, `q quit`) always stay at the end:
/// view groups that do not fit are dropped first.
pub fn hint_line_pinned(
    groups: Vec<Vec<Span<'static>>>,
    pinned: Vec<Vec<Span<'static>>>,
    width: u16,
) -> Line<'static> {
    let group_w = |g: &Vec<Span<'static>>| g.iter().map(Span::width).sum::<usize>();
    let sep_w = 3;
    let pinned_w: usize = pinned.iter().map(|g| group_w(g) + sep_w).sum();
    let mut budget = (width as usize).saturating_sub(1 + pinned_w);
    let mut kept = Vec::new();
    for g in groups {
        let w = group_w(&g) + sep_w;
        if w > budget {
            break;
        }
        budget -= w;
        kept.push(g);
    }
    kept.extend(pinned);
    hint_line(kept, width)
}

/// Bordered pane block with a title.
pub fn pane_block(title: Line<'static>) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_style(theme::muted())
        .title(title)
}

/// Vertical scrollbar on the right edge of `area` (a block's right border).
pub fn render_scrollbar(frame: &mut Frame, area: Rect, scroll: Scroll) {
    let mut state = ScrollbarState::new(scroll.positions())
        .position(scroll.offset)
        .viewport_content_length(scroll.viewport);
    let bar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
        .thumb_style(theme::accent())
        .track_style(theme::muted())
        .begin_style(theme::muted())
        .end_style(theme::muted());
    frame.render_stateful_widget(bar, area, &mut state);
}

/// The cursor row (see the UI conventions in `theme`): the blank gutter column becomes an
/// accent `▌`, every span keeps its foreground on a subtle background (muted text lifted to
/// gray, uncoloured text bold and bright), padded to `width`.
pub fn cursor_line(line: Line<'static>, width: usize) -> Line<'static> {
    let Line {
        mut spans,
        style,
        alignment,
    } = line;
    if let Some(first) = spans.first_mut() {
        if let Some(rest) = first.content.strip_prefix(' ') {
            first.content = rest.to_string().into();
        }
    }
    let mut out = vec![Span::styled(theme::CURSOR_MARK, theme::cursor_mark())];
    for s in spans {
        if !s.content.is_empty() {
            out.push(Span::styled(
                s.content,
                theme::on_cursor(style.patch(s.style)),
            ));
        }
    }
    let used: usize = out.iter().map(Span::width).sum();
    if used < width {
        out.push(Span::styled(
            " ".repeat(width - used),
            Style::new().bg(theme::CURSOR_BG),
        ));
    }
    Line {
        spans: out,
        style: Style::new(),
        alignment,
    }
}

/// Lists get a blank row before each group header (but the first) only when their
/// viewport has at least this many rows; shorter panes keep every row for content.
pub const GROUP_GAP_MIN_ROWS: usize = 8;

/// Whether a list pane `viewport` rows tall spaces its groups apart.
pub fn group_gap(viewport: usize) -> bool {
    viewport >= GROUP_GAP_MIN_ROWS
}

/// Blank spacing rows `rows` add before their group headers (one per header but the first).
pub fn gap_rows(rows: &[ListRow]) -> usize {
    rows.iter()
        .filter(|r| matches!(r, ListRow::Group(_)))
        .count()
        .saturating_sub(1)
}

/// How a group name is shortened when it does not fit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cut {
    /// Names: cut at the end with `…`.
    End,
    /// Paths: keep both ends, `~/work/…/retry`.
    Middle,
}

/// The group header row shared by every grouped list (see the UI conventions in `theme`):
/// gutter, disclosure marker (`▾` open, `▸` collapsed) in the accent colour, the name bold
/// in the accent colour, a muted thin rule, and the muted member count flush right. The
/// name is cut (`cut`) so the count always shows; the rule disappears first.
pub fn group_header(
    name: &str,
    count: usize,
    collapsed: bool,
    width: usize,
    cut: Cut,
) -> Line<'static> {
    let marker = if collapsed { "▸" } else { "▾" };
    let right = format!(" {count} ");
    // " ▾ " + name + " " + rule + right
    let fixed = 3 + str_width(&right);
    let budget = width.saturating_sub(fixed + 1).max(1);
    let name = match cut {
        Cut::End => truncate_end(name, budget),
        Cut::Middle => truncate_middle(name, budget),
    };
    let rule = width.saturating_sub(fixed + str_width(&name) + 1);
    let mut spans = vec![
        Span::raw(" "),
        Span::styled(marker, theme::accent()),
        Span::raw(" "),
        Span::styled(name, theme::heading()),
    ];
    if rule > 0 {
        spans.push(Span::raw(" "));
        spans.push(Span::styled("─".repeat(rule), theme::muted()));
    }
    spans.push(Span::styled(right, theme::muted()));
    cut_spans(spans, width).into()
}

/// Title line and folding hint of a group's details pane, around the group's `fields`.
pub fn group_details(
    title: &str,
    collapsed: bool,
    fields: Vec<Line<'static>>,
) -> Vec<Line<'static>> {
    let mut lines = vec![Line::from(vec![
        Span::styled(if collapsed { "▸ " } else { "▾ " }, theme::accent()),
        Span::styled(title.to_string(), theme::bold()),
    ])];
    lines.extend(fields);
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        if collapsed {
            "enter/space expands this group · Z folds / unfolds all"
        } else {
            "enter/space or z collapses this group · Z folds / unfolds all"
        },
        theme::muted(),
    )));
    lines
}

/// `n label` breakdown rows (`3 todo`, `1 doing`) as details fields: the first under
/// `label`, the rest continuing the value column. Zero counts are left out.
pub fn breakdown(label: &str, parts: Vec<(Vec<Span<'static>>, usize)>) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    for (spans, n) in parts.into_iter().filter(|(_, n)| *n > 0) {
        let mut v = vec![Span::raw(format!("{n} "))];
        v.extend(spans);
        out.push(if out.is_empty() {
            field(label, v)
        } else {
            field_cont(v)
        });
    }
    out
}

/// One row of a list as drawn: its lines (an item may take several) and whether it is a
/// group header.
pub struct ListBlock {
    pub header: bool,
    pub lines: Vec<Line<'static>>,
}

/// Draw list `blocks` (one per [`ListRow`], `cursor` indexing them) in `rows`: a blank row
/// before each group header but the first when the viewport is tall enough
/// ([`group_gap`]), the cursor block's first line kept visible (see [`render_rows`]).
pub fn render_list(
    frame: &mut Frame,
    rows: Rect,
    bar_area: Rect,
    blocks: Vec<ListBlock>,
    cursor: Option<usize>,
    offset: &Cell<usize>,
) {
    let gap = group_gap(rows.height as usize);
    let mut lines = Vec::new();
    let mut cursor_line = None;
    let mut seen_header = false;
    for (n, block) in blocks.into_iter().enumerate() {
        if block.header {
            if gap && seen_header {
                lines.push(Line::from(""));
            }
            seen_header = true;
        }
        if Some(n) == cursor {
            cursor_line = Some(lines.len());
        }
        lines.extend(block.lines);
    }
    render_rows(frame, rows, bar_area, lines, cursor_line, offset);
}

/// Draw list `lines` in `rows`, keeping row `cursor` visible. The offset persists in
/// `offset` between frames. A scrollbar goes on `bar_area`'s right edge when the list
/// overflows.
pub fn render_rows(
    frame: &mut Frame,
    rows: Rect,
    bar_area: Rect,
    mut lines: Vec<Line<'static>>,
    cursor: Option<usize>,
    offset: &Cell<usize>,
) {
    let viewport = rows.height as usize;
    let total = lines.len();
    let mut off = offset.get();
    if let Some(c) = cursor {
        if c < off {
            off = c;
        } else if viewport > 0 && c >= off + viewport {
            off = c + 1 - viewport;
        }
    }
    let scroll = Scroll::clamped(off, viewport, total);
    offset.set(scroll.offset);
    if let Some(c) = cursor {
        if let Some(line) = lines.get_mut(c) {
            *line = cursor_line(std::mem::take(line), rows.width as usize);
        }
    }
    let visible: Vec<Line> = lines
        .into_iter()
        .skip(scroll.offset)
        .take(viewport)
        .collect();
    frame.render_widget(Paragraph::new(visible), rows);
    if scroll.overflows() {
        render_scrollbar(
            frame,
            bar_area.inner(Margin {
                horizontal: 0,
                vertical: 1,
            }),
            scroll,
        );
    }
}

/// Draw `lines` wrapped inside `block`, scrolled by `offset` (clamped). Scrollbar and
/// `hint` on the border when it overflows. Returns the scroll shown.
pub fn render_scrolled(
    frame: &mut Frame,
    block: Block<'static>,
    lines: Vec<Line<'static>>,
    area: Rect,
    offset: usize,
    hint: Option<Line<'static>>,
) -> Scroll {
    let inner = block.inner(area);
    let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });
    let total = paragraph.line_count(inner.width);
    let scroll = Scroll::clamped(offset, inner.height as usize, total);
    let block = match hint {
        Some(hint) if scroll.overflows() => block.title_bottom(hint.right_aligned()),
        _ => block,
    };
    frame.render_widget(block, area);
    frame.render_widget(
        paragraph.scroll((scroll.offset.min(u16::MAX as usize) as u16, 0)),
        inner,
    );
    if scroll.overflows() {
        render_scrollbar(
            frame,
            area.inner(Margin {
                horizontal: 0,
                vertical: 1,
            }),
            scroll,
        );
    }
    scroll
}

/// `J/K scroll` hint for a details border.
pub fn scroll_hint(keys: &str) -> Line<'static> {
    Line::from(vec![
        Span::raw(" "),
        Span::styled(keys.to_string(), theme::key()),
        Span::styled(" scroll ", theme::muted()),
    ])
}

/// One-line input: `prompt` then the text, scrolled so the cursor stays visible. The
/// cursor cell is drawn reversed and the terminal cursor is placed there.
pub fn render_input(frame: &mut Frame, area: Rect, prompt: &str, input: &LineInput) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let prompt_w = str_width(prompt);
    let avail = (area.width as usize).saturating_sub(prompt_w + 1).max(1);
    let chars: Vec<char> = input.text.chars().collect();
    // Scroll the text so the cursor fits.
    let mut start = 0;
    while str_width(
        &chars[start..input.cursor.min(chars.len())]
            .iter()
            .collect::<String>(),
    ) >= avail
    {
        start += 1;
    }
    let mut before: String = chars[start..input.cursor.min(chars.len())].iter().collect();
    let mut at: String = chars
        .get(input.cursor)
        .map_or(" ".to_string(), |c| c.to_string());
    let after: String = clean(&chars.iter().skip(input.cursor + 1).collect::<String>());
    before = clean(&before);
    if at.chars().all(char::is_control) {
        at = " ".to_string();
    }
    let line = Line::from(vec![
        Span::styled(prompt.to_string(), theme::key()),
        Span::raw(before.clone()),
        // The caret cell: reversed like a terminal block cursor.
        Span::styled(at, Style::new().add_modifier(Modifier::REVERSED)),
        Span::raw(after),
    ]);
    frame.render_widget(Paragraph::new(line), area);
    let x = area.x + (prompt_w + str_width(&before)).min(area.width as usize - 1) as u16;
    frame.set_cursor_position(Position::new(x, area.y));
}

/// Bottom-border hint of a modal that does not fit.
const MODAL_SCROLL_HINT: &str = " ↑↓ scroll · other keys close ";

/// Centered modal at most `max_width` wide and as tall as its lines need (capped at the
/// screen), scrolled by `offset`. `footer` stays pinned below. Returns the scroll shown.
#[allow(clippy::too_many_arguments)]
pub fn render_modal(
    frame: &mut Frame,
    title: &str,
    color: Color,
    max_width: u16,
    lines: Vec<Line<'static>>,
    footer: Option<Line<'static>>,
    offset: usize,
) -> Scroll {
    let area = frame.area();
    let width = max_width.min(area.width.saturating_sub(4));
    let content_rows = Paragraph::new(lines.clone())
        .wrap(Wrap { trim: false })
        .line_count(width.saturating_sub(2));
    let footer_rows: u16 = if footer.is_some() { 2 } else { 0 };
    let height = u16::try_from(content_rows)
        .unwrap_or(u16::MAX)
        .saturating_add(footer_rows + 2)
        .min(area.height.saturating_sub(2));
    let modal = Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    };
    frame.render_widget(Clear, modal);
    let border = Style::new().fg(color);
    let title = Span::styled(
        format!(" {title} "),
        border.add_modifier(ratatui::style::Modifier::BOLD),
    );
    let viewport = height.saturating_sub(2 + footer_rows) as usize;
    let mut outer = Block::bordered().border_style(border).title(title.clone());
    if content_rows > viewport {
        outer = outer.title_bottom(
            Line::from(Span::styled(MODAL_SCROLL_HINT, theme::muted())).right_aligned(),
        );
    }
    frame.render_widget(outer, modal);
    let body_area = Rect {
        height: height.saturating_sub(1 + footer_rows),
        ..modal
    };
    let body = Block::new()
        .borders(Borders::LEFT | Borders::TOP | Borders::RIGHT)
        .border_style(border)
        .title(title);
    let scroll = render_scrolled(frame, body, lines, body_area, offset, None);
    if let Some(footer) = footer {
        let footer_area = Rect {
            x: modal.x + 1,
            y: body_area.bottom(),
            width: width.saturating_sub(2),
            height: modal
                .bottom()
                .saturating_sub(1)
                .saturating_sub(body_area.bottom()),
        };
        frame.render_widget(Paragraph::new(vec![Line::from(""), footer]), footer_area);
    }
    scroll
}

/// Width of the label column of details rows (`Commits  3 ahead`).
pub const LABEL_W: usize = 9;

/// `Label    value` details row with a fixed-width muted label.
pub fn field(label: &str, value: Vec<Span<'static>>) -> Line<'static> {
    let mut spans = vec![Span::styled(fit(label, LABEL_W), theme::muted())];
    spans.extend(value);
    Line::from(spans)
}

/// A details row continuing the value column (no label).
pub fn field_cont(value: Vec<Span<'static>>) -> Line<'static> {
    let mut spans = vec![Span::raw(" ".repeat(LABEL_W))];
    spans.extend(value);
    Line::from(spans)
}

/// Section heading inside a details pane.
pub fn section(title: &str) -> Line<'static> {
    Line::from(Span::styled(title.to_string(), theme::heading()))
}

/// One blank line, then a section heading.
pub fn push_section(lines: &mut Vec<Line<'static>>, title: &str) {
    lines.push(Line::from(""));
    lines.push(section(title));
}

fn is_blank(line: &Line) -> bool {
    line.spans.iter().all(|s| s.content.trim().is_empty())
}

/// Columns a wrapped continuation of `line` is indented by: the value column for field
/// rows, the text after a bullet, else the line's own indentation.
fn hang_of(line: &Line) -> usize {
    if let Some(first) = line.spans.first() {
        if line.spans.len() > 1
            && first.style == theme::muted()
            && first.width() == LABEL_W
            && first.content.ends_with(' ')
        {
            return LABEL_W;
        }
    }
    let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
    let lead = text.chars().take_while(|c| *c == ' ').count();
    let rest = &text[lead..];
    for marker in ["• ", "- "] {
        if rest.starts_with(marker) {
            return lead + str_width(marker);
        }
    }
    lead
}

/// `line` wrapped to `width` at spaces (hard breaks for long words), continuation rows
/// indented by [`hang_of`].
fn wrap_hanging(line: Line<'static>, width: usize) -> Vec<Line<'static>> {
    use unicode_width::UnicodeWidthChar;
    if width == 0 || line.width() <= width {
        return vec![line];
    }
    let mut hang = hang_of(&line);
    if hang * 2 > width {
        hang = 0;
    }
    let cells: Vec<(char, Style)> = line
        .spans
        .iter()
        .flat_map(|s| {
            let st = line.style.patch(s.style);
            s.content.chars().map(move |c| (c, st))
        })
        .collect();
    let cw = |c: char| c.width().unwrap_or(0);
    let mut rows: Vec<Vec<(char, Style)>> = Vec::new();
    let mut i = 0;
    while i < cells.len() {
        let first = rows.is_empty();
        if !first {
            while i < cells.len() && cells[i].0 == ' ' {
                i += 1;
            }
            if i >= cells.len() {
                break;
            }
        }
        let cap = if first { width } else { width - hang };
        let mut used = 0;
        let mut j = i;
        while j < cells.len() && used + cw(cells[j].0) <= cap {
            used += cw(cells[j].0);
            j += 1;
        }
        if j < cells.len() {
            // Break at the last space of the row, but not inside the label / indent.
            let min = if first { i + hang + 1 } else { i + 1 };
            if let Some(sp) = (min..j).rev().find(|&k| cells[k].0 == ' ') {
                j = sp;
            }
            if j == i {
                j = i + 1; // a cell wider than the row
            }
        }
        rows.push(cells[i..j].to_vec());
        i = j;
    }
    rows.into_iter()
        .enumerate()
        .map(|(n, row)| {
            let mut spans: Vec<Span<'static>> = Vec::new();
            if n > 0 && hang > 0 {
                spans.push(Span::raw(" ".repeat(hang)));
            }
            let mut buf = String::new();
            let mut cur: Option<Style> = None;
            for (c, st) in row {
                if let Some(prev) = cur.filter(|x| *x != st) {
                    spans.push(Span::styled(std::mem::take(&mut buf), prev));
                }
                cur = Some(st);
                buf.push(c);
            }
            if let Some(st) = cur {
                spans.push(Span::styled(buf, st));
            }
            Line::from(spans)
        })
        .collect()
}

/// Details pane lines made consistent: no leading, trailing or doubled blank lines, and
/// rows wider than `width` wrapped with a hanging indent.
pub fn tidy_details(lines: Vec<Line<'static>>, width: usize) -> Vec<Line<'static>> {
    let mut out: Vec<Line<'static>> = Vec::new();
    for line in lines {
        if is_blank(&line) {
            if out.last().is_some_and(|l| !is_blank(l)) {
                out.push(Line::from(""));
            }
            continue;
        }
        out.extend(wrap_hanging(line, width));
    }
    while out.last().is_some_and(is_blank) {
        out.pop();
    }
    out
}

/// `spans` cut to `width` columns; the span that overflows is cut with `…`.
pub fn cut_spans(spans: Vec<Span<'static>>, width: usize) -> Vec<Span<'static>> {
    let mut used = 0;
    let mut out = Vec::new();
    for s in spans {
        let w = s.width();
        if used + w > width {
            let rest = width - used;
            if rest > 0 {
                out.push(Span::styled(truncate_end(&s.content, rest), s.style));
            }
            break;
        }
        used += w;
        out.push(s);
    }
    out
}

/// One label of a filter / section bar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BarItem {
    pub label: &'static str,
    /// Shorter label used when the full bar does not fit.
    pub short: &'static str,
    pub count: usize,
    pub active: bool,
    pub hit: BarHit,
}

/// ` text ` with the text in the active-label style (bold + underline survive `NO_COLOR`).
fn bar_active(text: String) -> Vec<Span<'static>> {
    vec![
        Span::raw(" "),
        Span::styled(text, theme::active_label()),
        Span::raw(" "),
    ]
}

fn bar_style(item: &BarItem) -> Style {
    if item.count == 0 {
        theme::muted()
    } else {
        theme::soft()
    }
}

/// One-row filter / section bar: ` Done 1 · In progress 2 · Todo 5 `, the active label
/// accent, bold and underlined. Falls back to short labels, then to `Prev ‹ Active N › Next`
/// (neighbours only when they fit), so it never overflows `area`. Returns the line and the
/// screen area of each label for mouse clicks.
pub fn filter_bar(items: &[BarItem], area: Rect) -> (Line<'static>, Vec<(Rect, BarHit)>) {
    let width = area.width as usize;
    let hit_rect = |x: usize, w: usize| Rect::new(area.x + x as u16, area.y, w as u16, 1);
    for short in [false, true] {
        let mut spans = Vec::new();
        let mut hits = Vec::new();
        let mut x = 0;
        for (i, it) in items.iter().enumerate() {
            if i > 0 {
                spans.push(Span::styled("·", theme::muted()));
                x += 1;
            }
            let text = format!("{} {}", if short { it.short } else { it.label }, it.count);
            let w = str_width(&text) + 2;
            if it.active {
                spans.extend(bar_active(text));
            } else {
                spans.push(Span::styled(format!(" {text} "), bar_style(it)));
            }
            hits.push((x, w, it.hit));
            x += w;
        }
        if x <= width {
            let hits = hits
                .into_iter()
                .map(|(x, w, h)| (hit_rect(x, w), h))
                .collect();
            return (Line::from(spans), hits);
        }
    }
    // Only the active one, with the arrows standing for its neighbours.
    let Some(pos) = items.iter().position(|it| it.active) else {
        return (Line::default(), Vec::new());
    };
    let n = items.len();
    let prev = &items[(pos + n - 1) % n];
    let next = &items[(pos + 1) % n];
    let cur = &items[pos];
    let mut text = format!("{} {}", cur.label, cur.count);
    if str_width(&text) + 6 > width {
        text = format!("{} {}", cur.short, cur.count);
    }
    let core_w = str_width(&text) + 6; // " ‹ … › "
    let prev_text = format!("{} {}", prev.short, prev.count);
    let next_text = format!("{} {}", next.short, next.count);
    let with_neighbours = str_width(&prev_text) + core_w + str_width(&next_text) + 2 <= width;
    let mut spans = Vec::new();
    let mut hits = Vec::new();
    let mut x = 0;
    if with_neighbours {
        let w = str_width(&prev_text) + 1;
        spans.push(Span::styled(format!(" {prev_text}"), theme::muted()));
        hits.push((hit_rect(x, w), prev.hit));
        x += w;
    }
    spans.push(Span::styled(" ‹", theme::key()));
    hits.push((hit_rect(x, 2), prev.hit));
    x += 2;
    let w = str_width(&text) + 2;
    spans.extend(bar_active(text));
    hits.push((hit_rect(x, w), cur.hit));
    x += w;
    spans.push(Span::styled("› ", theme::key()));
    hits.push((hit_rect(x, 2), next.hit));
    x += 2;
    if with_neighbours {
        let w = str_width(&next_text) + 1;
        spans.push(Span::styled(format!("{next_text} "), theme::muted()));
        hits.push((hit_rect(x, w), next.hit));
    }
    let line_w: usize = spans.iter().map(Span::width).sum();
    if line_w > width {
        spans = cut_spans(spans, width);
        hits.retain(|(r, _)| r.right() <= area.right());
    }
    (Line::from(spans), hits)
}

/// `PROJECT/repo:branch` → `repo:branch`.
pub fn short_link(link: &str) -> String {
    match link.split_once(':') {
        Some((repo, branch)) => {
            let repo = repo.rsplit_once('/').map_or(repo, |(_, r)| r);
            format!("{repo}:{branch}")
        }
        None => link.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_pads_and_cuts() {
        assert_eq!(fit("abc", 5), "abc  ");
        assert_eq!(fit("abcdef", 4), "abc…");
        assert_eq!(fit_right("ab", 4), "  ab");
    }

    #[test]
    fn ages_are_compact() {
        assert_eq!(fmt_age(1000, Some(990)), "now");
        assert_eq!(fmt_age(10_000, Some(10_000 - 120)), "2m");
        assert_eq!(fmt_age(100_000, Some(100_000 - 7200)), "2h");
        assert_eq!(fmt_age(10_000_000, Some(10_000_000 - 16 * 86_400)), "16d");
        assert_eq!(fmt_age(0, None), "—");
    }

    #[test]
    fn short_link_drops_the_project() {
        assert_eq!(
            short_link("API/upload-svc:feature/x"),
            "upload-svc:feature/x"
        );
        assert_eq!(short_link("plain"), "plain");
    }

    #[test]
    fn hint_line_cuts_with_an_ellipsis() {
        let groups = vec![key_hint("j/k", "move"), key_hint("q", "quit")];
        let full: String = hint_line(groups.clone(), 40)
            .spans
            .iter()
            .map(|s| s.content.to_string())
            .collect();
        assert_eq!(full, " j/k move · q quit");
        let cut: String = hint_line(groups, 12)
            .spans
            .iter()
            .map(|s| s.content.to_string())
            .collect();
        assert!(cut.ends_with('…'), "{cut}");
        assert!(str_width(&cut) <= 12);
    }
}
