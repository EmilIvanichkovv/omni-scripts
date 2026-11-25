//! worktrack: unfinished work across repos, worktrees, PRs, Claude Code sessions and todos.
#![allow(dead_code)] // removed once every module is wired up

mod app;
mod bitbucket;
mod cli;
mod config;
mod discovery;
mod lgbc;
mod localgit;
mod logging;
mod model;
mod refresh;
mod sanitize;
mod sessions;
mod store;
mod summary;
mod todos;
mod ui;
mod workitems;

fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;
    println!("worktrack {}", env!("CARGO_PKG_VERSION"));
    Ok(())
}
