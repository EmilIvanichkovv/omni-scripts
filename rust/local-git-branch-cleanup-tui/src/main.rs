mod app;
mod cache;
mod clipboard;
mod git;
mod pr;
mod refresh;
mod remote;
mod tls;
mod ui;
mod update;
mod update_notice;
mod worktree;
mod worktree_view;

use app::App;
use clap::{Parser, Subcommand};
use color_eyre::Result;
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use pr::PullRequestProvider;
use ratatui::prelude::*;
use std::io::{self, Write};

#[derive(Parser, Debug)]
#[command(name = "local-git-branch-cleanup-tui")]
#[command(about = "Interactive TUI for cleaning up local Git branches", long_about = None)]
#[command(version)]
#[command(args_conflicts_with_subcommands = true)]
struct Args {
    #[command(subcommand)]
    command: Option<Command>,

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

    /// Enable GitHub PR integration (requires gh CLI)
    /// Shows PR status for branches with associated pull requests
    #[arg(long, short = 'g', conflicts_with = "bitbucket")]
    github: bool,

    /// Enable Bitbucket Data Center PR integration (requires BITBUCKET_TOKEN)
    /// Shows PR status for branches with associated pull requests
    #[arg(long, short = 'b', conflicts_with = "github")]
    bitbucket: bool,

    /// Override the Bitbucket base URL derived from origin (may include a context path)
    #[arg(long, requires = "bitbucket")]
    bitbucket_base_url: Option<String>,

    /// Override the Bitbucket project key derived from origin
    #[arg(long, requires = "bitbucket")]
    bitbucket_project: Option<String>,

    /// Override the Bitbucket repository slug derived from origin
    #[arg(long, requires = "bitbucket")]
    bitbucket_repo: Option<String>,

    /// Force sequential PR fetching (no tokio concurrency). For benchmarking only.
    #[arg(long, hide = true)]
    sequential: bool,

    /// Fetch PR data and print timing stats, then exit. For benchmarking only.
    #[arg(long, hide = true)]
    fetch_only: bool,

    /// Do not check for new versions
    #[arg(long)]
    no_update_check: bool,

