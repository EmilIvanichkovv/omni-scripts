// TUI rendering module

use crate::app::{App, FilterMode, SortMode, ViewMode};
use crate::git::BranchStatus;
use crate::pr::PrState;
use crate::worktree_view::{short_sha, Changes, WorktreeFilter, WorktreeState};
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, Clear, Paragraph, Row, Table, TableState, Wrap},
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

/// Render the TUI
pub fn render(frame: &mut Frame, app: &mut App) {
    // Create main layout: Header, [Search], [Filters], Content, Action Log, Footer
    let has_log = !app.action_log.is_empty();
    let show_filter = app.show_filter;
    let show_search = app.search_active || !app.search_query.is_empty();

    // Build constraints dynamically based on visible components
    let mut constraints = vec![Constraint::Length(3)]; // Header always present

    if show_search {
        constraints.push(Constraint::Length(3)); // Search bar
    }
    if show_filter {
        constraints.push(Constraint::Length(3)); // Filter tabs
    }
    constraints.push(Constraint::Min(5)); // Main content
    if has_log {
        constraints.push(Constraint::Length(6)); // Action log
    }
    constraints.push(Constraint::Length(3)); // Footer

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(frame.area());

    // Track current chunk index
    let mut idx = 0;

    render_header(frame, app, chunks[idx]);
    idx += 1;

    if show_search {
        render_search_box(frame, app, chunks[idx]);
        idx += 1;
    }

    if show_filter {
        render_filter_tabs(frame, app, chunks[idx]);
        idx += 1;
    }

    let content_idx = idx;
    idx += 1;

    // Split main content area into branch list (70%) and details pane (30%)
    let main_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(70), // Branch list
            Constraint::Percentage(30), // Details pane
        ])
        .split(chunks[content_idx]);

    if app.view == ViewMode::Worktrees {
        render_worktree_list(frame, app, main_chunks[0]);
        render_worktree_details(frame, app, main_chunks[1]);
    } else {
        render_branch_list(frame, app, main_chunks[0]);
        render_details_pane(frame, app, main_chunks[1]);
    }

    if has_log {
        render_action_log(frame, app, chunks[idx]);
        idx += 1;
    }
    render_footer(frame, app, chunks[idx]);

    // Render confirmation modal on top if shown
    if app.show_confirmation {
        render_confirmation_modal(frame, app);
    }

    // Render suggestions dropdown on top if shown (before help/info modals)
    if app.show_suggestions && !app.suggestions.is_empty() {
        render_suggestions_dropdown(frame, app, show_search, show_filter);
    }

    // Render help modal on top if shown
    if app.show_help {
        render_help_modal(frame, app.view);
    }

    // Render info modal on top if shown
    if app.show_info {
        render_info_modal(frame, app.view);
    }
}

/// Render the header with app name and repo info
fn render_header(frame: &mut Frame, app: &App, area: Rect) {
    let selected_info = if app.selected_count() > 0 {
        format!(" │ 📦 {} selected", app.selected_count())
    } else {
        String::new()
    };

    let force_indicator = if app.force_mode {
        " │ ⚠️ FORCE"
    } else {
        ""
    };

    let dry_run_indicator = if app.dry_run { " │ 🔍 DRY RUN" } else { "" };

    let view_indicator = if app.view == ViewMode::Worktrees {
        " │ 🌳 Worktrees"
    } else {
        ""
    };

    let remove_indicator = if app.view == ViewMode::Worktrees && app.worktree_remove_mode {
        " │ 🗑 REMOVE"
    } else {
        ""
    };

    // Sort indicator - show when not using default sort
    let sort_indicator = if app.sort_mode != SortMode::Status && app.view == ViewMode::Branches {
        format!(" │ 🔀 {}", app.sort_mode.label())
    } else {
        String::new()
    };

    let header_text = vec![Line::from(vec![
        Span::styled("🧹 ", Style::default()),
        Span::styled(
            "Local Git Branch Cleanup",
            Style::default()
                .fg(COLOR_ACCENT)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" │ ", Style::default().fg(COLOR_MUTED)),
        Span::styled("📂 ", Style::default()),
        Span::styled(&app.repo_path, Style::default().fg(COLOR_MUTED)),
        Span::styled(" │ ", Style::default().fg(COLOR_MUTED)),
        Span::styled("🌳 ", Style::default()),
        Span::styled(&app.trunk, Style::default().fg(COLOR_SUCCESS)),
        Span::styled(
            view_indicator,
            Style::default()
                .fg(COLOR_ACCENT)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            remove_indicator,
            Style::default()
                .fg(COLOR_DANGER)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(&selected_info, Style::default().fg(COLOR_SELECTED)),
        Span::styled(force_indicator, Style::default().fg(COLOR_DANGER)),
        Span::styled(dry_run_indicator, Style::default().fg(COLOR_WARNING)),
        Span::styled(&sort_indicator, Style::default().fg(COLOR_ACCENT)),
    ])];

    let header = Paragraph::new(header_text).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(COLOR_MUTED)),
    );

    frame.render_widget(header, area);
}

