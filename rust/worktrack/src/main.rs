//! worktrack: unfinished work across repos, worktrees, PRs, Claude Code sessions and todos.
//!
//! Mode selection, terminal lifecycle and the UI event loop (spec 10.3, 11.1). All IO other
//! than drawing and running the editor happens in `refresh::Background`.

mod app;
mod bitbucket;
mod cli;
mod config;
mod discovery;
// Copied files stay close to their LGBC source (spec 2.2) so syncs are mechanical diffs; some
// of their helpers are unused here.
#[allow(dead_code)]
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
mod zellij;

use std::io::{self, Stdout, Write as _};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use chrono::Local;
use clap::Parser;
use color_eyre::eyre::{eyre, Result, WrapErr};
use crossterm::event::{
    self, DisableFocusChange, DisableMouseCapture, EnableFocusChange, EnableMouseCapture, Event,
    KeyCode, KeyEventKind, KeyModifiers,
};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::{cursor, execute};
use ratatui::backend::CrosstermBackend;
use ratatui::text::Line;
use ratatui::widgets::{Block, Paragraph, Wrap};
use ratatui::Terminal;

use app::{App, EditorTarget, Effect, Tab};
use cli::{Cli, Cmd, HookCmd, TabArg, TodoCmd};
use config::Config;
use refresh::{AppEvent, Background, Command, Options, Paths};
use todos::{Priority, Todo, TodoOp, TodoPatch};

/// UI tick (spec 10.3).
const TICK: Duration = Duration::from_millis(100);

fn main() -> Result<()> {
    let cli = Cli::parse();
    // The hook must finish in < 200 ms: nothing else (config, logging, runtime) runs first.
    if let Some(Cmd::Hook {
        command: HookCmd::SessionEnd,
    }) = &cli.command
    {
        std::process::exit(sessions::hook::run_session_end(io::stdin().lock()));
    }

    color_eyre::install()?;
    install_panic_hook();
    logging::init();
    lgbc::tls::install_crypto_provider();

    let config_path = cli
        .config
        .clone()
        .unwrap_or_else(config::default_config_path);
    match &cli.command {
        None => run_tui(&cli, config_path),
        Some(cmd) => run_subcommand(cmd, &cli, config_path),
    }
}

// ---------------------------------------------------------------------------------------------
// subcommands

/// Config for a subcommand: a missing file prints a hint and uses the defaults.
fn load_config_or_default(path: &Path) -> Result<(Config, bool)> {
    if path.exists() {
        return Ok((Config::load(path)?, false));
    }
    eprintln!(
        "worktrack: no config at {}; using defaults (run `worktrack` to create one)",
        config::tildify(path)
    );
    Ok((Config::default(), true))
}

fn run_subcommand(cmd: &Cmd, cli: &Cli, config_path: PathBuf) -> Result<()> {
    match cmd {
        Cmd::Hook {
            command: HookCmd::SessionEnd,
        } => unreachable!("dispatched first"),
        Cmd::Hook {
            command: HookCmd::Install { apply },
        } => {
            let mut out = io::stdout().lock();
            sessions::hook::install(*apply, &sessions::hook::default_settings_path(), &mut out)?;
            Ok(())
        }
        Cmd::Todo {
            command:
                TodoCmd::Add {
                    text,
                    link,
                    priority,
                },
        } => {
            let (cfg, _) = load_config_or_default(&config_path)?;
            todo_add(&cfg, text, link.as_deref(), priority.as_deref())
        }
        Cmd::Summary { since, out } => {
            let (cfg, _) = load_config_or_default(&config_path)?;
            let one = refresh::Oneshot::open(cfg, Paths::defaults(config_path), false)?;
            let (_, md) = one.summary_markdown(*since)?;
            match out {
                Some(path) => {
                    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
                        std::fs::create_dir_all(dir)?;
                    }
                    std::fs::write(path, &md)
                        .wrap_err_with(|| format!("cannot write {}", path.display()))?;
                }
                None => print!("{md}"),
            }
            Ok(())
        }
        Cmd::Status { json } => {
            let (cfg, _) = load_config_or_default(&config_path)?;
            let jira = regex::Regex::new(&cfg.jira.key_pattern).ok();
            let browse = cfg.jira.browse_url.clone();
            let one = refresh::Oneshot::open(cfg, Paths::defaults(config_path), false)?;
            let mut stdout = io::stdout().lock();
            if *json {
                let items: Vec<serde_json::Value> = one
                    .items()
                    .iter()
                    .map(|i| status_json(i, jira.as_ref(), &browse))
                    .collect();
                serde_json::to_writer_pretty(&mut stdout, &items)?;
                writeln!(stdout)?;
            } else {
                for i in one.items() {
                    let pr = i.pr.as_ref().map(|p| format!("PR #{}", p.id));
                    writeln!(
                        stdout,
                        "{:<14} {:<24} {} {}",
                        i.state.label(),
                        i.repo_name,
                        i.branch,
                        pr.unwrap_or_default()
                    )?;
                }
            }
            Ok(())
        }
        Cmd::Doctor => doctor(cli, config_path),
    }
}

