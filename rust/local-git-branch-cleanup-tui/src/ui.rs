// TUI rendering module

use crate::app::{App, FilterMode, SortMode, ViewMode};
use crate::git::BranchStatus;
use crate::pr::PrState;
use crate::responsive::{self, LayoutMode, Scroll};
use crate::worktree_view::{short_sha, Changes, WorktreeFilter, WorktreeState};
use ratatui::{
    layout::{Alignment, Constraint, Direction, Flex, Layout, Margin, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, Borders, Cell, Clear, Paragraph, Row, Scrollbar, ScrollbarOrientation,
        ScrollbarState, Table, TableState, Wrap,
    },
    Frame,
};

// Color palette
const COLOR_ACCENT: Color = Color::Rgb(46, 196, 182); // #2EC4B6 - cyan
const COLOR_WARNING: Color = Color::Rgb(255, 184, 108); // #FFB86C - amber
const COLOR_DIVERGED: Color = Color::Rgb(241, 250, 140); // #F1FA8C - yellow, between pr-merged cyan and unmerged amber
const COLOR_LOCAL: Color = Color::Rgb(98, 114, 164); // #6272A4 - grey-blue for never-pushed branches
const COLOR_DANGER: Color = Color::Rgb(255, 85, 85); // #FF5555 - red
const COLOR_MUTED: Color = Color::Rgb(169, 177, 214); // #A9B1D6 - muted text
const COLOR_SUCCESS: Color = Color::Rgb(80, 250, 123); // #50FA7B - green
const COLOR_CURRENT: Color = Color::Rgb(189, 147, 249); // #BD93F9 - purple
const COLOR_SELECTED: Color = Color::Rgb(255, 121, 198); // #FF79C6 - pink for selected
                                                         // PR state colors
const COLOR_PR_OPEN: Color = Color::Rgb(255, 203, 107); // Yellow/amber for open PRs
const COLOR_PR_MERGED: Color = Color::Rgb(80, 250, 123); // Green for merged PRs
const COLOR_PR_CLOSED: Color = Color::Rgb(255, 85, 85); // Red for closed PRs

/// Render the TUI. Every size decision is made here from `frame.area()`, so
/// a resized terminal is laid out afresh on the next frame
/// (docs/specs/RESPONSIVE_LAYOUT.md).
pub fn render(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    if responsive::is_too_small(area) {
        render_too_small(frame, area);
        return;
    }

    let compact = responsive::is_compact(area);
    // Rows and columns the chrome's borders take
    let border = if compact { 0 } else { 2 };
    let inner_width = area.width.saturating_sub(border);

    let has_log = !app.action_log.is_empty();
    let show_filter = app.show_filter;
    let show_search = app.search_active || !app.search_query.is_empty();

    let header_rows = header_lines(app, inner_width).len() as u16;
    let filter_rows = filter_tab_lines(app, inner_width).len() as u16;
    let footer_rows = footer_lines(app, inner_width, compact).len() as u16;
    let log_rows = app.action_log.len().min(if compact { 1 } else { 4 }) as u16;

    // Header, [Search], [Filters], Content, [Action Log], Footer
    let mut constraints = vec![Constraint::Length(header_rows + border)];
    if show_search {
        constraints.push(Constraint::Length(1 + border));
    }
    if show_filter {
        constraints.push(Constraint::Length(filter_rows + border));
    }
    constraints.push(Constraint::Min(5)); // Main content
    if has_log {
        constraints.push(Constraint::Length(log_rows + 2));
    }
    constraints.push(Constraint::Length(footer_rows + border));

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(area);

    let mut idx = 0;
    render_header(frame, app, chunks[idx], compact);
    idx += 1;

    let mut search_area = None;
    if show_search {
        render_search_box(frame, app, chunks[idx], compact);
        search_area = Some(chunks[idx]);
        idx += 1;
    }

    if show_filter {
        render_filter_tabs(frame, app, chunks[idx], compact);
        idx += 1;
    }

    let content = chunks[idx];
    idx += 1;

    let arrangement = responsive::arrange(app.layout_mode, content);
    let (list_area, details_area) = responsive::split_content(arrangement, content);
    let details_hidden = details_area.is_none();
    app.details_area = details_area;

    if app.view == ViewMode::Worktrees {
        render_worktree_list(frame, app, list_area, details_hidden, compact);
    } else {
        render_branch_list(frame, app, list_area, details_hidden, compact);
    }
    if let Some(details_area) = details_area {
        let scroll = app.details_scroll_for_cursor();
        app.details_scroll = if app.view == ViewMode::Worktrees {
            render_worktree_details(frame, app, details_area, scroll)
        } else {
            render_details_pane(frame, app, details_area, scroll)
        };
    }

    if has_log {
        render_action_log(frame, app, chunks[idx]);
        idx += 1;
    }
    render_footer(frame, app, chunks[idx], compact);

    // Render confirmation modal on top if shown
    if app.show_confirmation {
        render_confirmation_modal(frame, app);
    }

    // Render suggestions dropdown on top if shown (before help/info modals)
    if app.show_suggestions && !app.suggestions.is_empty() {
        if let Some(search_area) = search_area {
            render_suggestions_dropdown(frame, app, search_area);
        }
    }

    // Render help modal on top if shown
    if app.show_help {
        render_help_modal(frame, app);
    }

    // Render info modal on top if shown
    if app.show_info {
        render_info_modal(frame, app);
    }
}

/// Shown instead of the layout when the terminal is below the minimum size
fn render_too_small(frame: &mut Frame, area: Rect) {
    let lines = vec![
        Line::from(Span::styled(
            "Terminal too small",
            Style::default()
                .fg(COLOR_WARNING)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            format!(
                "{}×{}, need {}×{}",
                area.width,
                area.height,
                responsive::MIN_WIDTH,
                responsive::MIN_HEIGHT
            ),
            Style::default().fg(COLOR_MUTED),
        )),
        Line::from(Span::styled("q quits", Style::default().fg(COLOR_MUTED))),
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

/// Block of a chrome panel (header, search, filters, footer): bordered, or
/// bare when the terminal is short. A bare block has no title either, since
/// a title would take a row of its own.
fn chrome_block<'a>(compact: bool, title: Option<Line<'a>>, border_color: Color) -> Block<'a> {
    if compact {
        return Block::default();
    }
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color));
    match title {
        Some(title) => block.title(title),
        None => block,
    }
}

/// Styled `key label` group of the footer hints
fn key_hint(key: &'static str, label: &'static str, key_color: Color) -> Vec<Span<'static>> {
    vec![
        Span::styled(
            key,
            Style::default().fg(key_color).add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!(" {label}"), Style::default().fg(COLOR_MUTED)),
    ]
}

/// `icon label` group of a legend
fn legend_item(icon: &'static str, label: &'static str, color: Color) -> Vec<Span<'static>> {
    vec![
        Span::styled(icon, Style::default().fg(color)),
        Span::styled(format!(" {label}"), Style::default().fg(COLOR_MUTED)),
    ]
}

/// Header content, wrapped to `width` columns (at most 3 lines)
fn header_lines(app: &App, width: u16) -> Vec<Line<'static>> {
    let bold = |color: Color| Style::default().fg(color).add_modifier(Modifier::BOLD);
    // The path is shortened so that it and the trunk always fit on one line:
    // "📂 " + path + " │ " + "🌳 " + trunk
    let trunk_width = Line::from(app.trunk.as_str()).width();
    let path_budget = (width as usize)
        .saturating_sub(3 + 3 + 3 + trunk_width)
        .max(10);
    let path = responsive::truncate_middle(&app.repo_path, path_budget);

    let mut groups = vec![
        vec![
            Span::raw("🧹 "),
            Span::styled("Local Git Branch Cleanup", bold(COLOR_ACCENT)),
        ],
        vec![
            Span::raw("📂 "),
            Span::styled(path, Style::default().fg(COLOR_MUTED)),
        ],
        vec![
            Span::raw("🌳 "),
            Span::styled(app.trunk.clone(), Style::default().fg(COLOR_SUCCESS)),
        ],
    ];
    if app.view == ViewMode::Worktrees {
        groups.push(vec![Span::styled("🌳 Worktrees", bold(COLOR_ACCENT))]);
        if app.worktree_remove_mode {
            groups.push(vec![Span::styled("🗑 REMOVE", bold(COLOR_DANGER))]);
        }
    }
    if app.selected_count() > 0 {
        groups.push(vec![Span::styled(
            format!("📦 {} selected", app.selected_count()),
            Style::default().fg(COLOR_SELECTED),
        )]);
    }
    if app.force_mode {
        groups.push(vec![Span::styled(
            "⚠️ FORCE",
            Style::default().fg(COLOR_DANGER),
        )]);
    }
    if app.dry_run {
        groups.push(vec![Span::styled(
            "🔍 DRY RUN",
            Style::default().fg(COLOR_WARNING),
        )]);
    }
    // Sort indicator - show when not using default sort
    if app.sort_mode != SortMode::Status && app.view == ViewMode::Branches {
        groups.push(vec![Span::styled(
            format!("🔀 {}", app.sort_mode.label()),
            Style::default().fg(COLOR_ACCENT),
        )]);
    }
    // Layout indicator - show when the arrangement is forced with `v`
    if app.layout_mode != LayoutMode::Auto {
        groups.push(vec![Span::styled(
            format!("▥ {}", app.layout_mode.label()),
            Style::default().fg(COLOR_ACCENT),
        )]);
    }

    responsive::flow_lines(
        groups,
        width,
        0,
        Span::styled(" │ ", Style::default().fg(COLOR_MUTED)),
        3,
        vec![Span::styled("…", Style::default().fg(COLOR_MUTED))],
    )
}

/// Render the header with app name and repo info
fn render_header(frame: &mut Frame, app: &App, area: Rect, compact: bool) {
    let block = chrome_block(compact, None, COLOR_MUTED);
    let width = block.inner(area).width;
    let header = Paragraph::new(header_lines(app, width)).block(block);
    frame.render_widget(header, area);
}

/// Filter tab content, wrapped to `width` columns (all 4 tabs always fit)
fn filter_tab_lines(app: &App, width: u16) -> Vec<Line<'static>> {
    // (label, count, active) per tab, keys 1-4 in order
    let tabs: Vec<(&str, usize, bool)> = match (app.view, &app.worktrees) {
        (ViewMode::Worktrees, Some(wt)) => [
            WorktreeFilter::Clean,
            WorktreeFilter::Dirty,
            WorktreeFilter::Stale,
            WorktreeFilter::All,
        ]
        .iter()
        .map(|f| (f.label(), wt.filter_count(*f), wt.filter == *f))
        .collect(),
        _ => [
            FilterMode::SafeMerged,
            FilterMode::GoneUpstream,
            FilterMode::Unmerged,
            FilterMode::All,
        ]
        .iter()
        .map(|f| (f.label(), app.filter_count(*f), app.current_filter == *f))
        .collect(),
    };

    let groups = tabs
        .into_iter()
        .enumerate()
        .map(|(i, (label, count, is_active))| {
            let style = if is_active {
                Style::default()
                    .fg(Color::White)
                    .bg(COLOR_ACCENT)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(COLOR_MUTED)
            };
            vec![Span::styled(
                format!("{} {} ({}) ", i + 1, label, count),
                style,
            )]
        })
        .collect();

    responsive::flow_lines(groups, width, 2, Span::raw(" "), 4, Vec::new())
}

/// Render filter tabs
fn render_filter_tabs(frame: &mut Frame, app: &App, area: Rect, compact: bool) {
    let title = Line::from(Span::styled(
        " Filters (1-4 or Tab) ",
        Style::default().fg(COLOR_ACCENT),
    ));
    let block = chrome_block(compact, Some(title), COLOR_MUTED);
    let width = block.inner(area).width;
    let tabs = Paragraph::new(filter_tab_lines(app, width)).block(block);
    frame.render_widget(tabs, area);
}

/// Helper function to render styled text with @author: command highlighting
fn render_styled_text<'a>(spans: &mut Vec<Span<'a>>, text: &'a str) {
    let mut remaining = text;

    while !remaining.is_empty() {
        if let Some(at_pos) = remaining.find('@') {
            // Add text before @ as normal
            if at_pos > 0 {
                spans.push(Span::styled(
                    &remaining[..at_pos],
                    Style::default().fg(Color::White),
                ));
            }

            remaining = &remaining[at_pos..];

            // Check for @author: pattern
            let lower = remaining.to_lowercase();
            if lower.starts_with("@author:") {
                // Style @author: in special color (magenta/pink)
                spans.push(Span::styled(
                    "@author:",
                    Style::default().fg(COLOR_SELECTED),
                ));

                let after_colon = &remaining[8..]; // Skip "@author:"

                // Check if value is quoted
                if let Some(content) = after_colon.strip_prefix('"') {
                    // Find closing quote
                    if let Some(close_quote) = content.find('"') {
                        // Style opening quote
                        spans.push(Span::styled("\"", Style::default().fg(COLOR_MUTED)));
                        // Style author name in accent color
                        spans.push(Span::styled(
                            &content[..close_quote],
                            Style::default().fg(COLOR_ACCENT),
                        ));
                        // Style closing quote
                        spans.push(Span::styled("\"", Style::default().fg(COLOR_MUTED)));
                        remaining = &after_colon[close_quote + 2..]; // Skip content + both quotes
                    } else {
                        // No closing quote yet (user still typing)
                        spans.push(Span::styled("\"", Style::default().fg(COLOR_MUTED)));
                        spans.push(Span::styled(content, Style::default().fg(COLOR_ACCENT)));
                        remaining = "";
                    }
                } else {
                    // Unquoted value - find end at space
                    let value_end = after_colon.find(' ').unwrap_or(after_colon.len());

                    if value_end > 0 {
                        spans.push(Span::styled(
                            &after_colon[..value_end],
                            Style::default().fg(COLOR_ACCENT),
                        ));
                    }
                    remaining = &after_colon[value_end..];
                }
            } else if lower.starts_with("@") && !lower.starts_with("@author:") {
                // Partial @ command being typed - style in special color
                let cmd_end = remaining[1..]
                    .find(|c: char| c.is_whitespace() || c == ':')
                    .map(|p| p + 1)
                    .unwrap_or(remaining.len());

                spans.push(Span::styled(
                    &remaining[..cmd_end],
                    Style::default().fg(COLOR_SELECTED),
                ));

                remaining = &remaining[cmd_end..];
            }
        } else {
            // No more @ symbols, add rest as normal text
            spans.push(Span::styled(remaining, Style::default().fg(Color::White)));
            break;
        }
    }
}