/// Render filter tabs
fn render_filter_tabs(frame: &mut Frame, app: &App, area: Rect) {
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

    let mut spans = vec![Span::styled("  ", Style::default())];

    for (i, (label, count, is_active)) in tabs.into_iter().enumerate() {
        let key = i + 1;

        // Add separator
        if i > 0 {
            spans.push(Span::styled(" ", Style::default()));
        }

        // Create tab label with key hint
        let label = format!("{} {} ({}) ", key, label, count);

        let style = if is_active {
            Style::default()
                .fg(Color::White)
                .bg(COLOR_ACCENT)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(COLOR_MUTED)
        };

        spans.push(Span::styled(label, style));
    }

    let tabs = Paragraph::new(Line::from(spans)).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(COLOR_MUTED))
            .title(" Filters (1-4 or Tab) ")
            .title_style(Style::default().fg(COLOR_ACCENT)),
    );

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
fn render_search_box(frame: &mut Frame, app: &App, area: Rect) {
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

    let search_box = Paragraph::new(search_text).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(border_color))
            .title(title)
            .title_style(Style::default().fg(border_color)),
    );

    frame.render_widget(search_box, area);
}

/// Render the autocomplete suggestions dropdown
fn render_suggestions_dropdown(
    frame: &mut Frame,
    app: &App,
    show_search: bool,
    _show_filter: bool,
) {
    if app.suggestions.is_empty() {
        return;
    }

    let area = frame.area();

    // Calculate position: below the search box
    // Header is 3 lines, search box is 3 lines (if shown)
    let y_offset = 3 + if show_search { 3 } else { 0 };

    // Calculate available space for dropdown
    let available_height = area.height.saturating_sub(y_offset as u16 + 2); // Reserve some space at bottom

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
    let dropdown_width = 40.min(area.width - 4);

    // Position dropdown below the search box, aligned to the left with some padding
    let dropdown_area = Rect {
        x: 3,
        y: y_offset as u16,
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

/// Render the branch list as a table
fn render_branch_list(frame: &mut Frame, app: &mut App, area: Rect) {
    // Split area inside the block: branch table + legend line at bottom
    let inner_area = area.inner(ratatui::layout::Margin {
        horizontal: 1,
        vertical: 1,
    });

    let branch_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(1),    // Branch table rows
            Constraint::Length(1), // Legend line (no border)
        ])
        .split(inner_area);

    let table_inner_area = branch_chunks[0];
    let legend_area = branch_chunks[1];

    // Update visible height in app for scroll calculations
    // Subtract 1 for the header row
    app.visible_height = table_inner_area.height as usize;

    // Get filtered branches
    let filtered_branches = app.filtered_branches();

    // Create table header (conditionally include PR column)
    let header_labels: Vec<&str> = if app.pr_provider.is_some() {
        vec!["", "☑", "Branch", "Last Commit", "Status", "PR"]
    } else {
        vec!["", "☑", "Branch", "Last Commit", "Status"]
    };

    let header_cells = header_labels.iter().map(|h| {
        Cell::from(*h).style(
            Style::default()
                .fg(COLOR_ACCENT)
                .add_modifier(Modifier::BOLD),
        )
    });
    let header = Row::new(header_cells).height(1);

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
                // Branch name
                Cell::from(branch.name.as_str()).style(if is_cursor {
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD)
                } else if is_checked {
                    Style::default().fg(COLOR_SELECTED)
                } else {
                    Style::default().fg(COLOR_MUTED)
                }),
                // Last commit time
                Cell::from(branch.last_commit_relative.as_str())
                    .style(Style::default().fg(COLOR_MUTED)),
                // Status (icon + label combined)
                Cell::from(format!(
                    "{} {}",
                    branch.status.icon(),
                    branch.status.label()
                ))
                .style(status_style),
            ];

            // Add PR column if PR integration is enabled
            if app.pr_provider.is_some() {
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

    // Column widths (conditionally include PR column)
    let widths: Vec<Constraint> = if app.pr_provider.is_some() {
        vec![
            Constraint::Length(2),  // Cursor indicator
            Constraint::Length(4),  // Checkbox
            Constraint::Min(15),    // Branch name
            Constraint::Length(15), // Last commit
            Constraint::Length(15), // Status (icon + label)
            Constraint::Length(10), // PR status
        ]
    } else {
        vec![
            Constraint::Length(2),  // Cursor indicator
            Constraint::Length(4),  // Checkbox
            Constraint::Min(15),    // Branch name
            Constraint::Length(15), // Last commit
            Constraint::Length(15), // Status (icon + label)
        ]
    };

    let title = format!(
        " Branches ({} shown, {} selected) ",
        filtered_branches.len(),
        app.selected_count()
    );

    // Render the outer block (border)
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(COLOR_MUTED))
        .title(title)
        .title_style(Style::default().fg(COLOR_ACCENT));
    frame.render_widget(block, area);

    // Render table without its own block (inside the outer block)
    let table = Table::new(rows, widths)
        .header(header)
        .row_highlight_style(Style::default().add_modifier(Modifier::BOLD));

    // Use TableState with manual scroll offset for "edge-only" scrolling
    let mut table_state = TableState::default()
        .with_selected(Some(app.selected_index))
        .with_offset(app.scroll_offset);
    frame.render_stateful_widget(table, table_inner_area, &mut table_state);

    // Render legend line (no border, just text)
    let mut legend_spans = vec![
        Span::styled("Legend: ", Style::default().fg(COLOR_MUTED)),
        Span::styled("✓ ", Style::default().fg(COLOR_SUCCESS)),
        Span::styled("merged  ", Style::default().fg(COLOR_MUTED)),
    ];

    // pr-merged/pr-diverged can only occur when a PR provider supplied merge evidence
    if app.pr_provider.is_some() {
        legend_spans.push(Span::styled("↑ ", Style::default().fg(COLOR_ACCENT)));
        legend_spans.push(Span::styled(
            "pr-merged  ",
            Style::default().fg(COLOR_MUTED),
        ));
        legend_spans.push(Span::styled("↕ ", Style::default().fg(COLOR_DIVERGED)));
        legend_spans.push(Span::styled(
            "pr-diverged  ",
            Style::default().fg(COLOR_MUTED),
        ));
    }

    legend_spans.extend([
        Span::styled("↗ ", Style::default().fg(COLOR_SUCCESS)),
        Span::styled("gone  ", Style::default().fg(COLOR_MUTED)),
        Span::styled("! ", Style::default().fg(COLOR_WARNING)),
        Span::styled("unmerged  ", Style::default().fg(COLOR_MUTED)),
        Span::styled("○ ", Style::default().fg(COLOR_LOCAL)),
        Span::styled("local  ", Style::default().fg(COLOR_MUTED)),
        Span::styled("⊘ ", Style::default().fg(COLOR_DANGER)),
        Span::styled("protected  ", Style::default().fg(COLOR_MUTED)),
        Span::styled("⑂ ", Style::default().fg(COLOR_DANGER)),
        Span::styled("protected-wt  ", Style::default().fg(COLOR_MUTED)),
        Span::styled("◉ ", Style::default().fg(COLOR_CURRENT)),
        Span::styled("current", Style::default().fg(COLOR_MUTED)),
    ]);

    // Add PR legend items when PR integration is enabled
    if app.pr_provider.is_some() {
        legend_spans.push(Span::styled("  │  PR: ", Style::default().fg(COLOR_MUTED)));
        legend_spans.push(Span::styled("🟢 ", Style::default().fg(COLOR_PR_MERGED)));
        legend_spans.push(Span::styled("merged  ", Style::default().fg(COLOR_MUTED)));
        legend_spans.push(Span::styled("🟡 ", Style::default().fg(COLOR_PR_OPEN)));
        legend_spans.push(Span::styled("open  ", Style::default().fg(COLOR_MUTED)));
        legend_spans.push(Span::styled("🔴 ", Style::default().fg(COLOR_PR_CLOSED)));
        legend_spans.push(Span::styled("closed", Style::default().fg(COLOR_MUTED)));
    }

    let legend = Line::from(legend_spans);

    let legend_widget = Paragraph::new(legend);
    frame.render_widget(legend_widget, legend_area);
}