fn todo_add(cfg: &Config, text: &str, link: Option<&str>, priority: Option<&str>) -> Result<()> {
    let title = sanitize::clean_line(text).trim().to_string();
    if title.is_empty() {
        return Err(eyre!("the todo text is empty"));
    }
    if let Some(l) = link {
        let ok = regex::Regex::new(r"^[^/:\s]+/[^/:\s]+:\S+$")
            .expect("static regex")
            .is_match(l);
        if !ok {
            return Err(eyre!(
                "--link must look like PROJECT/repo:branch, got `{l}`"
            ));
        }
    }
    let today = Local::now().date_naive();
    let mut todo = Todo::new(title.clone());
    todo.id = Some(todos::new_todo_id());
    todo.created = Some(today);
    todo.link = link.map(str::to_string);
    todo.source = Some("manual".into());
    todo.priority = match priority {
        Some("A") => Some(Priority::A),
        Some("B") => Some(Priority::B),
        Some("C") => Some(Priority::C),
        _ => None,
    };
    let id = todo.id.clone().unwrap_or_default();
    let mut file = todos::store::TodoFile::new(cfg.todo_file(), cfg.todos.archive_after_days);
    let outcome = file
        .apply(TodoOp::Add(todo), today)
        .map_err(|e| eyre!("{e}"))?;
    if let Some(msg) = outcome.dropped {
        return Err(eyre!("{msg}"));
    }
    println!("added {id}: {title} ({})", config::tildify(file.path()));
    Ok(())
}

/// One WorkItem for `status --json`.
fn status_json(
    i: &model::WorkItem,
    jira: Option<&regex::Regex>,
    browse_url: &str,
) -> serde_json::Value {
    let jira_key = jira
        .and_then(|re| re.find(&i.branch))
        .map(|m| m.as_str().to_string());
    let jira_url = jira_key
        .as_ref()
        .filter(|_| !browse_url.is_empty())
        .map(|k| browse_url.replace("{key}", k));
    let pr = i.pr.as_ref().map(|p| {
        serde_json::json!({
            "id": p.id,
            "state": p.state.as_str(),
            "title": p.title,
            "url": p.url,
            "author": p.author_slug,
            "open_tasks": p.open_tasks,
            "comments": p.comment_count,
            "reviewers": p.reviewers.iter().map(|r| serde_json::json!({
                "name": r.name,
                "slug": r.slug,
                "status": r.status,
            })).collect::<Vec<_>>(),
        })
    });
    serde_json::json!({
        "repo": i.repo_name,
        "repo_id": i.repo_id,
        "branch": i.branch,
        "state": i.state.as_str(),
        "label": i.state.label(),
        "unfinished": i.state.is_unfinished(),
        "flags": i.flags,
        "pr": pr,
        "links": {
            "todo_ref": i.link_ref(),
            "pr": i.pr.as_ref().map(|p| p.url.clone()),
            "jira_key": jira_key,
            "jira": jira_url,
            "worktree": i.local.worktree,
        },
        "abandoned_manual": i.abandoned_manual,
        "linked_todos": i.linked_todos,
        "sessions": i.session_count,
        "last_activity": i.last_activity,
        "state_since": i.state_since,
    })
}

/// `doctor`: diagnostics as text (local only, no network); exit 1 on problems.
fn doctor(_cli: &Cli, config_path: PathBuf) -> Result<()> {
    let mut problems = Vec::new();
    let cfg = if config_path.exists() {
        match Config::load(&config_path) {
            Ok(c) => c,
            Err(e) => {
                println!("Config:\n  {e}");
                println!("\nProblems:\n  config file is invalid");
                std::process::exit(1);
            }
        }
    } else {
        problems.push(format!(
            "no config at {} (run `worktrack` to create one); using defaults",
            config::tildify(&config_path)
        ));
        Config::default()
    };
    let one = refresh::Oneshot::open(cfg, Paths::defaults(config_path), true)?;
    for line in one.diagnostics() {
        println!("{line}");
    }
    problems.extend(one.problems());
    if problems.is_empty() {
        println!("\nNo problems found.");
        Ok(())
    } else {
        println!("\nProblems:");
        for p in &problems {
            println!("  {p}");
        }
        std::process::exit(1);
    }
}

// ---------------------------------------------------------------------------------------------
// terminal

type Tui = Terminal<CrosstermBackend<Stdout>>;