/// Render the search input box with border
fn render_search_box(frame: &mut Frame, app: &App, area: Rect, compact: bool) {
    let match_count = app.visible_len();

    let search_text = if app.search_query.is_empty() && !app.search_active {
        let placeholder = if app.view == ViewMode::Worktrees {
            "  Type to search worktrees (name, branch, path)..."
        } else {
            "  Type to search branches..."
        };
        Line::from(vec![Span::styled(
            placeholder,
            Style::default().fg(COLOR_MUTED),
        )])
    } else {
        // Build styled spans for the search query with cursor positioning
        let mut spans = vec![Span::styled("  ", Style::default())];

        let query = &app.search_query;
        let cursor_pos = app.search_cursor_pos;

        // Render the query with cursor at the correct position
        if query.is_empty() {
            if app.search_active {
                spans.push(Span::styled("_", Style::default().fg(COLOR_ACCENT)));
            }
        } else {
            // Split query at cursor position and render both parts with cursor in between
            let before_cursor = &query[..cursor_pos.min(query.len())];
            let after_cursor = &query[cursor_pos.min(query.len())..];

            // Render text before cursor with styling
            render_styled_text(&mut spans, before_cursor);

            // Render cursor
            if app.search_active {
                spans.push(Span::styled("_", Style::default().fg(COLOR_ACCENT)));
            }

            // Render text after cursor with styling
            render_styled_text(&mut spans, after_cursor);
        }

        spans.push(Span::styled(
            format!("  ({} matches)", match_count),
            Style::default().fg(COLOR_MUTED),
        ));

        Line::from(spans)
    };

    let border_color = if app.search_active {
        COLOR_ACCENT
    } else {
        COLOR_MUTED
    };

    let title = if app.search_active {
        " 🔍 Search (Enter to keep, Esc to clear) "
    } else {
        " 🔍 Search (/ to edit, Esc to clear) "
    };

    // Borderless in a short terminal: the title is gone, so the icon moves
    // into the line in place of the indent
    let search_text = if compact {
        let mut spans = search_text.spans;
        if let Some(first) = spans.first_mut() {
            *first = Span::styled(
                format!("🔍 {}", first.content.trim_start_matches("  ")),
                first.style,
            );
        }
        Line::from(spans)
    } else {
        search_text
    };

    let title = Line::from(Span::styled(title, Style::default().fg(border_color)));
    let search_box =
        Paragraph::new(search_text).block(chrome_block(compact, Some(title), border_color));

    frame.render_widget(search_box, area);
}

/// Render the autocomplete suggestions dropdown
fn render_suggestions_dropdown(frame: &mut Frame, app: &App, search_area: Rect) {
    if app.suggestions.is_empty() {
        return;
    }

    let area = frame.area();

    // Calculate position: right below the search box
    let y_offset = search_area.bottom();

    // Calculate available space for dropdown
    let available_height = area.height.saturating_sub(y_offset + 2); // Reserve some space at bottom

    // Calculate how many items we can show (accounting for borders and potential "..." indicators)
    let total_suggestions = app.suggestions.len();
    let max_visible_items = (available_height.saturating_sub(2)) as usize; // -2 for borders
    let visible_items = max_visible_items.min(total_suggestions);

    // If we can show all items, no need for scrolling indicators
    let needs_scroll = total_suggestions > visible_items;

    // Calculate scroll offset to keep selected item visible
    let selected_idx = app.suggestion_index.unwrap_or(0);
    let scroll_offset = if needs_scroll {
        // Keep selected item in view with some context
        if selected_idx < visible_items / 2 {
            0
        } else if selected_idx >= total_suggestions.saturating_sub(visible_items / 2) {
            total_suggestions.saturating_sub(visible_items)
        } else {
            selected_idx.saturating_sub(visible_items / 2)
        }
    } else {
        0
    };

    let dropdown_height = (visible_items + 2) as u16; // +2 for borders
    let dropdown_width = 40.min(area.width.saturating_sub(4));

    // Position dropdown below the search box, aligned to the left with some padding
    let dropdown_area = Rect {
        x: search_area.x + 3,
        y: y_offset,
        width: dropdown_width,
        height: dropdown_height,
    };

    // Ensure dropdown doesn't go off screen
    if dropdown_area.y + dropdown_area.height > area.height {
        return;
    }

    // Clear the area behind the dropdown
    frame.render_widget(Clear, dropdown_area);

    // Build suggestion lines with scroll indicators
    let mut lines: Vec<Line> = Vec::new();

    // Add "more above" indicator if needed
    let has_more_above = scroll_offset > 0;
    let has_more_below = scroll_offset + visible_items < total_suggestions;

    // Calculate items to display (may be reduced by 1 or 2 for scroll indicators)
    let display_start = scroll_offset;
    let mut items_to_show = visible_items;

    if has_more_above && items_to_show > 0 {
        items_to_show -= 1;
    }
    if has_more_below && items_to_show > 0 {
        items_to_show -= 1;
    }

    // Add "↑ more" indicator
    if has_more_above {
        lines.push(Line::from(Span::styled(
            format!("  ↑ {} more above", scroll_offset),
            Style::default()
                .fg(COLOR_MUTED)
                .add_modifier(Modifier::ITALIC),
        )));
    }

    // Add visible suggestions
    for (display_idx, suggestion) in app
        .suggestions
        .iter()
        .skip(display_start)
        .take(items_to_show + if has_more_above { 1 } else { 0 })
        .enumerate()
    {
        let actual_idx = display_start + display_idx;
        let is_selected = app.suggestion_index == Some(actual_idx);
        let prefix = if is_selected { "▸ " } else { "  " };

        // Determine if this is a command or an author
        let display_text = if suggestion == "author" {
            format!("{}@{}:", prefix, suggestion)
        } else if suggestion == "me" {
            format!("{}@author:{} (your branches)", prefix, suggestion)
        } else {
            format!("{}@author:{}", prefix, suggestion)
        };

        let style = if is_selected {
            Style::default()
                .fg(Color::Black)
                .bg(COLOR_ACCENT)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        };

        lines.push(Line::from(Span::styled(display_text, style)));
    }

    // Add "↓ more" indicator
    if has_more_below {
        let remaining = total_suggestions
            - (scroll_offset + items_to_show + if has_more_above { 1 } else { 0 });
        lines.push(Line::from(Span::styled(
            format!("  ↓ {} more below", remaining),
            Style::default()
                .fg(COLOR_MUTED)
                .add_modifier(Modifier::ITALIC),
        )));
    }

    let title = if app.search_query.contains("@author:") {
        " Authors ".to_string()
    } else {
        " Commands ".to_string()
    };

    let dropdown = Paragraph::new(lines).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(COLOR_ACCENT))
            .title(title)
            .title_style(
                Style::default()
                    .fg(COLOR_ACCENT)
                    .add_modifier(Modifier::BOLD),
            ),
    );

    frame.render_widget(dropdown, dropdown_area);
}

/// Draw a vertical scrollbar along the right edge of `area` (a block's
/// right border, typically)
fn render_scrollbar(frame: &mut Frame, area: Rect, scroll: Scroll) {
    let mut state = ScrollbarState::new(scroll.positions())
        .position(scroll.offset)
        .viewport_content_length(scroll.viewport);
    let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
        .thumb_style(Style::default().fg(COLOR_ACCENT))
        .track_style(Style::default().fg(COLOR_MUTED))
        .begin_style(Style::default().fg(COLOR_MUTED))
        .end_style(Style::default().fg(COLOR_MUTED));
    frame.render_stateful_widget(scrollbar, area, &mut state);
}

/// Bordered block of a list pane. With the details pane hidden, its bottom
/// border says how to bring it back.
fn list_block(title: String, details_hidden: bool) -> Block<'static> {
    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(COLOR_MUTED))
        .title(title)
        .title_style(Style::default().fg(COLOR_ACCENT));
    if details_hidden {
        block = block.title_bottom(
            Line::from(vec![
                Span::styled(" details hidden: ", Style::default().fg(COLOR_MUTED)),
                Span::styled(
                    "v",
                    Style::default()
                        .fg(COLOR_ACCENT)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(" layout ", Style::default().fg(COLOR_MUTED)),
            ])
            .right_aligned(),
        );
    }
    block
}

/// Rows a list pane keeps before the legend gives up space: its borders,
/// the table header and 2 rows
const LIST_PANE_MIN_HEIGHT: u16 = 5;

/// Draw the legend `groups` in their own box at the bottom of the list
/// column `area` and return what is left for the list pane. The legend
/// wraps onto at most 2 lines and ends in `… i about` when they are not
/// enough; it gets fewer lines, then none, before the list pane gets less
/// than `LIST_PANE_MIN_HEIGHT` rows. Compact: no border, a `Legend:` prefix
/// in place of the title.
fn render_legend(
    frame: &mut Frame,
    area: Rect,
    mut groups: Vec<Vec<Span<'static>>>,
    compact: bool,
) -> Rect {
    let border = if compact { 0 } else { 2 };
    let max_lines = area
        .height
        .saturating_sub(LIST_PANE_MIN_HEIGHT + border)
        .min(2) as usize;
    if max_lines == 0 {
        return area;
    }
    if compact {
        groups.insert(
            0,
            vec![Span::styled("Legend:", Style::default().fg(COLOR_MUTED))],
        );
    }
    let lines = responsive::flow_lines(
        groups,
        area.width.saturating_sub(border),
        0,
        Span::raw("  "),
        max_lines,
        key_hint("…", "i about", COLOR_ACCENT),
    );
    let [list, legend] = Layout::vertical([
        Constraint::Min(0),
        Constraint::Length(lines.len() as u16 + border),
    ])
    .areas(area);
    let block = chrome_block(
        compact,
        Some(Line::from(Span::styled(
            " Legend ",
            Style::default().fg(COLOR_ACCENT),
        ))),
        COLOR_MUTED,
    );
    frame.render_widget(Paragraph::new(lines).block(block), legend);
    list
}

/// The table area inside a list pane's border
fn list_table_area(area: Rect) -> Rect {
    area.inner(Margin {
        horizontal: 1,
        vertical: 1,
    })
}

/// Draw a list `table` of `row_count` rows in `table_area`, the cursor on
/// row `selected`, scrolled by `offset` rows. Returns the offset actually
/// shown, which the caller stores back in `App::scroll_offset` so it stays
/// in step with the screen after a resize. Draws a scrollbar on the right
/// border of `area` when rows are out of view.
fn render_list_table(
    frame: &mut Frame,
    table: Table,
    row_count: usize,
    selected: usize,
    offset: usize,
    area: Rect,
    table_area: Rect,
) -> usize {
    // Rows below the header row
    let viewport = table_area.height.saturating_sub(1) as usize;
    // After the terminal grows, pull the rows back up to fill the space
    let offset = Scroll::clamped(offset, viewport, row_count).offset;

    let mut state = TableState::default()
        .with_selected(Some(selected))
        .with_offset(offset);
    frame.render_stateful_widget(table, table_area, &mut state);
    // Ratatui moves the offset to keep the cursor row on screen
    let offset = state.offset();

    let scroll = Scroll::clamped(offset, viewport, row_count);
    if scroll.overflows() {
        let bar_area = Rect {
            x: area.x,
            y: table_area.y + 1,
            width: area.width,
            height: viewport as u16,
        };
        render_scrollbar(frame, bar_area, scroll);
    }
    offset
}

/// Resolved width of column `index` when `widths` share `width` columns the
/// way `Table` lays them out
fn column_width(widths: &[Constraint], width: u16, index: usize) -> usize {
    Layout::horizontal(widths.iter().copied())
        .spacing(1)
        .split(Rect::new(0, 0, width, 1))
        .get(index)
        .map_or(0, |r| r.width as usize)
}

/// Legend items of the branch table, one group per item
fn branch_legend_groups(app: &App) -> Vec<Vec<Span<'static>>> {
    let mut groups = vec![legend_item("✓", "merged", COLOR_SUCCESS)];
    // pr-merged/pr-diverged can only occur when a PR provider supplied merge evidence
    if app.pr_provider.is_some() {
        groups.push(legend_item("↑", "pr-merged", COLOR_ACCENT));
        groups.push(legend_item("↕", "pr-diverged", COLOR_DIVERGED));
    }
    groups.extend([
        legend_item("↗", "gone", COLOR_SUCCESS),
        legend_item("!", "unmerged", COLOR_WARNING),
        legend_item("○", "local", COLOR_LOCAL),
        legend_item("⊘", "protected", COLOR_DANGER),
        legend_item("⑂", "protected-wt", COLOR_DANGER),
        legend_item("◉", "current", COLOR_CURRENT),
    ]);
    // Add PR legend items when PR integration is enabled
    if app.pr_provider.is_some() {
        groups.push(vec![Span::styled(
            "│ PR:",
            Style::default().fg(COLOR_MUTED),
        )]);
        groups.push(legend_item("🟢", "merged", COLOR_PR_MERGED));
        groups.push(legend_item("🟡", "open", COLOR_PR_OPEN));
        groups.push(legend_item("🔴", "closed", COLOR_PR_CLOSED));
    }
    groups
}

/// Render the branch list as a table
fn render_branch_list(
    frame: &mut Frame,
    app: &mut App,
    area: Rect,
    details_hidden: bool,
    compact: bool,
) {
    let area = render_legend(frame, area, branch_legend_groups(app), compact);
    app.list_area = area;
    let table_area = list_table_area(area);

    // Rows that fit, for page up / down
    app.visible_height = table_area.height as usize;

    let has_pr = app.pr_provider.is_some();
    let cols = responsive::branch_columns(table_area.width, has_pr);

    // Column widths and header, leaving out what does not fit
    let mut widths = vec![
        Constraint::Length(2), // Cursor indicator
        Constraint::Length(4), // Checkbox
        Constraint::Min(15),   // Branch name
    ];
    let mut header_labels = vec!["", "☑", "Branch"];
    if cols.last_commit {
        widths.push(Constraint::Length(15));
        header_labels.push("Last Commit");
    }
    if cols.status_label {
        widths.push(Constraint::Length(15));
        header_labels.push("Status");
    } else {
        widths.push(Constraint::Length(responsive::ICON_COLUMN_WIDTH));
        header_labels.push("St");
    }
    if cols.pr {
        widths.push(Constraint::Length(10));
        header_labels.push("PR");
    }
    let name_width = column_width(&widths, table_area.width, 2);

    let header = Row::new(header_labels.iter().map(|h| {
        Cell::from(*h).style(
            Style::default()
                .fg(COLOR_ACCENT)
                .add_modifier(Modifier::BOLD),
        )
    }))
    .height(1);

    // Get filtered branches
    let filtered_branches = app.filtered_branches();

    // Create table rows from filtered branches
    let rows: Vec<Row> = filtered_branches
        .iter()
        .enumerate()
        .map(|(filtered_idx, branch)| {
            // Find original index in app.branches for selection state
            let original_idx = app
                .branches
                .iter()
                .position(|b| b.name == branch.name)
                .unwrap();
            let is_cursor = filtered_idx == app.selected_index;
            let is_checked = app.is_branch_selected(original_idx);
            let status_style = get_status_style(&branch.status);

            // Checkbox display
            let checkbox = if !branch.status.is_deletable() {
                "   " // No checkbox for protected
            } else if branch.status.requires_force() && !app.force_mode {
                " - " // Disabled checkbox for force-only branches without force
            } else if is_checked {
                "[✓]"
            } else {
                "[ ]"
            };

            let checkbox_style = if is_checked {
                Style::default().fg(COLOR_SELECTED)
            } else if !branch.status.is_deletable()
                || (branch.status.requires_force() && !app.force_mode)
            {
                Style::default().fg(COLOR_MUTED)
            } else {
                Style::default().fg(COLOR_ACCENT)
            };

            let mut cells = vec![
                // Cursor indicator
                Cell::from(if is_cursor { "▶" } else { " " })
                    .style(Style::default().fg(COLOR_ACCENT)),
                // Checkbox
                Cell::from(checkbox).style(checkbox_style),
                // Branch name, cut with `…` when too long (full name in details)
                Cell::from(responsive::truncate_end(&branch.name, name_width)).style(
                    if is_cursor {
                        Style::default()
                            .fg(Color::White)
                            .add_modifier(Modifier::BOLD)
                    } else if is_checked {
                        Style::default().fg(COLOR_SELECTED)
                    } else {
                        Style::default().fg(COLOR_MUTED)
                    },
                ),
            ];
            if cols.last_commit {
                cells.push(
                    Cell::from(branch.last_commit_relative.as_str())
                        .style(Style::default().fg(COLOR_MUTED)),
                );
            }
            // Status (icon + label combined, or icon alone when narrow)
            let status = if cols.status_label {
                format!("{} {}", branch.status.icon(), branch.status.label())
            } else {
                branch.status.icon().to_string()
            };
            cells.push(Cell::from(status).style(status_style));

            // Add PR column if PR integration is enabled and it fits
            if cols.pr {
                let (pr_text, pr_style) = match &branch.pr_info {
                    Some(pr) => {
                        let color = get_pr_state_color(&pr.state);
                        (
                            format!("{} #{}", pr.state.icon(), pr.number),
                            Style::default().fg(color),
                        )
                    }
                    None => ("⚪".to_string(), Style::default().fg(COLOR_MUTED)),
                };
                cells.push(Cell::from(pr_text).style(pr_style));
            }

            let row = Row::new(cells);
            if is_cursor {
                row.style(Style::default().bg(Color::Rgb(30, 35, 45)))
            } else if is_checked {
                row.style(Style::default().bg(Color::Rgb(40, 30, 40)))
            } else {
                row
            }
        })
        .collect();
    let row_count = rows.len();

    let title = format!(
        " Branches ({} shown, {} selected) ",
        filtered_branches.len(),
        app.selected_count()
    );
    frame.render_widget(list_block(title, details_hidden), area);

    // Render table without its own block (inside the outer block)
    let table = Table::new(rows, widths)
        .header(header)
        .row_highlight_style(Style::default().add_modifier(Modifier::BOLD));
    let offset = render_list_table(
        frame,
        table,
        row_count,
        app.selected_index,
        app.scroll_offset,
        area,
        table_area,
    );
    app.scroll_offset = offset;
}