/// Render the details pane for the selected branch
fn render_details_pane(frame: &mut Frame, app: &App, area: Rect) {
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

    let details = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(COLOR_MUTED))
                .title(" Details ")
                .title_style(Style::default().fg(COLOR_ACCENT)),
        )
        .wrap(Wrap { trim: false });

    frame.render_widget(details, area);
}

/// Render the action log panel
fn render_action_log(frame: &mut Frame, app: &App, area: Rect) {
    let log_lines: Vec<Line> = app
        .action_log
        .iter()
        .rev() // Show most recent first
        .take(4)
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

/// Render the footer with key hints
fn render_footer(frame: &mut Frame, app: &App, area: Rect) {
    let list_is_empty = match app.view {
        ViewMode::Branches => app.branches.is_empty(),
        ViewMode::Worktrees => app
            .worktrees
            .as_ref()
            .is_none_or(|wt| wt.worktrees.is_empty()),
    };
    let key_hints = if list_is_empty {
        Line::from(vec![
            Span::styled(" ", Style::default()),
            Span::styled(
                "?",
                Style::default()
                    .fg(COLOR_ACCENT)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" help  ", Style::default().fg(COLOR_MUTED)),
            Span::styled(
                "q / Esc Esc",
                Style::default()
                    .fg(COLOR_ACCENT)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" quit", Style::default().fg(COLOR_MUTED)),
        ])
    } else if app.view == ViewMode::Worktrees {
        worktree_footer_hints(app)
    } else {
        // Style for force mode - highlighted when active
        let force_style = if app.force_mode {
            Style::default()
                .fg(Color::Black)
                .bg(COLOR_DANGER)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
                .fg(COLOR_DANGER)
                .add_modifier(Modifier::BOLD)
        };

        // Style for dry run mode - highlighted when active
        let dry_style = if app.dry_run {
            Style::default()
                .fg(Color::Black)
                .bg(COLOR_WARNING)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
                .fg(COLOR_WARNING)
                .add_modifier(Modifier::BOLD)
        };

        Line::from(vec![
            Span::styled(" ", Style::default()),
            Span::styled(
                "↑↓",
                Style::default()
                    .fg(COLOR_ACCENT)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" nav  ", Style::default().fg(COLOR_MUTED)),
            Span::styled(
                "Space",
                Style::default()
                    .fg(COLOR_ACCENT)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" sel  ", Style::default().fg(COLOR_MUTED)),
            Span::styled(
                "a",
                Style::default()
                    .fg(COLOR_ACCENT)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" all  ", Style::default().fg(COLOR_MUTED)),
            Span::styled(
                "c",
                Style::default()
                    .fg(COLOR_ACCENT)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" clr  ", Style::default().fg(COLOR_MUTED)),
            Span::styled("f force", force_style),
            Span::styled("  ", Style::default()),
            Span::styled("d dry", dry_style),
            Span::styled("  ", Style::default()),
            Span::styled(
                "s",
                Style::default()
                    .fg(COLOR_ACCENT)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" sort  ", Style::default().fg(COLOR_MUTED)),
            Span::styled(
                "Enter",
                Style::default()
                    .fg(COLOR_SELECTED)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" del  ", Style::default().fg(COLOR_MUTED)),
            Span::styled(
                "/",
                Style::default()
                    .fg(COLOR_ACCENT)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" search  ", Style::default().fg(COLOR_MUTED)),
            Span::styled(
                "y",
                Style::default()
                    .fg(COLOR_ACCENT)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" copy  ", Style::default().fg(COLOR_MUTED)),
            Span::styled(
                "w",
                Style::default()
                    .fg(COLOR_ACCENT)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" worktrees  ", Style::default().fg(COLOR_MUTED)),
            Span::styled(
                "?",
                Style::default()
                    .fg(COLOR_ACCENT)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" help  ", Style::default().fg(COLOR_MUTED)),
            Span::styled(
                "q",
                Style::default()
                    .fg(COLOR_ACCENT)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" quit", Style::default().fg(COLOR_MUTED)),
        ])
    };

    // Add PR shortcut if enabled
    let key_hints = if app.pr_provider.is_some() && !list_is_empty {
        let mut spans = key_hints.spans;
        // Insert "o pr" before the quit
        let quit_idx = spans.len() - 2; // Position before "q" and " quit"
        spans.insert(
            quit_idx,
            Span::styled(
                "o",
                Style::default()
                    .fg(COLOR_PR_MERGED)
                    .add_modifier(Modifier::BOLD),
            ),
        );
        spans.insert(
            quit_idx + 1,
            Span::styled(" pr  ", Style::default().fg(COLOR_MUTED)),
        );
        Line::from(spans)
    } else {
        key_hints
    };

    // A one-off message (clipboard result, missing PR) replaces the hints
    let key_hints = match &app.flash {
        Some(message) => Line::from(Span::styled(
            format!(" {message}"),
            Style::default().fg(COLOR_WARNING),
        )),
        None => key_hints,
    };

    // After a first Esc, replace the hints with the quit confirmation prompt
    let key_hints = if app.esc_quit_armed_at.is_some() {
        Line::from(vec![
            Span::styled(" Press ", Style::default().fg(COLOR_WARNING)),
            Span::styled(
                "Esc",
                Style::default()
                    .fg(COLOR_WARNING)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" again to quit", Style::default().fg(COLOR_WARNING)),
        ])
    } else {
        key_hints
    };

    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(COLOR_MUTED));
    // On the bottom border, so they take no room from the key hints
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
    let footer = Paragraph::new(key_hints).block(block);

    frame.render_widget(footer, area);
}

