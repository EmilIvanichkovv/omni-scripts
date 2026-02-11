// TUI rendering module

use crate::app::App;
use crate::git::BranchStatus;
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, Clear, Paragraph, Row, Table, Wrap},
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
pub fn render(frame: &mut Frame, app: &App) {
    // Create main layout: Header, Content, Action Log, Footer
    let has_log = !app.action_log.is_empty();
    
    let chunks = if has_log {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),  // Header
                Constraint::Min(5),     // Branch list
                Constraint::Length(6),  // Action log
                Constraint::Length(3),  // Footer
            ])
            .split(frame.area())
    } else {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),  // Header
                Constraint::Min(5),     // Branch list
                Constraint::Length(3),  // Footer
            ])
            .split(frame.area())
    };

    render_header(frame, app, chunks[0]);
    render_branch_list(frame, app, chunks[1]);
    
    if has_log {
        render_action_log(frame, app, chunks[2]);
        render_footer(frame, app, chunks[3]);
    } else {
        render_footer(frame, app, chunks[2]);
    }

    // Render confirmation modal on top if shown
    if app.show_confirmation {
        render_confirmation_modal(frame, app);
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
        ]),
    ];

    let header = Paragraph::new(header_text)
        .block(Block::default().borders(Borders::ALL).border_style(Style::default().fg(COLOR_MUTED)));

    frame.render_widget(header, area);
}

/// Render the branch list as a table
fn render_branch_list(frame: &mut Frame, app: &App, area: Rect) {
    // Create table header
    let header_cells = ["", "☑", "Status", "Branch", "Last Commit", ""]
        .iter()
        .map(|h| Cell::from(*h).style(Style::default().fg(COLOR_ACCENT).add_modifier(Modifier::BOLD)));
    let header = Row::new(header_cells).height(1);

    // Create table rows
    let rows: Vec<Row> = app
        .branches
        .iter()
        .enumerate()
        .map(|(i, branch)| {
            let is_cursor = i == app.selected_index;
            let is_checked = app.is_branch_selected(i);
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
                // Status icon
                Cell::from(branch.status.icon()).style(status_style),
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
                // Status label
                Cell::from(branch.status.label()).style(status_style),
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
        Constraint::Length(3),   // Status icon
        Constraint::Min(20),     // Branch name
        Constraint::Length(15),  // Last commit
        Constraint::Length(12),  // Status label
    ];

    let title = format!(
        " Branches ({} total, {} deletable, {} selected) ",
        app.branches.len(),
        app.deletable_count(),
        app.selected_count()
    );

    let table = Table::new(rows, widths)
        .header(header)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(COLOR_MUTED))
                .title(title)
                .title_style(Style::default().fg(COLOR_ACCENT)),
        )
        .row_highlight_style(Style::default().add_modifier(Modifier::BOLD));

    frame.render_widget(table, area);
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
    let legend = Line::from(vec![
        Span::styled("● ", Style::default().fg(COLOR_SUCCESS)),
        Span::styled("merged  ", Style::default().fg(COLOR_MUTED)),
        Span::styled("◆ ", Style::default().fg(COLOR_WARNING)),
        Span::styled("gone  ", Style::default().fg(COLOR_MUTED)),
        Span::styled("▲ ", Style::default().fg(COLOR_WARNING)),
        Span::styled("unmerged  ", Style::default().fg(COLOR_MUTED)),
        Span::styled("⛔ ", Style::default().fg(COLOR_DANGER)),
        Span::styled("protected  ", Style::default().fg(COLOR_MUTED)),
        Span::styled("★ ", Style::default().fg(COLOR_CURRENT)),
        Span::styled("current", Style::default().fg(COLOR_MUTED)),
    ]);

    let key_hints = if app.branches.is_empty() {
        Line::from(vec![
            Span::styled("q", Style::default().fg(COLOR_ACCENT).add_modifier(Modifier::BOLD)),
            Span::styled(" quit", Style::default().fg(COLOR_MUTED)),
        ])
    } else {
        Line::from(vec![
            Span::styled("↑↓/jk", Style::default().fg(COLOR_ACCENT).add_modifier(Modifier::BOLD)),
            Span::styled(" nav  ", Style::default().fg(COLOR_MUTED)),
            Span::styled("Space", Style::default().fg(COLOR_ACCENT).add_modifier(Modifier::BOLD)),
            Span::styled(" select  ", Style::default().fg(COLOR_MUTED)),
            Span::styled("a", Style::default().fg(COLOR_ACCENT).add_modifier(Modifier::BOLD)),
            Span::styled(" all  ", Style::default().fg(COLOR_MUTED)),
            Span::styled("Enter", Style::default().fg(COLOR_SELECTED).add_modifier(Modifier::BOLD)),
            Span::styled(" delete  ", Style::default().fg(COLOR_MUTED)),
            Span::styled("q", Style::default().fg(COLOR_ACCENT).add_modifier(Modifier::BOLD)),
            Span::styled(" quit", Style::default().fg(COLOR_MUTED)),
        ])
    };

    let footer_text = vec![legend, key_hints];

    let footer = Paragraph::new(footer_text)
        .block(Block::default().borders(Borders::ALL).border_style(Style::default().fg(COLOR_MUTED)));

    frame.render_widget(footer, area);
}

/// Render the confirmation modal
fn render_confirmation_modal(frame: &mut Frame, app: &App) {
    let area = frame.area();
    
    // Calculate modal size
    let modal_width = 60.min(area.width - 4);
    let modal_height = 12.min(area.height - 4);
    
    let modal_area = Rect {
        x: (area.width - modal_width) / 2,
        y: (area.height - modal_height) / 2,
        width: modal_width,
        height: modal_height,
    };

    // Clear the area behind the modal
    frame.render_widget(Clear, modal_area);

    // Build modal content
    let selected_branches = app.get_selected_branches();
    let unmerged_count = selected_branches.iter()
        .filter(|b| b.status == BranchStatus::Unmerged)
        .count();

    let mut lines = vec![
        Line::from(""),
        Line::from(vec![
            Span::styled("  Delete ", Style::default().fg(COLOR_MUTED)),
            Span::styled(
                format!("{}", app.selected_count()),
                Style::default().fg(COLOR_DANGER).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" branch(es)?", Style::default().fg(COLOR_MUTED)),
        ]),
        Line::from(""),
    ];

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
        Span::styled("  ", Style::default()),
        Span::styled("y", Style::default().fg(COLOR_SUCCESS).add_modifier(Modifier::BOLD)),
        Span::styled(" confirm   ", Style::default().fg(COLOR_MUTED)),
        Span::styled("n", Style::default().fg(COLOR_DANGER).add_modifier(Modifier::BOLD)),
        Span::styled(" cancel", Style::default().fg(COLOR_MUTED)),
    ]));

    let modal = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(COLOR_DANGER))
                .title(" ⚠️  Confirm Deletion ")
                .title_style(Style::default().fg(COLOR_DANGER).add_modifier(Modifier::BOLD)),
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
