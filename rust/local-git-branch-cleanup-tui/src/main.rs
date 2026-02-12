mod app;
mod git;
mod ui;

use app::App;
use clap::Parser;
use color_eyre::Result;
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use git::BranchStatus;
use ratatui::prelude::*;
use std::io::{self, Write};

#[derive(Parser, Debug)]
#[command(name = "local-git-branch-cleanup-tui")]
#[command(about = "Interactive TUI for cleaning up local Git branches", long_about = None)]
#[command(version)]
struct Args {
    /// Override the default trunk branch
    #[arg(long)]
    trunk: Option<String>,

    /// Force delete unmerged branches (use with caution!)
    #[arg(long, short = 'f')]
    force: bool,

    /// Use CLI mode instead of TUI
    #[arg(long)]
    cli: bool,

    /// Dry run mode - preview actions without executing
    #[arg(long)]
    dry_run: bool,
}

fn main() -> Result<()> {
    // Initialize error handling
    color_eyre::install()?;

    let args = Args::parse();

    // Verify we're in a git repository
    let repo_path = match git::verify_repo() {
        Ok(path) => path,
        Err(e) => {
            eprintln!("❌ Error: {}", e);
            std::process::exit(1);
        }
    };

    // Get the trunk branch
    let trunk = git::get_default_branch(args.trunk.as_deref())?;

    // Get branches with classification
    let branches = match git::get_branches_with_classification(args.trunk.as_deref()) {
        Ok(branches) => branches,
        Err(e) => {
            eprintln!("❌ Error scanning branches: {}", e);
            std::process::exit(1);
        }
    };

    // Use CLI mode if --cli flag is set
    if args.cli {
        return run_cli_mode(&branches, &trunk, args.force, args.dry_run);
    }

    // Run TUI mode
    run_tui_mode(branches, repo_path, trunk, args.force, args.dry_run)
}