/// Render the confirmation modal
fn render_confirmation_modal(frame: &mut Frame, app: &App) {
    if app.view == ViewMode::Worktrees {
        let lines = worktree_confirmation_lines(app);
        let needed_height = lines.len() + 2;
        render_confirmation_box(frame, app, lines, needed_height);
        return;
    }

    // Build modal content first to calculate needed height
    let selected_branches = app.get_selected_branches();
    let unmerged_count = selected_branches
        .iter()
        .filter(|b| b.status.requires_force())
        .count();
    let gone_count = selected_branches
        .iter()
        .filter(|b| b.status == BranchStatus::GoneUpstream)
        .count();

    // Calculate how many lines we need:
    // 1 empty + 1 title + 1 empty + min(3, branches) + (1 if more) + (2 if unmerged/gone warning) + 1 empty + 3 confirmation = ~12 base
    let branch_lines = selected_branches.len().min(3);
    let more_line = if selected_branches.len() > 3 { 1 } else { 0 };
    let warning_lines = if unmerged_count > 0 || gone_count > 0 {
        2
    } else {
        0
    };
    let needed_height = 3 + branch_lines + more_line + warning_lines + 5; // header + branches + warnings + footer with hints

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

    // Show branch names (up to 3)
    for branch in selected_branches.iter().take(3) {
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

    if selected_branches.len() > 3 {
        lines.push(Line::from(vec![Span::styled(
            format!("  ... and {} more", selected_branches.len() - 3),
            Style::default().fg(COLOR_MUTED),
        )]));
    }

    // Warning for unmerged
    if unmerged_count > 0 {
        lines.push(Line::from(""));
        lines.push(Line::from(vec![Span::styled(
            format!("  ⚠️  {} unmerged (force delete)", unmerged_count),
            Style::default().fg(COLOR_DANGER),
        )]));
    }

    lines.push(Line::from(""));
    lines.push(confirmation_hint_line());

    render_confirmation_box(frame, app, lines, needed_height);
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

/// Draw the confirmation modal frame (centered, dry-run aware title) around `lines`
fn render_confirmation_box(frame: &mut Frame, app: &App, lines: Vec<Line>, needed_height: usize) {
    let area = frame.area();

    // Calculate modal size - ensure it fits and has reasonable bounds
    // (saturating, so a tiny terminal cannot underflow)
    let modal_width = 60.min(area.width.saturating_sub(4));
    let modal_height = (needed_height as u16)
        .max(12)
        .min(area.height.saturating_sub(4));

    let modal_area = Rect {
        x: area.width.saturating_sub(modal_width) / 2,
        y: area.height.saturating_sub(modal_height) / 2,
        width: modal_width,
        height: modal_height,
    };

    // Clear the area behind the modal
    frame.render_widget(Clear, modal_area);

    let (modal_border_color, modal_title) = if app.dry_run {
        (COLOR_WARNING, " 🔍 Preview (Dry Run) ")
    } else {
        (COLOR_DANGER, " ⚠️  Confirm Deletion ")
    };

    let modal = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(modal_border_color))
                .title(modal_title)
                .title_style(
                    Style::default()
                        .fg(modal_border_color)
                        .add_modifier(Modifier::BOLD),
                ),
        )
        .wrap(Wrap { trim: false })
        .alignment(Alignment::Left);

    frame.render_widget(modal, modal_area);
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
fn render_help_modal(frame: &mut Frame, view: ViewMode) {
    let area = frame.area();

    // Calculate modal size (larger for help content): 51 lines + border
    let modal_width = 70.min(area.width - 4);
    let modal_height = 53.min(area.height - 4);

    let modal_area = Rect {
        x: (area.width - modal_width) / 2,
        y: (area.height - modal_height) / 2,
        width: modal_width,
        height: modal_height,
    };

    // Clear the area behind the modal
    frame.render_widget(Clear, modal_area);

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

    let modal = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(COLOR_ACCENT))
                .title(" 📖 Help - Keyboard Shortcuts ")
                .title_style(
                    Style::default()
                        .fg(COLOR_ACCENT)
                        .add_modifier(Modifier::BOLD),
                ),
        )
        .wrap(Wrap { trim: false })
        .alignment(Alignment::Left);

    frame.render_widget(modal, modal_area);
}

