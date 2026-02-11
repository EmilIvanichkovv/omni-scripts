mod app;
mod git;
mod ui;

use clap::Parser;
use color_eyre::Result;
use std::io::{self, Write};

#[derive(Parser, Debug)]
#[command(name = "local-git-branch-cleanup-tui")]
#[command(about = "Interactive TUI for cleaning up local Git branches", long_about = None)]
struct Args {
    /// Override the default trunk branch
    #[arg(long)]
    trunk: Option<String>,
}

fn main() -> Result<()> {
    // Initialize error handling
    color_eyre::install()?;

    let _args = Args::parse();

    // Verify we're in a git repository
    match git::verify_repo() {
        Ok(repo_path) => {
            println!("📂 Repository: {}", repo_path);
        }
        Err(e) => {
            eprintln!("❌ Error: {}", e);
            std::process::exit(1);
        }
    }

    // Print header
    print_header();

    // Get branches without remote counterparts
    print_boxed_line("🔍 Scanning local git branches...");
    println!();

    let branches = match git::get_branches_without_remote() {
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

    // Display branches
    print_boxed_line(&format!(
        "📋 Local branches without a remote counterpart: ({})",
        branches.len()
    ));
    println!();

    for branch in &branches {
        print_boxed_line(&format!(
            "   • {} [Last update: {}]",
            branch.name, branch.last_commit_relative
        ));
    }

    // Print confirmation prompt
    print_separator();
    print_boxed_line("⚠️  These branches are not present on the remote.");
    print_boxed_line("🗑️  Do you want to delete them locally?");
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
    print_boxed_line("🗑️  Deleting selected branches...");
    print_separator();

    let mut deleted_branches = Vec::new();
    let mut failed_branches = Vec::new();

    for branch in &branches {
        match git::delete_branch(&branch.name) {
            Ok(_) => {
                print_boxed_line(&format!("   ✓ Deleted: {}", branch.name));
                deleted_branches.push(&branch.name);
            }
            Err(e) => {
                print_boxed_line(&format!("   ✗ Failed: {} ({})", branch.name, e));
                failed_branches.push(&branch.name);
            }
        }
    }

    // Print summary
    print_separator();
    if failed_branches.is_empty() {
        print_boxed_line(&format!(
            "✅ Cleanup complete! Deleted {} branches.",
            deleted_branches.len()
        ));
    } else {
        print_boxed_line(&format!(
            "⚠️  Cleanup finished with errors. Deleted: {}, Failed: {}",
            deleted_branches.len(),
            failed_branches.len()
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