/// Render the details pane for the selected branch
/// Returns the details scroll actually shown (clamped to the content)
fn render_details_pane(frame: &mut Frame, app: &App, area: Rect, scroll: usize) -> usize {
    let mut lines = vec![];

    if let Some(branch) = app.selected_branch() {
        // Branch name (large)
        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::styled("  ", Style::default()),
            Span::styled(
                &branch.name,
                Style::default()
                    .fg(COLOR_ACCENT)
                    .add_modifier(Modifier::BOLD),
            ),
        ]));
        lines.push(Line::from(""));

        // Status
        lines.push(Line::from(vec![
            Span::styled("  Status: ", Style::default().fg(COLOR_MUTED)),
            Span::styled(
                branch.status.icon(),
                Style::default().fg(get_status_color(&branch.status)),
            ),
            Span::styled(" ", Style::default()),
            Span::styled(
                branch.status.label(),
                Style::default().fg(get_status_color(&branch.status)),
            ),
        ]));

        // Status explanation
        let explanation = match branch.status {
            BranchStatus::SafeMerged => "Merged into trunk, safe to delete",
            BranchStatus::PrMerged => "PR merged; remote branch still exists",
            BranchStatus::PrDiverged => "PR merged but local diverged from remote",
            BranchStatus::GoneUpstream => "Remote branch deleted",
            BranchStatus::Unmerged => "Has unmerged commits",
            BranchStatus::Local => "Never pushed; exists only locally",
            BranchStatus::Protected => "Protected branch",
            BranchStatus::ProtectedWorktree => "Checked out in another worktree",
            BranchStatus::Current => "Currently checked out",
        };
        lines.push(Line::from(vec![
            Span::styled("  ", Style::default()),
            Span::styled(
                explanation,
                Style::default()
                    .fg(COLOR_MUTED)
                    .add_modifier(Modifier::ITALIC),
            ),
        ]));
        lines.push(Line::from(""));

        // Worktree holding the branch (git refuses to delete it)
        if let Some(path) = &branch.worktree {
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.to_string_lossy().into_owned());
            lines.push(Line::from(vec![
                Span::styled("  Worktree: ", Style::default().fg(COLOR_MUTED)),
                Span::styled(
                    format!("{} ({})", name, path.display()),
                    Style::default().fg(COLOR_CURRENT),
                ),
            ]));
            if !path.exists() {
                lines.push(Line::from(vec![Span::styled(
                    "  Prunable: directory missing, run `git worktree prune`",
                    Style::default()
                        .fg(COLOR_WARNING)
                        .add_modifier(Modifier::ITALIC),
                )]));
            }
            if let Some(shadowed) = &branch.worktree_shadowed_status {
                lines.push(Line::from(vec![
                    Span::styled("  Otherwise: ", Style::default().fg(COLOR_MUTED)),
                    Span::styled(
                        format!("{} {}", shadowed.icon(), shadowed.label()),
                        Style::default().fg(get_status_color(shadowed)),
                    ),
                ]));
            }
            lines.push(Line::from(""));
        }

        // Upstream info
        if let Some(upstream) = &branch.upstream {
            lines.push(Line::from(vec![
                Span::styled("  Upstream: ", Style::default().fg(COLOR_MUTED)),
                Span::styled(upstream, Style::default().fg(COLOR_SUCCESS)),
            ]));

            // Ahead/behind
            if let (Some(ahead), Some(behind)) = (branch.ahead, branch.behind) {
                if ahead > 0 || behind > 0 {
                    let mut parts = vec![Span::styled(
                        "  Divergence: ",
                        Style::default().fg(COLOR_MUTED),
                    )];

                    if ahead > 0 {
                        parts.push(Span::styled(
                            format!("↑{}", ahead),
                            Style::default().fg(COLOR_SUCCESS),
                        ));
                    }
                    if ahead > 0 && behind > 0 {
                        parts.push(Span::styled(" ", Style::default()));
                    }
                    if behind > 0 {
                        parts.push(Span::styled(
                            format!("↓{}", behind),
                            Style::default().fg(COLOR_WARNING),
                        ));
                    }

                    lines.push(Line::from(parts));
                }
            }
        } else {
            lines.push(Line::from(vec![
                Span::styled("  Upstream: ", Style::default().fg(COLOR_MUTED)),
                Span::styled("none", Style::default().fg(COLOR_WARNING)),
            ]));
        }
        lines.push(Line::from(""));

        // Last commit info
        lines.push(Line::from(vec![Span::styled(
            "  Last Commit:",
            Style::default()
                .fg(COLOR_MUTED)
                .add_modifier(Modifier::BOLD),
        )]));
        lines.push(Line::from(vec![
            Span::styled("  ", Style::default()),
            Span::styled(&branch.last_commit_sha, Style::default().fg(COLOR_WARNING)),
        ]));
        lines.push(Line::from(vec![
            Span::styled("  ", Style::default()),
            Span::styled(&branch.last_commit_author, Style::default().fg(COLOR_MUTED)),
        ]));
        lines.push(Line::from(vec![
            Span::styled("  ", Style::default()),
            Span::styled(
                &branch.last_commit_relative,
                Style::default()
                    .fg(COLOR_MUTED)
                    .add_modifier(Modifier::ITALIC),
            ),
        ]));
        lines.push(Line::from(""));

        // Commit message (word wrap)
        lines.push(Line::from(vec![Span::styled(
            "  Message:",
            Style::default()
                .fg(COLOR_MUTED)
                .add_modifier(Modifier::BOLD),
        )]));

        // Wrap long commit messages
        let max_width = area.width.saturating_sub(4) as usize;
        let words: Vec<&str> = branch.last_commit_message.split_whitespace().collect();
        let mut current_line = String::from("  ");

        for word in words {
            if current_line.len() + word.len() + 1 > max_width {
                lines.push(Line::from(Span::styled(
                    current_line.clone(),
                    Style::default().fg(Color::White),
                )));
                current_line = String::from("  ");
            }
            if !current_line.ends_with("  ") {
                current_line.push(' ');
            }
            current_line.push_str(word);
        }
        if current_line.len() > 2 {
            lines.push(Line::from(Span::styled(
                current_line,
                Style::default().fg(Color::White),
            )));
        }

        // PR info (if available and PR integration enabled)
        let mut shows_open_pr_hint = false;
        if app.pr_provider.is_some() {
            lines.push(Line::from(""));
            if let Some(pr) = &branch.pr_info {
                lines.push(Line::from(vec![Span::styled(
                    "  Pull Request:",
                    Style::default()
                        .fg(COLOR_MUTED)
                        .add_modifier(Modifier::BOLD),
                )]));

                // PR number and state
                lines.push(Line::from(vec![
                    Span::styled("  ", Style::default()),
                    Span::styled(
                        pr.state.icon(),
                        Style::default().fg(get_pr_state_color(&pr.state)),
                    ),
                    Span::styled(" PR #", Style::default().fg(COLOR_MUTED)),
                    Span::styled(format!("{}", pr.number), Style::default().fg(COLOR_ACCENT)),
                    Span::styled(" (", Style::default().fg(COLOR_MUTED)),
                    Span::styled(
                        pr.state.label(),
                        Style::default().fg(get_pr_state_color(&pr.state)),
                    ),
                    Span::styled(")", Style::default().fg(COLOR_MUTED)),
                ]));

                // PR title (word wrapped)
                let pr_max_width = area.width.saturating_sub(4) as usize;
                let pr_words: Vec<&str> = pr.title.split_whitespace().collect();
                let mut pr_line = String::from("  ");
                for word in pr_words {
                    if pr_line.len() + word.len() + 1 > pr_max_width {
                        lines.push(Line::from(Span::styled(
                            pr_line.clone(),
                            Style::default().fg(Color::White),
                        )));
                        pr_line = String::from("  ");
                    }
                    if !pr_line.ends_with("  ") {
                        pr_line.push(' ');
                    }
                    pr_line.push_str(word);
                }
                if pr_line.len() > 2 {
                    lines.push(Line::from(Span::styled(
                        pr_line,
                        Style::default().fg(Color::White),
                    )));
                }

                // Hint to open PR
                lines.push(Line::from(""));
                lines.push(Line::from(vec![
                    Span::styled("  Press ", Style::default().fg(COLOR_MUTED)),
                    Span::styled(
                        "o",
                        Style::default()
                            .fg(COLOR_ACCENT)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(" to open in browser", Style::default().fg(COLOR_MUTED)),
                ]));
                shows_open_pr_hint = true;
            } else {
                lines.push(Line::from(vec![
                    Span::styled("  Pull Request: ", Style::default().fg(COLOR_MUTED)),
                    Span::styled(
                        "none",
                        Style::default()
                            .fg(COLOR_MUTED)
                            .add_modifier(Modifier::ITALIC),
                    ),
                ]));
            }
        }

        // Hint to copy the branch name: right under the `o` hint when there
        // is one, otherwise after a blank line
        if !shows_open_pr_hint {
            lines.push(Line::from(""));
        }
        lines.push(Line::from(vec![
            Span::styled("  Press ", Style::default().fg(COLOR_MUTED)),
            Span::styled(
                "y",
                Style::default()
                    .fg(COLOR_ACCENT)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" to copy the branch name", Style::default().fg(COLOR_MUTED)),
        ]));
    } else {
        lines.push(Line::from(""));
        lines.push(Line::from(vec![Span::styled(
            "  No branch selected",
            Style::default()
                .fg(COLOR_MUTED)
                .add_modifier(Modifier::ITALIC),
        )]));
    }

    render_details_block(frame, lines, area, scroll)
}

/// Render the action log panel
fn render_action_log(frame: &mut Frame, app: &App, area: Rect) {
    let log_lines: Vec<Line> = app
        .action_log
        .iter()
        .rev() // Show most recent first, as many as fit
        .take(area.height.saturating_sub(2) as usize)
        .map(|entry| {
            if entry.success {
                Line::from(vec![
                    Span::styled("  ✓ ", Style::default().fg(COLOR_SUCCESS)),
                    Span::styled(&entry.branch_name, Style::default().fg(COLOR_MUTED)),
                    Span::styled(" - ", Style::default().fg(COLOR_MUTED)),
                    Span::styled(&entry.message, Style::default().fg(COLOR_SUCCESS)),
                ])
            } else {
                Line::from(vec![
                    Span::styled("  ✗ ", Style::default().fg(COLOR_DANGER)),
                    Span::styled(&entry.branch_name, Style::default().fg(COLOR_MUTED)),
                    Span::styled(" - ", Style::default().fg(COLOR_MUTED)),
                    Span::styled(&entry.message, Style::default().fg(COLOR_DANGER)),
                ])
            }
        })
        .collect();

    let title = format!(
        " Action Log ({} success, {} failed) ",
        app.deletion_success_count(),
        app.deletion_failure_count()
    );

    let log = Paragraph::new(log_lines).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(COLOR_MUTED))
            .title(title)
            .title_style(Style::default().fg(COLOR_ACCENT)),
    );

    frame.render_widget(log, area);
}

/// Style of a mode toggle hint (`f force`, `d dry`, `x remove mode`):
/// highlighted while the mode is on
fn toggle_style(on: bool, color: Color) -> Style {
    if on {
        Style::default()
            .fg(Color::Black)
            .bg(color)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(color).add_modifier(Modifier::BOLD)
    }
}

/// Footer key hints, one group per key, for the view and mode on screen
fn footer_hint_groups(app: &App) -> Vec<Vec<Span<'static>>> {
    let list_is_empty = match app.view {
        ViewMode::Branches => app.branches.is_empty(),
        ViewMode::Worktrees => app
            .worktrees
            .as_ref()
            .is_none_or(|wt| wt.worktrees.is_empty()),
    };
    if list_is_empty {
        return vec![
            key_hint("?", "help", COLOR_ACCENT),
            key_hint("q / Esc Esc", "quit", COLOR_ACCENT),
        ];
    }

    let mut groups = if app.view == ViewMode::Worktrees {
        worktree_footer_hints(app)
    } else {
        vec![
            key_hint("↑↓", "nav", COLOR_ACCENT),
            key_hint("Space", "sel", COLOR_ACCENT),
            key_hint("a", "all", COLOR_ACCENT),
            key_hint("c", "clr", COLOR_ACCENT),
            vec![Span::styled(
                "f force",
                toggle_style(app.force_mode, COLOR_DANGER),
            )],
            vec![Span::styled(
                "d dry",
                toggle_style(app.dry_run, COLOR_WARNING),
            )],
            key_hint("s", "sort", COLOR_ACCENT),
            key_hint("Enter", "del", COLOR_SELECTED),
            key_hint("/", "search", COLOR_ACCENT),
            key_hint("y", "copy", COLOR_ACCENT),
            key_hint("w", "worktrees", COLOR_ACCENT),
        ]
    };
    groups.push(key_hint("v", "layout", COLOR_ACCENT));
    if app.pr_provider.is_some() {
        groups.push(key_hint("o", "pr", COLOR_PR_MERGED));
    }
    groups.push(key_hint("?", "help", COLOR_ACCENT));
    groups.push(key_hint("q", "quit", COLOR_ACCENT));
    groups
}

