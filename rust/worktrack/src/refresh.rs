//! Scheduler, background tasks, channels to UI (spec 10.2, 10.3).
//!
//! CONTRACT: `AppEvent` flows from background tasks to the UI thread; `Command` flows from the
//! UI thread to the background. `ui/` and `app.rs` never perform IO themselves.
//!
//! # Shape
//!
//! [`Background::start`] spawns one thread that owns a multi-thread tokio runtime and runs the
//! [`Engine`] loop on it. The engine owns the DB writer, the todo file and its watcher, the
//! Bitbucket service, the summarizer pool and the current state (repos, worktrees, local git
//! state, PRs, CI, sessions, todos, WorkItems). It reacts to:
//!
//! - `Command`s from the UI (tokio unbounded channel), handled one at a time;
//! - the local reload timer (`refresh.local_interval_secs`), and the full sync timer
//!   (`refresh.sync_interval_mins`, unless `--no-auto-sync`);
//! - todo file watcher notifications;
//! - internal messages: a finished full sync, a finished session summary.
//!
//! A full sync (discovery, `git fetch`, Bitbucket) runs as its own task so it never delays
//! commands or local reloads; its result comes back as an internal message and is followed by a
//! local reload. Events go to the UI through a `std::sync::mpsc::Sender<AppEvent>` that the UI
//! drains every tick.
//!
//! # Startup
//!
//! Before any git call the engine emits the cached repos and worktrees (`load_discovery`), the
//! cached PR data, sessions and todos, WorkItems built from the `work_items` rows, the summary
//! and diagnostics. Then: a local reload, then a full sync.
//!
//! # CLI
//!
//! [`Oneshot`] runs the same engine without timers, network, rules or summarizer for
//! `worktrack summary`, `status --json` and `doctor`.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use chrono::{Local, NaiveDate};
use color_eyre::eyre::{eyre, Result};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};

use crate::bitbucket::{self, BitbucketService, SyncBranch, SyncInput, SyncRepo, SyncReport};
use crate::config::{self, Config};
use crate::discovery::{self, DiscoveryResult};
use crate::localgit;
use crate::model::{
    CiState, LocalBranchState, PrAvailability, PrData, Repo, RepoId, WorkItem, Worktree,
};
use crate::sessions::summarize::{OnDone, SummarizerConfig, SummaryPool};
use crate::sessions::{ingest, Session, SummaryStatus};
use crate::store::{self, DbWriter};
use crate::summary::{self, Window};
use crate::todos::rules::{self, Accepted, RulesContext, Suggestion};
use crate::todos::store::TodoFile;
use crate::todos::watch::TodoWatcher;
use crate::todos::{Todo, TodoOp};
use crate::workitems::{self, AssemblyInput, ItemKey, KnownItem, SessionInfo};

/// PR integration status shown in the header (spec 9.1, 5.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrStatus {
    Ok,
    NoToken,
    Offline,
    /// 401/403 etc: message and affected repos.
    Error {
        message: String,
        repos: Vec<String>,
    },
    /// Host unreachable: showing cached data of this age (secs).
    Cached {
        age_secs: i64,
    },
}

impl PrStatus {
    fn describe(&self) -> String {
        match self {
            Self::Ok => "ok".into(),
            Self::NoToken => "no token: PR data unavailable".into(),
            Self::Offline => "offline (--offline): cached PR data only".into(),
            Self::Error { message, repos } => format!("{message} ({})", repos.join(", ")),
            Self::Cached { age_secs } => {
                format!(
                    "host unreachable, showing cached data ({} min old)",
                    age_secs / 60
                )
            }
        }
    }
}

#[derive(Debug, Clone)]
pub enum AppEvent {
    ReposUpdated {
        repos: Vec<Repo>,
        worktrees: Vec<Worktree>,
    },
    WorkItemsUpdated(Vec<WorkItem>),
    PrUpdated {
        status: PrStatus,
        synced_at: Option<i64>,
    },
    SessionsUpdated(Vec<Session>),
    /// `Err` carries the parse error line; the UI keeps its last good state.
    TodosReloaded(Result<Vec<Todo>, String>),
    SuggestionsUpdated(Vec<Suggestion>),
    /// Daily summary for the current window: the structured sections and their Markdown.
    SummaryUpdated {
        since: chrono::NaiveDate,
        summary: summary::Summary,
        markdown: String,
    },
    Diagnostics(Vec<String>),
    Progress {
        task: String,
        done: usize,
        total: usize,
    },
    Status(String),
    Error {
        scope: String,
        message: String,
    },
}

/// Requests from the UI to the background.
#[derive(Debug, Clone)]
pub enum Command {
    LocalReload,
    FullSync,
    Todo(TodoOp),
    AcceptSuggestion {
        id: i64,
        edited_title: Option<String>,
    },
    RejectSuggestion {
        id: i64,
    },
    /// For KeepOrAbandon suggestions: `abandon = false` keeps.
    DecideSuggestion {
        id: i64,
        abandon: bool,
    },
    AcceptAll,
    RejectAll,
    SetAbandoned {
        repo_id: String,
        branch: String,
        abandoned: bool,
    },
    RemoveWorktree {
        repo_id: String,
        branch: String,
    },
    Resummarize {
        session_id: String,
    },
    /// Shift/recompute the summary window.
    Summary {
        since: chrono::NaiveDate,
    },
    WriteSummary {
        since: chrono::NaiveDate,
    },
    OpenUrl(String),
    /// Open the session in Zellij: a new tab running `claude --resume`, or focus its tab.
    OpenInZellij {
        session_id: String,
    },
    Quit,
}

/// Max concurrent `git fetch` runs in a full sync (spec 10.2).
const MAX_CONCURRENT_FETCH: usize = 4;
/// Meta key set after the first rules run (spec 7.5 first-run batch).
const META_FIRST_RUN_DONE: &str = "first_run_done";

/// Filesystem locations the engine uses besides the config's own paths. Tests point them at
/// temp dirs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    /// Shown in diagnostics.
    pub config: PathBuf,
    pub db: PathBuf,
    /// cwd of the summarizer; sessions there are ignored.
    pub summarizer_dir: PathBuf,
    /// `~/.claude/settings.json`, checked for the hook.
    pub claude_settings: PathBuf,
}

