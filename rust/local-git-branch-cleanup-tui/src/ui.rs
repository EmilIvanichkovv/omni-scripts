// TUI rendering module

use crate::app::{App, FilterMode};
use crate::git::BranchStatus;
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, Clear, Paragraph, Row, Table, TableState, Wrap},
    Frame,
};

// Color palette
const COLOR_ACCENT: Color = Color::Rgb(46, 196, 182);    // #2EC4B6 - cyan
const COLOR_WARNING: Color = Color::Rgb(255, 184, 108);  // #FFB86C - amber
const COLOR_DANGER: Color = Color::Rgb(255, 85, 85);     // #FF5555 - red
const COLOR_MUTED: Color = Color::Rgb(169, 177, 214);    // #A9B1D6 - muted text
const COLOR_SUCCESS: Color = Color::Rgb(80, 250, 123);   // #50FA7B - green
const COLOR_CURRENT: Color = Color::Rgb(189, 147, 249);  // #BD93F9 - purple
const COLOR_SELECTED: Color = Color::Rgb(255, 121, 198); // #FF79C6 - pink for selected

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
            Constraint::Percentage(70),  // Branch list
            Constraint::Percentage(30),  // Details pane
        ])
        .split(chunks[content_idx]);

    render_branch_list(frame, app, main_chunks[0]);
    render_details_pane(frame, app, main_chunks[1]);

    if has_log {
        render_action_log(frame, app, chunks[idx]);
        idx += 1;
    }
    render_footer(frame, app, chunks[idx]);

    // Render confirmation modal on top if shown
    if app.show_confirmation {
        render_confirmation_modal(frame, app);
    }

    // Render help modal on top if shown
    if app.show_help {
        render_help_modal(frame);
    }

    // Render info modal on top if shown
    if app.show_info {
        render_info_modal(frame);
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

    let dry_run_indicator = if app.dry_run {
        " │ 🔍 DRY RUN"
    } else {
        ""
    };

    let header_text = vec![
        Line::from(vec![
            Span::styled("🧹 ", Style::default()),
            Span::styled(
                "Local Git Branch Cleanup",
                Style::default().fg(COLOR_ACCENT).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" │ ", Style::default().fg(COLOR_MUTED)),
            Span::styled("📂 ", Style::default()),
            Span::styled(&app.repo_path, Style::default().fg(COLOR_MUTED)),
            Span::styled(" │ ", Style::default().fg(COLOR_MUTED)),
            Span::styled("🌳 ", Style::default()),
            Span::styled(&app.trunk, Style::default().fg(COLOR_SUCCESS)),
            Span::styled(&selected_info, Style::default().fg(COLOR_SELECTED)),
            Span::styled(force_indicator, Style::default().fg(COLOR_DANGER)),
            Span::styled(dry_run_indicator, Style::default().fg(COLOR_WARNING)),
        ]),
    ];

    let header = Paragraph::new(header_text)
        .block(Block::default().borders(Borders::ALL).border_style(Style::default().fg(COLOR_MUTED)));

    frame.render_widget(header, area);
}

/// Render filter tabs
fn render_filter_tabs(frame: &mut Frame, app: &App, area: Rect) {
    let filters = [
        FilterMode::SafeMerged,
        FilterMode::GoneUpstream,
        FilterMode::Unmerged,
        FilterMode::All,
    ];

    let mut spans = vec![Span::styled("  ", Style::default())];

    for (i, filter) in filters.iter().enumerate() {
        let count = app.filter_count(*filter);
        let is_active = app.current_filter == *filter;
        let key = i + 1;

        // Add separator
        if i > 0 {
            spans.push(Span::styled(" ", Style::default()));
        }

        // Create tab label with key hint
        let label = format!("{} {} ({}) ", key, filter.label(), count);

        let style = if is_active {
            Style::default()
                .fg(Color::White)
                .bg(COLOR_ACCENT)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
                .fg(COLOR_MUTED)
        };

        spans.push(Span::styled(label, style));
    }

    let tabs = Paragraph::new(Line::from(spans))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(COLOR_MUTED))
                .title(" Filters (1-4 or Tab) ")
                .title_style(Style::default().fg(COLOR_ACCENT)),
        );

    frame.render_widget(tabs, area);
}

