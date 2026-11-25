//! Command-line interface (spec 11.1).

use clap::{Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(
    name = "worktrack",
    version,
    about = "Unfinished work across repos, worktrees, PRs, Claude Code sessions and todos"
)]
pub struct Cli {
    /// Tab to open
    #[arg(long, value_enum)]
    pub tab: Option<TabArg>,
    /// Config file (default: $XDG_CONFIG_HOME/worktrack/config.toml)
    #[arg(long, global = true)]
    pub config: Option<PathBuf>,
    /// Disable fetch, Bitbucket and the summarizer
    #[arg(long, global = true)]
    pub offline: bool,
    /// Do not run the periodic full sync
    #[arg(long)]
    pub no_auto_sync: bool,
    /// Do not check for updates
    #[arg(long)]
    pub no_update_check: bool,
    #[command(subcommand)]
    pub command: Option<Cmd>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum TabArg {
    Work,
    Todos,
    Sessions,
    Summary,
    Inbox,
}

#[derive(Debug, Subcommand)]
pub enum Cmd {
    /// Print the daily summary as Markdown and exit
    Summary {
        /// Window start (default: previous workday)
        #[arg(long, value_parser = parse_date)]
        since: Option<chrono::NaiveDate>,
        /// Write to FILE instead of stdout
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Print WorkItems as JSON
    Status {
        #[arg(long)]
        json: bool,
    },
    /// Todo list commands
    Todo {
        #[command(subcommand)]
        command: TodoCmd,
    },
    /// Claude Code hook integration
    Hook {
        #[command(subcommand)]
        command: HookCmd,
    },
    /// Print diagnostics; exit 1 on problems
    Doctor,
}

#[derive(Debug, Subcommand)]
pub enum TodoCmd {
    /// Add a todo
    Add {
        text: String,
        /// Link to a branch: PROJECT/repo:branch
        #[arg(long)]
        link: Option<String>,
        #[arg(long, value_parser = ["A", "B", "C"])]
        priority: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
pub enum HookCmd {
    /// Called by Claude Code's SessionEnd hook; reads JSON from stdin
    SessionEnd,
    /// Print (or with --apply, install) the Claude Code settings snippet
    Install {
        #[arg(long)]
        apply: bool,
    },
}

fn parse_date(s: &str) -> Result<chrono::NaiveDate, String> {
    chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d")
        .map_err(|e| format!("expected YYYY-MM-DD: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_is_consistent() {
        Cli::command().debug_assert();
        let c = Cli::parse_from(["worktrack", "summary", "--since", "2026-11-24"]);
        assert!(matches!(
            c.command,
            Some(Cmd::Summary { since: Some(_), .. })
        ));
        assert!(Cli::try_parse_from(["worktrack", "summary", "--since", "24.11"]).is_err());
    }
}