/// Run the interactive TUI mode
fn run_tui_mode(branches: Vec<git::BranchInfo>, repo_path: String, trunk: String, force_mode: bool, dry_run: bool) -> Result<()> {
    // Set up terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Create app state
    let mut app = App::new(branches, repo_path, trunk);
    app.force_mode = force_mode;
    app.dry_run = dry_run;

    // Main loop
    loop {
        // Draw UI
        terminal.draw(|frame| ui::render(frame, &app))?;

        // Handle events
        if event::poll(std::time::Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    // Handle help modal
                    if app.show_help {
                        // Any key closes help modal
                        app.show_help = false;
                    } else if app.show_confirmation {
                        // Handle confirmation modal
                        match key.code {
                            KeyCode::Char('y') | KeyCode::Char('Y') => {
                                // Confirm deletion
                                if !app.dry_run {
                                    app.delete_selected_branches();
                                } else {
                                    // Dry run: just log what would happen
                                    let branches_to_preview: Vec<_> = app.get_selected_branches().iter().map(|b| b.name.clone()).collect();
                                    for branch_name in branches_to_preview {
                                        app.action_log.push(app::ActionLogEntry {
                                            branch_name: branch_name.clone(),
                                            success: true,
                                            message: format!("[DRY RUN] Would delete: {}", branch_name),
                                        });
                                    }
                                    app.clear_selection();
                                }
                                app.show_confirmation = false;
                                if !app.dry_run {
                                    app.refresh_branches();
                                }
                            }
                            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                                // Cancel deletion
                                app.show_confirmation = false;
                            }
                            _ => {}
                        }
                    } else if app.search_active {
                        // Handle search mode input
                        match key.code {
                            KeyCode::Esc => {
                                // Exit search and clear query
                                app.search_active = false;
                                app.search_query.clear();
                                app.selected_index = 0;
                            }
                            KeyCode::Enter => {
                                // Exit search but keep the query filter active
                                app.search_active = false;
                            }
                            KeyCode::Backspace => {
                                // Remove last character
                                app.search_query.pop();
                                app.selected_index = 0;
                            }
                            KeyCode::Char(c) => {
                                // Add character to search query
                                app.search_query.push(c);
                                app.selected_index = 0;
                            }
                            KeyCode::Down | KeyCode::Up => {
                                // Allow navigation while searching
                                if key.code == KeyCode::Down {
                                    app.select_next();
                                } else {
                                    app.select_prev();
                                }
                            }
                            _ => {}
                        }
                    } else {
                        // Normal mode
                        match key.code {
                            KeyCode::Char('q') => {
                                app.quit();
                            }
                            KeyCode::Esc => {
                                // If search query is active, clear it first
                                if !app.search_query.is_empty() {
                                    app.search_query.clear();
                                    app.selected_index = 0;
                                } else {
                                    app.quit();
                                }
                            }
                            KeyCode::Down | KeyCode::Char('j') => {
                                app.select_next();
                            }
                            KeyCode::Up | KeyCode::Char('k') => {
                                app.select_prev();
                            }
                            KeyCode::Char(' ') => {
                                // Toggle selection of current branch (using filtered index)
                                app.toggle_selection_at_cursor();
                            }
                            KeyCode::Char('a') => {
                                // Select all safe branches
                                app.select_all_safe();
                            }
                            KeyCode::Char('c') => {
                                // Clear selection
                                app.clear_selection();
                            }
                            KeyCode::Char('f') => {
                                // Toggle force mode
                                app.force_mode = !app.force_mode;
                                // Clear selection when toggling force mode
                                app.clear_selection();
                            }
                            KeyCode::Char('d') => {
                                // Toggle dry run mode
                                app.dry_run = !app.dry_run;
                            }
                            KeyCode::Char('?') => {
                                // Toggle help modal
                                app.show_help = !app.show_help;
                            }
                            KeyCode::Char('/') => {
                                // Enter search mode
                                app.search_active = true;
                            }
                            KeyCode::Char('F') => {
                                // Toggle filter bar visibility (Shift+F)
                                app.show_filter = !app.show_filter;
                            }
                            KeyCode::Tab => {
                                // Cycle to next filter
                                app.next_filter();
                            }
                            KeyCode::Char('1') | KeyCode::F(1) => {
                                // Safe merged filter
                                app.set_filter(app::FilterMode::SafeMerged);
                            }
                            KeyCode::Char('2') | KeyCode::F(2) => {
                                // Gone upstream filter
                                app.set_filter(app::FilterMode::GoneUpstream);
                            }
                            KeyCode::Char('3') | KeyCode::F(3) => {
                                // Unmerged filter
                                app.set_filter(app::FilterMode::Unmerged);
                            }
                            KeyCode::Char('4') | KeyCode::F(4) => {
                                // All branches filter
                                app.set_filter(app::FilterMode::All);
                            }
                            KeyCode::Enter => {
                                // Show confirmation if branches are selected
                                if app.selected_count() > 0 {
                                    app.show_confirmation = true;
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
        }

        if app.should_quit {
            break;
        }
    }

    // Restore terminal
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    Ok(())
}

/// Run the CLI mode (non-interactive)
fn run_cli_mode(branches: &[git::BranchInfo], trunk: &str, force: bool, dry_run: bool) -> Result<()> {
    println!("🌳 Trunk branch: {}", trunk);
    
    // Show mode indicators
    if force {
        println!("⚠️  FORCE MODE: Will force-delete unmerged branches");
    }
    if dry_run {
        println!("🔍 DRY RUN: Preview mode - no branches will be deleted");
    }
    
    println!();

    // Print header
    print_header();

    // If no branches found, exit
    if branches.is_empty() {
        print_boxed_line("✓ No local branches without a remote counterpart.");
        print_footer();
        return Ok(());
    }

    // Count by status
    let deletable_count = branches.iter().filter(|b| b.status.is_deletable()).count();
    let protected_count = branches.len() - deletable_count;

    // Display branches with status
    print_boxed_line(&format!(
        "📋 Found {} branches ({} deletable, {} protected):",
        branches.len(),
        deletable_count,
        protected_count
    ));
    println!();

    // Print legend
    print_boxed_line("   Legend: ✓ merged  ↗ gone  ! unmerged  ⊘ protected  ◉ current");
    print_separator();

    for branch in branches {
        let status_indicator = format!("{} {}", branch.status.icon(), branch.status.label());
        let line = format!(
            "   {} {:30} [{:>12}] {}",
            if branch.status.is_deletable() { "[ ]" } else { "   " },
            branch.name,
            branch.last_commit_relative,
            status_indicator
        );
        print_boxed_line(&line);
    }

    // If no deletable branches, exit
    if deletable_count == 0 {
        print_separator();
        print_boxed_line("ℹ️  No branches can be deleted (all protected or current).");
        print_footer();
        return Ok(());
    }

    // Print confirmation prompt
    print_separator();
    print_boxed_line("⚠️  These branches are not present on the remote.");

    // Show warning about unmerged branches
    let unmerged_count = branches
        .iter()
        .filter(|b| b.status == BranchStatus::Unmerged)
        .count();

    if unmerged_count > 0 && !force {
        print_boxed_line(&format!(
            "⚠️  {} branch(es) have UNMERGED commits - use --force to delete them",
            unmerged_count
        ));
    }

    print_boxed_line("🗑️  Do you want to delete the deletable branches?");
    println!();
    print_boxed_line("   [y] Yes, delete them");
    print_boxed_line("   [n] No, do not delete them");
    print_footer();

    // Get user confirmation
    print!("\n Your choice: ");
    io::stdout().flush()?;

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;

    if !input.trim().eq_ignore_ascii_case("y") {
        println!();
        print_header();
        print_boxed_line("✋ Operation cancelled. No branches were deleted.");
        print_footer();
        return Ok(());
    }

    // Delete branches
    println!();
    print_header();
    print_boxed_line("🗑️  Deleting branches...");
    print_separator();

    let mut deleted_count = 0;
    let mut skipped_count = 0;
    let mut failed_count = 0;

    for branch in branches {
        // Skip non-deletable branches
        if !branch.status.is_deletable() {
            print_boxed_line(&format!(
                "   ⏭️  Skipped: {} ({})",
                branch.name,
                branch.status.label()
            ));
            skipped_count += 1;
            continue;
        }

        // For unmerged branches, only delete if --force is set
        if branch.status == BranchStatus::Unmerged && !force {
            print_boxed_line(&format!(
                "   ⏭️  Skipped: {} (unmerged - use --force)",
                branch.name
            ));
            skipped_count += 1;
            continue;
        }

        // Use safe delete for merged/gone branches, force for unmerged
        let use_force = branch.status == BranchStatus::Unmerged;

        match git::delete_branch_with_mode(&branch.name, use_force) {
            Ok(_) => {
                let method = if use_force { "-D" } else { "-d" };
                print_boxed_line(&format!("   ✓ Deleted: {} ({})", branch.name, method));
                deleted_count += 1;
            }
            Err(e) => {
                print_boxed_line(&format!("   ✗ Failed: {} ({})", branch.name, e));
                failed_count += 1;
            }
        }
    }

    // Print summary
    print_separator();
    if failed_count == 0 {
        print_boxed_line(&format!(
            "✅ Cleanup complete! Deleted: {}, Skipped: {}",
            deleted_count, skipped_count
        ));
    } else {
        print_boxed_line(&format!(
            "⚠️  Cleanup finished. Deleted: {}, Skipped: {}, Failed: {}",
            deleted_count, skipped_count, failed_count
        ));
    }
    print_footer();

    Ok(())
}

fn print_header() {
    let width = 80;
    println!("┌{}┐", "─".repeat(width - 2));
    print_centered("🧹 Local Git Branch Cleanup", width);
    println!("├{}┤", "─".repeat(width - 2));
}

fn print_footer() {
    let width = 80;
    println!("└{}┘", "─".repeat(width - 2));
}

fn print_separator() {
    let width = 80;
    println!("├{}┤", "─".repeat(width - 2));
}

fn print_centered(text: &str, width: usize) {
    // Strip ANSI codes for length calculation
    let text_len = text.chars().count();
    let padding = (width - 2 - text_len) / 2;
    println!("│{}{:^width$}│", " ".repeat(padding), text, width = width - 2 - padding);
}

fn print_boxed_line(text: &str) {
    let width = 80;
    println!("│ {:<width$} │", text, width = width - 4);
}