/// Render the search input box with border
fn render_search_box(frame: &mut Frame, app: &App, area: Rect) {
    let match_count = app.filtered_branches().len();

    let cursor = if app.search_active { "▏" } else { "" };

    let search_text = if app.search_query.is_empty() && !app.search_active {
        Line::from(vec![
            Span::styled("  Type to search branches...", Style::default().fg(COLOR_MUTED)),
        ])
    } else {
        Line::from(vec![
            Span::styled("  ", Style::default()),
            Span::styled(&app.search_query, Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
            Span::styled(cursor, Style::default().fg(COLOR_ACCENT)),
            Span::styled(
                format!("  ({} matches)", match_count),
                Style::default().fg(COLOR_MUTED),
            ),
        ])
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

    let search_box = Paragraph::new(search_text)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(border_color))
                .title(title)
                .title_style(Style::default().fg(border_color)),
        );

    frame.render_widget(search_box, area);
}

/// Render the branch list as a table
fn render_branch_list(frame: &mut Frame, app: &mut App, area: Rect) {
    // Split area inside the block: branch table + legend line at bottom
    let inner_area = area.inner(ratatui::layout::Margin { horizontal: 1, vertical: 1 });

    let branch_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(1),     // Branch table rows
            Constraint::Length(1),  // Legend line (no border)
        ])
        .split(inner_area);

    let table_inner_area = branch_chunks[0];
    let legend_area = branch_chunks[1];

    // Update visible height in app for scroll calculations
    // Subtract 1 for the header row
    app.visible_height = table_inner_area.height as usize;

    // Get filtered branches
    let filtered_branches = app.filtered_branches();

    // Create table header
    let header_cells = ["", "☑", "Branch", "Last Commit", "Status"]
        .iter()
        .map(|h| Cell::from(*h).style(Style::default().fg(COLOR_ACCENT).add_modifier(Modifier::BOLD)));
    let header = Row::new(header_cells).height(1);

    // Create table rows from filtered branches
    let rows: Vec<Row> = filtered_branches
        .iter()
        .enumerate()
        .map(|(filtered_idx, branch)| {
            // Find original index in app.branches for selection state
            let original_idx = app.branches.iter().position(|b| b.name == branch.name).unwrap();
            let is_cursor = filtered_idx == app.selected_index;
            let is_checked = app.is_branch_selected(original_idx);
            let status_style = get_status_style(&branch.status);

            // Checkbox display
            let checkbox = if !branch.status.is_deletable() {
                "   " // No checkbox for protected
            } else if branch.status == BranchStatus::Unmerged && !app.force_mode {
                " - " // Disabled checkbox for unmerged without force
            } else if is_checked {
                "[✓]"
            } else {
                "[ ]"
            };

            let checkbox_style = if is_checked {
                Style::default().fg(COLOR_SELECTED)
            } else if !branch.status.is_deletable() || (branch.status == BranchStatus::Unmerged && !app.force_mode) {
                Style::default().fg(COLOR_MUTED)
            } else {
                Style::default().fg(COLOR_ACCENT)
            };

            let cells = vec![
                // Cursor indicator
                Cell::from(if is_cursor { "▶" } else { " " })
                    .style(Style::default().fg(COLOR_ACCENT)),
                // Checkbox
                Cell::from(checkbox).style(checkbox_style),
                // Branch name
                Cell::from(branch.name.as_str()).style(if is_cursor {
                    Style::default().fg(Color::White).add_modifier(Modifier::BOLD)
                } else if is_checked {
                    Style::default().fg(COLOR_SELECTED)
                } else {
                    Style::default().fg(COLOR_MUTED)
                }),
                // Last commit time
                Cell::from(branch.last_commit_relative.as_str())
                    .style(Style::default().fg(COLOR_MUTED)),
                // Status (icon + label combined)
                Cell::from(format!("{} {}", branch.status.icon(), branch.status.label())).style(status_style),
            ];

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

    // Column widths
    let widths = [
        Constraint::Length(2),   // Cursor indicator
        Constraint::Length(4),   // Checkbox
        Constraint::Min(15),     // Branch name
        Constraint::Length(15),  // Last commit
        Constraint::Length(12),  // Status (icon + label)
    ];

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
    let legend = Line::from(vec![
        Span::styled("Legend: ", Style::default().fg(COLOR_MUTED)),
        Span::styled("✓ ", Style::default().fg(COLOR_SUCCESS)),
        Span::styled("merged  ", Style::default().fg(COLOR_MUTED)),
        Span::styled("↗ ", Style::default().fg(COLOR_WARNING)),
        Span::styled("gone  ", Style::default().fg(COLOR_MUTED)),
        Span::styled("! ", Style::default().fg(COLOR_WARNING)),
        Span::styled("unmerged  ", Style::default().fg(COLOR_MUTED)),
        Span::styled("⊘ ", Style::default().fg(COLOR_DANGER)),
        Span::styled("protected  ", Style::default().fg(COLOR_MUTED)),
        Span::styled("◉ ", Style::default().fg(COLOR_CURRENT)),
        Span::styled("current", Style::default().fg(COLOR_MUTED)),
    ]);

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
            Span::styled(&branch.name, Style::default().fg(COLOR_ACCENT).add_modifier(Modifier::BOLD)),
        ]));
        lines.push(Line::from(""));

        // Status
        lines.push(Line::from(vec![
            Span::styled("  Status: ", Style::default().fg(COLOR_MUTED)),
            Span::styled(branch.status.icon(), Style::default().fg(get_status_color(&branch.status))),
            Span::styled(" ", Style::default()),
            Span::styled(branch.status.label(), Style::default().fg(get_status_color(&branch.status))),
        ]));

        // Status explanation
        let explanation = match branch.status {
            BranchStatus::SafeMerged => "Merged into trunk, safe to delete",
            BranchStatus::GoneUpstream => "Remote branch deleted",
            BranchStatus::Unmerged => "Has unmerged commits",
            BranchStatus::Protected => "Protected branch",
            BranchStatus::Current => "Currently checked out",
        };
        lines.push(Line::from(vec![
            Span::styled("  ", Style::default()),
            Span::styled(explanation, Style::default().fg(COLOR_MUTED).add_modifier(Modifier::ITALIC)),
        ]));
        lines.push(Line::from(""));

        // Upstream info
        if let Some(upstream) = &branch.upstream {
            lines.push(Line::from(vec![
                Span::styled("  Upstream: ", Style::default().fg(COLOR_MUTED)),
                Span::styled(upstream, Style::default().fg(COLOR_SUCCESS)),
            ]));

            // Ahead/behind
            if let (Some(ahead), Some(behind)) = (branch.ahead, branch.behind) {
                if ahead > 0 || behind > 0 {
                    let mut parts = vec![Span::styled("  Divergence: ", Style::default().fg(COLOR_MUTED))];

                    if ahead > 0 {
                        parts.push(Span::styled(format!("↑{}", ahead), Style::default().fg(COLOR_SUCCESS)));
                    }
                    if ahead > 0 && behind > 0 {
                        parts.push(Span::styled(" ", Style::default()));
                    }
                    if behind > 0 {
                        parts.push(Span::styled(format!("↓{}", behind), Style::default().fg(COLOR_WARNING)));
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
        lines.push(Line::from(vec![
            Span::styled("  Last Commit:", Style::default().fg(COLOR_MUTED).add_modifier(Modifier::BOLD)),
        ]));
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
            Span::styled(&branch.last_commit_relative, Style::default().fg(COLOR_MUTED).add_modifier(Modifier::ITALIC)),
        ]));
        lines.push(Line::from(""));

        // Commit message (word wrap)
        lines.push(Line::from(vec![
            Span::styled("  Message:", Style::default().fg(COLOR_MUTED).add_modifier(Modifier::BOLD)),
        ]));

        // Wrap long commit messages
        let max_width = area.width.saturating_sub(4) as usize;
        let words: Vec<&str> = branch.last_commit_message.split_whitespace().collect();
        let mut current_line = String::from("  ");

        for word in words {
            if current_line.len() + word.len() + 1 > max_width {
                lines.push(Line::from(Span::styled(current_line.clone(), Style::default().fg(Color::White))));
                current_line = String::from("  ");
            }
            if !current_line.ends_with("  ") {
                current_line.push(' ');
            }
            current_line.push_str(word);
        }
        if current_line.len() > 2 {
            lines.push(Line::from(Span::styled(current_line, Style::default().fg(Color::White))));
        }
    } else {
        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::styled("  No branch selected", Style::default().fg(COLOR_MUTED).add_modifier(Modifier::ITALIC)),
        ]));
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
    let log_lines: Vec<Line> = app.action_log
        .iter()
        .rev()  // Show most recent first
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

    let log = Paragraph::new(log_lines)
        .block(
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
    let key_hints = if app.branches.is_empty() {
        Line::from(vec![
            Span::styled(" ", Style::default()),
            Span::styled("?", Style::default().fg(COLOR_ACCENT).add_modifier(Modifier::BOLD)),
            Span::styled(" help  ", Style::default().fg(COLOR_MUTED)),
            Span::styled("q / Esc", Style::default().fg(COLOR_ACCENT).add_modifier(Modifier::BOLD)),
            Span::styled(" quit", Style::default().fg(COLOR_MUTED)),
        ])
    } else {
        // Style for force mode - highlighted when active
        let force_style = if app.force_mode {
            Style::default().fg(Color::Black).bg(COLOR_DANGER).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(COLOR_DANGER).add_modifier(Modifier::BOLD)
        };

        // Style for dry run mode - highlighted when active
        let dry_style = if app.dry_run {
            Style::default().fg(Color::Black).bg(COLOR_WARNING).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(COLOR_WARNING).add_modifier(Modifier::BOLD)
        };

        Line::from(vec![
            Span::styled(" ", Style::default()),
            Span::styled("↑↓", Style::default().fg(COLOR_ACCENT).add_modifier(Modifier::BOLD)),
            Span::styled(" nav  ", Style::default().fg(COLOR_MUTED)),
            Span::styled("Space", Style::default().fg(COLOR_ACCENT).add_modifier(Modifier::BOLD)),
            Span::styled(" select  ", Style::default().fg(COLOR_MUTED)),
            Span::styled("a", Style::default().fg(COLOR_ACCENT).add_modifier(Modifier::BOLD)),
            Span::styled(" all  ", Style::default().fg(COLOR_MUTED)),
            Span::styled("c", Style::default().fg(COLOR_ACCENT).add_modifier(Modifier::BOLD)),
            Span::styled(" clear  ", Style::default().fg(COLOR_MUTED)),
            Span::styled("f force", force_style),
            Span::styled("  ", Style::default()),
            Span::styled("d dry", dry_style),
            Span::styled("  ", Style::default()),
            Span::styled("Enter", Style::default().fg(COLOR_SELECTED).add_modifier(Modifier::BOLD)),
            Span::styled(" delete  ", Style::default().fg(COLOR_MUTED)),
            Span::styled("/", Style::default().fg(COLOR_ACCENT).add_modifier(Modifier::BOLD)),
            Span::styled(" search  ", Style::default().fg(COLOR_MUTED)),
            Span::styled("F", Style::default().fg(COLOR_ACCENT).add_modifier(Modifier::BOLD)),
            Span::styled(" filters  ", Style::default().fg(COLOR_MUTED)),
            Span::styled("?", Style::default().fg(COLOR_ACCENT).add_modifier(Modifier::BOLD)),
            Span::styled(" help  ", Style::default().fg(COLOR_MUTED)),
            Span::styled("i", Style::default().fg(COLOR_ACCENT).add_modifier(Modifier::BOLD)),
            Span::styled(" info  ", Style::default().fg(COLOR_MUTED)),
            Span::styled("q / Esc", Style::default().fg(COLOR_ACCENT).add_modifier(Modifier::BOLD)),
            Span::styled(" quit", Style::default().fg(COLOR_MUTED)),
        ])
    };

    let footer = Paragraph::new(key_hints)
        .block(Block::default().borders(Borders::ALL).border_style(Style::default().fg(COLOR_MUTED)));

    frame.render_widget(footer, area);
}

