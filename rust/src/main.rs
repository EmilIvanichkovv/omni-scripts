mod app;
mod git;
mod ui;

use clap::Parser;
use color_eyre::Result;

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

    println!("🧹 Local Git Branch Cleanup TUI");
    println!("Environment setup complete!");
    println!();
    println!("M1: Development Environment ✅");
    println!("- Rust toolchain configured via Nix");
    println!("- Dependencies added: ratatui, crossterm, color-eyre, clap, chrono");
    println!("- Module stubs created: app.rs, git.rs, ui.rs");
    println!();
    println!("Next: M2 - Core Git Integration");

    Ok(())
}