fn enter_terminal() -> Result<()> {
    enable_raw_mode()?;
    execute!(
        io::stdout(),
        EnterAlternateScreen,
        EnableMouseCapture,
        EnableFocusChange
    )?;
    Ok(())
}

fn restore_terminal() {
    let _ = disable_raw_mode();
    let _ = execute!(
        io::stdout(),
        DisableFocusChange,
        DisableMouseCapture,
        LeaveAlternateScreen,
        cursor::Show
    );
}

/// Restores the terminal when dropped (normal exit and `?` errors).
struct TerminalGuard {
    term: Tui,
}

impl TerminalGuard {
    fn enter() -> Result<Self> {
        enter_terminal()?;
        let term = Terminal::new(CrosstermBackend::new(io::stdout()))?;
        Ok(Self { term })
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        restore_terminal();
    }
}

/// Restore the terminal before color-eyre prints the panic report.
fn install_panic_hook() {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore_terminal();
        prev(info);
    }));
}

// ---------------------------------------------------------------------------------------------
// TUI

fn tab_from(arg: TabArg) -> Tab {
    match arg {
        TabArg::Work => Tab::Work,
        TabArg::Todos => Tab::Todos,
        TabArg::Sessions => Tab::Sessions,
        TabArg::Summary => Tab::Summary,
        TabArg::Inbox => Tab::Inbox,
    }
}

fn run_tui(cli: &Cli, config_path: PathBuf) -> Result<()> {
    // Load before entering the alternate screen so config errors print normally.
    let existing = if config_path.exists() {
        Some(Config::load(&config_path)?)
    } else {
        None
    };
    let mut guard = TerminalGuard::enter()?;
    let cfg = match existing {
        Some(c) => c,
        None => {
            if !first_run_screen(&mut guard.term, &config_path)? {
                return Ok(());
            }
            Config::parse(config::DEFAULT_CONFIG_TOML)?
        }
    };

    let paths = Paths::defaults(config_path);
    let today = Local::now().date_naive();
    // Records today's launch even with --tab, so a later launch today does not auto-open.
    let auto_open = store::open(&paths.db)
        .and_then(|c| summary::should_auto_open(&c, today, &cfg.workdays()))
        .unwrap_or(false);
    let tab = match cli.tab {
        Some(t) => tab_from(t),
        None if auto_open => Tab::Summary,
        None => Tab::Work,
    };

    let (ev_tx, ev_rx) = mpsc::channel();
    let bg = Background::start(
        cfg.clone(),
        paths,
        Options {
            offline: cli.offline,
            auto_sync: !cli.no_auto_sync,
            interactive: true,
        },
        ev_tx.clone(),
    )?;
    let mut app = App::new(&cfg, tab);
    app.no_color = std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty());
    app.home = dirs::home_dir();

    let result = event_loop(&mut guard.term, &mut app, &bg, &ev_rx, &ev_tx, &cfg);
    bg.shutdown(Duration::from_secs(2));
    drop(guard);
    result
}

fn event_loop(
    term: &mut Tui,
    app: &mut App,
    bg: &Background,
    ev_rx: &mpsc::Receiver<AppEvent>,
    ev_tx: &mpsc::Sender<AppEvent>,
    cfg: &Config,
) -> Result<()> {
    let mut next_tick = Instant::now();
    loop {
        if Instant::now() >= next_tick {
            app.tick(Local::now().fixed_offset());
            while let Ok(ev) = ev_rx.try_recv() {
                app.handle_event(ev);
            }
            next_tick = Instant::now() + TICK;
        }
        term.draw(|f| ui::draw(f, app))?;

        let timeout = next_tick.saturating_duration_since(Instant::now());
        if !event::poll(timeout)? {
            continue;
        }
        let effects = match event::read()? {
            // Ctrl-L: full repaint. ratatui only sends changed cells, so if anything outside the
            // TUI wrote to the terminal, the stray text stays until the screen is cleared.
            Event::Key(k)
                if k.code == KeyCode::Char('l') && k.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                term.clear()?;
                Vec::new()
            }
            Event::Key(k) => app.handle_key(k),
            Event::Mouse(m) => app.handle_mouse(m),
            Event::Resize(..) => {
                term.clear()?;
                Vec::new()
            }
            Event::FocusGained => {
                // Other programs may have drawn while the window was in the background.
                term.clear()?;
                bg.send(Command::LocalReload);
                Vec::new()
            }
            _ => Vec::new(),
        };
        for effect in effects {
            match effect {
                Effect::Quit => return Ok(()),
                Effect::Command(cmd) => bg.send(cmd),
                Effect::CopyToClipboard(text) => {
                    let tx = ev_tx.clone();
                    std::thread::spawn(move || {
                        if let Err(e) = lgbc::clipboard::copy_to_clipboard(&text) {
                            let _ = tx.send(AppEvent::Error {
                                scope: "clipboard".into(),
                                message: e,
                            });
                        }
                    });
                }
                Effect::OpenEditor { kind } => {
                    if let Err(e) = open_editor(term, app, bg, cfg, kind) {
                        app.set_error(format!("editor: {e}"));
                    }
                }
            }
        }
    }
}