/// Render the confirmation modal
fn render_confirmation_modal(frame: &mut Frame, app: &App) {
    let area = frame.area();

    // Build modal content first to calculate needed height
    let selected_branches = app.get_selected_branches();
    let unmerged_count = selected_branches.iter()
        .filter(|b| b.status == BranchStatus::Unmerged)
        .count();
    let gone_count = selected_branches.iter()
        .filter(|b| b.status == BranchStatus::GoneUpstream)
        .count();

    // Calculate how many lines we need:
    // 1 empty + 1 title + 1 empty + min(3, branches) + (1 if more) + (2 if unmerged/gone warning) + 1 empty + 3 confirmation = ~12 base
    let branch_lines = selected_branches.len().min(3);
    let more_line = if selected_branches.len() > 3 { 1 } else { 0 };
    let warning_lines = if unmerged_count > 0 || gone_count > 0 { 2 } else { 0 };
    let needed_height = 3 + branch_lines + more_line + warning_lines + 5; // header + branches + warnings + footer with hints

    // Calculate modal size - ensure it fits and has reasonable bounds
    let modal_width = 60.min(area.width - 4);
    let modal_height = (needed_height as u16).min(area.height - 4).max(12);

    let modal_area = Rect {
        x: (area.width - modal_width) / 2,
        y: (area.height - modal_height) / 2,
        width: modal_width,
        height: modal_height,
    };

    // Clear the area behind the modal
    frame.render_widget(Clear, modal_area);

    let mut lines = vec![
        Line::from(""),
    ];

    if app.dry_run {
        lines.push(Line::from(vec![
            Span::styled("  Preview ", Style::default().fg(COLOR_MUTED)),
            Span::styled(
                format!("{}", app.selected_count()),
                Style::default().fg(COLOR_WARNING).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" branch(es) ", Style::default().fg(COLOR_MUTED)),
            Span::styled("(Dry Run)", Style::default().fg(COLOR_WARNING).add_modifier(Modifier::BOLD)),
        ]));
    } else {
        lines.push(Line::from(vec![
            Span::styled("  Delete ", Style::default().fg(COLOR_MUTED)),
            Span::styled(
                format!("{}", app.selected_count()),
                Style::default().fg(COLOR_DANGER).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" branch(es)?", Style::default().fg(COLOR_MUTED)),
        ]));
    }
    lines.push(Line::from(""));

    // Show branch names (up to 3)
    for (i, branch) in selected_branches.iter().take(3).enumerate() {
        let prefix = if i == 0 { "  • " } else { "  • " };
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
        lines.push(Line::from(vec![
            Span::styled(
                format!("  ... and {} more", selected_branches.len() - 3),
                Style::default().fg(COLOR_MUTED),
            ),
        ]));
    }

    // Warning for unmerged
    if unmerged_count > 0 {
        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::styled(
                format!("  ⚠️  {} unmerged (force delete)", unmerged_count),
                Style::default().fg(COLOR_DANGER),
            ),
        ]));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled("y / Enter", Style::default().fg(COLOR_SUCCESS).add_modifier(Modifier::BOLD)),
        Span::styled(" confirm    ", Style::default().fg(COLOR_MUTED)),
        Span::styled("n / Esc", Style::default().fg(COLOR_DANGER).add_modifier(Modifier::BOLD)),
        Span::styled(" cancel", Style::default().fg(COLOR_MUTED)),
    ]).alignment(Alignment::Center));

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
                .title_style(Style::default().fg(modal_border_color).add_modifier(Modifier::BOLD)),
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
        BranchStatus::GoneUpstream => COLOR_WARNING,
        BranchStatus::Unmerged => COLOR_WARNING,
        BranchStatus::Protected => COLOR_DANGER,
        BranchStatus::Current => COLOR_CURRENT,
    }
}