/// Footer content wrapped to `width` columns: the key hints, or the quit
/// prompt / one-off message that replaces them. At most 3 lines (2 when
/// compact); hints that do not fit end in `… ? help`.
fn footer_lines(app: &App, width: u16, compact: bool) -> Vec<Line<'static>> {
    let max_lines = if compact { 2 } else { 3 };
    let warning = Style::default().fg(COLOR_WARNING);

    // After a first Esc, the quit confirmation prompt replaces the hints
    if app.esc_quit_armed_at.is_some() {
        return vec![Line::from(vec![
            Span::styled(" Press ", warning),
            Span::styled("Esc", warning.add_modifier(Modifier::BOLD)),
            Span::styled(" again to quit", warning),
        ])];
    }

    // A one-off message (clipboard result, missing PR) replaces the hints
    if let Some(message) = &app.flash {
        let lines: Vec<Line> = wrap_words(message, (width as usize).saturating_sub(1).max(1))
            .into_iter()
            .take(max_lines)
            .map(|line| Line::from(Span::styled(format!(" {line}"), warning)))
            .collect();
        return if lines.is_empty() {
            vec![Line::from("")]
        } else {
            lines
        };
    }

    let mut groups = footer_hint_groups(app);
    // A compact footer has no bottom border to carry the refresh status and
    // the update notice
    if compact {
        if let Some(status) = app.refresh_status(std::time::Instant::now()) {
            groups.push(vec![Span::styled(
                status,
                Style::default().fg(COLOR_ACCENT),
            )]);
        }
        if let Some(notice) = &app.update_notice {
            groups.push(vec![Span::styled(
                notice.clone(),
                Style::default().fg(COLOR_SUCCESS),
            )]);
        }
    }
    let more = vec![
        Span::styled("… ", Style::default().fg(COLOR_MUTED)),
        Span::styled(
            "?",
            Style::default()
                .fg(COLOR_ACCENT)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" help", Style::default().fg(COLOR_MUTED)),
    ];
    responsive::flow_lines(groups, width, 1, Span::raw("  "), max_lines, more)
}

/// Render the footer with key hints
fn render_footer(frame: &mut Frame, app: &App, area: Rect, compact: bool) {
    let mut block = chrome_block(compact, None, COLOR_MUTED);
    // On the bottom border, so they take no room from the key hints
    if !compact {
        if let Some(status) = app.refresh_status(std::time::Instant::now()) {
            block = block.title_bottom(
                Line::from(Span::styled(
                    format!(" {status} "),
                    Style::default().fg(COLOR_ACCENT),
                ))
                .left_aligned(),
            );
        }
        if let Some(notice) = &app.update_notice {
            block = block.title_bottom(
                Line::from(Span::styled(
                    format!(" {notice} "),
                    Style::default().fg(COLOR_SUCCESS),
                ))
                .right_aligned(),
            );
        }
    }
    let width = block.inner(area).width;
    let footer = Paragraph::new(footer_lines(app, width, compact)).block(block);
    frame.render_widget(footer, area);
}

/// Render the confirmation modal
fn render_confirmation_modal(frame: &mut Frame, app: &mut App) {
    let (color, title) = if app.dry_run {
        (COLOR_WARNING, " 🔍 Preview (Dry Run) ")
    } else {
        (COLOR_DANGER, " ⚠️  Confirm Deletion ")
    };
    let lines = if app.view == ViewMode::Worktrees {
        worktree_confirmation_lines(app)
    } else {
        branch_confirmation_lines(app)
    };
    // The y / n hint stays below the list however far it is scrolled
    let scroll = render_modal(
        frame,
        title,
        color,
        60,
        lines,
        Some(confirmation_hint_line()),
        app.modal_scroll,
        " ↑↓ scroll ",
    );
    app.modal_scroll = scroll.offset;
    app.modal_viewport = scroll.viewport;
}

/// Body of the confirmation modal in the branch view: every selected branch
fn branch_confirmation_lines(app: &App) -> Vec<Line<'_>> {
    let selected_branches = app.get_selected_branches();
    let unmerged_count = selected_branches
        .iter()
        .filter(|b| b.status.requires_force())
        .count();

    let mut lines = vec![Line::from("")];

    if app.dry_run {
        lines.push(Line::from(vec![
            Span::styled("  Preview ", Style::default().fg(COLOR_MUTED)),
            Span::styled(
                format!("{}", app.selected_count()),
                Style::default()
                    .fg(COLOR_WARNING)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" branch(es) ", Style::default().fg(COLOR_MUTED)),
            Span::styled(
                "(Dry Run)",
                Style::default()
                    .fg(COLOR_WARNING)
                    .add_modifier(Modifier::BOLD),
            ),
        ]));
    } else {
        lines.push(Line::from(vec![
            Span::styled("  Delete ", Style::default().fg(COLOR_MUTED)),
            Span::styled(
                format!("{}", app.selected_count()),
                Style::default()
                    .fg(COLOR_DANGER)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" branch(es)?", Style::default().fg(COLOR_MUTED)),
        ]));
    }
    lines.push(Line::from(""));

    // Every branch that will go; the modal scrolls when they do not fit
    for branch in selected_branches.iter() {
        let prefix = "  • ";
        lines.push(Line::from(vec![
            Span::styled(prefix, Style::default().fg(COLOR_MUTED)),
            Span::styled(&branch.name, Style::default().fg(COLOR_ACCENT)),
            Span::styled(
                format!(" ({})", branch.status.label()),
                Style::default().fg(get_status_color(&branch.status)),
            ),
        ]));
    }

    // Warning for unmerged
    if unmerged_count > 0 {
        lines.push(Line::from(""));
        lines.push(Line::from(vec![Span::styled(
            format!("  ⚠️  {} unmerged (force delete)", unmerged_count),
            Style::default().fg(COLOR_DANGER),
        )]));
    }

    lines
}

/// The `y / Enter confirm    n / Esc cancel` line of the confirmation modal
fn confirmation_hint_line() -> Line<'static> {
    Line::from(vec![
        Span::styled(
            "y / Enter",
            Style::default()
                .fg(COLOR_SUCCESS)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" confirm    ", Style::default().fg(COLOR_MUTED)),
        Span::styled(
            "n / Esc",
            Style::default()
                .fg(COLOR_DANGER)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" cancel", Style::default().fg(COLOR_MUTED)),
    ])
    .alignment(Alignment::Center)
}

/// Bottom-border hint of a help / about modal that does not fit
const MODAL_SCROLL_HINT: &str = " ↑↓ scroll · other keys close ";

/// Draw a centred modal at most `max_width` columns wide and as tall as its
/// `lines` need (capped at the screen), scrolled down by `offset` rows.
/// `footer` stays pinned below the scrolled lines; `scroll_hint` goes on the
/// bottom border when the lines do not fit. Returns the scroll shown, which
/// the caller stores back in `App::modal_scroll` / `App::modal_viewport`.
#[allow(clippy::too_many_arguments)]
fn render_modal(
    frame: &mut Frame,
    title: &str,
    color: Color,
    max_width: u16,
    lines: Vec<Line>,
    footer: Option<Line>,
    offset: usize,
    scroll_hint: &str,
) -> Scroll {
    let area = frame.area();
    let width = max_width.min(area.width.saturating_sub(4));
    let content_rows = Paragraph::new(lines.clone())
        .wrap(Wrap { trim: false })
        .line_count(width.saturating_sub(2));
    // A blank line, then the footer
    let footer_rows: u16 = if footer.is_some() { 2 } else { 0 };
    let height = u16::try_from(content_rows)
        .unwrap_or(u16::MAX)
        .saturating_add(footer_rows + 2)
        .min(area.height.saturating_sub(2));
    let modal_area = Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    };
    frame.render_widget(Clear, modal_area);

    let border_style = Style::default().fg(color);
    let title = Span::styled(title, border_style.add_modifier(Modifier::BOLD));
    let viewport = height.saturating_sub(2 + footer_rows) as usize;

    let mut outer = Block::bordered()
        .border_style(border_style)
        .title(title.clone());
    if content_rows > viewport {
        outer = outer.title_bottom(
            Line::from(Span::styled(
                scroll_hint.to_string(),
                Style::default().fg(COLOR_MUTED),
            ))
            .right_aligned(),
        );
    }
    frame.render_widget(outer, modal_area);

    // The scrolled part: the modal without its bottom border and footer rows
    let body_area = Rect {
        height: height.saturating_sub(1 + footer_rows),
        ..modal_area
    };
    let body_block = Block::new()
        .borders(Borders::LEFT | Borders::TOP | Borders::RIGHT)
        .border_style(border_style)
        .title(title);
    let scroll = render_scrolled(frame, body_block, lines, body_area, offset, None);

    if let Some(footer) = footer {
        let footer_area = Rect {
            x: modal_area.x + 1,
            y: body_area.bottom(),
            width: width.saturating_sub(2),
            height: modal_area
                .bottom()
                .saturating_sub(1)
                .saturating_sub(body_area.bottom()),
        };
        frame.render_widget(Paragraph::new(vec![Line::from(""), footer]), footer_area);
    }
    scroll
}

/// Get the style for a branch status
fn get_status_style(status: &BranchStatus) -> Style {
    Style::default().fg(get_status_color(status))
}

/// Get the color for a branch status
fn get_status_color(status: &BranchStatus) -> Color {
    match status {
        BranchStatus::SafeMerged => COLOR_SUCCESS,
        BranchStatus::PrMerged => COLOR_ACCENT,
        BranchStatus::PrDiverged => COLOR_DIVERGED,
        BranchStatus::GoneUpstream => COLOR_SUCCESS,
        BranchStatus::Unmerged => COLOR_WARNING,
        BranchStatus::Local => COLOR_LOCAL,
        BranchStatus::Protected => COLOR_DANGER,
        BranchStatus::ProtectedWorktree => COLOR_DANGER,
        BranchStatus::Current => COLOR_CURRENT,
    }
}

/// Get the color for a PR state
fn get_pr_state_color(state: &PrState) -> Color {
    match state {
        PrState::Open => COLOR_PR_OPEN,
        PrState::Merged => COLOR_PR_MERGED,
        PrState::Closed => COLOR_PR_CLOSED,
    }
}