/// Render the info modal
fn render_info_modal(frame: &mut Frame, view: ViewMode) {
    let area = frame.area();

    // Calculate modal size
    let modal_width = 65.min(area.width - 4);
    let modal_height = if view == ViewMode::Worktrees { 26 } else { 24 }.min(area.height - 4);

    let modal_area = Rect {
        x: (area.width - modal_width) / 2,
        y: (area.height - modal_height) / 2,
        width: modal_width,
        height: modal_height,
    };

    // Clear the area behind the modal
    frame.render_widget(Clear, modal_area);

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

    let modal = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(COLOR_ACCENT))
                .title(" ℹ️  About ")
                .title_style(
                    Style::default()
                        .fg(COLOR_ACCENT)
                        .add_modifier(Modifier::BOLD),
                ),
        )
        .wrap(Wrap { trim: false })
        .alignment(Alignment::Left);

    frame.render_widget(modal, modal_area);
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

/// Render the worktree list as a table
fn render_worktree_list(frame: &mut Frame, app: &mut App, area: Rect) {
    let inner_area = area.inner(ratatui::layout::Margin {
        horizontal: 1,
        vertical: 1,
    });
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(inner_area);
    let table_area = chunks[0];
    let legend_area = chunks[1];

    app.visible_height = table_area.height as usize;
    let app = &*app;

    let show_pr = app.pr_provider.is_some();
    let mut header_labels = vec!["", "", "Name", "Branch", "State", "Changes", "Last Commit"];
    if show_pr {
        header_labels.push("PR");
    }
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

            let mut cells = vec![
                Cell::from(if is_cursor { "▶" } else { " " })
                    .style(Style::default().fg(COLOR_ACCENT)),
                Cell::from(mark).style(mark_style),
                Cell::from(wt.name()).style(name_style),
                Cell::from(wt.branch_label()).style(Style::default().fg(COLOR_MUTED)),
                Cell::from(format!("{} {}", state.icon(), state.label()))
                    .style(Style::default().fg(get_worktree_state_color(&state))),
                Cell::from(changes).style(changes_style),
                Cell::from(last_commit).style(Style::default().fg(COLOR_MUTED)),
            ];
            if show_pr {
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

    // Name is sized to the longest worktree name (over all worktrees, so it
    // does not jump when filters change); Branch takes all remaining space,
    // since branch names are usually much longer than directory names
    let name_width = app
        .worktrees
        .as_ref()
        .map_or(0, |wt| worktree_name_width(&wt.worktrees));
    let mut widths = vec![
        Constraint::Length(2),          // Cursor
        Constraint::Length(4),          // Checkbox / main / locked marker
        Constraint::Length(name_width), // Name
        Constraint::Fill(1),            // Branch
        Constraint::Length(14),         // State
        Constraint::Length(10),         // Changes
        Constraint::Length(15),         // Last commit
    ];
    if show_pr {
        widths.push(Constraint::Length(10));
    }

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

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(COLOR_MUTED))
        .title(title)
        .title_style(Style::default().fg(COLOR_ACCENT));
    frame.render_widget(block, area);

    let table = Table::new(rows, widths)
        .header(header)
        .row_highlight_style(Style::default().add_modifier(Modifier::BOLD));
    let mut table_state = TableState::default()
        .with_selected(Some(app.selected_index))
        .with_offset(app.scroll_offset);
    frame.render_stateful_widget(table, table_area, &mut table_state);

    let legend = Line::from(vec![
        Span::styled("Legend: ", Style::default().fg(COLOR_MUTED)),
        Span::styled("◆ ", Style::default().fg(COLOR_ACCENT)),
        Span::styled("main  ", Style::default().fg(COLOR_MUTED)),
        Span::styled("◉ ", Style::default().fg(COLOR_CURRENT)),
        Span::styled("current  ", Style::default().fg(COLOR_MUTED)),
        Span::styled("✗ ", Style::default().fg(COLOR_DANGER)),
        Span::styled("prunable  ", Style::default().fg(COLOR_MUTED)),
        Span::styled("🔒 ", Style::default().fg(COLOR_MUTED)),
        Span::styled("locked  ", Style::default().fg(COLOR_MUTED)),
        Span::styled("✎ ", Style::default().fg(COLOR_WARNING)),
        Span::styled("dirty  ", Style::default().fg(COLOR_MUTED)),
        Span::styled("○ ", Style::default().fg(COLOR_LOCAL)),
        Span::styled("detached  ", Style::default().fg(COLOR_MUTED)),
        Span::styled("│ ", Style::default().fg(COLOR_MUTED)),
        Span::styled("M", Style::default().fg(COLOR_WARNING)),
        Span::styled(" modified  ", Style::default().fg(COLOR_MUTED)),
        Span::styled("S", Style::default().fg(COLOR_WARNING)),
        Span::styled(" staged  ", Style::default().fg(COLOR_MUTED)),
        Span::styled("N", Style::default().fg(COLOR_WARNING)),
        Span::styled(" new", Style::default().fg(COLOR_MUTED)),
    ]);
    frame.render_widget(Paragraph::new(legend), legend_area);
}