/// Render the help modal
fn render_help_modal(frame: &mut Frame) {
    let area = frame.area();

    // Calculate modal size (larger for help content)
    let modal_width = 70.min(area.width - 4);
    let modal_height = 36.min(area.height - 4);

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
        Line::from(vec![
            Span::styled("  Navigation", Style::default().fg(COLOR_ACCENT).add_modifier(Modifier::BOLD)),
        ]),
        Line::from(vec![
            Span::styled("    ↑/k", Style::default().fg(COLOR_SUCCESS).add_modifier(Modifier::BOLD)),
            Span::styled("          Move cursor up", Style::default().fg(COLOR_MUTED)),
        ]),
        Line::from(vec![
            Span::styled("    ↓/j", Style::default().fg(COLOR_SUCCESS).add_modifier(Modifier::BOLD)),
            Span::styled("          Move cursor down", Style::default().fg(COLOR_MUTED)),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("  Search", Style::default().fg(COLOR_ACCENT).add_modifier(Modifier::BOLD)),
        ]),
        Line::from(vec![
            Span::styled("    /", Style::default().fg(COLOR_SUCCESS).add_modifier(Modifier::BOLD)),
            Span::styled("             Start search (type to filter by name)", Style::default().fg(COLOR_MUTED)),
        ]),
        Line::from(vec![
            Span::styled("    Esc", Style::default().fg(COLOR_SUCCESS).add_modifier(Modifier::BOLD)),
            Span::styled("           Exit search and clear query", Style::default().fg(COLOR_MUTED)),
        ]),
        Line::from(vec![
            Span::styled("    Enter", Style::default().fg(COLOR_SUCCESS).add_modifier(Modifier::BOLD)),
            Span::styled("         Exit search and keep filter", Style::default().fg(COLOR_MUTED)),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("  Filters", Style::default().fg(COLOR_ACCENT).add_modifier(Modifier::BOLD)),
        ]),
        Line::from(vec![
            Span::styled("    F", Style::default().fg(COLOR_SUCCESS).add_modifier(Modifier::BOLD)),
            Span::styled("             Toggle filter bar visibility", Style::default().fg(COLOR_MUTED)),
        ]),
        Line::from(vec![
            Span::styled("    1 / F1", Style::default().fg(COLOR_SUCCESS).add_modifier(Modifier::BOLD)),
            Span::styled("        Show safe merged branches", Style::default().fg(COLOR_MUTED)),
        ]),
        Line::from(vec![
            Span::styled("    2 / F2", Style::default().fg(COLOR_SUCCESS).add_modifier(Modifier::BOLD)),
            Span::styled("        Show upstream gone branches", Style::default().fg(COLOR_MUTED)),
        ]),
        Line::from(vec![
            Span::styled("    3 / F3", Style::default().fg(COLOR_SUCCESS).add_modifier(Modifier::BOLD)),
            Span::styled("        Show unmerged branches", Style::default().fg(COLOR_MUTED)),
        ]),
        Line::from(vec![
            Span::styled("    4 / F4", Style::default().fg(COLOR_SUCCESS).add_modifier(Modifier::BOLD)),
            Span::styled("        Show all branches", Style::default().fg(COLOR_MUTED)),
        ]),
        Line::from(vec![
            Span::styled("    Tab", Style::default().fg(COLOR_SUCCESS).add_modifier(Modifier::BOLD)),
            Span::styled("           Cycle through filters", Style::default().fg(COLOR_MUTED)),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("  Selection", Style::default().fg(COLOR_ACCENT).add_modifier(Modifier::BOLD)),
        ]),
        Line::from(vec![
            Span::styled("    Space", Style::default().fg(COLOR_SUCCESS).add_modifier(Modifier::BOLD)),
            Span::styled("         Toggle selection for current branch", Style::default().fg(COLOR_MUTED)),
        ]),
        Line::from(vec![
            Span::styled("    a", Style::default().fg(COLOR_SUCCESS).add_modifier(Modifier::BOLD)),
            Span::styled("             Select/deselect all safe branches", Style::default().fg(COLOR_MUTED)),
        ]),
        Line::from(vec![
            Span::styled("    c", Style::default().fg(COLOR_SUCCESS).add_modifier(Modifier::BOLD)),
            Span::styled("             Clear all selections", Style::default().fg(COLOR_MUTED)),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("  Actions", Style::default().fg(COLOR_ACCENT).add_modifier(Modifier::BOLD)),
        ]),
        Line::from(vec![
            Span::styled("    Enter", Style::default().fg(COLOR_SUCCESS).add_modifier(Modifier::BOLD)),
            Span::styled("         Confirm and delete selected branches", Style::default().fg(COLOR_MUTED)),
        ]),
        Line::from(vec![
            Span::styled("    f", Style::default().fg(COLOR_DANGER).add_modifier(Modifier::BOLD)),
            Span::styled("             Toggle force mode (allow unmerged)", Style::default().fg(COLOR_MUTED)),
        ]),
        Line::from(vec![
            Span::styled("    d", Style::default().fg(COLOR_WARNING).add_modifier(Modifier::BOLD)),
            Span::styled("             Toggle dry run mode (preview only)", Style::default().fg(COLOR_MUTED)),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("  Other", Style::default().fg(COLOR_ACCENT).add_modifier(Modifier::BOLD)),
        ]),
        Line::from(vec![
            Span::styled("    i", Style::default().fg(COLOR_SUCCESS).add_modifier(Modifier::BOLD)),
            Span::styled("             Show info about the tool", Style::default().fg(COLOR_MUTED)),
        ]),
        Line::from(vec![
            Span::styled("    ?", Style::default().fg(COLOR_SUCCESS).add_modifier(Modifier::BOLD)),
            Span::styled("             Show this help", Style::default().fg(COLOR_MUTED)),
        ]),
        Line::from(vec![
            Span::styled("    q / Esc", Style::default().fg(COLOR_SUCCESS).add_modifier(Modifier::BOLD)),
            Span::styled("       Quit application", Style::default().fg(COLOR_MUTED)),
        ]),
        Line::from(""),
    ];

    let modal = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(COLOR_ACCENT))
                .title(" 📖 Help - Keyboard Shortcuts ")
                .title_style(Style::default().fg(COLOR_ACCENT).add_modifier(Modifier::BOLD)),
        )
        .wrap(Wrap { trim: false })
        .alignment(Alignment::Left);

    frame.render_widget(modal, modal_area);
}