/// Render the help modal
fn render_help_modal(frame: &mut Frame, app: &mut App) {
    let view = app.view;
    let lines = vec![
        Line::from(""),
        Line::from(vec![Span::styled(
            "  Navigation",
            Style::default()
                .fg(COLOR_ACCENT)
                .add_modifier(Modifier::BOLD),
        )]),
        Line::from(vec![
            Span::styled(
                "    ↑ / k          ",
                Style::default()
                    .fg(COLOR_SUCCESS)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("Move cursor up", Style::default().fg(COLOR_MUTED)),
        ]),
        Line::from(vec![
            Span::styled(
                "    ↓ / j          ",
                Style::default()
                    .fg(COLOR_SUCCESS)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("Move cursor down", Style::default().fg(COLOR_MUTED)),
        ]),
        Line::from(vec![
            Span::styled(
                "    Home / g / ^U  ",
                Style::default()
                    .fg(COLOR_SUCCESS)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("Go to first item", Style::default().fg(COLOR_MUTED)),
        ]),
        Line::from(vec![
            Span::styled(
                "    End / G / ^D   ",
                Style::default()
                    .fg(COLOR_SUCCESS)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("Go to last item", Style::default().fg(COLOR_MUTED)),
        ]),
        Line::from(vec![
            Span::styled(
                "    PgUp / PgDn    ",
                Style::default()
                    .fg(COLOR_SUCCESS)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("Move by one page", Style::default().fg(COLOR_MUTED)),
        ]),
        Line::from(""),
        Line::from(vec![Span::styled(
            "  Search",
            Style::default()
                .fg(COLOR_ACCENT)
                .add_modifier(Modifier::BOLD),
        )]),
        Line::from(vec![
            Span::styled(
                "    /",
                Style::default()
                    .fg(COLOR_SUCCESS)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "             Start search (type to filter by name)",
                Style::default().fg(COLOR_MUTED),
            ),
        ]),
        Line::from(vec![
            Span::styled(
                "    @author:name",
                Style::default()
                    .fg(COLOR_SUCCESS)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "  Filter by branch author",
                Style::default().fg(COLOR_MUTED),
            ),
        ]),
        Line::from(vec![
            Span::styled(
                "    @author:me",
                Style::default()
                    .fg(COLOR_SUCCESS)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "    Filter by your branches",
                Style::default().fg(COLOR_MUTED),
            ),
        ]),
        Line::from(vec![
            Span::styled(
                "    Tab / Enter",
                Style::default()
                    .fg(COLOR_SUCCESS)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "   Accept autocomplete suggestion",
                Style::default().fg(COLOR_MUTED),
            ),
        ]),
        Line::from(vec![
            Span::styled(
                "    Esc",
                Style::default()
                    .fg(COLOR_SUCCESS)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "           Exit search and clear query",
                Style::default().fg(COLOR_MUTED),
            ),
        ]),
        Line::from(vec![
            Span::styled(
                "    Enter",
                Style::default()
                    .fg(COLOR_SUCCESS)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "         Exit search and keep filter",
                Style::default().fg(COLOR_MUTED),
            ),
        ]),
        Line::from(""),
        Line::from(vec![Span::styled(
            "  Filters",
            Style::default()
                .fg(COLOR_ACCENT)
                .add_modifier(Modifier::BOLD),
        )]),
        Line::from(vec![
            Span::styled(
                "    F",
                Style::default()
                    .fg(COLOR_SUCCESS)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "             Toggle filter bar visibility",
                Style::default().fg(COLOR_MUTED),
            ),
        ]),
        Line::from(vec![
            Span::styled(
                "    1 / F1",
                Style::default()
                    .fg(COLOR_SUCCESS)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "        Show safe merged branches",
                Style::default().fg(COLOR_MUTED),
            ),
        ]),
        Line::from(vec![
            Span::styled(
                "    2 / F2",
                Style::default()
                    .fg(COLOR_SUCCESS)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "        Show upstream gone branches",
                Style::default().fg(COLOR_MUTED),
            ),
        ]),
        Line::from(vec![
            Span::styled(
                "    3 / F3",
                Style::default()
                    .fg(COLOR_SUCCESS)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "        Show unmerged branches",
                Style::default().fg(COLOR_MUTED),
            ),
        ]),
        Line::from(vec![
            Span::styled(
                "    4 / F4",
                Style::default()
                    .fg(COLOR_SUCCESS)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "        Show all branches",
                Style::default().fg(COLOR_MUTED),
            ),
        ]),
        Line::from(vec![
            Span::styled(
                "    Tab",
                Style::default()
                    .fg(COLOR_SUCCESS)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "           Cycle through filters",
                Style::default().fg(COLOR_MUTED),
            ),
        ]),
        Line::from(""),
        Line::from(vec![Span::styled(
            "  Sorting",
            Style::default()
                .fg(COLOR_ACCENT)
                .add_modifier(Modifier::BOLD),
        )]),
        Line::from(vec![
            Span::styled(
                "    s",
                Style::default()
                    .fg(COLOR_SUCCESS)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "             Cycle sort modes:",
                Style::default().fg(COLOR_MUTED),
            ),
        ]),
        Line::from(vec![Span::styled(
            "                  Status → Name → Active → Created",
            Style::default().fg(COLOR_MUTED),
        )]),
        Line::from(""),
        Line::from(vec![Span::styled(
            "  Selection",
            Style::default()
                .fg(COLOR_ACCENT)
                .add_modifier(Modifier::BOLD),
        )]),
        Line::from(vec![
            Span::styled(
                "    Space",
                Style::default()
                    .fg(COLOR_SUCCESS)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "         Toggle selection for current branch",
                Style::default().fg(COLOR_MUTED),
            ),
        ]),
        Line::from(vec![
            Span::styled(
                "    a",
                Style::default()
                    .fg(COLOR_SUCCESS)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "             Select/deselect all safe branches",
                Style::default().fg(COLOR_MUTED),
            ),
        ]),
        Line::from(vec![
            Span::styled(
                "    c",
                Style::default()
                    .fg(COLOR_SUCCESS)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "             Clear all selections",
                Style::default().fg(COLOR_MUTED),
            ),
        ]),
        Line::from(vec![Span::styled(
            "    Not selectable: ⊘ protected  ⑂ protected-wt  ◉ current",
            Style::default().fg(COLOR_MUTED),
        )]),
        Line::from(""),
        Line::from(vec![Span::styled(
            "  Actions",
            Style::default()
                .fg(COLOR_ACCENT)
                .add_modifier(Modifier::BOLD),
        )]),
        Line::from(vec![
            Span::styled(
                "    Enter",
                Style::default()
                    .fg(COLOR_SUCCESS)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "         Confirm and delete selected branches",
                Style::default().fg(COLOR_MUTED),
            ),
        ]),
        Line::from(vec![
            Span::styled(
                "    f",
                Style::default()
                    .fg(COLOR_DANGER)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "             Toggle force mode (allow unmerged)",
                Style::default().fg(COLOR_MUTED),
            ),
        ]),
        Line::from(vec![
            Span::styled(
                "    d",
                Style::default()
                    .fg(COLOR_WARNING)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "             Toggle dry run mode (preview only)",
                Style::default().fg(COLOR_MUTED),
            ),
        ]),
        Line::from(""),
        Line::from(vec![Span::styled(
            "  PR Integration (--github / --bitbucket)",
            Style::default()
                .fg(COLOR_ACCENT)
                .add_modifier(Modifier::BOLD),
        )]),
        Line::from(vec![
            Span::styled(
                "    o",
                Style::default()
                    .fg(COLOR_PR_MERGED)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "             Open PR in browser (if PR exists)",
                Style::default().fg(COLOR_MUTED),
            ),
        ]),
        Line::from(vec![Span::styled(
            "    PR Status: 🟢 merged  🟡 open  🔴 closed  ⚪ none",
            Style::default().fg(COLOR_MUTED),
        )]),
        Line::from(""),
        help_heading("  Layout & scrolling"),
        // This help's key column is one narrower than `help_row`'s
        Line::from(vec![
            Span::styled(
                "    v             ",
                Style::default()
                    .fg(COLOR_SUCCESS)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "Cycle layout: auto/side by side/stacked/list",
                Style::default().fg(COLOR_MUTED),
            ),
        ]),
        Line::from(vec![
            Span::styled(
                "    J / K         ",
                Style::default()
                    .fg(COLOR_SUCCESS)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "Scroll the details pane (also Shift+↓ / ↑)",
                Style::default().fg(COLOR_MUTED),
            ),
        ]),
        Line::from(vec![
            Span::styled(
                "    Mouse wheel   ",
                Style::default()
                    .fg(COLOR_SUCCESS)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "Scroll the pane under the pointer",
                Style::default().fg(COLOR_MUTED),
            ),
        ]),
        Line::from(vec![
            Span::styled(
                "    ↑↓ PgUp PgDn  ",
                Style::default()
                    .fg(COLOR_SUCCESS)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "Scroll this help; any other key closes",
                Style::default().fg(COLOR_MUTED),
            ),
        ]),
        Line::from(""),
        Line::from(vec![Span::styled(
            "  Other",
            Style::default()
                .fg(COLOR_ACCENT)
                .add_modifier(Modifier::BOLD),
        )]),
        Line::from(vec![
            Span::styled(
                "    i",
                Style::default()
                    .fg(COLOR_SUCCESS)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "             Show info about the tool",
                Style::default().fg(COLOR_MUTED),
            ),
        ]),
        Line::from(vec![
            Span::styled(
                "    w",
                Style::default()
                    .fg(COLOR_SUCCESS)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "             Switch to the worktree view",
                Style::default().fg(COLOR_MUTED),
            ),
        ]),
        Line::from(vec![
            Span::styled(
                "    y",
                Style::default()
                    .fg(COLOR_SUCCESS)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "             Copy the branch name to the clipboard",
                Style::default().fg(COLOR_MUTED),
            ),
        ]),
        Line::from(vec![
            Span::styled(
                "    r",
                Style::default()
                    .fg(COLOR_SUCCESS)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "             Reload the branch list",
                Style::default().fg(COLOR_MUTED),
            ),
        ]),
        Line::from(vec![
            Span::styled(
                "    R",
                Style::default()
                    .fg(COLOR_SUCCESS)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "             Fetch the remote, reload, re-fetch PRs",
                Style::default().fg(COLOR_MUTED),
            ),
        ]),
        Line::from(vec![
            Span::styled(
                "    ?",
                Style::default()
                    .fg(COLOR_SUCCESS)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "             Show this help",
                Style::default().fg(COLOR_MUTED),
            ),
        ]),
        Line::from(vec![
            Span::styled(
                "    q / Esc Esc",
                Style::default()
                    .fg(COLOR_SUCCESS)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("   Quit application", Style::default().fg(COLOR_MUTED)),
        ]),
        Line::from(""),
    ];
    let lines = if view == ViewMode::Worktrees {
        worktree_help_lines()
    } else {
        lines
    };

    let scroll = render_modal(
        frame,
        " 📖 Help - Keyboard Shortcuts ",
        COLOR_ACCENT,
        70,
        lines,
        None,
        app.modal_scroll,
        MODAL_SCROLL_HINT,
    );
    app.modal_scroll = scroll.offset;
    app.modal_viewport = scroll.viewport;
}

/// Render the info modal
fn render_info_modal(frame: &mut Frame, app: &mut App) {
    let view = app.view;

    let lines = vec![
        Line::from(""),
        Line::from(vec![Span::styled(
            "  Local Git Branch Cleanup TUI",
            Style::default()
                .fg(COLOR_ACCENT)
                .add_modifier(Modifier::BOLD),
        )]),
        Line::from(""),
        Line::from(vec![Span::styled(
            "  A terminal interface to help you clean up local Git branches",
            Style::default().fg(COLOR_MUTED),
        )]),
        Line::from(vec![Span::styled(
            "  that are no longer needed.",
            Style::default().fg(COLOR_MUTED),
        )]),
        Line::from(""),
        Line::from(vec![Span::styled(
            "  Branch Status Types:",
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )]),
        Line::from(""),
        Line::from(vec![
            Span::styled("    ✓ merged    ", Style::default().fg(COLOR_SUCCESS)),
            Span::styled(
                "Branch has been merged into trunk",
                Style::default().fg(COLOR_MUTED),
            ),
        ]),
        Line::from(vec![
            Span::styled("    ↑ pr-merged ", Style::default().fg(COLOR_ACCENT)),
            Span::styled(
                "PR merged; remote branch still exists",
                Style::default().fg(COLOR_MUTED),
            ),
        ]),
        Line::from(vec![
            Span::styled("    ↕ pr-diverged ", Style::default().fg(COLOR_DIVERGED)),
            Span::styled(
                "PR merged but local diverged from remote",
                Style::default().fg(COLOR_MUTED),
            ),
        ]),
        Line::from(vec![
            Span::styled("    ↗ gone      ", Style::default().fg(COLOR_SUCCESS)),
            Span::styled(
                "Remote tracking branch was deleted",
                Style::default().fg(COLOR_MUTED),
            ),
        ]),
        Line::from(vec![
            Span::styled("    ! unmerged  ", Style::default().fg(COLOR_WARNING)),
            Span::styled(
                "Has commits not in trunk (requires force)",
                Style::default().fg(COLOR_MUTED),
            ),
        ]),
        Line::from(vec![
            Span::styled("    ○ local     ", Style::default().fg(COLOR_LOCAL)),
            Span::styled(
                "Never pushed to the remote (requires force)",
                Style::default().fg(COLOR_MUTED),
            ),
        ]),
        Line::from(vec![
            Span::styled("    ⊘ protected ", Style::default().fg(COLOR_DANGER)),
            Span::styled(
                "Protected branch (main/master/develop)",
                Style::default().fg(COLOR_MUTED),
            ),
        ]),
        Line::from(vec![
            Span::styled("    ⑂ protected-wt ", Style::default().fg(COLOR_DANGER)),
            Span::styled(
                "Checked out in another worktree",
                Style::default().fg(COLOR_MUTED),
            ),
        ]),
        Line::from(vec![
            Span::styled("    ◉ current   ", Style::default().fg(COLOR_CURRENT)),
            Span::styled(
                "Currently checked out branch",
                Style::default().fg(COLOR_MUTED),
            ),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("  Press ", Style::default().fg(COLOR_MUTED)),
            Span::styled(
                "?",
                Style::default()
                    .fg(COLOR_ACCENT)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" for keyboard shortcuts", Style::default().fg(COLOR_MUTED)),
        ]),
        Line::from(""),
        Line::from(vec![Span::styled(
            "  Press any key to close",
            Style::default().fg(COLOR_MUTED),
        )]),
        Line::from(""),
    ];
    let lines = if view == ViewMode::Worktrees {
        worktree_info_lines()
    } else {
        lines
    };

    let scroll = render_modal(
        frame,
        " ℹ️  About ",
        COLOR_ACCENT,
        65,
        lines,
        None,
        app.modal_scroll,
        MODAL_SCROLL_HINT,
    );
    app.modal_scroll = scroll.offset;
    app.modal_viewport = scroll.viewport;
}

// ---------------------------------------------------------------------------
// Worktree view (`--worktrees` / `w`)
// ---------------------------------------------------------------------------

/// Get the color for a worktree state
/// Width of the worktree table's Name column: the longest worktree name in
/// display columns, at least the "Name" header and at most
/// `MAX_WORKTREE_NAME_WIDTH` (longer names are cut off)
fn worktree_name_width(worktrees: &[crate::worktree_view::WorktreeInfo]) -> u16 {
    const MAX_WORKTREE_NAME_WIDTH: usize = 30;
    let longest = worktrees
        .iter()
        .map(|w| Line::from(w.name()).width())
        .max()
        .unwrap_or(0);
    longest.clamp("Name".len(), MAX_WORKTREE_NAME_WIDTH) as u16
}

fn get_worktree_state_color(state: &WorktreeState) -> Color {
    match state {
        WorktreeState::Current => COLOR_CURRENT,
        WorktreeState::Prunable => COLOR_DANGER,
        WorktreeState::Locked => COLOR_MUTED,
        WorktreeState::Dirty => COLOR_WARNING,
        WorktreeState::Bare => COLOR_MUTED,
        WorktreeState::Detached => COLOR_LOCAL,
        WorktreeState::Branch(status) => get_status_color(status),
        WorktreeState::Unknown => COLOR_MUTED,
    }
}

/// One-line explanation of a worktree state for the details pane
fn worktree_state_explanation(state: &WorktreeState) -> &'static str {
    match state {
        WorktreeState::Current => "The worktree this tool runs in",
        WorktreeState::Prunable => "Directory is gone; removed with git worktree prune",
        WorktreeState::Locked => "Locked with git worktree lock; never removed",
        WorktreeState::Dirty => "Uncommitted changes; removal needs force",
        WorktreeState::Bare => "Bare repository; has no working tree",
        WorktreeState::Detached => "HEAD is detached; no branch checked out",
        WorktreeState::Branch(_) => "Clean; showing the branch's status",
        WorktreeState::Unknown => "Branch not found among local branches",
    }
}

/// Split `text` into lines of at most `width` characters at word boundaries
fn wrap_words(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        if !current.is_empty() && current.chars().count() + 1 + word.chars().count() > width {
            lines.push(std::mem::take(&mut current));
        }
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(word);
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines
}

/// Legend items of the worktree table, one group per item
fn worktree_legend_groups() -> Vec<Vec<Span<'static>>> {
    vec![
        legend_item("◆", "main", COLOR_ACCENT),
        legend_item("◉", "current", COLOR_CURRENT),
        legend_item("✗", "prunable", COLOR_DANGER),
        legend_item("🔒", "locked", COLOR_MUTED),
        legend_item("✎", "dirty", COLOR_WARNING),
        legend_item("○", "detached", COLOR_LOCAL),
        vec![Span::styled("│", Style::default().fg(COLOR_MUTED))],
        vec![
            Span::styled("M", Style::default().fg(COLOR_WARNING)),
            Span::styled(" modified", Style::default().fg(COLOR_MUTED)),
        ],
        vec![
            Span::styled("S", Style::default().fg(COLOR_WARNING)),
            Span::styled(" staged", Style::default().fg(COLOR_MUTED)),
        ],
        vec![
            Span::styled("N", Style::default().fg(COLOR_WARNING)),
            Span::styled(" new", Style::default().fg(COLOR_MUTED)),
        ],
    ]
}

/// Render the worktree list as a table
fn render_worktree_list(
    frame: &mut Frame,
    app: &mut App,
    area: Rect,
    details_hidden: bool,
    compact: bool,
) {
    let area = render_legend(frame, area, worktree_legend_groups(), compact);
    app.list_area = area;
    let table_area = list_table_area(area);
    // Rows that fit, for page up / down
    app.visible_height = table_area.height as usize;

    let show_pr = app.pr_provider.is_some();
    // Name is sized to the longest worktree name (over all worktrees, so it
    // does not jump when filters change), at most a third of the table;
    // Branch takes all remaining space, since branch names are usually much
    // longer than directory names
    let name_width = app
        .worktrees
        .as_ref()
        .map_or(0, |wt| worktree_name_width(&wt.worktrees))
        .min(table_area.width / 3)
        .max(4);
    let cols = responsive::worktree_columns(table_area.width, name_width, show_pr);

    let mut header_labels = vec!["", "", "Name", "Branch"];
    let mut widths = vec![
        Constraint::Length(2),          // Cursor
        Constraint::Length(4),          // Checkbox / main / locked marker
        Constraint::Length(name_width), // Name
        Constraint::Fill(1),            // Branch
    ];
    if cols.state_label {
        header_labels.push("State");
        widths.push(Constraint::Length(14));
    } else {
        header_labels.push("St");
        widths.push(Constraint::Length(responsive::ICON_COLUMN_WIDTH));
    }
    if cols.changes {
        header_labels.push("Changes");
        widths.push(Constraint::Length(10));
    }
    if cols.last_commit {
        header_labels.push("Last Commit");
        widths.push(Constraint::Length(15));
    }
    if cols.pr {
        header_labels.push("PR");
        widths.push(Constraint::Length(10));
    }
    let branch_width = column_width(&widths, table_area.width, 3);

    let header = Row::new(header_labels.iter().map(|h| {
        Cell::from(*h).style(
            Style::default()
                .fg(COLOR_ACCENT)
                .add_modifier(Modifier::BOLD),
        )
    }))
    .height(1);

    let filtered = app.filtered_worktrees();
    let selected = app.worktrees.as_ref().map(|wt| &wt.selected);
    let rows: Vec<Row> = filtered
        .iter()
        .enumerate()
        .map(|(idx, wt)| {
            let is_cursor = idx == app.selected_index;
            let is_checked = selected.is_some_and(|s| s.contains(&wt.entry.path));
            let state = wt.state();

            let (mark, mark_style) = if wt.entry.is_main {
                (" ◆", Style::default().fg(COLOR_ACCENT))
            } else if wt.is_locked() {
                (" 🔒", Style::default().fg(COLOR_MUTED))
            } else if !app.worktree_remove_mode {
                // Read-only explorer: no checkboxes until `x` turns removal on
                ("   ", Style::default())
            } else if is_checked {
                ("[✓]", Style::default().fg(COLOR_SELECTED))
            } else if wt.is_selectable(app.force_mode) {
                ("[ ]", Style::default().fg(COLOR_ACCENT))
            } else if wt.is_current || wt.entry.is_bare {
                ("   ", Style::default())
            } else if wt.changes == Changes::Loading {
                (" … ", Style::default().fg(COLOR_MUTED))
            } else {
                // Dirty without force mode, or status unreadable
                (" - ", Style::default().fg(COLOR_MUTED))
            };

            let (changes, changes_style) = match &wt.changes {
                Changes::Loading => ("…".to_string(), Style::default().fg(COLOR_MUTED)),
                Changes::NotApplicable => ("-".to_string(), Style::default().fg(COLOR_MUTED)),
                Changes::Failed(_) => ("error".to_string(), Style::default().fg(COLOR_DANGER)),
                Changes::Loaded(c) if c.is_clean() => {
                    (c.compact(), Style::default().fg(COLOR_MUTED))
                }
                Changes::Loaded(c) => (c.compact(), Style::default().fg(COLOR_WARNING)),
            };

            let last_commit = if wt.is_prunable() || wt.commit.relative.is_empty() {
                "-".to_string()
            } else {
                wt.commit.relative.clone()
            };

            let name_style = if is_cursor {
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD)
            } else if is_checked {
                Style::default().fg(COLOR_SELECTED)
            } else {
                Style::default().fg(COLOR_MUTED)
            };

            // Name and branch are cut with `…` when too long (full values in details)
            let state_text = if cols.state_label {
                format!("{} {}", state.icon(), state.label())
            } else {
                state.icon().to_string()
            };
            let mut cells = vec![
                Cell::from(if is_cursor { "▶" } else { " " })
                    .style(Style::default().fg(COLOR_ACCENT)),
                Cell::from(mark).style(mark_style),
                Cell::from(responsive::truncate_end(&wt.name(), name_width as usize))
                    .style(name_style),
                Cell::from(responsive::truncate_end(&wt.branch_label(), branch_width))
                    .style(Style::default().fg(COLOR_MUTED)),
                Cell::from(state_text).style(Style::default().fg(get_worktree_state_color(&state))),
            ];
            if cols.changes {
                cells.push(Cell::from(changes).style(changes_style));
            }
            if cols.last_commit {
                cells.push(Cell::from(last_commit).style(Style::default().fg(COLOR_MUTED)));
            }
            if cols.pr {
                let (pr_text, pr_style) = match &wt.pr_info {
                    Some(pr) => (
                        format!("{} #{}", pr.state.icon(), pr.number),
                        Style::default().fg(get_pr_state_color(&pr.state)),
                    ),
                    None => ("⚪".to_string(), Style::default().fg(COLOR_MUTED)),
                };
                cells.push(Cell::from(pr_text).style(pr_style));
            }

            let row = Row::new(cells);
            if is_cursor {
                row.style(Style::default().bg(Color::Rgb(30, 35, 45)))
            } else if is_checked {
                row.style(Style::default().bg(Color::Rgb(40, 30, 40)))
            } else {
                row
            }
        })
        .collect();
    let row_count = rows.len();

    let loading = app
        .worktrees
        .as_ref()
        .is_some_and(|wt| wt.worktrees.iter().any(|w| w.changes == Changes::Loading));
    let title = format!(
        " Worktrees ({} shown, {} selected){} ",
        filtered.len(),
        app.selected_count(),
        if loading { " - reading status…" } else { "" }
    );
    frame.render_widget(list_block(title, details_hidden), area);

    let table = Table::new(rows, widths)
        .header(header)
        .row_highlight_style(Style::default().add_modifier(Modifier::BOLD));
    let offset = render_list_table(
        frame,
        table,
        row_count,
        app.selected_index,
        app.scroll_offset,
        area,
        table_area,
    );
    app.scroll_offset = offset;
}

/// Render the details pane for the worktree under the cursor
/// Returns the details scroll actually shown (clamped to the content)
fn render_worktree_details(frame: &mut Frame, app: &App, area: Rect, scroll: usize) -> usize {
    let label = |text: &'static str| Span::styled(text, Style::default().fg(COLOR_MUTED));
    let heading = |text: &'static str| {
        Line::from(Span::styled(
            text,
            Style::default()
                .fg(COLOR_MUTED)
                .add_modifier(Modifier::BOLD),
        ))
    };
    let max_width = area.width.saturating_sub(6).max(10) as usize;
    let mut lines = vec![Line::from("")];

    let Some(wt) = app.selected_worktree() else {
        lines.push(Line::from(Span::styled(
            "  No worktree selected",
            Style::default()
                .fg(COLOR_MUTED)
                .add_modifier(Modifier::ITALIC),
        )));
        return render_details_block(frame, lines, area, scroll);
    };

    let state = wt.state();
    let state_color = get_worktree_state_color(&state);
    let mut title = vec![
        Span::styled("  ", Style::default()),
        Span::styled(
            wt.name(),
            Style::default()
                .fg(COLOR_ACCENT)
                .add_modifier(Modifier::BOLD),
        ),
    ];
    if wt.entry.is_main {
        title.push(Span::styled(" ◆ main", Style::default().fg(COLOR_ACCENT)));
    }
    lines.push(Line::from(title));
    lines.push(Line::from(""));

    // State
    lines.push(Line::from(vec![
        label("  State: "),
        Span::styled(
            format!("{} {}", state.icon(), state.label()),
            Style::default().fg(state_color),
        ),
    ]));
    lines.push(Line::from(Span::styled(
        format!("  {}", worktree_state_explanation(&state)),
        Style::default()
            .fg(COLOR_MUTED)
            .add_modifier(Modifier::ITALIC),
    )));
    if let Some(reason) = &wt.entry.locked {
        let reason = if reason.is_empty() {
            "no reason given"
        } else {
            reason
        };
        lines.push(Line::from(vec![
            label("  Lock reason: "),
            Span::styled(reason.to_string(), Style::default().fg(COLOR_WARNING)),
        ]));
    }
    if let Some(reason) = &wt.entry.prunable {
        lines.push(Line::from(label("  Prunable:")));
        for part in wrap_words(reason, max_width) {
            lines.push(Line::from(Span::styled(
                format!("  {part}"),
                Style::default().fg(COLOR_DANGER),
            )));
        }
    }
    lines.push(Line::from(""));

    // Path
    lines.push(heading("  Path:"));
    let path = wt.entry.path.to_string_lossy();
    let path_chars: Vec<char> = path.chars().collect();
    for chunk in path_chars.chunks(max_width) {
        lines.push(Line::from(Span::styled(
            format!("  {}", chunk.iter().collect::<String>()),
            Style::default().fg(Color::White),
        )));
    }
    lines.push(Line::from(""));

    // Branch, upstream, divergence
    lines.push(Line::from(vec![
        label("  Branch: "),
        Span::styled(wt.branch_label(), Style::default().fg(COLOR_SUCCESS)),
    ]));
    if let Some(status) = &wt.branch_status {
        lines.push(Line::from(vec![
            label("  Branch status: "),
            Span::styled(
                format!("{} {}", status.icon(), status.label()),
                Style::default().fg(get_status_color(status)),
            ),
        ]));
    }
    if wt.entry.branch.is_some() {
        match &wt.upstream {
            Some(upstream) => {
                lines.push(Line::from(vec![
                    label("  Upstream: "),
                    Span::styled(upstream.clone(), Style::default().fg(COLOR_SUCCESS)),
                ]));
                if let (Some(ahead), Some(behind)) = (wt.ahead, wt.behind) {
                    if ahead > 0 || behind > 0 {
                        lines.push(Line::from(vec![
                            label("  Divergence: "),
                            Span::styled(format!("↑{ahead}"), Style::default().fg(COLOR_SUCCESS)),
                            Span::styled(" ", Style::default()),
                            Span::styled(format!("↓{behind}"), Style::default().fg(COLOR_WARNING)),
                        ]));
                    }
                }
            }
            None => lines.push(Line::from(vec![
                label("  Upstream: "),
                Span::styled("none", Style::default().fg(COLOR_WARNING)),
            ])),
        }
    }
    if !wt.entry.head.is_empty() {
        lines.push(Line::from(vec![
            label("  HEAD: "),
            Span::styled(
                short_sha(&wt.entry.head).to_string(),
                Style::default().fg(COLOR_WARNING),
            ),
        ]));
    }
    lines.push(Line::from(""));

    // Last commit
    if !wt.commit.subject.is_empty() || !wt.commit.author.is_empty() {
        lines.push(heading("  Last Commit:"));
        lines.push(Line::from(vec![
            Span::styled("  ", Style::default()),
            Span::styled(wt.commit.author.clone(), Style::default().fg(COLOR_MUTED)),
            Span::styled(
                format!(", {}", wt.commit.relative),
                Style::default()
                    .fg(COLOR_MUTED)
                    .add_modifier(Modifier::ITALIC),
            ),
        ]));
        for part in wrap_words(&wt.commit.subject, max_width) {
            lines.push(Line::from(Span::styled(
                format!("  {part}"),
                Style::default().fg(Color::White),
            )));
        }
        lines.push(Line::from(""));
    }

    // Changes
    match &wt.changes {
        Changes::Loading => lines.push(Line::from(vec![
            label("  Changes: "),
            Span::styled("… reading git status", Style::default().fg(COLOR_MUTED)),
        ])),
        Changes::NotApplicable => lines.push(Line::from(vec![
            label("  Changes: "),
            Span::styled("- (no working tree)", Style::default().fg(COLOR_MUTED)),
        ])),
        Changes::Failed(e) => {
            lines.push(Line::from(vec![
                label("  Changes: "),
                Span::styled("git status failed", Style::default().fg(COLOR_DANGER)),
            ]));
            for part in wrap_words(e, max_width) {
                lines.push(Line::from(Span::styled(
                    format!("  {part}"),
                    Style::default().fg(COLOR_DANGER),
                )));
            }
        }
        Changes::Loaded(c) => {
            let color = if c.is_clean() {
                COLOR_SUCCESS
            } else {
                COLOR_WARNING
            };
            lines.push(Line::from(vec![
                label("  Changes: "),
                Span::styled(c.compact(), Style::default().fg(color)),
            ]));
            // One block per non-empty group: Modified, Staged, New
            for group in c.groups() {
                lines.push(Line::from(Span::styled(
                    format!("  {} ({})", group.label, group.count),
                    Style::default().fg(COLOR_WARNING),
                )));
                for path in group.paths {
                    lines.push(Line::from(Span::styled(
                        format!("    {path}"),
                        Style::default().fg(COLOR_MUTED),
                    )));
                }
                if group.count > group.paths.len() {
                    lines.push(Line::from(Span::styled(
                        format!("    … and {} more", group.count - group.paths.len()),
                        Style::default()
                            .fg(COLOR_MUTED)
                            .add_modifier(Modifier::ITALIC),
                    )));
                }
            }
        }
    }

    // PR info
    if app.pr_provider.is_some() {
        lines.push(Line::from(""));
        match &wt.pr_info {
            Some(pr) => {
                lines.push(heading("  Pull Request:"));
                lines.push(Line::from(vec![
                    Span::styled("  ", Style::default()),
                    Span::styled(
                        pr.state.icon(),
                        Style::default().fg(get_pr_state_color(&pr.state)),
                    ),
                    Span::styled(" PR #", Style::default().fg(COLOR_MUTED)),
                    Span::styled(pr.number.to_string(), Style::default().fg(COLOR_ACCENT)),
                    Span::styled(
                        format!(" ({})", pr.state.label()),
                        Style::default().fg(get_pr_state_color(&pr.state)),
                    ),
                ]));
                for part in wrap_words(&pr.title, max_width) {
                    lines.push(Line::from(Span::styled(
                        format!("  {part}"),
                        Style::default().fg(Color::White),
                    )));
                }
            }
            None => lines.push(Line::from(vec![
                label("  Pull Request: "),
                Span::styled(
                    "none",
                    Style::default()
                        .fg(COLOR_MUTED)
                        .add_modifier(Modifier::ITALIC),
                ),
            ])),
        }
    }

    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        label("  Press "),
        Span::styled(
            "y",
            Style::default()
                .fg(COLOR_ACCENT)
                .add_modifier(Modifier::BOLD),
        ),
        label(" to copy the path"),
    ]));

    render_details_block(frame, lines, area, scroll)
}