    /// Start in the worktree view (press `w` to switch views); with --cli,
    /// print the worktree table and exit
    #[arg(long, short = 'w')]
    worktrees: bool,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Update to the newest release
    Update {
        /// Only report whether an update is available (exit code 10 if one is)
        #[arg(long)]
        check: bool,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize error handling
    color_eyre::install()?;
    tls::install_crypto_provider();

    let args = Args::parse();

    // Works from any directory, so it runs before the git repository check.
    if let Some(Command::Update { check }) = args.command {
        std::process::exit(update::run(check).await);
    }

    // Verify we're in a git repository
    let repo_path = match git::verify_repo() {
        Ok(path) => path,
        Err(e) => {
            eprintln!("❌ Error: {}", e);
            std::process::exit(1);
        }
    };

    // Runs in the background while branches and PRs load; never delays startup.
    let mut update_notice = update_notice::UpdateNotice::start(args.no_update_check);

    // Get the trunk branch
    let trunk = git::get_default_branch(args.trunk.as_deref())?;

    // Get branches with classification
    let mut branches = match git::get_branches_with_classification(args.trunk.as_deref()) {
        Ok(branches) => branches,
        Err(e) => {
            eprintln!("❌ Error scanning branches: {}", e);
            std::process::exit(1);
        }
    };

    // Select the PR provider (exactly zero or one; clap enforces the conflict)
    let provider: Option<std::sync::Arc<dyn pr::PullRequestProvider>> = if args.github {
        let provider = pr::github::GitHubProvider::new();
        match provider.validate().await {
            Err(e) => {
                // A missing gh CLI is non-fatal: warn, disable PR integration,
                // and continue with local-only behavior (backward compatible).
                eprintln!("⚠️  {}", e);
                None
            }
            Ok(()) => Some(std::sync::Arc::new(provider) as _),
        }
    } else if args.bitbucket {
        // Bitbucket configuration/authentication/validation failures are fatal
        // before any UI is drawn (spec sections 4.4 and 9.4).
        match build_bitbucket_provider(&args).await {
            Ok(provider) => Some(provider),
            Err(e) => {
                eprintln!("❌ {}", e);
                std::process::exit(1);
            }
        }
    } else {
        None
    };

    // Fetch PR info if a provider is enabled
    let pr_session = match provider {
        None => None,
        Some(provider) => {
            let cache_key = provider.cache_key();
            let kind = provider.kind();
            let ttl = std::time::Duration::from_secs(3600);

            eprintln!("🔗 Fetching {} PR info...", kind.label());
            let t0 = std::time::Instant::now();
            let mut pr_cache = None;
            let report = match cache::PrCache::open(&cache_key, ttl) {
                Ok(opened) => {
                    let opened = pr_cache.insert(opened);
                    opened
                        .evict_stale(std::time::Duration::from_secs(30 * 24 * 60 * 60))
                        .ok();
                    pr::fetch_pr_info_for_branches(
                        provider,
                        &mut branches,
                        Some(opened),
                        args.sequential,
                    )
                    .await
                }
                Err(e) => {
                    eprintln!("⚠️  PR cache unavailable ({}), fetching live data.", e);
                    pr::fetch_pr_info_for_branches(provider, &mut branches, None, args.sequential)
                        .await
                }
            };
            // PR data can prove merges that git ancestry cannot see
            // (squash/rebase merges with a still-existing remote branch).
            git::apply_pr_merge_status(&mut branches);
            print_fetch_summary(&report, t0.elapsed());
            if args.fetch_only {
                std::process::exit(if report.failed > 0 { 1 } else { 0 });
            }
            Some(PrSession {
                kind,
                cache: pr_cache,
            })
        }
    };

    // Use CLI mode if --cli flag is set
    if args.cli {
        let result = if args.worktrees {
            run_worktree_cli_mode(
                &branches,
                &repo_path,
                &trunk,
                args.force,
                args.dry_run,
                pr_session.as_ref().map(|session| session.kind),
            )
        } else {
            run_cli_mode(
                &branches,
                &trunk,
                args.force,
                args.dry_run,
                pr_session.as_ref(),
            )
        };
        // On stderr, so it never mixes with the output scripts read.
        if let Some(message) = update_notice.message() {
            eprintln!("{message}");
        }
        return result;
    }

    // Run TUI mode
    run_tui_mode(
        branches,
        repo_path,
        trunk,
        args.force,
        args.dry_run,
        pr_session,
        args.worktrees,
        update_notice,
    )
}

/// Resolve Bitbucket configuration (CLI > env > derived-from-origin), require
/// a non-empty `BITBUCKET_TOKEN`, construct the HTTP client, and validate
/// repository access with one metadata request.
async fn build_bitbucket_provider(
    args: &Args,
) -> Result<std::sync::Arc<dyn pr::PullRequestProvider>, pr::PrProviderError> {
    use pr::bitbucket::{self, BitbucketConfigInput};

    // Derivation failures are not fatal by themselves: explicit CLI/env values
    // may fill every gap. resolve_repository reports what is still missing.
    let derived = remote::read_origin()
        .ok()
        .and_then(|r| remote::infer_bitbucket_repository(&r).ok());

    let repository = bitbucket::resolve_repository(BitbucketConfigInput {
        cli_base_url: args.bitbucket_base_url.clone(),
        cli_project: args.bitbucket_project.clone(),
        cli_repo: args.bitbucket_repo.clone(),
        env_base_url: std::env::var("BITBUCKET_BASE_URL").ok(),
        env_project: std::env::var("BITBUCKET_PROJECT").ok(),
        env_repo: std::env::var("BITBUCKET_REPO").ok(),
        derived_base_url: derived.as_ref().map(|d| d.base_url.clone()),
        derived_project: derived.as_ref().map(|d| d.project_key.clone()),
        derived_repo: derived.as_ref().map(|d| d.repository_slug.clone()),
    })?;

    // The token is env-only and never part of Args (Args derives Debug).
    let token = std::env::var("BITBUCKET_TOKEN")
        .ok()
        .filter(|t| !t.trim().is_empty())
        .ok_or_else(|| {
            pr::PrProviderError::Configuration(
                "BITBUCKET_TOKEN is not set. Export a Bitbucket Data Center HTTP access token \
                 with repository read permission."
                    .to_string(),
            )
        })?;

    let provider = bitbucket::BitbucketProvider::new(repository, token)?;
    provider.validate().await?;
    Ok(std::sync::Arc::new(provider))
}

/// Print the user-visible PR fetch summary.
///
/// Failed lookups are listed (up to five examples) and are never cached, so
/// they will be retried on the next run.
fn print_fetch_summary(report: &pr::FetchReport, elapsed: std::time::Duration) {
    let fetched = report.fetched_with_pr + report.fetched_without_pr;
    eprintln!(
        "   {} from cache, {} fetched, {} failed",
        report.cache_hits, fetched, report.failed
    );
    eprintln!("   fetch completed in {:.2}s", elapsed.as_secs_f64());
    if report.failed > 0 {
        eprintln!(
            "   ⚠️  {} PR lookup(s) failed and were not cached:",
            report.failed
        );
        for err in report.errors.iter().take(5) {
            eprintln!("      {}: {}", err.branch, err.error);
        }
    }
    if report.cache_hits == 0 && report.cache_misses > 20 {
        eprintln!("   (tip: subsequent runs will be instant — results are cached for 1h)");
    }
}

/// PR integration chosen for this run. The cache stays open after the startup
/// fetch so deleting a branch can drop its cached PR entry.
struct PrSession {
    kind: pr::PrProviderKind,
    cache: Option<cache::PrCache>,
}

/// Run the interactive TUI mode
#[allow(clippy::too_many_arguments)]
fn run_tui_mode(
    branches: Vec<git::BranchInfo>,
    repo_path: String,
    trunk: String,
    force_mode: bool,
    dry_run: bool,
    pr_session: Option<PrSession>,
    start_in_worktrees: bool,
    mut update_notice: update_notice::UpdateNotice,
) -> Result<()> {
    // Get current git user for @author:me support
    let current_git_user = git::get_current_git_user().unwrap_or_default();

    // Create app state
    let mut app = App::new(branches, repo_path, trunk, current_git_user);
    app.force_mode = force_mode;
    app.dry_run = dry_run;
    if let Some(session) = pr_session {
        app.pr_provider = Some(session.kind);
        app.pr_cache = session.cache;
    }

    // Fail before the terminal is taken over when the worktree list is unreadable
    if start_in_worktrees {
        app.load_worktrees()?;
        app.toggle_view();
    }

    // Set up terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Main loop
    loop {
        app.expire_esc_quit(std::time::Instant::now());
        app.update_notice = update_notice.message();
        app.poll_worktree_status();
        app.poll_refresh();

        // Draw UI
        terminal.draw(|frame| ui::render(frame, &mut app))?;

        // Handle events
        if event::poll(std::time::Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    // Any key other than Esc cancels a pending Esc quit
                    if key.code != KeyCode::Esc {
                        app.esc_quit_armed_at = None;
                    }
                    // Any key dismisses a footer message
                    app.flash = None;

                    // Handle help modal
                    if app.show_help {
                        // Any key closes help modal
                        app.show_help = false;
                    } else if app.show_info {
                        // Any key closes info modal
                        app.show_info = false;
                    } else if app.show_confirmation {
                        // Handle confirmation modal
                        match key.code {
                            KeyCode::Char('b') | KeyCode::Char('B')
                                if app.view == app::ViewMode::Worktrees =>
                            {
                                // Opt in to deleting the removed worktrees' branches
                                if let Some(wt) = app.worktrees.as_mut() {
                                    wt.delete_branches = !wt.delete_branches;
                                }
                            }
                            KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter
                                if app.view == app::ViewMode::Worktrees =>
                            {
                                // Remove worktrees (dry run is handled inside)
                                app.remove_selected_worktrees();
                            }
                            KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
                                // Confirm deletion
                                if !app.dry_run {
                                    app.delete_selected_branches();
                                } else {
                                    // Dry run: just log what would happen
                                    let branches_to_preview: Vec<_> = app
                                        .get_selected_branches()
                                        .iter()
                                        .map(|b| b.name.clone())
                                        .collect();
                                    for branch_name in branches_to_preview {
                                        app.action_log.push(app::ActionLogEntry {
                                            branch_name: branch_name.clone(),
                                            success: true,
                                            message: format!(
                                                "[DRY RUN] Would delete: {}",
                                                branch_name
                                            ),
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
                                app.search_cursor_pos = 0;
                                app.selected_index = 0;
                                app.scroll_offset = 0;
                                app.hide_suggestions();
                            }
                            KeyCode::Tab => {
                                // Accept suggestion if showing, otherwise ignore
                                if app.show_suggestions {
                                    app.accept_suggestion();
                                }
                            }
                            KeyCode::Enter => {
                                // Accept suggestion if showing, otherwise exit search
                                if app.show_suggestions {
                                    app.accept_suggestion();
                                } else {
                                    // Exit search but keep the query filter active
                                    app.search_active = false;
                                    app.hide_suggestions();
                                }
                            }
                            KeyCode::Left => {
                                // Move cursor left in search query
                                app.search_cursor_left();
                            }
                            KeyCode::Right => {
                                // Move cursor right in search query
                                app.search_cursor_right();
                            }
                            KeyCode::Home => {
                                // Move cursor to start of search query
                                app.search_cursor_start();
                            }
                            KeyCode::End => {
                                // Move cursor to end of search query
                                app.search_cursor_end();
                            }
                            KeyCode::Down => {
                                // Navigate suggestions if showing, otherwise exit search
                                if app.show_suggestions {
                                    app.suggestion_next();
                                } else {
                                    app.search_active = false;
                                    app.select_next();
                                }
                            }
                            KeyCode::Up => {
                                // Navigate suggestions if showing, otherwise exit search
                                if app.show_suggestions {
                                    app.suggestion_prev();
                                } else {
                                    app.search_active = false;
                                    app.select_prev();
                                }
                            }
                            KeyCode::Backspace => {
                                // Delete character before cursor
                                app.search_backspace();
                            }
                            KeyCode::Delete => {
                                // Delete character at cursor
                                app.search_delete();
                            }
                            KeyCode::Char(c) => {
                                // Insert character at cursor position
                                app.search_insert_char(c);
                            }
                            _ => {}
                        }
                    } else if app.view == app::ViewMode::Worktrees {
                        handle_worktree_view_key(&mut app, key);
                    } else {
                        // Normal mode
                        // Check for Ctrl key combinations first
                        if key.modifiers.contains(KeyModifiers::CONTROL) {
                            match key.code {
                                KeyCode::Char('u') => {
                                    // Ctrl+U: Go to top (like vim)
                                    app.go_to_top();
                                }
                                KeyCode::Char('d') => {
                                    // Ctrl+D: Go to bottom (like vim)
                                    app.go_to_bottom();
                                }
                                _ => {}
                            }
                        } else {
                            match key.code {
                                KeyCode::Char('q') => {
                                    app.quit();
                                }
                                KeyCode::Esc => {
                                    // If search query is active, clear it first
                                    if !app.search_query.is_empty() {
                                        app.search_query.clear();
                                        app.selected_index = 0;
                                        app.scroll_offset = 0;
                                    } else {
                                        app.press_esc_to_quit(std::time::Instant::now());
                                    }
                                }
                                KeyCode::Down | KeyCode::Char('j') => {
                                    app.select_next();
                                }
                                KeyCode::Up | KeyCode::Char('k') => {
                                    app.select_prev();
                                }
                                KeyCode::Home | KeyCode::Char('g') => {
                                    // Go to top of list
                                    app.go_to_top();
                                }
                                KeyCode::End | KeyCode::Char('G') => {
                                    // Go to bottom of list
                                    app.go_to_bottom();
                                }
                                KeyCode::PageUp => {
                                    // Move up by one page
                                    app.page_up();
                                }
                                KeyCode::PageDown => {
                                    // Move down by one page
                                    app.page_down();
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
                                    // Toggle force mode; turning it off drops
                                    // only the selections that need force
                                    app.toggle_force_mode();
                                }
                                KeyCode::Char('d') => {
                                    // Toggle dry run mode
                                    app.dry_run = !app.dry_run;
                                }
                                KeyCode::Char('?') => {
                                    // Toggle help modal
                                    app.show_help = !app.show_help;
                                }
                                KeyCode::Char('i') => {
                                    // Toggle info modal
                                    app.show_info = !app.show_info;
                                }
                                KeyCode::Char('s') => {
                                    // Cycle sort mode
                                    app.cycle_sort_mode();
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
                                KeyCode::Char('o') => {
                                    // Open PR URL in browser (if PR integration enabled and PR exists)
                                    if app.pr_provider.is_some() {
                                        app.open_selected_pr();
                                    }
                                }
                                KeyCode::Char('y') => {
                                    // Copy the branch name under the cursor
                                    app.copy_selected_branch_name();
                                }
                                KeyCode::Char('w') => {
                                    // Switch to the worktree view
                                    app.toggle_view();
                                }
                                KeyCode::Char('r') => {
                                    // Reload the branch list in the background
                                    app.request_refresh(refresh::RefreshKind::Local);
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

/// Normal-mode keys of the worktree view. Navigation, search, help/info,
/// force, dry run and quit behave as in the branch view.
fn handle_worktree_view_key(app: &mut App, key: event::KeyEvent) {
    use worktree_view::WorktreeFilter;

    if key.modifiers.contains(KeyModifiers::CONTROL) {
        match key.code {
            KeyCode::Char('u') => app.go_to_top(),
            KeyCode::Char('d') => app.go_to_bottom(),
            _ => {}
        }
        return;
    }

    match key.code {
        KeyCode::Char('q') => app.quit(),
        KeyCode::Esc => {
            if !app.search_query.is_empty() {
                app.search_query.clear();
                app.search_cursor_pos = 0;
                app.selected_index = 0;
                app.scroll_offset = 0;
            } else {
                app.press_esc_to_quit(std::time::Instant::now());
            }
        }
        KeyCode::Down | KeyCode::Char('j') => app.select_next(),
        KeyCode::Up | KeyCode::Char('k') => app.select_prev(),
        KeyCode::Home | KeyCode::Char('g') => app.go_to_top(),
        KeyCode::End | KeyCode::Char('G') => app.go_to_bottom(),
        KeyCode::PageUp => app.page_up(),
        KeyCode::PageDown => app.page_down(),
        KeyCode::Char(' ') => app.toggle_worktree_at_cursor(),
        KeyCode::Char('a') => app.select_all_stale_worktrees(),
        KeyCode::Char('c') => {
            if app.require_worktree_remove_mode() {
                app.clear_selection();
            }
        }
        KeyCode::Char('x') => app.toggle_worktree_remove_mode(),
        KeyCode::Char('r') => app.request_refresh(refresh::RefreshKind::Local),
        KeyCode::Char('f') => app.toggle_force_mode(),
        KeyCode::Char('d') => app.dry_run = !app.dry_run,
        KeyCode::Char('?') => app.show_help = !app.show_help,
        KeyCode::Char('i') => app.show_info = !app.show_info,
        KeyCode::Char('/') => app.search_active = true,
        KeyCode::Char('F') => app.show_filter = !app.show_filter,
        KeyCode::Tab => app.next_worktree_filter(),
        KeyCode::Char('1') | KeyCode::F(1) => app.set_worktree_filter(WorktreeFilter::Clean),
        KeyCode::Char('2') | KeyCode::F(2) => app.set_worktree_filter(WorktreeFilter::Dirty),
        KeyCode::Char('3') | KeyCode::F(3) => app.set_worktree_filter(WorktreeFilter::Stale),
        KeyCode::Char('4') | KeyCode::F(4) => app.set_worktree_filter(WorktreeFilter::All),
        KeyCode::Char('o') => {
            if app.pr_provider.is_some() && !app.open_selected_pr() {
                app.flash = Some("No pull request for this worktree's branch".to_string());
            }
        }
        KeyCode::Char('y') => app.copy_selected_worktree_path(),
        KeyCode::Char('w') => app.toggle_view(),
        KeyCode::Enter => app.request_worktree_removal(),
        _ => {}
    }
}

/// `--worktrees --cli`: print the worktree table and exit. Nothing is removed;
/// `--dry-run` also prints the commands the TUI would run for the clean stale
/// worktrees (and, with `--force`, the dirty stale ones).
fn run_worktree_cli_mode(
    branches: &[git::BranchInfo],
    repo_path: &str,
    trunk: &str,
    force: bool,
    dry_run: bool,
    pr_provider: Option<pr::PrProviderKind>,
) -> Result<()> {
    use worktree_view::{Changes, WorktreeView};

    let mut view = WorktreeView::load(branches, std::path::Path::new(repo_path))?;
    view.wait_for_status();

    println!("🌳 Trunk branch: {}", trunk);
    if force {
        println!(
            "⚠️  FORCE MODE: Dirty worktrees count as removable (git worktree remove --force)"
        );
    }
    if dry_run {
        println!("🔍 DRY RUN: Preview mode - no worktrees will be removed");
    }
    if let Some(kind) = pr_provider {
        println!("🔗 {} PR integration enabled", kind.label());
    }
    println!();

    print_header_w(WT_CLI_WIDTH);
    let dirty = view.worktrees.iter().filter(|w| w.is_dirty()).count();
    let stale = view.worktrees.iter().filter(|w| w.is_stale()).count();
    print_boxed_line_w(
        &format!(
            "🌳 Worktrees: {} ({} dirty, {} stale)",
            view.worktrees.len(),
            dirty,
            stale
        ),
        WT_CLI_WIDTH,
    );
    println!();
    print_boxed_line_w(
        "   Legend: ◆ main  ◉ current  ✗ prunable  🔒 locked  ✎ dirty  ○ detached  (else the branch status)",
        WT_CLI_WIDTH,
    );
    print_boxed_line_w("   Changes: M modified  S staged  N new", WT_CLI_WIDTH);
    print_separator_w(WT_CLI_WIDTH);
    print_boxed_line_w(
        &format!(
            "       {:<20} {:<26} {:<12} {:<9} {}",
            "Name", "Branch", "State", "Changes", "Last commit"
        ),
        WT_CLI_WIDTH,
    );

    for wt in &view.worktrees {
        let marker = if wt.entry.is_main {
            " ◆ "
        } else if wt.is_locked() {
            " 🔒"
        } else if wt.is_selectable(force) {
            "[ ]"
        } else {
            "   "
        };
        let state = wt.state();
        let changes = match &wt.changes {
            Changes::Loaded(c) => c.compact(),
            Changes::Loading => "…".to_string(),
            Changes::Failed(_) => "error".to_string(),
            Changes::NotApplicable => "-".to_string(),
        };
        let last_commit = if wt.is_prunable() || wt.commit.relative.is_empty() {
            "-".to_string()
        } else {
            wt.commit.relative.clone()
        };
        let pr = match (&pr_provider, &wt.pr_info) {
            (Some(_), Some(pr)) => format!("  {} PR #{}", pr.state.icon(), pr.number),
            _ => String::new(),
        };
        print_boxed_line_w(
            &format!(
                "   {} {:<20} {:<26} {:<12} {:<9} {}{}",
                marker,
                truncate(&wt.name(), 20),
                truncate(&wt.branch_label(), 26),
                format!("{} {}", state.icon(), state.label()),
                changes,
                last_commit,
                pr
            ),
            WT_CLI_WIDTH,
        );
    }

    let candidates: Vec<_> = view
        .worktrees
        .iter()
        .filter(|w| w.is_stale())
        .filter_map(|w| w.removal_action(force).map(|a| (w, a)))
        .collect();
    print_separator_w(WT_CLI_WIDTH);
    if candidates.is_empty() {
        print_boxed_line_w("ℹ️  No stale worktrees to remove.", WT_CLI_WIDTH);
    } else if dry_run {
        print_boxed_line_w(
            &format!("🔍 Would remove {} stale worktree(s):", candidates.len()),
            WT_CLI_WIDTH,
        );
        for (wt, action) in &candidates {
            print_boxed_line_w(
                &format!("   [DRY RUN] {}", action.command_line(&wt.entry.path)),
                WT_CLI_WIDTH,
            );
        }
    } else {
        print_boxed_line_w(
            &format!(
                "🧹 {} stale worktree(s) can be removed - run without --cli (--worktrees) to remove them",
                candidates.len()
            ),
            WT_CLI_WIDTH,
        );
    }
    print_footer_w(WT_CLI_WIDTH);
    Ok(())
}

/// Shorten `text` to `max` characters, ending with `…` when cut
fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_string()
    } else {
        let mut cut: String = text.chars().take(max.saturating_sub(1)).collect();
        cut.push('…');
        cut
    }
}

/// Box width of the worktree CLI table (wider than the branch output)
const WT_CLI_WIDTH: usize = 110;

/// Run the CLI mode (non-interactive)
fn run_cli_mode(
    branches: &[git::BranchInfo],
    trunk: &str,
    force: bool,
    dry_run: bool,
    pr_session: Option<&PrSession>,
) -> Result<()> {
    let pr_provider = pr_session.map(|session| session.kind);
    let pr_cache = pr_session.and_then(|session| session.cache.as_ref());
    println!("🌳 Trunk branch: {}", trunk);

    // Show mode indicators
    if force {
        println!("⚠️  FORCE MODE: Will force-delete unmerged branches");
    }
    if dry_run {
        println!("🔍 DRY RUN: Preview mode - no branches will be deleted");
    }
    if let Some(kind) = pr_provider {
        println!("🔗 {} PR integration enabled", kind.label());
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
    let legend = if pr_provider.is_some() {
        "   Legend: ✓ merged  ↑ pr-merged  ↕ pr-diverged  ↗ gone  ! unmerged  ○ local  ⊘ protected  ⑂ protected-wt  ◉ current  │  PR: 🟢 merged  🟡 open  🔴 closed"
    } else {
        "   Legend: ✓ merged  ↗ gone  ! unmerged  ○ local  ⊘ protected  ⑂ protected-wt  ◉ current"
    };
    print_boxed_line(legend);
    print_separator();

    for branch in branches {
        let status_indicator = format!("{} {}", branch.status.icon(), branch.status.label());
        let pr_indicator = if pr_provider.is_some() {
            match &branch.pr_info {
                Some(pr) => format!(" {} PR #{}", pr.state.icon(), pr.number),
                None => "".to_string(),
            }
        } else {
            "".to_string()
        };
        let worktree_indicator = match &branch.worktree {
            Some(path) => format!(
                " (worktree: {})",
                path.file_name()
                    .map(|n| n.to_string_lossy())
                    .unwrap_or_else(|| path.to_string_lossy())
            ),
            None => String::new(),
        };
        let line = format!(
            "   {} {:30} [{:>12}] {}{}{}",
            if branch.status.is_deletable() {
                "[ ]"
            } else {
                "   "
            },
            branch.name,
            branch.last_commit_relative,
            status_indicator,
            pr_indicator,
            worktree_indicator
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
        .filter(|b| b.status.requires_force())
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

        // For unmerged/pr-diverged branches, only delete if --force is set
        if branch.status.requires_force() && !force {
            print_boxed_line(&format!(
                "   ⏭️  Skipped: {} ({} - use --force)",
                branch.name,
                branch.status.label()
            ));
            skipped_count += 1;
            continue;
        }

        // Use safe delete for merged/gone branches, force for unmerged/pr-diverged
        let use_force = branch.status.requires_force();

        match git::delete_branch_with_mode(&branch.name, use_force) {
            Ok(_) => {
                let method = if use_force { "-D" } else { "-d" };
                print_boxed_line(&format!("   ✓ Deleted: {} ({})", branch.name, method));
                deleted_count += 1;
                // Best effort: a stale entry only costs a re-fetch later.
                if let Some(cache) = pr_cache {
                    cache.invalidate(&branch.name).ok();
                }
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
    print_header_w(80);
}

fn print_header_w(width: usize) {
    println!("┌{}┐", "─".repeat(width - 2));
    print_centered("🧹 Local Git Branch Cleanup", width);
    println!("├{}┤", "─".repeat(width - 2));
}

fn print_footer() {
    print_footer_w(80);
}

fn print_footer_w(width: usize) {
    println!("└{}┘", "─".repeat(width - 2));
}

fn print_separator() {
    print_separator_w(80);
}

fn print_separator_w(width: usize) {
    println!("├{}┤", "─".repeat(width - 2));
}

fn print_centered(text: &str, width: usize) {
    // Strip ANSI codes for length calculation
    let text_len = text.chars().count();
    let padding = (width - 2 - text_len) / 2;
    println!(
        "│{}{:^width$}│",
        " ".repeat(padding),
        text,
        width = width - 2 - padding
    );
}

fn print_boxed_line(text: &str) {
    print_boxed_line_w(text, 80);
}

fn print_boxed_line_w(text: &str, width: usize) {
    println!("│ {:<width$} │", text, width = width - 4);
}