/// Render the info modal
fn render_info_modal(frame: &mut Frame) {
    let area = frame.area();

    // Calculate modal size
    let modal_width = 65.min(area.width - 4);
    let modal_height = 22.min(area.height - 4);

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
        Line::from(vec![
            Span::styled("  Local Git Branch Cleanup TUI", Style::default().fg(COLOR_ACCENT).add_modifier(Modifier::BOLD)),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("  A terminal interface to help you clean up local Git branches", Style::default().fg(COLOR_MUTED)),
        ]),
        Line::from(vec![
            Span::styled("  that are no longer needed.", Style::default().fg(COLOR_MUTED)),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("  Branch Status Types:", Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("    ✓ merged    ", Style::default().fg(COLOR_SUCCESS)),
            Span::styled("Branch has been merged into trunk", Style::default().fg(COLOR_MUTED)),
        ]),
        Line::from(vec![
            Span::styled("    ↗ gone      ", Style::default().fg(COLOR_WARNING)),
            Span::styled("Remote tracking branch was deleted", Style::default().fg(COLOR_MUTED)),
        ]),
        Line::from(vec![
            Span::styled("    ! unmerged  ", Style::default().fg(COLOR_WARNING)),
            Span::styled("Has commits not in trunk (requires force)", Style::default().fg(COLOR_MUTED)),
        ]),
        Line::from(vec![
            Span::styled("    ⊘ protected ", Style::default().fg(COLOR_DANGER)),
            Span::styled("Protected branch (main/master/develop)", Style::default().fg(COLOR_MUTED)),
        ]),
        Line::from(vec![
            Span::styled("    ◉ current   ", Style::default().fg(COLOR_CURRENT)),
            Span::styled("Currently checked out branch", Style::default().fg(COLOR_MUTED)),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("  Press ", Style::default().fg(COLOR_MUTED)),
            Span::styled("?", Style::default().fg(COLOR_ACCENT).add_modifier(Modifier::BOLD)),
            Span::styled(" for keyboard shortcuts", Style::default().fg(COLOR_MUTED)),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("  Press any key to close", Style::default().fg(COLOR_MUTED)),
        ]),
        Line::from(""),
    ];

    let modal = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(COLOR_ACCENT))
                .title(" ℹ️  About ")
                .title_style(Style::default().fg(COLOR_ACCENT).add_modifier(Modifier::BOLD)),
        )
        .wrap(Wrap { trim: false })
        .alignment(Alignment::Left);

    frame.render_widget(modal, modal_area);
}