/// Draw the bordered `Details` pane, scrolled down by `scroll` rows.
/// Returns the scroll actually shown.
fn render_details_block(frame: &mut Frame, lines: Vec<Line>, area: Rect, scroll: usize) -> usize {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(COLOR_MUTED))
        .title(" Details ")
        .title_style(Style::default().fg(COLOR_ACCENT));
    let hint = Line::from(vec![
        Span::raw(" "),
        Span::styled(
            "J/K",
            Style::default()
                .fg(COLOR_ACCENT)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" scroll ", Style::default().fg(COLOR_MUTED)),
    ]);
    render_scrolled(frame, block, lines, area, scroll, Some(hint)).offset
}

/// Draw `lines` wrapped inside `block`, scrolled down by `offset` rows
/// (clamped so the view does not run past the end). When they do not all
/// fit, draws a scrollbar on the block's right border and `hint` on its
/// bottom border. Returns the scroll shown.
fn render_scrolled(
    frame: &mut Frame,
    block: Block,
    lines: Vec<Line>,
    area: Rect,
    offset: usize,
    hint: Option<Line>,
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

/// Footer key hints of the worktree view, one group per key (the shared
/// `v` / `o` / `?` / `q` hints are added by `footer_hint_groups`)
fn worktree_footer_hints(app: &App) -> Vec<Vec<Span<'static>>> {
    let mut groups = vec![key_hint("↑↓", "nav", COLOR_ACCENT)];
    if app.worktree_remove_mode {
        groups.extend([
            key_hint("Space", "sel", COLOR_ACCENT),
            key_hint("a", "stale", COLOR_ACCENT),
            key_hint("c", "clr", COLOR_ACCENT),
            vec![Span::styled(
                "f force",
                toggle_style(app.force_mode, COLOR_DANGER),
            )],
            vec![Span::styled(
                "d dry",
                toggle_style(app.dry_run, COLOR_WARNING),
            )],
            key_hint("Enter", "remove", COLOR_SELECTED),
        ]);
    }
    groups.extend([
        vec![Span::styled(
            "x remove mode",
            toggle_style(app.worktree_remove_mode, COLOR_DANGER),
        )],
        key_hint("y", "copy path", COLOR_ACCENT),
        key_hint("/", "search", COLOR_ACCENT),
        key_hint("w", "branches", COLOR_ACCENT),
    ]);
    groups
}