/// `ui.editor`, else `$VISUAL`, `$EDITOR`, `vi`, split like a shell would.
fn editor_command(cfg: &Config) -> Result<Vec<String>> {
    let raw = [
        Some(cfg.ui.editor.clone()),
        std::env::var("VISUAL").ok(),
        std::env::var("EDITOR").ok(),
    ]
    .into_iter()
    .flatten()
    .find(|s| !s.trim().is_empty())
    .unwrap_or_else(|| "vi".into());
    let words = shell_words::split(&raw).map_err(|e| eyre!("cannot parse editor `{raw}`: {e}"))?;
    if words.is_empty() {
        return Err(eyre!("empty editor command"));
    }
    Ok(words)
}

/// Suspend the TUI, run the editor on `path`, resume.
fn run_editor(term: &mut Tui, cfg: &Config, path: &Path) -> Result<()> {
    let words = editor_command(cfg)?;
    restore_terminal();
    let status = std::process::Command::new(&words[0])
        .args(&words[1..])
        .arg(path)
        .status();
    enter_terminal()?;
    term.clear()?;
    let status = status.map_err(|e| eyre!("cannot run {}: {e}", words[0]))?;
    if !status.success() {
        return Err(eyre!("{} exited with {status}", words[0]));
    }
    Ok(())
}

fn open_editor(
    term: &mut Tui,
    app: &mut App,
    bg: &Background,
    cfg: &Config,
    kind: EditorTarget,
) -> Result<()> {
    match kind {
        // The watcher reloads the file after the editor saves it.
        EditorTarget::WholeFile => run_editor(term, cfg, &cfg.todo_file()),
        EditorTarget::NotesOf(id) => {
            let notes = app
                .todos
                .iter()
                .find(|t| t.id.as_deref() == Some(id.as_str()))
                .map(|t| t.notes.clone())
                .ok_or_else(|| eyre!("todo {id} not found"))?;
            let tmp = std::env::temp_dir()
                .join(format!("worktrack-notes-{}-{id}.md", std::process::id()));
            std::fs::write(&tmp, &notes)?;
            let ran = run_editor(term, cfg, &tmp);
            let edited = std::fs::read_to_string(&tmp);
            let _ = std::fs::remove_file(&tmp);
            ran?;
            let edited = edited?.trim_end().to_string();
            if edited != notes.trim_end() {
                bg.send(Command::Todo(TodoOp::Update {
                    id,
                    patch: TodoPatch {
                        notes: Some(edited),
                        ..TodoPatch::default()
                    },
                }));
            }
            Ok(())
        }
    }
}

/// Missing config: explain and offer to write the commented default. Returns false to quit.
fn first_run_screen(term: &mut Tui, path: &Path) -> Result<bool> {
    let shown = config::tildify(path);
    loop {
        term.draw(|f| {
            let text = vec![
                Line::from("Welcome to worktrack."),
                Line::from(""),
                Line::from(format!("There is no config file at {shown}.")),
                Line::from(""),
                Line::from(
                    "worktrack scans discovery.roots (default ~/work and ~/src) for git \
                     repositories with a Bitbucket Data Center origin, and tracks your \
                     unfinished branches, PRs, Claude Code sessions and todos.",
                ),
                Line::from(""),
                Line::from("Set BITBUCKET_TOKEN in the environment for PR data."),
                Line::from(""),
                Line::from(format!(
                    "y  write the commented default config to {shown} and start"
                )),
                Line::from("q  quit"),
            ];
            let p = Paragraph::new(text)
                .wrap(Wrap { trim: false })
                .block(Block::bordered().title(" worktrack: first run "));
            f.render_widget(p, f.area());
        })?;
        let Event::Key(k) = event::read()? else {
            continue;
        };
        if k.kind == KeyEventKind::Release {
            continue;
        }
        match k.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                if let Some(dir) = path.parent() {
                    std::fs::create_dir_all(dir)?;
                }
                std::fs::write(path, config::DEFAULT_CONFIG_TOML)
                    .wrap_err_with(|| format!("cannot write {}", path.display()))?;
                return Ok(true);
            }
            KeyCode::Char('c') if k.modifiers.contains(KeyModifiers::CONTROL) => return Ok(false),
            KeyCode::Char('q') | KeyCode::Esc => return Ok(false),
            _ => {}
        }
    }
}
