mod app;
mod git;
mod ui;

use clap::Parser;
use color_eyre::Result;
use git::BranchStatus;
use std::io::{self, Write};

#[derive(Parser, Debug)]
#[command(name = "local-git-branch-cleanup-tui")]
#[command(about = "Interactive TUI for cleaning up local Git branches", long_about = None)]
struct Args {
    /// Override the default trunk branch
    #[arg(long)]
    trunk: Option<String>,

    /// Force delete unmerged branches (use with caution!)
    #[arg(long, short = 'f')]
    force: bool,
}

fn main() -> Result<()> {
    // Initialize error handling
    color_eyre::install()?;

    let args = Args::parse();

    // Verify we're in a git repository
    match git::verify_repo() {
        Ok(path) => {
            println!("📂 Repository: {}", path);
        }
        Err(e) => {
            eprintln!("❌ Error: {}", e);
            std::process::exit(1);
        }
    };

    // Get the trunk branch
    let trunk = git::get_default_branch(args.trunk.as_deref())?;
    println!("🌳 Trunk branch: {}", trunk);
    println!();

    // Print header
    print_header();

    // Get branches with classification
    print_boxed_line("🔍 Scanning local git branches...");
    println!();

    let branches = match git::get_branches_with_classification(args.trunk.as_deref()) {
        Ok(branches) => branches,
        Err(e) => {
            eprintln!("❌ Error scanning branches: {}", e);
            std::process::exit(1);
        }
    };

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
    print_boxed_line("   Legend: ● merged  ◆ gone  ▲ unmerged  ⛔ protected  ★ current");
    print_separator();

    for branch in &branches {
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

    if unmerged_count > 0 && !args.force {
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

    for branch in &branches {
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
        if branch.status == BranchStatus::Unmerged && !args.force {
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