/// Body of the confirmation modal in the worktree view
fn worktree_confirmation_lines(app: &App) -> Vec<Line<'static>> {
    use crate::worktree_view::RemovalAction;

    let Some(view) = app.worktrees.as_ref() else {
        return Vec::new();
    };
    let selected: Vec<_> = view
        .selected_infos()
        .into_iter()
        .filter_map(|w| w.removal_action(app.force_mode).map(|a| (w, a)))
        .collect();

    let mut lines = vec![Line::from("")];
    let count = Span::styled(
        selected.len().to_string(),
        Style::default()
            .fg(if app.dry_run {
                COLOR_WARNING
            } else {
                COLOR_DANGER
            })
            .add_modifier(Modifier::BOLD),
    );
    if app.dry_run {
        lines.push(Line::from(vec![
            Span::styled("  Preview removing ", Style::default().fg(COLOR_MUTED)),
            count,
            Span::styled(" worktree(s) ", Style::default().fg(COLOR_MUTED)),
            Span::styled(
                "(Dry Run)",
                Style::default()
                    .fg(COLOR_WARNING)
                    .add_modifier(Modifier::BOLD),
            ),
        ]));
    } else {
        lines.push(Line::from(vec![
            Span::styled("  Remove ", Style::default().fg(COLOR_MUTED)),
            count,
            Span::styled(" worktree(s)?", Style::default().fg(COLOR_MUTED)),
        ]));
    }
    lines.push(Line::from(""));

    for (wt, action) in selected.iter() {
        let how = match action {
            RemovalAction::Remove => "remove",
            RemovalAction::ForceRemove => "remove --force",
            RemovalAction::Prune => "prune",
        };
        lines.push(Line::from(vec![
            Span::styled("  • ", Style::default().fg(COLOR_MUTED)),
            Span::styled(wt.name(), Style::default().fg(COLOR_ACCENT)),
            Span::styled(
                format!(" ({}) ", wt.branch_label()),
                Style::default().fg(COLOR_MUTED),
            ),
            Span::styled(
                how,
                Style::default().fg(get_worktree_state_color(&wt.state())),
            ),
        ]));
    }
    let forced = selected
        .iter()
        .filter(|(_, a)| *a == RemovalAction::ForceRemove)
        .count();
    if forced > 0 {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            format!("  ⚠️  {forced} dirty: uncommitted changes will be lost"),
            Style::default().fg(COLOR_DANGER),
        )));
    }
    if selected.iter().any(|(_, a)| *a == RemovalAction::Prune) {
        lines.push(Line::from(Span::styled(
            "  git worktree prune cleans up every prunable worktree",
            Style::default().fg(COLOR_MUTED),
        )));
    }

    lines.push(Line::from(""));
    let (state, style) = if view.delete_branches {
        (
            "ON (-d/-D as in the branch view)",
            Style::default()
                .fg(COLOR_DANGER)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        ("off", Style::default().fg(COLOR_MUTED))
    };
    lines.push(Line::from(vec![
        Span::styled(
            "  b",
            Style::default()
                .fg(COLOR_ACCENT)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" also delete branches: ", Style::default().fg(COLOR_MUTED)),
        Span::styled(state, style),
    ]));
    lines
}

/// One `key  description` row of the help modal
fn help_row(key: &'static str, description: &'static str, color: Color) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("    {key:<15}"),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
        Span::styled(description, Style::default().fg(COLOR_MUTED)),
    ])
}

/// A section heading of the help and info modals
fn help_heading(title: &'static str) -> Line<'static> {
    Line::from(Span::styled(
        title,
        Style::default()
            .fg(COLOR_ACCENT)
            .add_modifier(Modifier::BOLD),
    ))
}

/// Help modal content in the worktree view
fn worktree_help_lines() -> Vec<Line<'static>> {
    vec![
        Line::from(""),
        help_heading("  Navigation"),
        help_row("↑ / k", "Move cursor up", COLOR_SUCCESS),
        help_row("↓ / j", "Move cursor down", COLOR_SUCCESS),
        help_row("Home / g / ^U", "Go to first item", COLOR_SUCCESS),
        help_row("End / G / ^D", "Go to last item", COLOR_SUCCESS),
        help_row("PgUp / PgDn", "Move by one page", COLOR_SUCCESS),
        Line::from(""),
        help_heading("  Search"),
        help_row("/", "Search worktree name, branch or path", COLOR_SUCCESS),
        help_row("Esc", "Exit search and clear query", COLOR_SUCCESS),
        help_row("Enter", "Exit search and keep filter", COLOR_SUCCESS),
        Line::from(""),
        help_heading("  Filters"),
        help_row("F", "Toggle filter bar visibility", COLOR_SUCCESS),
        help_row("1 / F1", "Clean worktrees (no changes)", COLOR_SUCCESS),
        help_row("2 / F2", "Dirty worktrees", COLOR_SUCCESS),
        help_row(
            "3 / F3",
            "Stale worktrees (merged, gone, prunable)",
            COLOR_SUCCESS,
        ),
        help_row("4 / F4", "All worktrees", COLOR_SUCCESS),
        help_row("Tab", "Cycle through filters", COLOR_SUCCESS),
        Line::from(""),
        help_heading("  Selection (removal is off until x)"),
        help_row("x", "Turn remove mode on/off (off at start)", COLOR_DANGER),
        help_row(
            "Space",
            "Toggle selection for current worktree",
            COLOR_SUCCESS,
        ),
        help_row(
            "a",
            "Select/deselect all clean stale worktrees",
            COLOR_SUCCESS,
        ),
        help_row("c", "Clear all selections", COLOR_SUCCESS),
        Line::from(""),
        help_heading("  Actions"),
        help_row(
            "Enter",
            "Confirm and remove selected worktrees",
            COLOR_SUCCESS,
        ),
        help_row(
            "b",
            "In the confirmation: also delete branches",
            COLOR_SUCCESS,
        ),
        help_row(
            "f",
            "Toggle force mode (allow dirty worktrees)",
            COLOR_DANGER,
        ),
        help_row("d", "Toggle dry run mode (preview only)", COLOR_WARNING),
        help_row(
            "y",
            "Copy the worktree path to the clipboard",
            COLOR_SUCCESS,
        ),
        help_row(
            "o",
            "Open PR in browser (--github / --bitbucket)",
            COLOR_PR_MERGED,
        ),
        Line::from(""),
        help_heading("  Layout & scrolling"),
        help_row(
            "v",
            "Cycle layout: auto/side by side/stacked/list",
            COLOR_SUCCESS,
        ),
        help_row(
            "J / K",
            "Scroll the details pane (also Shift+↓ / ↑)",
            COLOR_SUCCESS,
        ),
        help_row(
            "Mouse wheel",
            "Scroll the pane under the pointer",
            COLOR_SUCCESS,
        ),
        help_row(
            "↑↓ PgUp PgDn",
            "Scroll this help; any other key closes",
            COLOR_SUCCESS,
        ),
        Line::from(""),
        help_heading("  Other"),
        help_row("w", "Switch back to the branch view", COLOR_SUCCESS),
        help_row("r", "Reload worktrees and branches", COLOR_SUCCESS),
        help_row("R", "Fetch the remote, reload, re-fetch PRs", COLOR_SUCCESS),
        help_row("i", "Show worktree states", COLOR_SUCCESS),
        help_row("?", "Show this help", COLOR_SUCCESS),
        help_row("q / Esc Esc", "Quit application", COLOR_SUCCESS),
        Line::from(""),
    ]
}

