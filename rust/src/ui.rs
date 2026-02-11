// TUI rendering module

use crate::app::App;
use crate::git::BranchStatus;
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, Paragraph, Row, Table},
    Frame,
};

// Color palette
const COLOR_ACCENT: Color = Color::Rgb(46, 196, 182);    // #2EC4B6 - cyan
const COLOR_WARNING: Color = Color::Rgb(255, 184, 108);  // #FFB86C - amber
const COLOR_DANGER: Color = Color::Rgb(255, 85, 85);     // #FF5555 - red
const COLOR_MUTED: Color = Color::Rgb(169, 177, 214);    // #A9B1D6 - muted text
const COLOR_SUCCESS: Color = Color::Rgb(80, 250, 123);   // #50FA7B - green
const COLOR_CURRENT: Color = Color::Rgb(189, 147, 249);  // #BD93F9 - purple

/// Render the TUI
pub fn render(frame: &mut Frame, app: &App) {
    // Create main layout: Header, Content, Footer
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),  // Header
            Constraint::Min(5),     // Branch list
            Constraint::Length(3),  // Footer
        ])
        .split(frame.area());

    render_header(frame, app, chunks[0]);
    render_branch_list(frame, app, chunks[1]);
    render_footer(frame, app, chunks[2]);
}

/// Render the header with app name and repo info
fn render_header(frame: &mut Frame, app: &App, area: Rect) {
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
        ]),
    ];

    let header = Paragraph::new(header_text)
        .block(Block::default().borders(Borders::ALL).border_style(Style::default().fg(COLOR_MUTED)));

    frame.render_widget(header, area);
}

/// Render the branch list as a table
fn render_branch_list(frame: &mut Frame, app: &App, area: Rect) {
    // Create table header
    let header_cells = ["", "Status", "Branch", "Last Commit", ""]
        .iter()
        .map(|h| Cell::from(*h).style(Style::default().fg(COLOR_ACCENT).add_modifier(Modifier::BOLD)));
    let header = Row::new(header_cells).height(1);

    // Create table rows
    let rows: Vec<Row> = app
        .branches
        .iter()
        .enumerate()
        .map(|(i, branch)| {
            let is_selected = i == app.selected_index;
            let status_style = get_status_style(&branch.status);

            let cells = vec![
                // Selection indicator
                Cell::from(if is_selected { "▶" } else { " " })
                    .style(Style::default().fg(COLOR_ACCENT)),
                // Status icon
                Cell::from(branch.status.icon()).style(status_style),
                // Branch name
                Cell::from(branch.name.as_str()).style(if is_selected {
                    Style::default().fg(Color::White).add_modifier(Modifier::BOLD)
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
            if is_selected {
                row.style(Style::default().bg(Color::Rgb(30, 35, 45)))
            } else {
                row
            }
        })
        .collect();

    // Column widths
    let widths = [
        Constraint::Length(2),   // Selection indicator
        Constraint::Length(3),   // Status icon
        Constraint::Min(20),     // Branch name
        Constraint::Length(15),  // Last commit
        Constraint::Length(12),  // Status label
    ];

    let title = format!(
        " Branches ({} total, {} deletable) ",
        app.branches.len(),
        app.deletable_count()
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
            Span::styled(" navigate  ", Style::default().fg(COLOR_MUTED)),
            Span::styled("q", Style::default().fg(COLOR_ACCENT).add_modifier(Modifier::BOLD)),
            Span::styled(" quit", Style::default().fg(COLOR_MUTED)),
        ])
    };

    let footer_text = vec![legend, key_hints];

    let footer = Paragraph::new(footer_text)
        .block(Block::default().borders(Borders::ALL).border_style(Style::default().fg(COLOR_MUTED)));

    frame.render_widget(footer, area);
}

/// Get the style for a branch status
fn get_status_style(status: &BranchStatus) -> Style {
    match status {
        BranchStatus::SafeMerged => Style::default().fg(COLOR_SUCCESS),
        BranchStatus::GoneUpstream => Style::default().fg(COLOR_WARNING),
        BranchStatus::Unmerged => Style::default().fg(COLOR_WARNING),
        BranchStatus::Protected => Style::default().fg(COLOR_DANGER),
        BranchStatus::Current => Style::default().fg(COLOR_CURRENT),
    }
}