impl Paths {
    pub fn defaults(config: PathBuf) -> Self {
        Self {
            config,
            db: config::db_path(),
            summarizer_dir: config::summarizer_dir(),
            claude_settings: crate::sessions::hook::default_settings_path(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Options {
    /// `--offline`: no fetch, no Bitbucket, no summarizer.
    pub offline: bool,
    /// Periodic full sync (`--no-auto-sync` turns it off; the startup sync still runs).
    pub auto_sync: bool,
    /// TUI mode: rules, todo watcher, summarizer, todo file created when missing.
    pub interactive: bool,
}

// ---------------------------------------------------------------------------------------------
// Background (TUI)

/// Handle on the background thread. Dropping it without [`Background::shutdown`] leaves the
/// thread running until the process exits.
pub struct Background {
    cmd_tx: UnboundedSender<Command>,
    done_rx: std::sync::mpsc::Receiver<()>,
}

impl Background {
    /// Open the DB, build the engine and run it on a new thread with its own tokio runtime.
    pub fn start(
        cfg: Config,
        paths: Paths,
        opts: Options,
        events: std::sync::mpsc::Sender<AppEvent>,
    ) -> Result<Self> {
        let (cmd_tx, cmd_rx) = unbounded_channel();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(4)
            .thread_name("worktrack-bg")
            .enable_all()
            .build()?;
        let engine = {
            let _guard = rt.enter();
            Engine::new(cfg, paths, opts, events)?
        };
        std::thread::Builder::new()
            .name("worktrack-engine".into())
            .spawn(move || {
                rt.block_on(engine.run(cmd_rx));
                // Abandon blocking tasks still running (a slow `git fetch`).
                rt.shutdown_timeout(Duration::from_millis(500));
                let _ = done_tx.send(());
            })?;
        Ok(Self { cmd_tx, done_rx })
    }

    pub fn send(&self, cmd: Command) {
        let _ = self.cmd_tx.send(cmd);
    }

    /// Ask the engine to stop and wait up to `timeout` for it.
    pub fn shutdown(self, timeout: Duration) {
        let _ = self.cmd_tx.send(Command::Quit);
        let _ = self.done_rx.recv_timeout(timeout);
    }
}

// ---------------------------------------------------------------------------------------------
// Oneshot (CLI)

/// The engine run once, synchronously, without network, rules or summarizer: cached PR data,
/// a local git reload and a session scan. Used by `summary`, `status` and `doctor`.
pub struct Oneshot {
    rt: tokio::runtime::Runtime,
    engine: Engine,
}

impl Oneshot {
    /// `discover`: run discovery first (always for `doctor`; otherwise only when the cache has
    /// no repos yet).
    pub fn open(cfg: Config, paths: Paths, always_discover: bool) -> Result<Self> {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        // Nobody listens: sends fail silently.
        let (events, _) = std::sync::mpsc::channel();
        let opts = Options {
            offline: true,
            auto_sync: false,
            interactive: false,
        };
        let engine = rt.block_on(async move {
            let mut e = Engine::new(cfg, paths, opts, events)?;
            e.load_cache().await;
            if always_discover || e.repos.is_empty() {
                e.discover_now().await;
            }
            e.local_reload().await;
            Ok::<_, color_eyre::Report>(e)
        })?;
        Ok(Self { rt, engine })
    }

    pub fn items(&self) -> &[WorkItem] {
        &self.engine.items
    }

    /// The summary Markdown for the window starting `since` (default: previous workday).
    pub fn summary_markdown(&self, since: Option<NaiveDate>) -> Result<(Window, String)> {
        self.rt.block_on(self.engine.summary_markdown(since))
    }

    /// Diagnostics as text, skipped repos included.
    pub fn diagnostics(&self) -> Vec<String> {
        self.engine.diagnostics(true)
    }

    /// Problems that make `doctor` exit 1.
    pub fn problems(&self) -> Vec<String> {
        self.engine.problems()
    }
}

// ---------------------------------------------------------------------------------------------
// Engine

enum Internal {
    SyncDone(Box<SyncDone>),
    Summarized { session_id: String, ok: bool },
}

struct SyncDone {
    discovery: Option<DiscoveryResult>,
    /// `(repo name, error)` per repo; `None` = fetched fine.
    fetches: Vec<(String, Option<String>)>,
    /// `None` when offline.
    report: Option<SyncReport>,
    errors: Vec<(String, String)>,
}

struct Engine {
    cfg: Arc<Config>,
    paths: Paths,
    opts: Options,
    events: std::sync::mpsc::Sender<AppEvent>,
    writer: DbWriter,
    todo_file: TodoFile,
    todo_watcher: Option<TodoWatcher>,
    watch_rx: Option<UnboundedReceiver<()>>,
    bb: Arc<BitbucketService>,
    pool: Option<SummaryPool>,
    internal_tx: UnboundedSender<Internal>,
    internal_rx: Option<UnboundedReceiver<Internal>>,

    repos: Vec<Repo>,
    worktrees: Vec<Worktree>,
    discovery_diags: Vec<String>,
    local: HashMap<RepoId, Vec<LocalBranchState>>,
    reused: HashSet<ItemKey>,
    prs: HashMap<ItemKey, Option<PrData>>,
    ci: HashMap<String, Option<CiState>>,
    username: Option<String>,
    pr_status: PrStatus,
    synced_at: Option<i64>,
    items: Vec<WorkItem>,
    sessions: Vec<Session>,
    scan: Option<ingest::ScanReport>,
    todos: Vec<Todo>,
    todo_error: Option<String>,
    summary_since: Option<NaiveDate>,
    syncing: bool,
    /// Current problems by scope; shown in the status line once, kept in diagnostics until the
    /// scope succeeds again (spec 11.2).
    errors: BTreeMap<String, String>,
    /// Ports recently handed out to Zellij layout tabs (`C`).
    zellij_ports: Arc<crate::zellij::layout::PortLedger>,
}

fn today() -> NaiveDate {
    Local::now().date_naive()
}

impl Engine {
    /// Must be called inside a tokio runtime context.
    fn new(
        cfg: Config,
        paths: Paths,
        opts: Options,
        events: std::sync::mpsc::Sender<AppEvent>,
    ) -> Result<Self> {
        let conn = store::open(&paths.db)
            .map_err(|e| eyre!("cannot open database {}: {e}", paths.db.display()))?;
        let writer = DbWriter::spawn(conn, paths.db.clone());
        let pool =
            (opts.interactive && !opts.offline && cfg.sessions.enabled && cfg.sessions.summarize)
                .then(|| {
                    let mut sc = SummarizerConfig::from_config(&cfg);
                    sc.workdir = paths.summarizer_dir.clone();
                    SummaryPool::new(sc)
                });
        let (internal_tx, internal_rx) = unbounded_channel();
        let bb = Arc::new(BitbucketService::from_env());
        let pr_status = if opts.offline {
            PrStatus::Offline
        } else if !bb.has_token() {
            PrStatus::NoToken
        } else {
            PrStatus::Ok
        };
        Ok(Self {
            todo_file: TodoFile::new(cfg.todo_file(), cfg.todos.archive_after_days),
            cfg: Arc::new(cfg),
            paths,
            opts,
            events,
            writer,
            todo_watcher: None,
            watch_rx: None,
            bb,
            pool,
            internal_tx,
            internal_rx: Some(internal_rx),
            zellij_ports: Arc::default(),
            repos: Vec::new(),
            worktrees: Vec::new(),
            discovery_diags: Vec::new(),
            local: HashMap::new(),
            reused: HashSet::new(),
            prs: HashMap::new(),
            ci: HashMap::new(),
            username: None,
            pr_status,
            synced_at: None,
            items: Vec::new(),
            sessions: Vec::new(),
            scan: None,
            todos: Vec::new(),
            todo_error: None,
            summary_since: None,
            syncing: false,
            errors: BTreeMap::new(),
        })
    }

    fn emit(&self, ev: AppEvent) {
        let _ = self.events.send(ev);
    }

    fn status(&self, msg: impl Into<String>) {
        self.emit(AppEvent::Status(msg.into()));
    }

    /// Record a problem; emit it to the status line when it is new or changed.
    fn set_error(&mut self, scope: impl Into<String>, message: impl Into<String>) {
        let scope = scope.into();
        let message = crate::sanitize::clean_line(&message.into());
        tracing::warn!("{scope}: {message}");
        if self.errors.get(&scope) != Some(&message) {
            self.emit(AppEvent::Error {
                scope: scope.clone(),
                message: message.clone(),
            });
            self.errors.insert(scope, message);
        }
    }

    fn clear_error(&mut self, scope: &str) {
        self.errors.remove(scope);
    }

    fn clear_errors_with_prefix(&mut self, prefix: &str) {
        self.errors.retain(|k, _| !k.starts_with(prefix));
    }

    fn repo_name(&self, repo_id: &str) -> String {
        self.repos
            .iter()
            .find(|r| r.id == repo_id)
            .map(Repo::display_name)
            .unwrap_or_else(|| repo_id.to_string())
    }

    // -----------------------------------------------------------------------------------------
    // main loop

    async fn run(mut self, mut cmd_rx: UnboundedReceiver<Command>) {
        let mut internal_rx = self.internal_rx.take().expect("internal receiver");
        self.load_cache().await;
        self.start_todo_watcher();
        self.local_reload().await;
        self.start_full_sync();

        let local_period = Duration::from_secs(self.cfg.refresh.local_interval_secs.max(1));
        let sync_period = Duration::from_secs(self.cfg.refresh.sync_interval_mins.max(1) * 60);
        let start = tokio::time::Instant::now();
        let mut local_tick = tokio::time::interval_at(start + local_period, local_period);
        local_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut sync_tick = tokio::time::interval_at(start + sync_period, sync_period);
        sync_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut watch_rx = self.watch_rx.take();

        loop {
            tokio::select! {
                cmd = cmd_rx.recv() => match cmd {
                    None | Some(Command::Quit) => break,
                    Some(cmd) => self.handle_command(cmd).await,
                },
                Some(msg) = internal_rx.recv() => self.handle_internal(msg).await,
                _ = local_tick.tick() => self.local_reload().await,
                _ = sync_tick.tick(), if self.opts.auto_sync => self.start_full_sync(),
                Some(()) = recv_opt(&mut watch_rx) => self.todos_changed().await,
            }
        }
    }

    // -----------------------------------------------------------------------------------------
    // startup from cache

    /// Emit everything known from the DB and the todo file, before any git call.
    async fn load_cache(&mut self) {
        match self.writer.write(|c| discovery::load_discovery(c)).await {
            Ok((repos, worktrees)) => {
                self.repos = repos;
                self.worktrees = worktrees;
            }
            Err(e) => self.set_error("database", format!("cached repos: {e}")),
        }
        match self
            .writer
            .write(|c| bitbucket::load_cached_pr_data(c))
            .await
        {
            Ok(cached) => {
                self.prs = cached.prs.into_iter().map(|(k, row)| (k, row.pr)).collect();
                self.ci = cached.ci;
                self.synced_at = cached.newest_fetched_at;
            }
            Err(e) => self.set_error("database", format!("cached PR data: {e}")),
        }
        if self.cfg.sessions.enabled {
            match self.writer.write(|c| ingest::load_sessions(c)).await {
                Ok(s) => self.sessions = s,
                Err(e) => self.set_error("sessions", e.to_string()),
            }
        }
        self.load_todos();
        match self.writer.write(|c| workitems::load_known(c)).await {
            Ok(known) => self.items = self.cached_items(&known),
            Err(e) => self.set_error("database", format!("cached work items: {e}")),
        }
        self.emit(AppEvent::ReposUpdated {
            repos: self.repos.clone(),
            worktrees: self.worktrees.clone(),
        });
        self.emit(AppEvent::PrUpdated {
            status: self.pr_status.clone(),
            synced_at: self.synced_at,
        });
        self.emit(AppEvent::SessionsUpdated(self.sessions.clone()));
        self.emit_todos();
        self.emit(AppEvent::WorkItemsUpdated(self.items.clone()));
        self.emit_suggestions().await;
        self.emit_summary().await;
        self.emit_diagnostics();
    }

    /// WorkItems from the `work_items` rows and cached PRs, for the first frame. The local
    /// reload replaces them moments later.
    fn cached_items(&self, known: &HashMap<ItemKey, KnownItem>) -> Vec<WorkItem> {
        let now = store::now();
        let sessions = self.session_infos(now);
        let links = self.todo_links();
        let mut items: Vec<WorkItem> = known
            .iter()
            .filter_map(|(key, k)| {
                let repo = self
                    .repos
                    .iter()
                    .find(|r| r.id == key.0 && r.skipped_reason.is_none())?;
                let finished = !k.state.is_unfinished();
                let since = k.state_since.or(k.last_activity).unwrap_or(k.first_seen);
                if finished && now - since > workitems::PRUNE_AFTER_SECS {
                    return None;
                }
                let worktree = self
                    .worktrees
                    .iter()
                    .find(|w| w.repo_id == key.0 && w.branch.as_deref() == Some(key.1.as_str()));
                let local = LocalBranchState {
                    branch: key.1.clone(),
                    local_exists: true,
                    worktree: worktree.map(|w| w.path.clone()),
                    tip: worktree.and_then(|w| w.head.clone()),
                    ..LocalBranchState::default()
                };
                let pr = self.prs.get(key).cloned().flatten();
                let ci = local
                    .tip
                    .as_ref()
                    .and_then(|sha| self.ci.get(sha).copied().flatten());
                let session = sessions.get(key);
                let flags =
                    workitems::compute_flags(k.state, &local, pr.as_ref(), ci, session, now);
                Some(WorkItem {
                    repo_id: key.0.clone(),
                    repo_name: repo.display_name(),
                    branch: key.1.clone(),
                    state: k.state,
                    flags,
                    local,
                    pr,
                    abandoned_manual: k.abandoned_manual,
                    last_activity: k.last_activity,
                    state_since: k.state_since,
                    linked_todos: links.get(key).copied().unwrap_or(0),
                    session_count: session.map_or(0, |s| s.count),
                })
            })
            .collect();
        items.sort_by(|a, b| (&a.repo_name, &a.branch).cmp(&(&b.repo_name, &b.branch)));
        items
    }

    // -----------------------------------------------------------------------------------------
    // todos

    fn load_todos(&mut self) {
        if !self.opts.interactive && !self.todo_file.path().exists() {
            return;
        }
        match self.todo_file.load() {
            Ok(l) => {
                self.todos = l.todos;
                self.todo_error = None;
                self.normalize_todos();
            }
            Err(e) => self.todo_error = Some(e.to_string()),
        }
    }

    /// Hand-written items get ids and dates so the TUI can address them (spec 7.2 rule 4).
    fn normalize_todos(&mut self) {
        if !self.opts.interactive || self.todos.iter().all(|t| t.id.is_some()) {
            return;
        }
        match self.todo_file.normalize_now(today()) {
            Ok(o) => self.todos = o.todos,
            Err(e) => self.todo_error = Some(e.to_string()),
        }
    }

    fn emit_todos(&self) {
        match &self.todo_error {
            Some(e) => self.emit(AppEvent::TodosReloaded(Err(e.clone()))),
            None => self.emit(AppEvent::TodosReloaded(Ok(self.todos.clone()))),
        }
    }

    fn start_todo_watcher(&mut self) {
        if !self.opts.interactive || !self.todo_file.path().exists() {
            return;
        }
        match TodoWatcher::spawn_channel(self.todo_file.path(), self.todo_file.shared_hash()) {
            Ok((w, rx)) => {
                self.todo_watcher = Some(w);
                self.watch_rx = Some(rx);
            }
            Err(e) => self.set_error("todo watcher", e.to_string()),
        }
    }

    async fn todos_changed(&mut self) {
        match self.todo_file.reload_if_changed() {
            Ok(None) => return,
            Ok(Some(l)) => {
                self.todos = l.todos;
                self.todo_error = None;
                self.normalize_todos();
            }
            Err(e) => self.todo_error = Some(e.to_string()),
        }
        self.emit_todos();
        self.after_todos_changed().await;
    }

    /// Apply todo operations; returns whether anything was written.
    fn apply_todo_ops(&mut self, ops: Vec<TodoOp>) -> bool {
        let mut changed = false;
        for op in ops {
            match self.todo_file.apply(op, today()) {
                Ok(o) => {
                    self.todos = o.todos;
                    self.todo_error = None;
                    changed |= o.written;
                    if let Some(msg) = o.dropped {
                        self.status(msg);
                    }
                }
                Err(e) => {
                    self.todo_error = Some(e.to_string());
                    break;
                }
            }
        }
        self.emit_todos();
        changed
    }

    /// Linked todo counts changed: refresh WorkItems and the summary without git.
    async fn after_todos_changed(&mut self) {
        self.assemble_and_persist(store::now()).await;
        self.emit(AppEvent::WorkItemsUpdated(self.items.clone()));
        self.emit_summary().await;
        self.emit_diagnostics();
    }

    fn todo_links(&self) -> HashMap<ItemKey, u32> {
        let by_name: HashMap<String, RepoId> = self
            .repos
            .iter()
            .rev()
            .map(|r| (r.display_name(), r.id.clone()))
            .collect();
        let mut out = HashMap::new();
        for t in &self.todos {
            let Some((name, branch)) = t.link.as_deref().and_then(|l| l.split_once(':')) else {
                continue;
            };
            if let Some(id) = by_name.get(name) {
                *out.entry((id.clone(), branch.to_string())).or_insert(0) += 1;
            }
        }
        out
    }

    // -----------------------------------------------------------------------------------------
    // local reload

    fn session_infos(&self, now: i64) -> HashMap<ItemKey, SessionInfo> {
        let mut out: HashMap<ItemKey, SessionInfo> = HashMap::new();
        for s in &self.sessions {
            for l in &s.links {
                let e = out
                    .entry((l.repo_id.clone(), l.branch.clone()))
                    .or_default();
                e.count += 1;
                e.live |= s.is_live(now);
                e.last_activity = e.last_activity.max(s.last_activity_at);
            }
        }
        out
    }

    /// Discovery only (no fetch), used by the CLI when the cache is empty.
    async fn discover_now(&mut self) {
        let cfg = Arc::clone(&self.cfg);
        match tokio::task::spawn_blocking(move || discovery::discover(&cfg)).await {
            Ok(d) => {
                let stored = d.clone();
                if let Err(e) = self
                    .writer
                    .write(move |c| discovery::store_discovery(c, &stored, store::now()))
                    .await
                {
                    self.set_error("database", format!("store discovery: {e}"));
                }
                self.apply_discovery(d);
            }
            Err(e) => self.set_error("discovery", e.to_string()),
        }
    }

    fn apply_discovery(&mut self, d: DiscoveryResult) {
        self.repos = d.repos;
        self.worktrees = d.worktrees;
        self.discovery_diags = d.diagnostics;
        let tracked: HashSet<&RepoId> = self.repos.iter().map(|r| &r.id).collect();
        self.local.retain(|id, _| tracked.contains(id));
    }

    /// Spec 10.2 local reload: git per repo, session scan, WorkItems, rules, events.
    async fn local_reload(&mut self) {
        let now = store::now();
        let repos = self.repos.clone();
        let mut prs_by_repo: HashMap<RepoId, HashMap<String, PrData>> = HashMap::new();
        for ((rid, branch), pr) in &self.prs {
            if let Some(pr) = pr {
                prs_by_repo
                    .entry(rid.clone())
                    .or_default()
                    .insert(branch.clone(), pr.clone());
            }
        }
        let git = tokio::task::spawn_blocking(move || {
            let lr = localgit::local_reload(&repos);
            let mut reused = HashSet::new();
            for repo in &repos {
                let (Some(rl), Some(prs)) = (lr.repos.get(&repo.id), prs_by_repo.get(&repo.id))
                else {
                    continue;
                };
                for b in localgit::compute_reused(repo, &rl.branches, prs) {
                    reused.insert((repo.id.clone(), b));
                }
            }
            (lr, reused)
        })
        .await;
        match git {
            Ok((lr, reused)) => {
                let failed: HashSet<&RepoId> = lr.errors.iter().map(|(id, _)| id).collect();
                let mut worktrees: Vec<Worktree> = self
                    .worktrees
                    .iter()
                    .filter(|w| failed.contains(&w.repo_id))
                    .cloned()
                    .collect();
                worktrees.extend(lr.all_worktrees());
                self.worktrees = worktrees;
                self.local.extend(lr.branches_by_repo());
                for (id, rl) in &lr.repos {
                    for w in &rl.warnings {
                        tracing::info!("{}: {w}", self.repo_name(id));
                    }
                }
                for repo in self.repos.clone() {
                    let scope = format!("git {}", repo.display_name());
                    match lr.errors.iter().find(|(id, _)| *id == repo.id) {
                        Some((_, e)) => self.set_error(scope, e.clone()),
                        None => self.clear_error(&scope),
                    }
                }
                self.reused = reused;
            }
            Err(e) => self.set_error("local reload", e.to_string()),
        }

        if self.cfg.sessions.enabled {
            let projects = self.cfg.claude_projects_dir();
            let summ = self.paths.summarizer_dir.clone();
            let wts = self.worktrees.clone();
            match self
                .writer
                .write(move |c| {
                    let report = ingest::refresh(c, &projects, &summ, &wts, now)?;
                    Ok((report, ingest::load_sessions(c)?))
                })
                .await
            {
                Ok((report, sessions)) => {
                    self.sessions = sessions;
                    self.scan = Some(report);
                    self.clear_error("sessions");
                }
                Err(e) => self.set_error("sessions", e.to_string()),
            }
        }

        self.assemble_and_persist(now).await;
        if self.opts.interactive {
            self.run_rules(now).await;
        }

        self.emit(AppEvent::ReposUpdated {
            repos: self.repos.clone(),
            worktrees: self.worktrees.clone(),
        });
        self.emit(AppEvent::WorkItemsUpdated(self.items.clone()));
        self.emit(AppEvent::SessionsUpdated(self.sessions.clone()));
        self.emit_suggestions().await;
        self.emit_summary().await;
        self.spawn_summaries().await;
        self.emit_diagnostics();
    }

    fn pr_availability(&self) -> PrAvailability {
        if self.bb.has_token() {
            PrAvailability::Available
        } else {
            PrAvailability::Unavailable
        }
    }

    /// Assemble WorkItems from the current state and persist them (state history).
    async fn assemble_and_persist(&mut self, now: i64) {
        let known = match self.writer.write(|c| workitems::load_known(c)).await {
            Ok(k) => k,
            Err(e) => {
                self.set_error("database", format!("work items: {e}"));
                return;
            }
        };
        let prs: HashMap<ItemKey, PrData> = self
            .prs
            .iter()
            .filter_map(|(k, v)| v.clone().map(|p| (k.clone(), p)))
            .collect();
        let ci: HashMap<String, CiState> = self
            .ci
            .iter()
            .filter_map(|(k, v)| v.map(|c| (k.clone(), c)))
            .collect();
        let sessions = self.session_infos(now);
        let links = self.todo_links();
        let my_user = (!self.cfg.identity.bitbucket_user.trim().is_empty())
            .then(|| self.cfg.identity.bitbucket_user.trim().to_string())
            .or_else(|| self.username.clone());
        let items = workitems::assemble(&AssemblyInput {
            repos: &self.repos,
            local: &self.local,
            prs: &prs,
            pr_availability: self.pr_availability(),
            reused_after_merge: &self.reused,
            ci_by_sha: &ci,
            sessions: &sessions,
            todo_links: &links,
            known: &known,
            my_bitbucket_user: my_user.as_deref(),
            my_emails: &self.cfg.identity.emails,
            now,
        });
        let to_store = items.clone();
        match self
            .writer
            .write(move |c| workitems::persist(c, &to_store, now))
            .await
        {
            Ok(transitions) => {
                for t in transitions {
                    tracing::debug!("{}:{} {:?} -> {:?}", t.repo_id, t.branch, t.from, t.to);
                }
            }
            Err(e) => self.set_error("database", format!("persist work items: {e}")),
        }
        self.items = items;
    }

    async fn run_rules(&mut self, now: i64) {
        // Before the first discovery there is nothing to judge; keep the first-run batch for
        // when repos are known.
        if self.repos.is_empty() {
            return;
        }
        let items = self.items.clone();
        let sessions = self.sessions.clone();
        let todos = self.todos.clone();
        let cfg = Arc::clone(&self.cfg);
        let names: HashMap<RepoId, String> = self
            .repos
            .iter()
            .map(|r| (r.id.clone(), r.display_name()))
            .collect();
        let out = self
            .writer
            .write(move |c| {
                let first_run = store::meta_get(c, META_FIRST_RUN_DONE)?.is_none();
                let out = rules::run(
                    c,
                    &RulesContext {
                        work_items: &items,
                        sessions: &sessions,
                        todos: &todos,
                        rules: &cfg.todos.rules,
                        repo_names: &names,
                        first_run,
                        now,
                    },
                )?;
                if first_run {
                    store::meta_set(c, META_FIRST_RUN_DONE, "1")?;
                }
                Ok(out)
            })
            .await;
        match out {
            Ok(out) => {
                self.clear_error("rules");
                let accepted = Accepted {
                    todo_ops: out.todo_ops,
                    abandon: None,
                };
                let mut changed = self.apply_accepted(accepted).await;
                for (rid, branch) in out.abandon {
                    changed |= self.mark_abandoned(rid, branch, true).await;
                }
                if changed {
                    self.assemble_and_persist(now).await;
                }
            }
            Err(e) => self.set_error("rules", e.to_string()),
        }
    }

    /// Apply the todo ops and abandon mark of an accepted suggestion. Returns whether anything
    /// changed (WorkItems need reassembly).
    async fn apply_accepted(&mut self, a: Accepted) -> bool {
        let mut changed = false;
        if !a.todo_ops.is_empty() {
            changed |= self.apply_todo_ops(a.todo_ops);
        }
        if let Some((rid, branch)) = a.abandon {
            changed |= self.mark_abandoned(rid, branch, true).await;
        }
        changed
    }

    async fn mark_abandoned(&mut self, repo_id: String, branch: String, abandoned: bool) -> bool {
        let (rid, b) = (repo_id.clone(), branch.clone());
        match self
            .writer
            .write(move |c| workitems::set_abandoned_manual(c, &rid, &b, abandoned))
            .await
        {
            Ok(found) => found,
            Err(e) => {
                self.set_error("database", format!("abandon {branch}: {e}"));
                false
            }
        }
    }

    async fn spawn_summaries(&mut self) {
        let Some(pool) = self.pool.clone() else {
            return;
        };
        let min_turns = self.cfg.sessions.min_turns_to_summarize;
        if let Err(e) = pool
            .spawn_due(&self.writer, min_turns, self.on_summary_done())
            .await
        {
            self.set_error("summarizer", e.to_string());
        }
    }

    fn on_summary_done(&self) -> OnDone {
        let tx = self.internal_tx.clone();
        Arc::new(move |id: &str, ok: bool| {
            let _ = tx.send(Internal::Summarized {
                session_id: id.to_string(),
                ok,
            });
        })
    }

    async fn reload_sessions(&mut self) {
        match self.writer.write(|c| ingest::load_sessions(c)).await {
            Ok(s) => self.sessions = s,
            Err(e) => self.set_error("sessions", e.to_string()),
        }
        self.emit(AppEvent::SessionsUpdated(self.sessions.clone()));
    }

    // -----------------------------------------------------------------------------------------
    // full sync

    fn start_full_sync(&mut self) {
        if self.syncing {
            return;
        }
        self.syncing = true;
        let ctx = SyncCtx {
            cfg: Arc::clone(&self.cfg),
            writer: self.writer.clone(),
            bb: Arc::clone(&self.bb),
            events: self.events.clone(),
            repos: self.repos.clone(),
            offline: self.opts.offline,
            username: self.username.clone(),
        };
        let tx = self.internal_tx.clone();
        tokio::spawn(async move {
            let done = full_sync(ctx).await;
            let _ = tx.send(Internal::SyncDone(Box::new(done)));
        });
    }

    async fn handle_internal(&mut self, msg: Internal) {
        match msg {
            Internal::SyncDone(done) => {
                let done = *done;
                if let Some(d) = done.discovery {
                    self.apply_discovery(d);
                    self.clear_error("discovery");
                }
                for (scope, e) in done.errors {
                    self.set_error(scope, e);
                }
                for (name, err) in done.fetches {
                    let scope = format!("fetch {name}");
                    match err {
                        Some(e) => self.set_error(scope, e),
                        None => self.clear_error(&scope),
                    }
                }
                if let Some(report) = done.report {
                    self.apply_report(report);
                }
                self.emit(AppEvent::PrUpdated {
                    status: self.pr_status.clone(),
                    synced_at: self.synced_at,
                });
                self.emit(AppEvent::Progress {
                    task: "reloading".into(),
                    done: 0,
                    total: 1,
                });
                self.local_reload().await;
                self.emit(AppEvent::Progress {
                    task: "syncing".into(),
                    done: 1,
                    total: 1,
                });
                self.syncing = false;
            }
            Internal::Summarized { session_id, ok } => {
                let scope = format!("summarizer {session_id}");
                if ok {
                    self.clear_error(&scope);
                } else {
                    self.set_error(scope, "summary failed (s in Sessions retries)");
                }
                self.reload_sessions().await;
                self.emit_summary().await;
                self.emit_diagnostics();
            }
        }
    }

    fn apply_report(&mut self, report: SyncReport) {
        for (key, latest) in report.latest_prs {
            self.prs.insert(key, latest.pr);
        }
        self.ci.extend(report.ci);
        if report.username.is_some() {
            self.username = report.username;
        }
        self.clear_errors_with_prefix("bitbucket ");
        for e in report.errors {
            self.set_error(format!("bitbucket {}", e.repo_name), e.message);
        }
        if report.ci_errors.is_empty() {
            self.clear_error("build status");
        } else {
            let n = report.ci_errors.len();
            let first = report.ci_errors[0].clone();
            self.set_error("build status", format!("{n} lookups failed, e.g. {first}"));
        }
        if !matches!(report.status, PrStatus::NoToken | PrStatus::Offline) {
            self.synced_at = Some(report.synced_at);
        }
        self.pr_status = report.status;
    }

    // -----------------------------------------------------------------------------------------
    // commands

    async fn handle_command(&mut self, cmd: Command) {
        let now = store::now();
        match cmd {
            Command::LocalReload => self.local_reload().await,
            Command::FullSync => {
                if self.syncing {
                    self.status("a full sync is already running");
                } else {
                    self.start_full_sync();
                }
            }
            Command::Todo(op) => {
                if self.apply_todo_ops(vec![op]) {
                    self.after_todos_changed().await;
                }
            }
            Command::AcceptSuggestion { id, edited_title } => {
                let todos = self.todos.clone();
                let r = self
                    .writer
                    .write(move |c| rules::accept(c, id, edited_title.as_deref(), &todos, now))
                    .await;
                self.after_decision(r).await;
            }
            Command::DecideSuggestion { id, abandon } => {
                let todos = self.todos.clone();
                let r = self
                    .writer
                    .write(move |c| rules::decide(c, id, abandon, &todos, now))
                    .await;
                self.after_decision(r).await;
            }
            Command::RejectSuggestion { id } => {
                match self.writer.write(move |c| rules::reject(c, id, now)).await {
                    Ok(true) => {}
                    Ok(false) => self.status("that suggestion is no longer pending"),
                    Err(e) => self.set_error("inbox", e.to_string()),
                }
                self.emit_suggestions().await;
                self.emit_summary().await;
            }
            Command::AcceptAll => {
                let todos = self.todos.clone();
                let r = self
                    .writer
                    .write(move |c| rules::accept_all(c, &todos, now))
                    .await
                    .map(Some);
                self.after_decision(r).await;
            }
            Command::RejectAll => {
                match self.writer.write(move |c| rules::reject_all(c, now)).await {
                    Ok(n) => self.status(format!("rejected {n} suggestions")),
                    Err(e) => self.set_error("inbox", e.to_string()),
                }
                self.emit_suggestions().await;
                self.emit_summary().await;
            }
            Command::SetAbandoned {
                repo_id,
                branch,
                abandoned,
            } => {
                if self
                    .mark_abandoned(repo_id, branch.clone(), abandoned)
                    .await
                {
                    self.assemble_and_persist(now).await;
                    self.emit(AppEvent::WorkItemsUpdated(self.items.clone()));
                    self.emit_summary().await;
                    let what = if abandoned { "marked" } else { "unmarked" };
                    self.status(format!("{what} {branch} as abandoned"));
                } else {
                    self.status(format!("{branch} is not tracked"));
                }
            }
            Command::RemoveWorktree { repo_id, branch } => {
                self.remove_worktree(&repo_id, &branch).await;
            }
            Command::Resummarize { session_id } => {
                let Some(pool) = self.pool.clone() else {
                    self.status("the summarizer is off (--offline or sessions.summarize = false)");
                    return;
                };
                match pool
                    .resummarize(&self.writer, session_id, self.on_summary_done())
                    .await
                {
                    Ok(Some(_)) => self.status("summarizing again…"),
                    Ok(None) => self.status("this session is already being summarized"),
                    Err(e) => self.set_error("summarizer", e.to_string()),
                }
                self.reload_sessions().await;
            }
            Command::Summary { since } => {
                self.summary_since = Some(since);
                self.emit_summary().await;
            }
            Command::WriteSummary { since } => match self.summary_markdown(Some(since)).await {
                Ok((window, md)) => {
                    let dir = self.cfg.export_dir();
                    match summary::write_export(&dir, window.today, &md) {
                        Ok(p) => {
                            self.clear_error("summary");
                            self.status(format!("summary written to {}", config::tildify(&p)));
                        }
                        Err(e) => self.set_error("summary", e.to_string()),
                    }
                }
                Err(e) => self.set_error("summary", e.to_string()),
            },
            Command::OpenUrl(url) => {
                let r = tokio::task::spawn_blocking(move || {
                    crate::lgbc::git::open_url_in_browser(&url).map_err(|e| e.to_string())
                })
                .await;
                match r {
                    Ok(Ok(())) => {}
                    Ok(Err(e)) => self.set_error("browser", e),
                    Err(e) => self.set_error("browser", e.to_string()),
                }
            }
            Command::OpenInZellij { session_id } => self.open_in_zellij(&session_id),
            Command::Quit => {}
        }
    }

    /// Runs on its own task: a few `zellij` calls with timeouts must not hold up other commands.
    fn open_in_zellij(&mut self, session_id: &str) {
        if !self.cfg.zellij.enabled {
            self.status("Zellij is off (zellij.enabled = false)");
            return;
        }
        let Some(session) = self.sessions.iter().find(|s| s.id == session_id).cloned() else {
            self.emit(AppEvent::Error {
                scope: "zellij".into(),
                message: "that session is no longer known".into(),
            });
            return;
        };
        let req = crate::zellij::OpenRequest {
            session,
            worktrees: self.worktrees.clone(),
            rules: self.cfg.zellij.target.clone(),
            env_session: std::env::var("ZELLIJ_SESSION_NAME").ok(),
            ports: self.zellij_ports.clone(),
        };
        let events = self.events.clone();
        tokio::spawn(async move {
            let ev = match crate::zellij::open(req).await {
                Ok(msg) => AppEvent::Status(msg),
                Err(e) => {
                    let message = crate::sanitize::clean_line(&e);
                    tracing::warn!("zellij: {message}");
                    AppEvent::Error {
                        scope: "zellij".into(),
                        message,
                    }
                }
            };
            let _ = events.send(ev);
        });
    }

    async fn after_decision(&mut self, r: Result<Option<Accepted>>) {
        match r {
            Ok(Some(a)) => {
                if self.apply_accepted(a).await {
                    self.after_todos_changed().await;
                }
            }
            Ok(None) => self.status("that suggestion is no longer pending"),
            Err(e) => self.set_error("inbox", e.to_string()),
        }
        self.emit_suggestions().await;
        self.emit_summary().await;
    }

    async fn remove_worktree(&mut self, repo_id: &str, branch: &str) {
        let item = self
            .items
            .iter()
            .find(|i| i.repo_id == repo_id && i.branch == branch)
            .cloned();
        let repo = self.repos.iter().find(|r| r.id == repo_id).cloned();
        let (Some(item), Some(repo)) = (item, repo) else {
            self.status(format!("{branch} is not tracked"));
            return;
        };
        let Some(plan) = localgit::cleanup_plan(&repo, &item) else {
            self.status(format!("nothing to clean up for {branch}"));
            return;
        };
        let state = item.state;
        tracing::info!("cleanup: {}", plan.commands().join("; "));
        let r = tokio::task::spawn_blocking(move || {
            localgit::run_cleanup(&plan, state).map_err(|e| e.to_string())
        })
        .await;
        match r {
            Ok(Ok(done)) => {
                self.clear_error("cleanup");
                self.status(done.join(", "));
            }
            Ok(Err(e)) => self.set_error("cleanup", e),
            Err(e) => self.set_error("cleanup", e.to_string()),
        }
        self.local_reload().await;
    }

    // -----------------------------------------------------------------------------------------
    // summary, suggestions, diagnostics

    async fn summary_markdown(&self, since: Option<NaiveDate>) -> Result<(Window, String)> {
        let (window, s) = self.build_summary(since).await?;
        Ok((window, summary::to_markdown(&s)))
    }

    async fn build_summary(&self, since: Option<NaiveDate>) -> Result<(Window, summary::Summary)> {
        let today = today();
        let window = match since.or(self.summary_since) {
            Some(s) => Window::for_since(s, today),
            None => Window::default_for(today, &self.cfg.workdays()),
        };
        let start = window.start_ts;
        let (events, unresolved) = self
            .writer
            .write(move |c| {
                Ok((
                    summary::load_events_since(c, start)?,
                    summary::load_unresolved_choices(c)?,
                ))
            })
            .await?;
        let s = summary::build(
            &summary::SummaryInput {
                items: &self.items,
                todos: &self.todos,
                sessions: &self.sessions,
                events: &events,
                unresolved: &unresolved,
            },
            &window,
        );
        Ok((window, s))
    }

    async fn emit_summary(&mut self) {
        match self.build_summary(None).await {
            Ok((window, summary)) => {
                self.clear_error("summary");
                let markdown = summary::to_markdown(&summary);
                self.emit(AppEvent::SummaryUpdated {
                    since: window.since,
                    summary,
                    markdown,
                });
            }
            Err(e) => self.set_error("summary", e.to_string()),
        }
    }

    async fn emit_suggestions(&mut self) {
        match self.writer.write(|c| rules::list_pending(c)).await {
            Ok(s) => self.emit(AppEvent::SuggestionsUpdated(s)),
            Err(e) => self.set_error("inbox", e.to_string()),
        }
    }

    fn emit_diagnostics(&self) {
        self.emit(AppEvent::Diagnostics(self.diagnostics(false)));
    }

    /// Spec 9.7 diagnostics. `Section:` lines are headings. The TUI lists skipped repos and the
    /// todo parse error itself; `with_repos` adds them for `doctor`.
    fn diagnostics(&self, with_repos: bool) -> Vec<String> {
        let mut out = vec![
            "General:".to_string(),
            format!("  version  {}", env!("CARGO_PKG_VERSION")),
            format!("  config   {}", config::tildify(&self.paths.config)),
            format!("  db       {}", config::tildify(self.writer.path())),
            format!("  todos    {}", config::tildify(self.todo_file.path())),
        ];
        if let Some(w) = self
            .todo_watcher
            .as_ref()
            .filter(|w| w.file() != self.todo_file.path())
        {
            out.push(format!("  watching {}", config::tildify(w.file())));
        }
        if self.opts.offline && self.opts.interactive {
            out.push("  mode     offline (no fetch, Bitbucket or summarizer)".into());
        }

        out.push("Bitbucket:".into());
        out.push(if self.bb.has_token() {
            format!("  token  set ({})", bitbucket::TOKEN_ENV)
        } else {
            format!(
                "  token  not set: export {} for PR data",
                bitbucket::TOKEN_ENV
            )
        });
        let user = self.cfg.identity.bitbucket_user.trim();
        out.push(match (&self.username, user.is_empty()) {
            (_, false) => format!("  user   {user} (config)"),
            (Some(u), true) => format!("  user   {u} (from Bitbucket)"),
            (None, true) => "  user   unknown (resolved on the next sync)".into(),
        });
        if self.opts.interactive {
            out.push(format!("  status {}", self.pr_status.describe()));
        }

        let tracked = self
            .repos
            .iter()
            .filter(|r| r.skipped_reason.is_none())
            .count();
        out.push("Repos:".into());
        out.push(format!(
            "  {tracked} tracked, {} skipped, {} work items",
            self.repos.len() - tracked,
            self.items.len()
        ));
        // Discovery reports skipped repos as `skipped <path>: <reason>`. The TUI lists skipped
        // repos itself, so only `doctor` (`with_repos`) keeps those lines.
        for d in &self.discovery_diags {
            if with_repos || !d.starts_with("skipped ") {
                out.push(format!("  {d}"));
            }
        }
        if with_repos && self.discovery_diags.is_empty() {
            for r in self.repos.iter().filter(|r| r.skipped_reason.is_some()) {
                out.push(format!(
                    "  skipped {}: {}",
                    config::tildify(&r.common_dir),
                    r.skipped_reason.as_deref().unwrap_or_default()
                ));
            }
        }

        out.push("Hook:".into());
        out.push(
            if crate::sessions::hook::is_installed(&self.paths.claude_settings) {
                "  SessionEnd hook installed".into()
            } else {
                format!(
                    "  SessionEnd hook not installed in {} (worktrack hook install)",
                    config::tildify(&self.paths.claude_settings)
                )
            },
        );

        out.push("Sessions:".into());
        if !self.cfg.sessions.enabled {
            out.push("  disabled (sessions.enabled = false)".into());
        } else {
            out.push(format!(
                "  {} sessions from {}",
                self.sessions.len(),
                config::tildify(&self.cfg.claude_projects_dir())
            ));
            if let Some(scan) = &self.scan {
                if scan.malformed_lines > 0 {
                    out.push(format!(
                        "  {} malformed transcript lines skipped",
                        scan.malformed_lines
                    ));
                }
                for e in &scan.errors {
                    out.push(format!("  scan: {e}"));
                }
            }
            let failed = self
                .sessions
                .iter()
                .filter(|s| s.summary_status == SummaryStatus::Failed)
                .count();
            let summarizer = if self.pool.is_some() {
                "on"
            } else if !self.cfg.sessions.summarize {
                "off (sessions.summarize = false)"
            } else if !self.opts.interactive {
                "runs in the TUI"
            } else if self.opts.offline {
                "off (offline)"
            } else {
                "off"
            };
            out.push(format!("  summarizer {summarizer}"));
            if failed > 0 {
                out.push(format!(
                    "  {failed} summaries failed (s in Sessions retries)"
                ));
            }
        }

        if with_repos {
            if let Some(e) = &self.todo_error {
                out.push("Todo file:".into());
                out.push(format!("  parse error: {e}"));
            }
        }

        if !self.errors.is_empty() {
            out.push("Last errors:".into());
            for (scope, msg) in &self.errors {
                out.push(format!("  {scope}: {msg}"));
            }
        }
        out
    }

    fn problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        if !self.bb.has_token() {
            out.push(format!("{} is not set", bitbucket::TOKEN_ENV));
        }
        if !self.repos.iter().any(|r| r.skipped_reason.is_none()) {
            out.push("no Bitbucket repositories found under discovery.roots".into());
        }
        if let Some(e) = &self.todo_error {
            out.push(format!("todo file: {e}"));
        }
        for (scope, msg) in &self.errors {
            out.push(format!("{scope}: {msg}"));
        }
        out
    }
}

async fn recv_opt(rx: &mut Option<UnboundedReceiver<()>>) -> Option<()> {
    match rx {
        Some(rx) => rx.recv().await,
        None => std::future::pending().await,
    }
}

// ---------------------------------------------------------------------------------------------
// full sync task

struct SyncCtx {
    cfg: Arc<Config>,
    writer: DbWriter,
    bb: Arc<BitbucketService>,
    events: std::sync::mpsc::Sender<AppEvent>,
    /// Known repos, used when discovery fails.
    repos: Vec<Repo>,
    offline: bool,
    username: Option<String>,
}

impl SyncCtx {
    fn progress(&self, task: &str, done: usize, total: usize) {
        let _ = self.events.send(AppEvent::Progress {
            task: task.into(),
            done,
            total,
        });
    }
}

/// Discovery, `git fetch --prune` per repo, Bitbucket (spec 10.2). Never fails as a whole.
async fn full_sync(ctx: SyncCtx) -> SyncDone {
    let mut done = SyncDone {
        discovery: None,
        fetches: Vec::new(),
        report: None,
        errors: Vec::new(),
    };
    ctx.progress("discovering", 0, 1);
    let cfg = Arc::clone(&ctx.cfg);
    let tracked: Vec<Repo> =
        match tokio::task::spawn_blocking(move || discovery::discover(&cfg)).await {
            Ok(d) => {
                let stored = d.clone();
                if let Err(e) = ctx
                    .writer
                    .write(move |c| discovery::store_discovery(c, &stored, store::now()))
                    .await
                {
                    done.errors
                        .push(("database".into(), format!("store discovery: {e}")));
                }
                let tracked = d.tracked_repos().cloned().collect();
                done.discovery = Some(d);
                tracked
            }
            Err(e) => {
                done.errors.push(("discovery".into(), e.to_string()));
                ctx.repos
                    .iter()
                    .filter(|r| r.skipped_reason.is_none())
                    .cloned()
                    .collect()
            }
        };

    if ctx.offline {
        return done;
    }

    // git fetch, at most 4 at a time.
    let total = tracked.len();
    ctx.progress("fetching", 0, total.max(1));
    let sem = Arc::new(tokio::sync::Semaphore::new(MAX_CONCURRENT_FETCH));
    let mut set = tokio::task::JoinSet::new();
    for repo in &tracked {
        let sem = Arc::clone(&sem);
        let dir = repo.git_dir_for_commands.clone();
        let name = repo.display_name();
        set.spawn(async move {
            let _permit = sem.acquire_owned().await;
            let r = tokio::task::spawn_blocking(move || {
                crate::lgbc::git::fetch_remote_in(&dir, "origin").map_err(|e| e.to_string())
            })
            .await
            .unwrap_or_else(|e| Err(e.to_string()));
            (name, r.err())
        });
    }
    let mut n = 0;
    while let Some(r) = set.join_next().await {
        n += 1;
        ctx.progress("fetching", n, total.max(1));
        if let Ok(r) = r {
            done.fetches.push(r);
        }
    }

    // Bitbucket.
    ctx.progress("bitbucket", 0, 1);
    let input = if ctx.bb.has_token() {
        match sync_input(&ctx, tracked).await {
            Ok(i) => i,
            Err(e) => {
                done.errors.push(("bitbucket".into(), e.to_string()));
                SyncInput::default()
            }
        }
    } else {
        SyncInput::default()
    };
    let report = ctx.bb.sync(input).await;
    let (prs, builds) = (report.pr_rows.clone(), report.build_rows.clone());
    if let Err(e) = ctx
        .writer
        .write(move |c| bitbucket::write_sync_rows(c, &prs, &builds))
        .await
    {
        done.errors
            .push(("database".into(), format!("store PR cache: {e}")));
    }
    done.report = Some(report);
    ctx.progress("bitbucket", 1, 1);
    done
}

/// Branches to resolve PRs for: every non-protected branch that is tracked already or mine by
/// worktree / commit email (rule 2, PR author, is covered by the dashboard request).
async fn sync_input(ctx: &SyncCtx, tracked: Vec<Repo>) -> Result<SyncInput> {
    let (cached, known) = ctx
        .writer
        .write(|c| Ok((bitbucket::load_pr_cache(c)?, workitems::load_known(c)?)))
        .await?;
    let cfg = Arc::clone(&ctx.cfg);
    let user = if cfg.identity.bitbucket_user.trim().is_empty() {
        ctx.username.clone().unwrap_or_default()
    } else {
        cfg.identity.bitbucket_user.trim().to_string()
    };
    let cached_for_git = cached.clone();
    let repos = tokio::task::spawn_blocking(move || {
        let merged: HashMap<(String, String), String> = cached_for_git
            .iter()
            .filter_map(|row| {
                bitbucket::merged_head(row)
                    .map(|h| ((row.repo_id.clone(), row.branch.clone()), h.to_string()))
            })
            .collect();
        let lr = localgit::local_reload(&tracked);
        let mut out = Vec::new();
        for repo in &tracked {
            let (Some(identity), Some(rl)) = (repo.bb.clone(), lr.repos.get(&repo.id)) else {
                continue;
            };
            let branches = rl
                .branches
                .iter()
                .filter(|b| !b.is_protected)
                .filter_map(|b| {
                    let key = (repo.id.clone(), b.branch.clone());
                    let k = known.get(&key);
                    if k.is_none() && !workitems::is_mine(b, None, None, &cfg.identity.emails) {
                        return None;
                    }
                    let unfinished = k.map_or(!(b.tip_in_trunk && b.has_own_commits), |k| {
                        k.state.is_unfinished()
                    });
                    let reused = match (merged.get(&key), b.tip.as_deref()) {
                        (Some(head), Some(tip)) => {
                            localgit::reused_after_merge(&repo.git_dir_for_commands, head, tip)
                                .unwrap_or(false)
                        }
                        _ => false,
                    };
                    Some(SyncBranch {
                        branch: b.branch.clone(),
                        tip: b.remote_tip.clone().or_else(|| b.tip.clone()),
                        unfinished,
                        reused_after_merge: reused,
                    })
                })
                .collect();
            out.push(SyncRepo {
                repo_id: repo.id.clone(),
                identity,
                branches,
            });
        }
        out
    })
    .await?;
    Ok(SyncInput {
        repos,
        cached,
        pr_cache_ttl_mins: ctx.cfg.refresh.pr_cache_ttl_mins,
        bitbucket_user: user,
        now: store::now(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discovery::testutil::*;
    use crate::model::WorkState;
    use std::sync::mpsc::Receiver;
    use std::time::Instant;

    fn setup(tmp: &std::path::Path) -> (Config, Paths) {
        let mut cfg = Config::default();
        cfg.discovery.roots = vec![tmp.join("roots").to_string_lossy().into_owned()];
        cfg.todos.file = tmp.join("todos.md").to_string_lossy().into_owned();
        cfg.sessions.claude_projects_dir = tmp.join("claude").to_string_lossy().into_owned();
        cfg.sessions.summarize = false;
        cfg.summary.export_dir = tmp.join("summaries").to_string_lossy().into_owned();
        let paths = Paths {
            config: tmp.join("config.toml"),
            db: tmp.join("data").join("worktrack.db"),
            summarizer_dir: tmp.join("summarizer"),
            claude_settings: tmp.join("settings.json"),
        };
        (cfg, paths)
    }

    /// Wait for an event matching `pred`.
    fn wait_for<T>(rx: &Receiver<AppEvent>, mut pred: impl FnMut(&AppEvent) -> Option<T>) -> T {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let ev = rx.recv_timeout(left).expect("timed out waiting for event");
            if let Some(t) = pred(&ev) {
                return t;
            }
        }
    }

    #[test]
    fn offline_background_assembles_items_and_writes_todos() {
        let tmp = tempfile::tempdir().unwrap();
        let roots = tmp.path().join("roots");
        std::fs::create_dir_all(&roots).unwrap();
        let f = Fixture::new(&roots);

        // LocalOnly: a branch in a linked worktree, never pushed.
        let wt_local = roots.join("wt-local");
        git(
            &f.work,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "feature/local",
                wt_local.to_str().unwrap(),
            ],
        );
        commit(&wt_local, "a.txt", "local work");
        // Pushed: a branch in another worktree, pushed to origin, no PR (offline).
        let wt_pushed = roots.join("wt-pushed");
        git(
            &f.work,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "feature/pushed",
                wt_pushed.to_str().unwrap(),
            ],
        );
        commit(&wt_pushed, "b.txt", "pushed work");
        f.push(&wt_pushed, "feature/pushed");

        let (cfg, paths) = setup(tmp.path());
        let todo_path = cfg.todo_file();
        let (tx, rx) = std::sync::mpsc::channel();
        let bg = Background::start(
            cfg,
            paths,
            Options {
                offline: true,
                auto_sync: false,
                interactive: true,
            },
            tx,
        )
        .unwrap();

        let states = wait_for(&rx, |ev| match ev {
            AppEvent::WorkItemsUpdated(items) if items.len() >= 2 => Some(
                items
                    .iter()
                    .map(|i| (i.branch.clone(), i.state))
                    .collect::<HashMap<_, _>>(),
            ),
            _ => None,
        });
        assert_eq!(states.get("feature/local"), Some(&WorkState::LocalOnly));
        assert_eq!(states.get("feature/pushed"), Some(&WorkState::Pushed));
        assert!(
            !states.contains_key("main"),
            "protected trunk is not a WorkItem"
        );

        bg.send(Command::Todo(TodoOp::Add(Todo::new(
            "Write integration tests",
        ))));
        wait_for(&rx, |ev| match ev {
            AppEvent::TodosReloaded(Ok(todos))
                if todos.iter().any(|t| t.title == "Write integration tests") =>
            {
                Some(())
            }
            _ => None,
        });
        let text = std::fs::read_to_string(&todo_path).unwrap();
        assert!(text.contains("Write integration tests"), "{text}");
        bg.shutdown(Duration::from_secs(5));
    }

    #[test]
    fn oneshot_builds_summary_and_items() {
        let tmp = tempfile::tempdir().unwrap();
        let roots = tmp.path().join("roots");
        std::fs::create_dir_all(&roots).unwrap();
        let f = Fixture::new(&roots);
        git(&f.work, &["checkout", "-q", "-b", "feature/x"]);
        commit(&f.work, "x.txt", "x");

        let (cfg, paths) = setup(tmp.path());
        let todo_path = cfg.todo_file();
        let one = Oneshot::open(cfg, paths, false).unwrap();
        assert!(one
            .items()
            .iter()
            .any(|i| i.branch == "feature/x" && i.state == WorkState::LocalOnly));
        let (_, md) = one.summary_markdown(None).unwrap();
        assert!(md.starts_with("# Summary"), "{md}");
        assert!(!todo_path.exists(), "the CLI does not create the todo file");
        assert!(one.diagnostics().iter().any(|l| l == "Hook:"));
    }
}