/// Info modal content in the worktree view
fn worktree_info_lines() -> Vec<Line<'static>> {
    let state = |icon_label: &'static str, color: Color, description: &'static str| {
        Line::from(vec![
            Span::styled(format!("    {icon_label:<13}"), Style::default().fg(color)),
            Span::styled(description, Style::default().fg(COLOR_MUTED)),
        ])
    };
    let muted =
        |text: &'static str| Line::from(Span::styled(text, Style::default().fg(COLOR_MUTED)));
    vec![
        Line::from(""),
        help_heading("  Worktree View"),
        Line::from(""),
        muted("  One row per git worktree: main worktree (◆) first, then by"),
        muted("  last activity. Read-only at start: press x to turn on remove"),
        muted("  mode. Locked, main and current worktrees are never"),
        muted("  removed."),
        Line::from(""),
        Line::from(Span::styled(
            "  Worktree States (highest priority first):",
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        state("◉ current", COLOR_CURRENT, "The worktree this tool runs in"),
        state(
            "✗ prunable",
            COLOR_DANGER,
            "Directory gone (git worktree prune)",
        ),
        state("🔒 locked", COLOR_MUTED, "Locked with git worktree lock"),
        state(
            "✎ dirty",
            COLOR_WARNING,
            "Uncommitted changes (needs force)",
        ),
        state("○ detached", COLOR_LOCAL, "No branch checked out"),
        muted("    otherwise the branch status (never protected-wt here)"),
        muted("    (merged, pr-merged, gone, unmerged, local, ...)"),
        Line::from(""),
        muted("  Changes: M modified  S staged  N new  … loading"),
        Line::from(""),
        Line::from(vec![
            Span::styled("  Press ", Style::default().fg(COLOR_MUTED)),
            Span::styled(
                "?",
                Style::default()
                    .fg(COLOR_ACCENT)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" for keyboard shortcuts", Style::default().fg(COLOR_MUTED)),
        ]),
        muted("  Press any key to close"),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::BranchInfo;
    use ratatui::{backend::TestBackend, Terminal};

    fn footer_text(app: &App) -> String {
        let mut terminal = Terminal::new(TestBackend::new(120, 3)).unwrap();
        terminal
            .draw(|frame| render_footer(frame, app, frame.area(), false))
            .unwrap();
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    #[test]
    fn footer_shows_the_update_notice_when_one_is_set() {
        let mut app = App::new(Vec::new(), "/repo".into(), "main".into(), String::new());
        assert!(!footer_text(&app).contains("Update available"));

        app.update_notice = Some("Update available: 9.9.9 (run: x update)".into());
        assert!(footer_text(&app).contains("Update available: 9.9.9 (run: x update)"));
    }

    fn details_text(app: &App) -> String {
        let mut terminal = Terminal::new(TestBackend::new(250, 30)).unwrap();
        terminal
            .draw(|frame| {
                render_details_pane(frame, app, frame.area(), 0);
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn worktree_branch(path: &str) -> BranchInfo {
        BranchInfo {
            name: "feature".to_string(),
            upstream: None,
            last_commit_relative: "1 day ago".to_string(),
            status: BranchStatus::ProtectedWorktree,
            last_commit_sha: "abc1234".to_string(),
            last_commit_author: "Test".to_string(),
            last_commit_message: "test".to_string(),
            ahead: None,
            behind: None,
            last_activity_timestamp: 0,
            branch_created_timestamp: 0,
            branch_author: "Test".to_string(),
            pr_info: None,
            worktree: Some(std::path::PathBuf::from(path)),
            worktree_shadowed_status: Some(BranchStatus::SafeMerged),
        }
    }

    #[test]
    fn details_pane_names_the_worktree_holding_the_branch() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("wt-feature");
        std::fs::create_dir(&path).unwrap();
        let path = path.to_string_lossy().into_owned();
        let app = App::new(
            vec![worktree_branch(&path)],
            "/repo".into(),
            "main".into(),
            String::new(),
        );

        let text = details_text(&app);
        assert!(text.contains("protected-wt"));
        assert!(text.contains("Checked out in another worktree"));
        assert!(text.contains(&format!("Worktree: wt-feature ({})", path)));
        assert!(text.contains("Otherwise: ✓ merged"));
        assert!(!text.contains("Prunable"));
    }

    #[test]
    fn details_pane_notes_a_prunable_worktree() {
        let app = App::new(
            vec![worktree_branch("/nonexistent/wt-gone")],
            "/repo".into(),
            "main".into(),
            String::new(),
        );

        let text = details_text(&app);
        assert!(text.contains("Worktree: wt-gone (/nonexistent/wt-gone)"));
        assert!(text.contains("Prunable: directory missing"));
    }

    #[test]
    fn details_pane_shows_the_copy_hint_under_the_open_pr_hint() {
        let mut branch = worktree_branch("/nonexistent/wt");
        branch.pr_info = Some(crate::pr::PrInfo {
            number: 42,
            state: PrState::Open,
            title: "A pull request".to_string(),
            url: "https://example.com/pr/42".to_string(),
        });
        let mut app = App::new(vec![branch], "/repo".into(), "main".into(), String::new());
        app.pr_provider = Some(crate::pr::PrProviderKind::GitHub);

        let text = details_text(&app);
        let lines: Vec<&str> = text.lines().collect();
        let open = lines
            .iter()
            .position(|l| l.contains("Press o to open in browser"))
            .expect("open PR hint");
        assert!(lines[open + 1].contains("Press y to copy the branch name"));
    }

    #[test]
    fn details_pane_shows_the_copy_hint_without_pr_integration() {
        let app = App::new(
            vec![worktree_branch("/nonexistent/wt")],
            "/repo".into(),
            "main".into(),
            String::new(),
        );
        let text = details_text(&app);
        assert!(text.contains("Press y to copy the branch name"));
        assert!(!text.contains("to open in browser"));
    }

    fn worktree_app() -> App {
        use crate::worktree::WorktreeEntry;
        use crate::worktree_view::{
            parse_porcelain_status, ChangeCounts, CommitSummary, WorktreeInfo, WorktreeView,
        };
        let row = |path: &str, branch: Option<&str>, changes: Changes| WorktreeInfo {
            entry: WorktreeEntry {
                path: path.into(),
                head: "1a2b3c4d5e".to_string(),
                branch: branch.map(str::to_string),
                is_main: path == "/repo",
                is_bare: false,
                is_detached: branch.is_none(),
                locked: None,
                prunable: None,
            },
            is_current: path == "/repo",
            changes,
            branch_status: branch.map(|_| BranchStatus::SafeMerged),
            upstream: Some("origin/x".to_string()),
            ahead: Some(1),
            behind: Some(2),
            commit: CommitSummary {
                sha: "1a2b3c4".to_string(),
                author: "Author".to_string(),
                subject: "A commit subject long enough to wrap in the details pane".to_string(),
                relative: "2 days ago".to_string(),
                timestamp: 1,
            },
            pr_info: None,
        };
        let mut app = App::new(Vec::new(), "/repo".into(), "main".into(), String::new());
        app.worktrees = Some(WorktreeView::from_infos(vec![
            row(
                "/repo",
                Some("main"),
                Changes::Loaded(ChangeCounts::default()),
            ),
            row(
                "/wt/fix-login",
                Some("fix/login"),
                Changes::Loaded(parse_porcelain_status(" M a.rs\nM  b.rs\n?? c\n")),
            ),
            row("/wt/review", None, Changes::Loading),
        ]));
        app.toggle_view();
        app
    }

    fn screen_text(app: &mut App) -> String {
        let mut terminal = Terminal::new(TestBackend::new(160, 50)).unwrap();
        terminal.draw(|frame| render(frame, app)).unwrap();
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    #[test]
    fn worktree_name_column_fits_the_longest_name_within_limits() {
        let app = worktree_app();
        let worktrees = &app.worktrees.as_ref().unwrap().worktrees;
        assert_eq!(worktree_name_width(worktrees), 9); // "fix-login"
        assert_eq!(worktree_name_width(&[]), 4); // the "Name" header

        let mut long = worktrees[1].clone();
        long.entry.path = format!("/wt/{}", "x".repeat(40)).into();
        assert_eq!(worktree_name_width(&[long]), 30);
    }

    #[test]
    fn worktree_branch_column_gets_the_spare_width() {
        let mut app = worktree_app();
        let long_branch = "feature/a-really-long-branch-name-for-the-table";
        app.worktrees.as_mut().unwrap().worktrees[1].entry.branch = Some(long_branch.to_string());
        let text = screen_text(&mut app);
        assert!(text.contains(long_branch), "branch name was cut off");
    }

    #[test]
    fn worktree_view_renders_table_details_and_modals() {
        let mut app = worktree_app();
        let text = screen_text(&mut app);
        assert!(text.contains("│ 🌳") && text.contains("Worktrees (3 shown, 0 selected)"));
        assert!(text.contains("fix-login"));
        assert!(text.contains("(detached @ 1a2b3c4)"));
        assert!(text.contains("1M 1S 1N"));
        assert!(!text.contains("1?"));
        assert!(text.contains("…"));
        assert!(text.contains("w branches"));

        // fix-login is dirty: the details pane groups its changed paths
        app.select_next();
        let text = screen_text(&mut app);
        let modified = text.find("Modified (1)").expect("Modified group");
        let staged = text.find("Staged (1)").expect("Staged group");
        let new = text.find("New (1)").expect("New group");
        assert!(modified < staged && staged < new);

        // Read-only at start: no checkboxes, browse hints, no REMOVE badge
        assert!(!text.contains("[ ]"));
        assert!(!text.contains("REMOVE"));
        assert!(text.contains("x remove mode"));
        assert!(!text.contains("Space sel"));

        // x turns removal on: badge, checkboxes and removal hints appear
        app.toggle_worktree_remove_mode();
        let text = screen_text(&mut app);
        assert!(text.contains("🗑 REMOVE"));
        assert!(text.contains("Space sel"));
        // The fixture's only removable row is dirty: a checkbox in force mode
        app.force_mode = true;
        assert!(screen_text(&mut app).contains("[ ]"));
        app.force_mode = false;

        // fix-login is dirty: selectable only in force mode
        app.toggle_worktree_at_cursor();
        assert_eq!(app.selected_count(), 0);
        app.force_mode = true;
        app.toggle_worktree_at_cursor();
        app.show_confirmation = true;
        let text = screen_text(&mut app);
        assert!(text.contains("Remove 1 worktree(s)?"));
        assert!(text.contains("remove --force"));
        assert!(text.contains("also delete branches"));

        app.show_confirmation = false;
        app.show_help = true;
        assert!(screen_text(&mut app).contains("Copy the worktree path"));
        app.show_help = false;
        app.show_info = true;
        assert!(screen_text(&mut app).contains("Worktree States"));
    }

    // --- Responsive layout (docs/specs/RESPONSIVE_LAYOUT.md) -----------------

    /// Render the whole UI at `width`×`height`, one string per screen row
    fn screen_rows(app: &mut App, width: u16, height: u16) -> Vec<String> {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| render(frame, app)).unwrap();
        let buffer = terminal.backend().buffer();
        (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect()
    }

    fn screen_at(app: &mut App, width: u16, height: u16) -> String {
        screen_rows(app, width, height).join("\n")
    }

    fn named_branch(name: &str, status: BranchStatus) -> BranchInfo {
        BranchInfo {
            name: name.to_string(),
            worktree: None,
            worktree_shadowed_status: None,
            status,
            ..worktree_branch("/unused")
        }
    }

    /// An app with `n` merged branches named `branch-000`, `branch-001`, ...
    fn many_branches_app(n: usize) -> App {
        let branches = (0..n)
            .map(|i| named_branch(&format!("branch-{i:03}"), BranchStatus::SafeMerged))
            .collect();
        App::new(branches, "/repo".into(), "main".into(), String::new())
    }

    const SIZES: [(u16, u16); 10] = [
        (1, 1),
        (10, 5),
        (39, 9),
        (40, 10),
        (60, 20),
        (80, 24),
        (80, 40),
        (100, 30),
        (120, 40),
        (200, 60),
    ];

    #[test]
    fn every_screen_renders_at_every_size_without_panicking() {
        for pr in [false, true] {
            for (width, height) in SIZES {
                let mut apps = vec![many_branches_app(50), worktree_app()];
                for app in &mut apps {
                    if pr {
                        app.pr_provider = Some(crate::pr::PrProviderKind::GitHub);
                    }
                    app.action_log.push(crate::app::ActionLogEntry {
                        branch_name: "branch-000".into(),
                        success: true,
                        message: "deleted".into(),
                    });
                    app.show_filter = true;
                    app.search_query = "bra".into();
                    screen_at(app, width, height);

                    app.select_all_safe();
                    app.show_confirmation = true;
                    screen_at(app, width, height);
                    app.show_confirmation = false;

                    app.show_help = true;
                    app.modal_scroll = usize::MAX;
                    screen_at(app, width, height);
                    app.show_help = false;

                    app.show_info = true;
                    screen_at(app, width, height);
                    app.show_info = false;
                }
            }
        }
    }

    #[test]
    fn too_small_terminal_shows_a_message_instead_of_the_layout() {
        let mut app = many_branches_app(3);
        let text = screen_at(&mut app, 39, 12);
        assert!(text.contains("Terminal too small"));
        assert!(text.contains("39×12, need 40×10"));
        assert!(!text.contains("branch-000"));
    }

    #[test]
    fn footer_hints_wrap_instead_of_being_cut_off() {
        let mut app = many_branches_app(3);
        // One line would need ~140 columns; at 80 they wrap onto more lines
        let text = screen_at(&mut app, 80, 30);
        for hint in [
            "↑↓ nav",
            "Enter del",
            "w worktrees",
            "v layout",
            "? help",
            "q quit",
        ] {
            assert!(text.contains(hint), "missing {hint:?}");
        }
    }

    #[test]
    fn footer_points_to_help_when_hints_do_not_fit() {
        let mut app = many_branches_app(3);
        // Compact (borderless) footer: at most 2 lines at 40 columns
        let text = screen_at(&mut app, 40, 12);
        assert!(text.contains("… ? help"));
    }

    #[test]
    fn cursor_row_stays_on_screen_after_the_terminal_shrinks() {
        let mut app = many_branches_app(100);
        screen_at(&mut app, 120, 60);
        for _ in 0..80 {
            app.select_next();
        }
        assert!(screen_at(&mut app, 120, 60).contains("branch-080"));

        let text = screen_at(&mut app, 80, 15);
        assert!(text.contains("branch-080"), "{text}");
        // The stored offset follows what is on screen
        assert!(app.scroll_offset > 0);
        assert!(app.scroll_offset <= 80);
    }

    #[test]
    fn growing_terminal_fills_the_new_rows() {
        let mut app = many_branches_app(30);
        screen_at(&mut app, 120, 20);
        app.go_to_bottom();
        screen_at(&mut app, 120, 20);
        assert!(app.scroll_offset > 0);

        // Tall enough for every branch: nothing stays scrolled out of view
        let text = screen_at(&mut app, 120, 60);
        assert_eq!(app.scroll_offset, 0);
        assert!(text.contains("branch-000") && text.contains("branch-029"));
    }

    #[test]
    fn list_shows_a_scrollbar_only_when_it_overflows() {
        let mut app = many_branches_app(100);
        assert!(screen_at(&mut app, 120, 30).contains('█'));

        let mut app = many_branches_app(3);
        assert!(!screen_at(&mut app, 120, 30).contains('█'));
    }

    #[test]
    fn auto_layout_follows_the_terminal_size() {
        let mut app = many_branches_app(5);

        // Wide: details to the right of the list, on the same rows
        let rows = screen_rows(&mut app, 140, 40);
        let row = rows.iter().position(|r| r.contains(" Branches (")).unwrap();
        assert!(rows[row].contains(" Details "));

        // Narrow and tall: details below the list
        let rows = screen_rows(&mut app, 80, 40);
        let list_row = rows.iter().position(|r| r.contains(" Branches (")).unwrap();
        let details_row = rows.iter().position(|r| r.contains(" Details ")).unwrap();
        assert!(details_row > list_row);

        // Narrow and short: list only, with a pointer to `v`
        let text = screen_at(&mut app, 80, 16);
        assert!(!text.contains(" Details "));
        assert!(text.contains("details hidden"));
    }

    #[test]
    fn forced_layout_mode_overrides_auto() {
        let mut app = many_branches_app(5);
        app.layout_mode = LayoutMode::ListOnly;
        let text = screen_at(&mut app, 200, 60);
        assert!(!text.contains(" Details "));
        assert!(text.contains("▥ list only"));

        app.layout_mode = LayoutMode::Stacked;
        let rows = screen_rows(&mut app, 200, 60);
        let list_row = rows.iter().position(|r| r.contains(" Branches (")).unwrap();
        let details_row = rows.iter().position(|r| r.contains(" Details ")).unwrap();
        assert!(details_row > list_row);
    }

    #[test]
    fn narrow_table_drops_columns_and_marks_cut_names() {
        let mut app = App::new(
            vec![named_branch(
                "feature/a-very-long-branch-name-that-cannot-fit",
                BranchStatus::SafeMerged,
            )],
            "/repo".into(),
            "main".into(),
            String::new(),
        );
        app.layout_mode = LayoutMode::ListOnly;
        let text = screen_at(&mut app, 50, 20);
        assert!(!text.contains("Last Commit"));
        assert!(text.contains("feature/a-very-long-bra…"), "{text}");
        assert!(text.contains('…'));
    }

    #[test]
    fn help_modal_scrolls_to_its_last_line() {
        let mut app = many_branches_app(3);
        app.show_help = true;
        let text = screen_at(&mut app, 80, 24);
        assert!(!text.contains("Quit application"));
        assert!(text.contains("↑↓ scroll"));

        app.modal_scroll = usize::MAX;
        let text = screen_at(&mut app, 80, 24);
        assert!(text.contains("Quit application"));
        // Clamped to the last page
        assert!(app.modal_scroll < 100);
    }

    #[test]
    fn confirmation_lists_every_selected_branch_and_keeps_the_hint() {
        let mut app = many_branches_app(30);
        app.select_all_safe();
        app.show_confirmation = true;

        let text = screen_at(&mut app, 120, 60);
        assert!(text.contains("branch-000") && text.contains("branch-029"));
        assert!(!text.contains("more"));

        // Too short for all 30: scrolls, and the y / n hint stays visible
        let text = screen_at(&mut app, 120, 24);
        assert!(text.contains("y / Enter"));
        assert!(text.contains("↑↓ scroll"));
        app.modal_scroll = usize::MAX;
        let text = screen_at(&mut app, 120, 24);
        assert!(text.contains("branch-029"));
        assert!(text.contains("y / Enter"));
    }

    #[test]
    fn details_scroll_is_clamped_and_resets_on_another_item() {
        let mut app = worktree_app();
        app.details_scroll = 1000;
        app.details_anchor = Some((app.view, app.selected_index));
        // Stacked at 80×30: the details pane is too short for its content
        screen_at(&mut app, 80, 30);
        assert!(app.details_scroll > 0 && app.details_scroll < 1000);

        app.select_next();
        screen_at(&mut app, 80, 30);
        assert_eq!(app.details_scroll, 0);
    }

    #[test]
    fn worktree_view_fits_a_narrow_terminal() {
        let mut app = worktree_app();
        let text = screen_at(&mut app, 60, 30);
        assert!(text.contains("fix-login"));
        assert!(!text.contains("Last Commit"));
        assert!(text.contains("x remove mode"));
    }

    #[test]
    fn long_repo_path_is_shortened_in_the_middle() {
        let mut app = many_branches_app(1);
        app.repo_path = format!("/home/user/{}/omni-scripts", "deep/".repeat(20));
        let text = screen_at(&mut app, 60, 30);
        assert!(text.contains("/home/"), "{text}");
        assert!(text.contains("main"));
        assert!(text.contains("omni-scripts"));
    }

    #[test]
    fn mouse_wheel_scrolls_the_pane_under_the_pointer() {
        let mut app = worktree_app();
        // Stacked at 80×30: details below the list, too short for its content
        screen_at(&mut app, 80, 30);
        let details = app.details_area.expect("details shown");
        let list = app.list_area;

        // Over the details pane: scrolls the details, cursor stays
        app.scroll_at(details.x + 2, details.y + 2, true);
        screen_at(&mut app, 80, 30);
        assert!(app.details_scroll > 0);
        assert_eq!(app.selected_index, 0);

        // Over the list: moves the cursor (details start at the top again)
        app.scroll_at(list.x + 2, list.y + 2, true);
        screen_at(&mut app, 80, 30);
        assert_eq!(app.selected_index, 1);
        assert_eq!(app.details_scroll, 0);

        // Elsewhere (header): nothing happens
        app.scroll_at(1, 0, true);
        assert_eq!(app.selected_index, 1);
        assert_eq!(app.details_scroll, 0);

        // An open modal takes the wheel wherever the pointer is
        app.show_help = true;
        app.scroll_at(list.x + 2, list.y + 2, true);
        assert_eq!(app.modal_scroll, 3);
        assert_eq!(app.selected_index, 1);
    }

    #[test]
    fn legend_has_its_own_box_below_the_list() {
        let mut app = many_branches_app(3);
        let rows = screen_rows(&mut app, 140, 40);
        let legend_row = rows.iter().position(|r| r.contains(" Legend ")).unwrap();
        // Its top border sits right below the list pane's bottom border
        assert!(rows[legend_row].starts_with('┌'));
        assert!(rows[legend_row - 1].starts_with('└'));
        assert!(rows[legend_row + 1].contains("✓ merged"));
        assert!(!rows.iter().any(|r| r.contains("Legend:")));

        // Compact (borderless) chrome: a plain line with a prefix instead
        let text = screen_at(&mut app, 80, 24);
        assert!(text.contains("Legend:"));
        assert!(!text.contains(" Legend "));
    }

    #[test]
    fn legend_gives_way_before_the_list_does() {
        // List pane keeps 5 rows; a boxed legend needs 2 border rows + 1 line
        let list_height = |height: u16, compact: bool| {
            let mut terminal = Terminal::new(TestBackend::new(60, height)).unwrap();
            let mut list = Rect::default();
            terminal
                .draw(|frame| {
                    list = render_legend(frame, frame.area(), worktree_legend_groups(), compact)
                })
                .unwrap();
            list.height
        };
        assert_eq!(list_height(7, false), 7); // no room: no legend
        assert_eq!(list_height(8, false), 5); // box with 1 line
        assert_eq!(list_height(20, false), 16); // box with 2 lines
        assert_eq!(list_height(6, true), 5); // compact: 1 line, no border
    }
}