/// Render the details pane for the worktree under the cursor
fn render_worktree_details(frame: &mut Frame, app: &App, area: Rect) {
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
        render_details_block(frame, lines, area);
        return;
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

    render_details_block(frame, lines, area);
}

/// Draw the bordered `Details` pane
fn render_details_block(frame: &mut Frame, lines: Vec<Line>, area: Rect) {
    let details = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(COLOR_MUTED))
                .title(" Details ")
                .title_style(Style::default().fg(COLOR_ACCENT)),
        )
        .wrap(Wrap { trim: false });
    frame.render_widget(details, area);
}

/// Footer key hints of the worktree view
fn worktree_footer_hints(app: &App) -> Line<'static> {
    let key = |k: &'static str| {
        Span::styled(
            k,
            Style::default()
                .fg(COLOR_ACCENT)
                .add_modifier(Modifier::BOLD),
        )
    };
    let text = |t: &'static str| Span::styled(t, Style::default().fg(COLOR_MUTED));
    let toggle_style = |on: bool, color: Color| {
        if on {
            Style::default()
                .fg(Color::Black)
                .bg(color)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(color).add_modifier(Modifier::BOLD)
        }
    };

    let mut spans = vec![
        Span::styled(" ", Style::default()),
        key("↑↓"),
        text(" nav  "),
    ];
    if app.worktree_remove_mode {
        spans.extend([
            key("Space"),
            text(" sel  "),
            key("a"),
            text(" stale  "),
            key("c"),
            text(" clr  "),
            Span::styled("f force", toggle_style(app.force_mode, COLOR_DANGER)),
            text("  "),
            Span::styled("d dry", toggle_style(app.dry_run, COLOR_WARNING)),
            text("  "),
            Span::styled(
                "Enter",
                Style::default()
                    .fg(COLOR_SELECTED)
                    .add_modifier(Modifier::BOLD),
            ),
            text(" remove  "),
        ]);
    }
    spans.extend([
        Span::styled(
            "x remove mode",
            toggle_style(app.worktree_remove_mode, COLOR_DANGER),
        ),
        text("  "),
        key("y"),
        text(" copy path  "),
        key("/"),
        text(" search  "),
        key("w"),
        text(" branches  "),
        key("?"),
        text(" help  "),
        key("q"),
        text(" quit"),
    ]);
    Line::from(spans)
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

    for (wt, action) in selected.iter().take(4) {
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
    if selected.len() > 4 {
        lines.push(Line::from(Span::styled(
            format!("  ... and {} more", selected.len() - 4),
            Style::default().fg(COLOR_MUTED),
        )));
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
    lines.push(Line::from(""));
    lines.push(confirmation_hint_line());
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
        help_heading("  Other"),
        help_row("w", "Switch back to the branch view", COLOR_SUCCESS),
        help_row("r", "Reload worktrees and branches", COLOR_SUCCESS),
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
            .draw(|frame| render_footer(frame, app, frame.area()))
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
            .draw(|frame| render_details_pane(frame, app, frame.area()))
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
}
