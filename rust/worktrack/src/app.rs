//! App state and actions (spec 9). OWNED BY: UI agent.
//!
//! `App` holds everything the views draw and turns key presses into state changes and
//! [`Effect`]s. It performs no IO: no filesystem, git, network, clipboard or process spawning.
//! The event loop in `main.rs` performs the effects.
//!
//! # Wiring (for the integrator)
//!
//! ```text
//! let mut app = App::new(&config, initial_tab);        // initial_tab from --tab / auto-open
//! app.no_color = std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty());
//! app.home = dirs::home_dir();                          // for `~` in displayed paths
//! loop {                                                // 100 ms tick (spec 10.3)
//!     app.tick(chrono::Local::now().fixed_offset());    // clock + status timeout
//!     while let Ok(ev) = rx.try_recv() { app.handle_event(ev); }
//!     terminal.draw(|f| ui::draw(f, &app))?;
//!     for effect in app.handle_key(key) /* or handle_mouse(mouse) */ {
//!         match effect {
//!             Effect::Command(cmd) => cmd_tx.send(cmd),
//!             Effect::CopyToClipboard(text) => lgbc::clipboard::copy(text),
//!             Effect::OpenEditor { kind } => { /* suspend TUI, run editor, resume */ }
//!             Effect::Quit => break,
//!         }
//!     }
//! }
//! ```
//!
//! `ui::draw` takes `&App`; the few things the renderer has to tell the app back (pane areas
//! for mouse hit tests, scroll limits, list offsets, the arrangement chosen for the terminal
//! size) live in `Cell`s.

use std::cell::Cell;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Datelike, FixedOffset, NaiveDate, TimeZone, Weekday};
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseEvent, MouseEventKind};
use ratatui::layout::{Position, Rect};

use crate::config::Config;
use crate::lgbc::responsive::{Arrangement, LayoutMode};
use crate::model::{CiState, Repo, WorkItem, WorkState, Worktree};
use crate::refresh::{AppEvent, Command, PrStatus};
use crate::sessions::Session;
use crate::todos::rules::{Suggestion, SuggestionPayload, SuggestionStatus};
use crate::todos::{Priority, Todo, TodoOp, TodoPatch, TodoStatus};

/// Wall clock as seen by the app; its offset is the local time zone used for display.
pub type Now = DateTime<FixedOffset>;

/// Status messages disappear after this many seconds (spec 9.1).
pub const STATUS_TIMEOUT_SECS: i64 = 5;
/// Rows one mouse wheel notch moves.
const WHEEL_ROWS: isize = 3;

/// What the event loop must do after a key press. `App` itself never performs IO.
#[derive(Debug, Clone)]
pub enum Effect {
    /// Send to the background (`refresh.rs`).
    Command(Command),
    CopyToClipboard(String),
    /// Suspend the TUI and run the editor (spec 7.4).
    OpenEditor {
        kind: EditorTarget,
    },
    Quit,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditorTarget {
    /// The notes of the todo with this id, through a temp file.
    NotesOf(String),
    /// The whole todo file.
    WholeFile,
}

/// The five tabs (spec 9).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Tab {
    Work,
    Todos,
    Sessions,
    Summary,
    Inbox,
}

impl Tab {
    pub const ALL: [Tab; 5] = [
        Tab::Work,
        Tab::Todos,
        Tab::Sessions,
        Tab::Summary,
        Tab::Inbox,
    ];

    pub fn index(self) -> usize {
        Self::ALL.iter().position(|t| *t == self).unwrap_or(0)
    }

    pub fn title(self) -> &'static str {
        match self {
            Tab::Work => "Work",
            Tab::Todos => "Todos",
            Tab::Sessions => "Sessions",
            Tab::Summary => "Summary",
            Tab::Inbox => "Inbox",
        }
    }

    /// Parse the `--tab` CLI value.
    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|t| t.title().eq_ignore_ascii_case(s))
    }

    fn next(self) -> Self {
        Self::ALL[(self.index() + 1) % Self::ALL.len()]
    }

    fn prev(self) -> Self {
        Self::ALL[(self.index() + Self::ALL.len() - 1) % Self::ALL.len()]
    }

    /// Whether the tab has a details pane.
    pub fn has_details(self) -> bool {
        !matches!(self, Tab::Summary)
    }
}

/// Work view filters, cycled with `f` (spec 9.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WorkFilter {
    #[default]
    Unfinished,
    NeedsAttention,
    Cleanup,
    Done,
    All,
}

impl WorkFilter {
    pub fn label(self) -> &'static str {
        match self {
            Self::Unfinished => "Unfinished",
            Self::NeedsAttention => "Needs attention",
            Self::Cleanup => "Cleanup",
            Self::Done => "Done",
            Self::All => "All",
        }
    }
    fn next(self) -> Self {
        match self {
            Self::Unfinished => Self::NeedsAttention,
            Self::NeedsAttention => Self::Cleanup,
            Self::Cleanup => Self::Done,
            Self::Done => Self::All,
            Self::All => Self::Unfinished,
        }
    }
}

/// Todos view filters (spec 9.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TodoFilter {
    #[default]
    Active,
    Done,
    All,
}

impl TodoFilter {
    pub fn label(self) -> &'static str {
        match self {
            Self::Active => "Active",
            Self::Done => "Done",
            Self::All => "All",
        }
    }
    fn next(self) -> Self {
        match self {
            Self::Active => Self::Done,
            Self::Done => Self::All,
            Self::All => Self::Active,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusKind {
    Info,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusMessage {
    pub text: String,
    pub kind: StatusKind,
    /// Unix seconds when set.
    pub at: i64,
}

/// A one-line text input (search, todo title, suggestion text).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LineInput {
    pub text: String,
    /// Cursor position in chars.
    pub cursor: usize,
}

impl LineInput {
    pub fn with_text(text: &str) -> Self {
        Self {
            text: text.to_string(),
            cursor: text.chars().count(),
        }
    }

    fn byte_at(&self, char_idx: usize) -> usize {
        self.text
            .char_indices()
            .nth(char_idx)
            .map_or(self.text.len(), |(i, _)| i)
    }

    /// Apply an editing key. Returns false for keys it does not handle.
    fn edit(&mut self, key: KeyEvent) -> bool {
        let len = self.text.chars().count();
        match key.code {
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.text.clear();
                self.cursor = 0;
            }
            KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => self.cursor = 0,
            KeyCode::Char('e') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.cursor = len
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                let at = self.byte_at(self.cursor);
                self.text.insert(at, c);
                self.cursor += 1;
            }
            KeyCode::Backspace if self.cursor > 0 => {
                let at = self.byte_at(self.cursor - 1);
                self.text.remove(at);
                self.cursor -= 1;
            }
            KeyCode::Delete if self.cursor < len => {
                let at = self.byte_at(self.cursor);
                self.text.remove(at);
            }
            KeyCode::Left => self.cursor = self.cursor.saturating_sub(1),
            KeyCode::Right => self.cursor = (self.cursor + 1).min(len),
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = len,
            KeyCode::Backspace | KeyCode::Delete => {}
            _ => return false,
        }
        true
    }
}

/// What an open line input is for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputPurpose {
    /// Typing the search query of the current tab.
    Search,
    /// New todo, optionally linked (`a` in Todos, `t` in Work).
    AddTodo { link: Option<String> },
    /// New title of a todo (`e`).
    EditTodo { id: String },
    /// Edited text of an AddTodo suggestion, accepted on Enter (`e` in Inbox).
    EditSuggestion { id: i64 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveInput {
    pub purpose: InputPurpose,
    pub input: LineInput,
}

/// A pending destructive or bulk action waiting for `y`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Confirm {
    Abandon {
        repo_id: String,
        branch: String,
        repo_name: String,
    },
    RemoveWorktree {
        repo_id: String,
        branch: String,
        /// Shell commands the background will run, shown in the dialog (spec 11.3).
        commands: Vec<String>,
    },
    DeleteTodo {
        id: String,
        title: String,
    },
    AcceptAll {
        count: usize,
    },
    RejectAll {
        count: usize,
    },
}

/// Fuzzy picker over WorkItems for a todo's link (`L`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkPicker {
    pub todo_id: String,
    pub query: LineInput,
    pub cursor: usize,
    /// Whether the todo already has a link (adds a "remove link" entry).
    pub has_link: bool,
}

/// One entry of the link picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PickerEntry {
    RemoveLink,
    Item(usize),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Overlay {
    Help,
    Diagnostics,
    Confirm(Confirm),
    LinkPicker(LinkPicker),
}

/// A row of the Work list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkRow {
    Group {
        repo_id: String,
        name: String,
        count: usize,
        collapsed: bool,
    },
    /// Index into `App::work_items`.
    Item(usize),
}

/// A row of the Todos list. The cursor only stops on `Todo` rows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TodoRow {
    Header(String),
    /// Index into `App::todos`.
    Todo(usize),
}

/// Layout facts the renderer reports back (pane areas for the mouse, scroll limits).
#[derive(Debug, Default)]
pub struct RenderFeedback {
    pub arrangement: Cell<Option<Arrangement>>,
    pub list_area: Cell<Rect>,
    pub details_area: Cell<Option<Rect>>,
    pub list_viewport: Cell<usize>,
    pub details_max_scroll: Cell<usize>,
    pub overlay_max_scroll: Cell<usize>,
    pub summary_max_scroll: Cell<usize>,
    pub too_small: Cell<bool>,
    /// First visible row of each tab's list.
    pub list_offset: [Cell<usize>; 5],
}

/// All UI state.
#[derive(Debug)]
pub struct App {
    // ---- data from the background ----
    pub repos: Vec<Repo>,
    pub worktrees: Vec<Worktree>,
    pub work_items: Vec<WorkItem>,
    pub pr_status: Option<PrStatus>,
    /// Unix seconds of the last full sync.
    pub synced_at: Option<i64>,
    pub sessions: Vec<Session>,
    pub todos: Vec<Todo>,
    /// Last todo file parse error; the todos shown are the last good state.
    pub todo_error: Option<String>,
    pub suggestions: Vec<Suggestion>,
    pub summary_markdown: Option<String>,
    /// Start of the summary window shown (or requested).
    pub summary_since: Option<NaiveDate>,
    pub diagnostics: Vec<String>,
    /// Recent errors (newest last), also shown in diagnostics.
    pub errors: Vec<String>,
    /// `(task, done, total)` of the running background task.
    pub progress: Option<(String, usize, usize)>,
    pub status: Option<StatusMessage>,

    // ---- environment, set by main ----
    pub now: Now,
    pub no_color: bool,
    /// Home directory, shown as `~` in paths.
    pub home: Option<PathBuf>,

    // ---- config bits ----
    jira_re: Option<regex::Regex>,
    pub jira_browse_url: String,
    workdays: Vec<Weekday>,

    // ---- view state ----
    pub tab: Tab,
    pub layout_mode: LayoutMode,
    pub work_filter: WorkFilter,
    pub todo_filter: TodoFilter,
    pub todo_grouped: bool,
    pub collapsed: HashSet<String>,
    pub search: [String; 5],
    pub cursor: [usize; 5],
    pub details_scroll: usize,
    /// List-only layout: details shown full screen.
    pub details_open: bool,
    pub summary_scroll: usize,
    pub input: Option<ActiveInput>,
    pub overlay: Option<Overlay>,
    pub overlay_scroll: usize,
    pub feedback: RenderFeedback,
}

/// POSIX single-quote escaping (spec 11.3).
pub fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// `cd '<path>' && claude --resume <id>` (spec 6.5).
pub fn resume_command(path: &Path, session_id: &str) -> String {
    format!(
        "cd {} && claude --resume {}",
        shell_quote(&path.to_string_lossy()),
        shell_quote_if_needed(session_id)
    )
}

/// Session ids are UUIDs; quote anything else so it cannot break the command.
fn shell_quote_if_needed(s: &str) -> String {
    if !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    {
        s.to_string()
    } else {
        shell_quote(s)
    }
}

/// The latest workday strictly before `date` (spec 8.1). With no workdays: the day before.
pub fn previous_workday(date: NaiveDate, workdays: &[Weekday]) -> NaiveDate {
    let mut d = date;
    for _ in 0..7 {
        d = d.pred_opt().unwrap_or(d);
        if workdays.contains(&d.weekday()) {
            return d;
        }
    }
    date.pred_opt().unwrap_or(date)
}

/// The first workday strictly after `date`.
pub fn next_workday(date: NaiveDate, workdays: &[Weekday]) -> NaiveDate {
    let mut d = date;
    for _ in 0..7 {
        d = d.succ_opt().unwrap_or(d);
        if workdays.contains(&d.weekday()) {
            return d;
        }
    }
    date.succ_opt().unwrap_or(date)
}

/// Lowercase subsequence match score; `None` when `needle` is not a subsequence of `hay`.
/// Lower is better: the span the match covers.
pub fn fuzzy_score(needle: &str, hay: &str) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    let hay: Vec<char> = hay.to_lowercase().chars().collect();
    let needle: Vec<char> = needle.to_lowercase().chars().collect();
    let mut best: Option<usize> = None;
    for start in 0..hay.len() {
        if hay[start] != needle[0] {
            continue;
        }
        let mut ni = 1;
        let mut end = start;
        for (i, c) in hay.iter().enumerate().skip(start + 1) {
            if ni == needle.len() {
                break;
            }
            if *c == needle[ni] {
                ni += 1;
                end = i;
            }
        }
        if ni == needle.len() {
            let span = end - start;
            best = Some(best.map_or(span, |b| b.min(span)));
        }
    }
    best
}

impl App {
    pub fn new(config: &Config, initial_tab: Tab) -> Self {
        let epoch = FixedOffset::east_opt(0)
            .expect("zero offset")
            .timestamp_opt(0, 0)
            .single()
            .expect("epoch");
        Self {
            repos: Vec::new(),
            worktrees: Vec::new(),
            work_items: Vec::new(),
            pr_status: None,
            synced_at: None,
            sessions: Vec::new(),
            todos: Vec::new(),
            todo_error: None,
            suggestions: Vec::new(),
            summary_markdown: None,
            summary_since: None,
            diagnostics: Vec::new(),
            errors: Vec::new(),
            progress: None,
            status: None,
            now: epoch,
            no_color: false,
            home: None,
            jira_re: regex::Regex::new(&config.jira.key_pattern).ok(),
            jira_browse_url: config.jira.browse_url.clone(),
            workdays: config.workdays(),
            tab: initial_tab,
            layout_mode: LayoutMode::Auto,
            work_filter: WorkFilter::default(),
            todo_filter: TodoFilter::default(),
            todo_grouped: false,
            collapsed: HashSet::new(),
            search: Default::default(),
            cursor: [0; 5],
            details_scroll: 0,
            details_open: false,
            summary_scroll: 0,
            input: None,
            overlay: None,
            overlay_scroll: 0,
            feedback: RenderFeedback::default(),
        }
    }

    // ------------------------------------------------------------------
    // Clock and events
    // ------------------------------------------------------------------

    pub fn now_ts(&self) -> i64 {
        self.now.timestamp()
    }

    pub fn today(&self) -> NaiveDate {
        self.now.date_naive()
    }

    /// Advance the clock: clears the status line after 5 s and sets the default summary
    /// window on the first tick.
    pub fn tick(&mut self, now: Now) {
        self.now = now;
        if self
            .status
            .as_ref()
            .is_some_and(|s| now.timestamp() - s.at >= STATUS_TIMEOUT_SECS)
        {
            self.status = None;
        }
        if self.summary_since.is_none() {
            self.summary_since = Some(previous_workday(self.today(), &self.workdays));
        }
    }

    pub fn set_status(&mut self, text: impl Into<String>) {
        self.status = Some(StatusMessage {
            text: text.into(),
            kind: StatusKind::Info,
            at: self.now_ts(),
        });
    }

    pub fn set_error(&mut self, text: impl Into<String>) {
        self.status = Some(StatusMessage {
            text: text.into(),
            kind: StatusKind::Error,
            at: self.now_ts(),
        });
    }

    pub fn handle_event(&mut self, event: AppEvent) {
        let keys: Vec<Option<String>> = Tab::ALL.iter().map(|t| self.selected_key(*t)).collect();
        match event {
            AppEvent::ReposUpdated { repos, worktrees } => {
                self.repos = repos;
                self.worktrees = worktrees;
            }
            AppEvent::WorkItemsUpdated(items) => self.work_items = items,
            AppEvent::PrUpdated { status, synced_at } => {
                self.pr_status = Some(status);
                if synced_at.is_some() {
                    self.synced_at = synced_at;
                }
            }
            AppEvent::SessionsUpdated(sessions) => self.sessions = sessions,
            AppEvent::TodosReloaded(Ok(todos)) => {
                self.todos = todos;
                self.todo_error = None;
            }
            AppEvent::TodosReloaded(Err(e)) => {
                self.set_error(format!("todo file: {e}"));
                self.todo_error = Some(e);
            }
            AppEvent::SuggestionsUpdated(s) => self.suggestions = s,
            AppEvent::SummaryUpdated { since, markdown } => {
                self.summary_since = Some(since);
                self.summary_markdown = Some(markdown);
            }
            AppEvent::Diagnostics(lines) => self.diagnostics = lines,
            AppEvent::Progress { task, done, total } => {
                self.progress = (done < total).then_some((task, done, total));
            }
            AppEvent::Status(msg) => self.set_status(msg),
            AppEvent::Error { scope, message } => {
                let line = format!("{scope}: {message}");
                self.errors.push(line.clone());
                if self.errors.len() > 20 {
                    self.errors.remove(0);
                }
                self.set_error(line);
            }
        }
        for (tab, key) in Tab::ALL.iter().zip(keys) {
            self.reselect(*tab, key);
        }
    }

    // ------------------------------------------------------------------
    // Derived lists
    // ------------------------------------------------------------------

    fn query(&self, tab: Tab) -> &str {
        &self.search[tab.index()]
    }

    /// Window start of the summary as unix seconds (local midnight).
    fn summary_window_start(&self) -> Option<i64> {
        let since = self.summary_since?;
        let midnight = since.and_hms_opt(0, 0, 0)?;
        self.now
            .offset()
            .from_local_datetime(&midnight)
            .single()
            .map(|d| d.timestamp())
    }

    fn work_filter_matches(&self, item: &WorkItem) -> bool {
        if item.state == WorkState::Protected {
            return false;
        }
        match self.work_filter {
            WorkFilter::Unfinished => item.state.is_unfinished(),
            WorkFilter::NeedsAttention => {
                matches!(item.state, WorkState::PrNeedsWork | WorkState::PrDeclined)
                    || item.flags.ci == Some(CiState::Failed)
                    || item.flags.stale
            }
            WorkFilter::Cleanup => item.flags.cleanup,
            WorkFilter::Done => {
                item.state.is_done()
                    && match (item.state_since, self.summary_window_start()) {
                        (Some(at), Some(start)) => at >= start,
                        _ => true,
                    }
            }
            WorkFilter::All => true,
        }
    }

    fn work_search_matches(&self, item: &WorkItem) -> bool {
        let q = self.query(Tab::Work).trim().to_lowercase();
        q.split_whitespace().all(|word| {
            if let Some(repo) = word.strip_prefix("@repo:") {
                item.repo_name.to_lowercase().contains(repo)
            } else if let Some(state) = word.strip_prefix("@state:") {
                item.state.as_str().to_lowercase().contains(state)
                    || item.state.label().to_lowercase().contains(state)
            } else {
                item.branch.to_lowercase().contains(word)
                    || item.repo_name.to_lowercase().contains(word)
                    || item
                        .pr
                        .as_ref()
                        .is_some_and(|p| p.title.to_lowercase().contains(word))
            }
        })
    }

    /// Visible Work rows: groups by repo, newest activity first.
    pub fn work_rows(&self) -> Vec<WorkRow> {
        let searching = !self.query(Tab::Work).trim().is_empty();
        let mut items: Vec<usize> = (0..self.work_items.len())
            .filter(|&i| {
                let it = &self.work_items[i];
                self.work_filter_matches(it) && self.work_search_matches(it)
            })
            .collect();
        items.sort_by(|&a, &b| {
            let (a, b) = (&self.work_items[a], &self.work_items[b]);
            b.last_activity
                .cmp(&a.last_activity)
                .then_with(|| a.branch.cmp(&b.branch))
        });
        // Groups ordered by their newest item (items are already newest first).
        let mut groups: Vec<(String, String, Vec<usize>)> = Vec::new();
        for i in items {
            let it = &self.work_items[i];
            match groups.iter_mut().find(|g| g.0 == it.repo_id) {
                Some(g) => g.2.push(i),
                None => groups.push((it.repo_id.clone(), it.repo_name.clone(), vec![i])),
            }
        }
        let mut rows = Vec::new();
        for (repo_id, name, members) in groups {
            let collapsed = !searching && self.collapsed.contains(&repo_id);
            rows.push(WorkRow::Group {
                repo_id,
                name,
                count: members.len(),
                collapsed,
            });
            if !collapsed {
                rows.extend(members.into_iter().map(WorkRow::Item));
            }
        }
        rows
    }

    /// Number of WorkItems passing the filter and search.
    pub fn work_item_count(&self) -> usize {
        self.work_items
            .iter()
            .filter(|it| self.work_filter_matches(it) && self.work_search_matches(it))
            .count()
    }

    pub fn selected_work_row(&self) -> Option<WorkRow> {
        self.work_rows().get(self.cursor[0]).cloned()
    }

    pub fn selected_work_item(&self) -> Option<&WorkItem> {
        match self.selected_work_row()? {
            WorkRow::Item(i) => self.work_items.get(i),
            WorkRow::Group { .. } => None,
        }
    }

    /// Repo part of a todo link, `PROJECT/repo:branch` → `PROJECT/repo`.
    pub fn link_repo(link: &str) -> &str {
        link.split_once(':').map_or(link, |(r, _)| r)
    }

    /// Todos passing filter and search, in display order (grouped by repo with `G`).
    pub fn visible_todos(&self) -> Vec<usize> {
        let q = self.query(Tab::Todos).trim().to_lowercase();
        let mut out: Vec<usize> = (0..self.todos.len())
            .filter(|&i| {
                let t = &self.todos[i];
                let pass = match self.todo_filter {
                    TodoFilter::Active => t.status.is_active(),
                    TodoFilter::Done => !t.status.is_active(),
                    TodoFilter::All => true,
                };
                pass && q.split_whitespace().all(|w| {
                    t.title.to_lowercase().contains(w)
                        || t.link.as_deref().unwrap_or("").to_lowercase().contains(w)
                        || t.notes.to_lowercase().contains(w)
                })
            })
            .collect();
        if self.todo_grouped {
            out.sort_by_key(|&i| self.todo_group_name(&self.todos[i]));
        }
        out
    }

    pub fn todo_group_name(&self, t: &Todo) -> String {
        t.link
            .as_deref()
            .map(|l| Self::link_repo(l).to_string())
            .unwrap_or_else(|| "~(no link)".to_string())
    }

    /// Display rows of the Todos list, with group headers when grouped.
    pub fn todo_rows(&self) -> Vec<TodoRow> {
        let mut rows = Vec::new();
        let mut last: Option<String> = None;
        for i in self.visible_todos() {
            if self.todo_grouped {
                let g = self.todo_group_name(&self.todos[i]);
                if last.as_ref() != Some(&g) {
                    rows.push(TodoRow::Header(g.trim_start_matches('~').to_string()));
                    last = Some(g);
                }
            }
            rows.push(TodoRow::Todo(i));
        }
        rows
    }

    pub fn selected_todo(&self) -> Option<&Todo> {
        let i = *self.visible_todos().get(self.cursor[1])?;
        self.todos.get(i)
    }

    /// Sessions passing the search, newest first.
    pub fn visible_sessions(&self) -> Vec<usize> {
        let q = self.query(Tab::Sessions).trim().to_lowercase();
        let mut out: Vec<usize> = (0..self.sessions.len())
            .filter(|&i| {
                let s = &self.sessions[i];
                q.split_whitespace().all(|w| {
                    self.session_title(s).to_lowercase().contains(w)
                        || s.branches.iter().any(|b| b.to_lowercase().contains(w))
                        || s.cwd.as_deref().unwrap_or("").to_lowercase().contains(w)
                        || self
                            .session_repo_name(s)
                            .is_some_and(|r| r.to_lowercase().contains(w))
                })
            })
            .collect();
        out.sort_by(|&a, &b| {
            let t = |s: &Session| s.last_activity_at.or(s.started_at);
            t(&self.sessions[b])
                .cmp(&t(&self.sessions[a]))
                .then_with(|| self.sessions[a].id.cmp(&self.sessions[b].id))
        });
        out
    }

    pub fn selected_session(&self) -> Option<&Session> {
        let i = *self.visible_sessions().get(self.cursor[2])?;
        self.sessions.get(i)
    }

    pub fn session_title(&self, s: &Session) -> String {
        s.summary
            .as_ref()
            .map(|x| x.title.clone())
            .filter(|t| !t.trim().is_empty())
            .or_else(|| s.title.clone())
            .unwrap_or_else(|| "(untitled)".to_string())
    }

    pub fn repo_name(&self, repo_id: &str) -> Option<String> {
        self.repos
            .iter()
            .find(|r| r.id == repo_id)
            .map(Repo::display_name)
            .or_else(|| {
                self.work_items
                    .iter()
                    .find(|w| w.repo_id == repo_id)
                    .map(|w| w.repo_name.clone())
            })
    }

    pub fn session_repo_name(&self, s: &Session) -> Option<String> {
        s.links.first().and_then(|l| self.repo_name(&l.repo_id))
    }

    pub fn session_branch(&self, s: &Session) -> Option<String> {
        s.links
            .first()
            .map(|l| l.branch.clone())
            .or_else(|| s.branches.first().cloned())
    }

    /// Worktree path a session can be resumed in: the worktree of a linked branch, else the
    /// longest known worktree path containing its cwd.
    pub fn session_worktree(&self, s: &Session) -> Option<PathBuf> {
        for link in &s.links {
            if let Some(wt) = self.worktrees.iter().find(|w| {
                w.repo_id == link.repo_id && w.branch.as_deref() == Some(link.branch.as_str())
            }) {
                return Some(wt.path.clone());
            }
        }
        let cwd = Path::new(s.cwd.as_deref()?);
        self.worktrees
            .iter()
            .filter(|w| cwd.starts_with(&w.path))
            .max_by_key(|w| w.path.as_os_str().len())
            .map(|w| w.path.clone())
    }

    /// The resume command for a session, or `None` when no worktree is known.
    pub fn session_resume(&self, s: &Session) -> Option<String> {
        self.session_worktree(s)
            .map(|path| resume_command(&path, &s.id))
    }

    /// Sessions linked to a WorkItem, newest first.
    pub fn item_sessions(&self, item: &WorkItem) -> Vec<&Session> {
        let mut out: Vec<&Session> = self
            .sessions
            .iter()
            .filter(|s| {
                s.links
                    .iter()
                    .any(|l| l.repo_id == item.repo_id && l.branch == item.branch)
            })
            .collect();
        out.sort_by_key(|s| std::cmp::Reverse(s.last_activity_at.or(s.started_at)));
        out
    }

    pub fn item_todos(&self, item: &WorkItem) -> Vec<&Todo> {
        let link = item.link_ref();
        self.todos
            .iter()
            .filter(|t| t.link.as_deref() == Some(link.as_str()))
            .collect()
    }

    /// WorkItem a `PROJECT/repo:branch` link points to.
    pub fn item_for_link(&self, link: &str) -> Option<&WorkItem> {
        self.work_items.iter().find(|w| w.link_ref() == link)
    }

    pub fn jira_key(&self, text: &str) -> Option<String> {
        self.jira_re
            .as_ref()?
            .find(text)
            .map(|m| m.as_str().to_string())
    }

    pub fn jira_url(&self, key: &str) -> Option<String> {
        (!self.jira_browse_url.is_empty()).then(|| self.jira_browse_url.replace("{key}", key))
    }

    pub fn pending_suggestions(&self) -> Vec<usize> {
        let q = self.query(Tab::Inbox).trim().to_lowercase();
        let mut out: Vec<usize> = (0..self.suggestions.len())
            .filter(|&i| {
                let s = &self.suggestions[i];
                s.status == SuggestionStatus::Pending
                    && q.split_whitespace().all(|w| {
                        let (text, link) = suggestion_text(s);
                        s.rule.to_lowercase().contains(w)
                            || text.to_lowercase().contains(w)
                            || link.unwrap_or("").to_lowercase().contains(w)
                    })
            })
            .collect();
        out.sort_by(|&a, &b| {
            let (a, b) = (&self.suggestions[a], &self.suggestions[b]);
            b.created_at.cmp(&a.created_at).then(b.id.cmp(&a.id))
        });
        out
    }

    pub fn pending_count(&self) -> usize {
        self.suggestions
            .iter()
            .filter(|s| s.status == SuggestionStatus::Pending)
            .count()
    }

    pub fn selected_suggestion(&self) -> Option<&Suggestion> {
        let i = *self.pending_suggestions().get(self.cursor[4])?;
        self.suggestions.get(i)
    }

    pub fn any_session_live(&self) -> bool {
        let now = self.now_ts();
        self.sessions.iter().any(|s| s.is_live(now))
    }

    /// Entries of the link picker, best match first.
    pub fn picker_entries(&self, picker: &LinkPicker) -> Vec<PickerEntry> {
        let mut scored: Vec<(usize, usize)> = self
            .work_items
            .iter()
            .enumerate()
            .filter(|(_, w)| w.state != WorkState::Protected)
            .filter_map(|(i, w)| fuzzy_score(&picker.query.text, &w.link_ref()).map(|s| (s, i)))
            .collect();
        scored.sort_by(|a, b| {
            a.0.cmp(&b.0).then_with(|| {
                self.work_items[b.1]
                    .last_activity
                    .cmp(&self.work_items[a.1].last_activity)
            })
        });
        let mut out = Vec::new();
        if picker.has_link && picker.query.text.is_empty() {
            out.push(PickerEntry::RemoveLink);
        }
        out.extend(scored.into_iter().map(|(_, i)| PickerEntry::Item(i)));
        out
    }

    // ------------------------------------------------------------------
    // Selection bookkeeping
    // ------------------------------------------------------------------

    /// Number of cursor positions in a tab's list.
    pub fn row_count(&self, tab: Tab) -> usize {
        match tab {
            Tab::Work => self.work_rows().len(),
            Tab::Todos => self.visible_todos().len(),
            Tab::Sessions => self.visible_sessions().len(),
            Tab::Summary => 0,
            Tab::Inbox => self.pending_suggestions().len(),
        }
    }

    fn row_keys(&self, tab: Tab) -> Vec<String> {
        match tab {
            Tab::Work => self
                .work_rows()
                .into_iter()
                .map(|r| match r {
                    WorkRow::Group { repo_id, .. } => format!("g:{repo_id}"),
                    WorkRow::Item(i) => {
                        let w = &self.work_items[i];
                        format!("i:{}:{}", w.repo_id, w.branch)
                    }
                })
                .collect(),
            Tab::Todos => self
                .visible_todos()
                .into_iter()
                .map(|i| todo_key(&self.todos[i]))
                .collect(),
            Tab::Sessions => self
                .visible_sessions()
                .into_iter()
                .map(|i| self.sessions[i].id.clone())
                .collect(),
            Tab::Summary => Vec::new(),
            Tab::Inbox => self
                .pending_suggestions()
                .into_iter()
                .map(|i| self.suggestions[i].id.to_string())
                .collect(),
        }
    }

    fn selected_key(&self, tab: Tab) -> Option<String> {
        self.row_keys(tab).get(self.cursor[tab.index()]).cloned()
    }

    /// Put the cursor back on `key` after the list changed; clamp when it is gone.
    fn reselect(&mut self, tab: Tab, key: Option<String>) {
        let keys = self.row_keys(tab);
        let idx = tab.index();
        if let Some(pos) = key.and_then(|k| keys.iter().position(|x| *x == k)) {
            self.cursor[idx] = pos;
        } else {
            self.cursor[idx] = self.cursor[idx].min(keys.len().saturating_sub(1));
        }
    }

    fn move_cursor(&mut self, delta: isize) {
        let count = self.row_count(self.tab);
        let idx = self.tab.index();
        let before = self.cursor[idx];
        self.cursor[idx] = if count == 0 {
            0
        } else {
            before
                .saturating_add_signed(delta)
                .min(count.saturating_sub(1))
        };
        if self.cursor[idx] != before {
            self.details_scroll = 0;
        }
    }

    fn set_cursor(&mut self, pos: usize) {
        let count = self.row_count(self.tab);
        self.cursor[self.tab.index()] = pos.min(count.saturating_sub(1));
        self.details_scroll = 0;
    }

    fn half_page(&self) -> isize {
        (self.feedback.list_viewport.get() / 2).max(1) as isize
    }

    fn switch_tab(&mut self, tab: Tab) {
        if self.tab != tab {
            self.tab = tab;
            self.details_scroll = 0;
            self.details_open = false;
        }
    }

    fn arrangement(&self) -> Arrangement {
        self.feedback
            .arrangement
            .get()
            .unwrap_or(Arrangement::SideBySide)
    }

    fn scroll_details(&mut self, delta: isize) {
        let max = self.feedback.details_max_scroll.get();
        self.details_scroll = self.details_scroll.saturating_add_signed(delta).min(max);
    }

    fn scroll_summary(&mut self, delta: isize) {
        let max = self.feedback.summary_max_scroll.get();
        self.summary_scroll = self.summary_scroll.saturating_add_signed(delta).min(max);
    }

    fn scroll_overlay(&mut self, delta: isize) {
        let max = self.feedback.overlay_max_scroll.get();
        self.overlay_scroll = self.overlay_scroll.saturating_add_signed(delta).min(max);
    }

    // ------------------------------------------------------------------
    // Keys
    // ------------------------------------------------------------------

    pub fn handle_key(&mut self, key: KeyEvent) -> Vec<Effect> {
        if key.kind == KeyEventKind::Release {
            return Vec::new();
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return vec![Effect::Quit];
        }
        if self.feedback.too_small.get() {
            return match key.code {
                KeyCode::Char('q') => vec![Effect::Quit],
                _ => Vec::new(),
            };
        }
        if self.overlay.is_some() {
            return self.overlay_key(key);
        }
        if self.input.is_some() {
            return self.input_key(key);
        }
        if self.details_open {
            match key.code {
                KeyCode::Esc => {
                    self.details_open = false;
                    return Vec::new();
                }
                KeyCode::Char('j') | KeyCode::Down => {
                    self.scroll_details(1);
                    return Vec::new();
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    self.scroll_details(-1);
                    return Vec::new();
                }
                KeyCode::Enter if self.tab == Tab::Sessions => return self.jump_to_session_item(),
                KeyCode::Enter => return Vec::new(),
                _ => {}
            }
        }
        if let Some(effects) = self.tab_key(key) {
            return effects;
        }
        self.global_key(key)
    }

    fn global_key(&mut self, key: KeyEvent) -> Vec<Effect> {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Char('q') => return vec![Effect::Quit],
            KeyCode::Char(c @ '1'..='5') => {
                self.switch_tab(Tab::ALL[(c as u8 - b'1') as usize]);
            }
            KeyCode::Tab => self.switch_tab(self.tab.next()),
            KeyCode::BackTab => self.switch_tab(self.tab.prev()),
            KeyCode::Char('d') if ctrl => {
                let n = self.half_page();
                self.move_or_scroll(n);
            }
            KeyCode::Char('u') if ctrl => {
                let n = self.half_page();
                self.move_or_scroll(-n);
            }
            KeyCode::Char('j') | KeyCode::Down => self.move_or_scroll(1),
            KeyCode::Char('k') | KeyCode::Up => self.move_or_scroll(-1),
            KeyCode::PageDown => {
                let n = self.half_page() * 2;
                self.move_or_scroll(n);
            }
            KeyCode::PageUp => {
                let n = self.half_page() * 2;
                self.move_or_scroll(-n);
            }
            KeyCode::Char('g') | KeyCode::Home => {
                if self.tab == Tab::Summary {
                    self.summary_scroll = 0;
                } else {
                    self.set_cursor(0);
                }
            }
            KeyCode::Char('G') | KeyCode::End => {
                if self.tab == Tab::Summary {
                    self.summary_scroll = self.feedback.summary_max_scroll.get();
                } else {
                    self.set_cursor(usize::MAX);
                }
            }
            KeyCode::Char('J') => {
                if self.tab == Tab::Summary {
                    self.scroll_summary(1);
                } else {
                    self.scroll_details(1);
                }
            }
            KeyCode::Char('K') => {
                if self.tab == Tab::Summary {
                    self.scroll_summary(-1);
                } else {
                    self.scroll_details(-1);
                }
            }
            KeyCode::Char('/') => {
                if self.tab == Tab::Summary {
                    self.set_status("the summary has no search");
                } else {
                    let text = self.search[self.tab.index()].clone();
                    self.input = Some(ActiveInput {
                        purpose: InputPurpose::Search,
                        input: LineInput::with_text(&text),
                    });
                }
            }
            KeyCode::Char('f') => self.cycle_filter(),
            KeyCode::Char('v') => {
                self.layout_mode = self.layout_mode.next();
                self.details_open = false;
                self.set_status(format!("layout: {}", self.layout_mode.label()));
            }
            KeyCode::Char('r') => {
                self.set_status("reloading…");
                return vec![Effect::Command(Command::LocalReload)];
            }
            KeyCode::Char('R') => {
                self.set_status("full sync started");
                return vec![Effect::Command(Command::FullSync)];
            }
            KeyCode::Char('i') => self.open_overlay(Overlay::Diagnostics),
            KeyCode::Char('?') => self.open_overlay(Overlay::Help),
            KeyCode::Esc => {
                let idx = self.tab.index();
                if !self.search[idx].is_empty() {
                    let key = self.selected_key(self.tab);
                    self.search[idx].clear();
                    self.reselect(self.tab, key);
                }
            }
            KeyCode::Enter => {
                if self.arrangement() == Arrangement::ListOnly
                    && self.tab.has_details()
                    && self.row_count(self.tab) > 0
                {
                    self.details_open = true;
                    self.details_scroll = 0;
                }
            }
            _ => {}
        }
        Vec::new()
    }

    fn move_or_scroll(&mut self, delta: isize) {
        if self.tab == Tab::Summary {
            self.scroll_summary(delta);
        } else {
            self.move_cursor(delta);
        }
    }

    fn open_overlay(&mut self, overlay: Overlay) {
        self.overlay = Some(overlay);
        self.overlay_scroll = 0;
    }

    fn cycle_filter(&mut self) {
        let key = self.selected_key(self.tab);
        match self.tab {
            Tab::Work => {
                self.work_filter = self.work_filter.next();
                self.set_status(format!("filter: {}", self.work_filter.label()));
            }
            Tab::Todos => {
                self.todo_filter = self.todo_filter.next();
                self.set_status(format!("filter: {}", self.todo_filter.label()));
            }
            _ => self.set_status("no filters in this view"),
        }
        self.reselect(self.tab, key);
    }

    /// Keys specific to the current tab. `None` = not handled, fall through to global keys.
    fn tab_key(&mut self, key: KeyEvent) -> Option<Vec<Effect>> {
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            return None;
        }
        match self.tab {
            Tab::Work => self.work_key(key),
            Tab::Todos => self.todos_key(key),
            Tab::Sessions => self.sessions_key(key),
            Tab::Summary => self.summary_key(key),
            Tab::Inbox => self.inbox_key(key),
        }
    }

    fn work_key(&mut self, key: KeyEvent) -> Option<Vec<Effect>> {
        let row = self.selected_work_row();
        let item = self.selected_work_item().cloned();
        let none = Some(Vec::new());
        match key.code {
            KeyCode::Enter => match row {
                Some(WorkRow::Group { repo_id, .. }) => {
                    if !self.query(Tab::Work).trim().is_empty() {
                        self.set_status("groups stay open while searching");
                    } else if !self.collapsed.remove(&repo_id) {
                        self.collapsed.insert(repo_id);
                    }
                    none
                }
                _ => None,
            },
            KeyCode::Char('o') => {
                let item = item?;
                if let Some(pr) = &item.pr {
                    return Some(vec![Effect::Command(Command::OpenUrl(pr.url.clone()))]);
                }
                match self.jira_key(&item.branch).and_then(|k| self.jira_url(&k)) {
                    Some(url) => Some(vec![Effect::Command(Command::OpenUrl(url))]),
                    None => {
                        self.set_status("no PR or Jira link for this branch");
                        none
                    }
                }
            }
            KeyCode::Char('y') => {
                let item = item?;
                self.set_status(format!("copied {}", item.branch));
                Some(vec![Effect::CopyToClipboard(item.branch)])
            }
            KeyCode::Char('Y') => {
                let item = item?;
                match item.local.worktree {
                    Some(path) => {
                        let text = path.to_string_lossy().into_owned();
                        self.set_status(format!("copied {text}"));
                        Some(vec![Effect::CopyToClipboard(text)])
                    }
                    None => {
                        self.set_status("this branch has no worktree");
                        none
                    }
                }
            }
            KeyCode::Char('t') => {
                let item = item?;
                if self.refuse_todo_edit() {
                    return none;
                }
                self.input = Some(ActiveInput {
                    purpose: InputPurpose::AddTodo {
                        link: Some(item.link_ref()),
                    },
                    input: LineInput::default(),
                });
                none
            }
            KeyCode::Char('c') => {
                let item = item?;
                let latest = self.item_sessions(&item).first().map(|s| (*s).clone());
                match latest {
                    Some(s) => Some(self.copy_resume(&s)),
                    None => {
                        self.set_status("no session on this branch");
                        none
                    }
                }
            }
            KeyCode::Char('A') => {
                let item = item?;
                if item.state == WorkState::Abandoned {
                    self.set_status("already abandoned");
                    return none;
                }
                self.open_overlay(Overlay::Confirm(Confirm::Abandon {
                    repo_id: item.repo_id,
                    branch: item.branch,
                    repo_name: item.repo_name,
                }));
                none
            }
            KeyCode::Char('U') => {
                let item = item?;
                if !item.abandoned_manual {
                    self.set_status("not marked abandoned");
                    return none;
                }
                self.set_status(format!("unmarked {}", item.branch));
                Some(vec![Effect::Command(Command::SetAbandoned {
                    repo_id: item.repo_id,
                    branch: item.branch,
                    abandoned: false,
                })])
            }
            KeyCode::Char('X') => {
                let item = item?;
                self.start_remove(&item);
                none
            }
            _ => None,
        }
    }

    fn start_remove(&mut self, item: &WorkItem) {
        if !item.flags.cleanup {
            self.set_status("X only removes cleanup items (done or abandoned)");
            return;
        }
        if item.local.dirty || item.flags.dirty {
            self.set_error("worktree has uncommitted changes; refusing to remove it");
            return;
        }
        let mut commands = Vec::new();
        if let Some(path) = &item.local.worktree {
            commands.push(format!(
                "git worktree remove {}",
                shell_quote(&path.to_string_lossy())
            ));
        }
        if item.state.is_done() && item.local.local_exists {
            commands.push(format!("git branch -d {}", shell_quote(&item.branch)));
        }
        if commands.is_empty() {
            self.set_status("nothing to remove");
            return;
        }
        self.open_overlay(Overlay::Confirm(Confirm::RemoveWorktree {
            repo_id: item.repo_id.clone(),
            branch: item.branch.clone(),
            commands,
        }));
    }

    fn copy_resume(&mut self, s: &Session) -> Vec<Effect> {
        match self.session_resume(s) {
            Some(cmd) => {
                self.set_status("copied resume command");
                vec![Effect::CopyToClipboard(cmd)]
            }
            None => {
                self.set_error("no worktree known for this session; nothing copied");
                Vec::new()
            }
        }
    }

    /// Todo edits are refused while the file does not parse (spec 7.3).
    fn refuse_todo_edit(&mut self) -> bool {
        if self.todo_error.is_some() {
            self.set_error("todo file has a parse error; fix it first (O opens it)");
            true
        } else {
            false
        }
    }

    /// The selected todo's id, with a status message when it has none (yet).
    fn selected_todo_id(&mut self) -> Option<String> {
        let todo = self.selected_todo()?;
        match todo.id.clone() {
            Some(id) => Some(id),
            None => {
                self.set_status("this todo has no id yet; it gets one on the next write");
                None
            }
        }
    }

    fn todo_update(&mut self, id: String, patch: TodoPatch) -> Vec<Effect> {
        vec![Effect::Command(Command::Todo(TodoOp::Update { id, patch }))]
    }

    fn todos_key(&mut self, key: KeyEvent) -> Option<Vec<Effect>> {
        let none = Some(Vec::new());
        let today = self.today();
        match key.code {
            KeyCode::Char('O') => Some(vec![Effect::OpenEditor {
                kind: EditorTarget::WholeFile,
            }]),
            KeyCode::Char('G') => {
                let key = self.selected_key(Tab::Todos);
                self.todo_grouped = !self.todo_grouped;
                self.reselect(Tab::Todos, key);
                none
            }
            KeyCode::Char('a') => {
                if !self.refuse_todo_edit() {
                    self.input = Some(ActiveInput {
                        purpose: InputPurpose::AddTodo { link: None },
                        input: LineInput::default(),
                    });
                }
                none
            }
            KeyCode::Char('e' | 'E' | ' ' | 's' | 'p' | 'L' | '+' | '-' | 'D' | 'd') => {
                if self.selected_todo().is_none() || self.refuse_todo_edit() {
                    return none;
                }
                let Some(id) = self.selected_todo_id() else {
                    return none;
                };
                let todo = self.selected_todo()?.clone();
                let effects = match key.code {
                    KeyCode::Char('e') => {
                        self.input = Some(ActiveInput {
                            purpose: InputPurpose::EditTodo { id },
                            input: LineInput::with_text(&todo.title),
                        });
                        Vec::new()
                    }
                    KeyCode::Char('E') => vec![Effect::OpenEditor {
                        kind: EditorTarget::NotesOf(id),
                    }],
                    KeyCode::Char(' ') => {
                        let patch = if todo.status == TodoStatus::Done {
                            TodoPatch {
                                status: Some(TodoStatus::Todo),
                                done: Some(None),
                                ..Default::default()
                            }
                        } else {
                            TodoPatch {
                                status: Some(TodoStatus::Done),
                                done: Some(Some(today)),
                                ..Default::default()
                            }
                        };
                        self.todo_update(id, patch)
                    }
                    KeyCode::Char('s') => {
                        let next = match todo.status {
                            TodoStatus::Todo => TodoStatus::Doing,
                            TodoStatus::Doing => TodoStatus::Done,
                            TodoStatus::Done | TodoStatus::Dropped => TodoStatus::Todo,
                        };
                        let done = if next == TodoStatus::Done {
                            Some(Some(today))
                        } else if todo.done.is_some() {
                            Some(None)
                        } else {
                            None
                        };
                        self.todo_update(
                            id,
                            TodoPatch {
                                status: Some(next),
                                done,
                                ..Default::default()
                            },
                        )
                    }
                    KeyCode::Char('p') => {
                        let next = match todo.priority {
                            Some(Priority::A) => Some(Priority::B),
                            Some(Priority::B) => Some(Priority::C),
                            Some(Priority::C) => None,
                            None => Some(Priority::A),
                        };
                        self.todo_update(
                            id,
                            TodoPatch {
                                priority: Some(next),
                                ..Default::default()
                            },
                        )
                    }
                    KeyCode::Char('D') => {
                        let patch = if todo.status == TodoStatus::Dropped {
                            TodoPatch {
                                status: Some(TodoStatus::Todo),
                                done: Some(None),
                                ..Default::default()
                            }
                        } else {
                            TodoPatch {
                                status: Some(TodoStatus::Dropped),
                                done: Some(Some(today)),
                                ..Default::default()
                            }
                        };
                        self.todo_update(id, patch)
                    }
                    KeyCode::Char('d') => {
                        self.open_overlay(Overlay::Confirm(Confirm::DeleteTodo {
                            id,
                            title: todo.title,
                        }));
                        Vec::new()
                    }
                    KeyCode::Char('L') => {
                        self.open_overlay(Overlay::LinkPicker(LinkPicker {
                            todo_id: id,
                            query: LineInput::default(),
                            cursor: 0,
                            has_link: todo.link.is_some(),
                        }));
                        Vec::new()
                    }
                    KeyCode::Char('+') => self.move_todo(&id, true),
                    KeyCode::Char('-') => self.move_todo(&id, false),
                    _ => Vec::new(),
                };
                Some(effects)
            }
            _ => None,
        }
    }

    /// `+`/`-`: move an Active todo one place up/down in file order (spec 7.3 `Move`).
    fn move_todo(&mut self, id: &str, up: bool) -> Vec<Effect> {
        let active: Vec<&Todo> = self.todos.iter().filter(|t| t.status.is_active()).collect();
        let Some(pos) = active.iter().position(|t| t.id.as_deref() == Some(id)) else {
            self.set_status("only active todos can be moved");
            return Vec::new();
        };
        let after = if up {
            match pos {
                0 => return Vec::new(),
                1 => None,
                p => active[p - 2].id.clone(),
            }
        } else {
            match active.get(pos + 1) {
                Some(next) => next.id.clone(),
                None => return Vec::new(),
            }
        };
        if after.is_none() && !(up && pos == 1) {
            self.set_status("neighbour has no id yet; try again after the next write");
            return Vec::new();
        }
        // Keep the cursor on the moved item once the reload arrives.
        let delta = if up { -1 } else { 1 };
        if self.todo_filter == TodoFilter::Active && !self.todo_grouped {
            self.move_cursor(delta);
        }
        vec![Effect::Command(Command::Todo(TodoOp::Move {
            id: id.to_string(),
            after,
        }))]
    }

    fn sessions_key(&mut self, key: KeyEvent) -> Option<Vec<Effect>> {
        match key.code {
            KeyCode::Char('c') => {
                let s = self.selected_session()?.clone();
                Some(self.copy_resume(&s))
            }
            KeyCode::Char('s') => {
                let s = self.selected_session()?;
                let id = s.id.clone();
                self.set_status("summary queued");
                Some(vec![Effect::Command(Command::Resummarize {
                    session_id: id,
                })])
            }
            KeyCode::Enter if self.arrangement() != Arrangement::ListOnly => {
                Some(self.jump_to_session_item())
            }
            _ => None,
        }
    }

    /// Enter in Sessions: select the session's linked WorkItem in the Work tab.
    fn jump_to_session_item(&mut self) -> Vec<Effect> {
        let Some(s) = self.selected_session() else {
            return Vec::new();
        };
        let Some(link) = s.links.first().cloned() else {
            self.set_status("this session is not linked to a branch");
            return Vec::new();
        };
        if !self
            .work_items
            .iter()
            .any(|w| w.repo_id == link.repo_id && w.branch == link.branch)
        {
            self.set_status(format!("{} is not a tracked work item", link.branch));
            return Vec::new();
        }
        self.switch_tab(Tab::Work);
        self.search[0].clear();
        self.collapsed.remove(&link.repo_id);
        let key = format!("i:{}:{}", link.repo_id, link.branch);
        if !self.row_keys(Tab::Work).contains(&key) {
            self.work_filter = WorkFilter::All;
        }
        self.reselect(Tab::Work, Some(key));
        Vec::new()
    }

    fn summary_key(&mut self, key: KeyEvent) -> Option<Vec<Effect>> {
        let since = self
            .summary_since
            .unwrap_or_else(|| previous_workday(self.today(), &self.workdays));
        match key.code {
            KeyCode::Char('[') => {
                let new = previous_workday(since, &self.workdays);
                Some(self.request_summary(new))
            }
            KeyCode::Char(']') => {
                let new = next_workday(since, &self.workdays);
                if new > self.today() {
                    self.set_status("the window cannot start after today");
                    return Some(Vec::new());
                }
                Some(self.request_summary(new))
            }
            KeyCode::Char('y') => match &self.summary_markdown {
                Some(md) => {
                    let md = md.clone();
                    self.set_status("copied summary Markdown");
                    Some(vec![Effect::CopyToClipboard(md)])
                }
                None => {
                    self.set_status("the summary is not built yet");
                    Some(Vec::new())
                }
            },
            KeyCode::Char('w') => Some(vec![Effect::Command(Command::WriteSummary { since })]),
            _ => None,
        }
    }

    fn request_summary(&mut self, since: NaiveDate) -> Vec<Effect> {
        self.summary_since = Some(since);
        self.summary_scroll = 0;
        vec![Effect::Command(Command::Summary { since })]
    }

    fn inbox_key(&mut self, key: KeyEvent) -> Option<Vec<Effect>> {
        let none = Some(Vec::new());
        match key.code {
            KeyCode::Char('A') | KeyCode::Char('X') => {
                let count = self.pending_count();
                if count == 0 {
                    self.set_status("the inbox is empty");
                    return none;
                }
                let confirm = if key.code == KeyCode::Char('A') {
                    Confirm::AcceptAll { count }
                } else {
                    Confirm::RejectAll { count }
                };
                self.open_overlay(Overlay::Confirm(confirm));
                none
            }
            KeyCode::Enter | KeyCode::Char('x' | 'e' | 'k' | 'b') => {
                let Some(s) = self.selected_suggestion().cloned() else {
                    // `k` still moves up on an empty list (harmless); nothing to act on.
                    return none;
                };
                let choice = needs_choice(&s);
                let effects = match key.code {
                    KeyCode::Enter if choice => {
                        self.set_status("choose k keep or b abandon");
                        Vec::new()
                    }
                    KeyCode::Enter => vec![Effect::Command(Command::AcceptSuggestion {
                        id: s.id,
                        edited_title: None,
                    })],
                    KeyCode::Char('x') => {
                        vec![Effect::Command(Command::RejectSuggestion { id: s.id })]
                    }
                    KeyCode::Char('e') => {
                        if let SuggestionPayload::AddTodo { title, .. } = &s.payload {
                            self.input = Some(ActiveInput {
                                purpose: InputPurpose::EditSuggestion { id: s.id },
                                input: LineInput::with_text(title),
                            });
                        } else {
                            self.set_status("only todo suggestions can be edited");
                        }
                        Vec::new()
                    }
                    KeyCode::Char(c @ ('k' | 'b')) => {
                        if choice {
                            vec![Effect::Command(Command::DecideSuggestion {
                                id: s.id,
                                abandon: c == 'b',
                            })]
                        } else {
                            self.set_status("k / b are for keep-or-abandon suggestions");
                            Vec::new()
                        }
                    }
                    _ => Vec::new(),
                };
                Some(effects)
            }
            _ => None,
        }
    }

    fn input_key(&mut self, key: KeyEvent) -> Vec<Effect> {
        let Some(mut active) = self.input.take() else {
            return Vec::new();
        };
        match key.code {
            KeyCode::Esc => {
                if active.purpose == InputPurpose::Search {
                    let tab_key = self.selected_key(self.tab);
                    self.search[self.tab.index()].clear();
                    self.reselect(self.tab, tab_key);
                }
                Vec::new()
            }
            KeyCode::Enter => self.submit_input(active),
            KeyCode::Down | KeyCode::Up if active.purpose == InputPurpose::Search => {
                // Leave search mode and move, like LGBC.
                self.move_cursor(if key.code == KeyCode::Down { 1 } else { -1 });
                Vec::new()
            }
            _ => {
                active.input.edit(key);
                if active.purpose == InputPurpose::Search {
                    let idx = self.tab.index();
                    if self.search[idx] != active.input.text {
                        self.search[idx] = active.input.text.clone();
                        self.cursor[idx] = 0;
                        self.details_scroll = 0;
                    }
                }
                self.input = Some(active);
                Vec::new()
            }
        }
    }

    fn submit_input(&mut self, active: ActiveInput) -> Vec<Effect> {
        let text = active.input.text.trim().to_string();
        match active.purpose {
            InputPurpose::Search => Vec::new(),
            _ if text.is_empty() => {
                self.set_status("empty text; nothing saved");
                Vec::new()
            }
            InputPurpose::AddTodo { link } => {
                let mut todo = Todo::new(text);
                todo.link = link;
                todo.created = Some(self.today());
                todo.source = Some("manual".into());
                self.set_status("todo added");
                vec![Effect::Command(Command::Todo(TodoOp::Add(todo)))]
            }
            InputPurpose::EditTodo { id } => self.todo_update(
                id,
                TodoPatch {
                    title: Some(text),
                    ..Default::default()
                },
            ),
            InputPurpose::EditSuggestion { id } => {
                vec![Effect::Command(Command::AcceptSuggestion {
                    id,
                    edited_title: Some(text),
                })]
            }
        }
    }

    fn overlay_key(&mut self, key: KeyEvent) -> Vec<Effect> {
        let Some(overlay) = self.overlay.take() else {
            return Vec::new();
        };
        match overlay {
            Overlay::Help | Overlay::Diagnostics => {
                match key.code {
                    KeyCode::Char('j') | KeyCode::Down => self.scroll_overlay(1),
                    KeyCode::Char('k') | KeyCode::Up => self.scroll_overlay(-1),
                    KeyCode::PageDown | KeyCode::Char(' ') => self.scroll_overlay(10),
                    KeyCode::PageUp => self.scroll_overlay(-10),
                    KeyCode::Char('q') => return vec![Effect::Quit],
                    _ => {
                        self.overlay_scroll = 0;
                        return Vec::new();
                    }
                }
                self.overlay = Some(overlay);
                Vec::new()
            }
            Overlay::Confirm(confirm) => match key.code {
                KeyCode::Char('y') | KeyCode::Enter => self.confirmed(confirm),
                KeyCode::Char('j') | KeyCode::Down => {
                    self.scroll_overlay(1);
                    self.overlay = Some(Overlay::Confirm(confirm));
                    Vec::new()
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    self.scroll_overlay(-1);
                    self.overlay = Some(Overlay::Confirm(confirm));
                    Vec::new()
                }
                KeyCode::Char('n') | KeyCode::Esc | KeyCode::Char('q') => {
                    self.set_status("cancelled");
                    Vec::new()
                }
                _ => {
                    self.overlay = Some(Overlay::Confirm(confirm));
                    Vec::new()
                }
            },
            Overlay::LinkPicker(mut picker) => {
                let count = self.picker_entries(&picker).len();
                match key.code {
                    KeyCode::Esc => return Vec::new(),
                    KeyCode::Enter => {
                        let entry = self.picker_entries(&picker).get(picker.cursor).cloned();
                        let link = match entry {
                            None => {
                                self.overlay = Some(Overlay::LinkPicker(picker));
                                return Vec::new();
                            }
                            Some(PickerEntry::RemoveLink) => None,
                            Some(PickerEntry::Item(i)) => Some(self.work_items[i].link_ref()),
                        };
                        self.set_status(match &link {
                            Some(l) => format!("linked to {l}"),
                            None => "link removed".to_string(),
                        });
                        return self.todo_update(
                            picker.todo_id,
                            TodoPatch {
                                link: Some(link),
                                ..Default::default()
                            },
                        );
                    }
                    KeyCode::Down => {
                        picker.cursor = (picker.cursor + 1).min(count.saturating_sub(1))
                    }
                    KeyCode::Char('n') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        picker.cursor = (picker.cursor + 1).min(count.saturating_sub(1))
                    }
                    KeyCode::Up => picker.cursor = picker.cursor.saturating_sub(1),
                    KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        picker.cursor = picker.cursor.saturating_sub(1)
                    }
                    _ => {
                        if picker.query.edit(key) {
                            picker.cursor = 0;
                        }
                    }
                }
                self.overlay = Some(Overlay::LinkPicker(picker));
                Vec::new()
            }
        }
    }

    fn confirmed(&mut self, confirm: Confirm) -> Vec<Effect> {
        self.overlay_scroll = 0;
        let cmd = match confirm {
            Confirm::Abandon {
                repo_id, branch, ..
            } => {
                self.set_status(format!("marked {branch} abandoned"));
                Command::SetAbandoned {
                    repo_id,
                    branch,
                    abandoned: true,
                }
            }
            Confirm::RemoveWorktree {
                repo_id, branch, ..
            } => {
                self.set_status(format!("removing {branch}…"));
                Command::RemoveWorktree { repo_id, branch }
            }
            Confirm::DeleteTodo { id, .. } => {
                self.set_status("todo deleted");
                Command::Todo(TodoOp::Delete { id })
            }
            Confirm::AcceptAll { .. } => Command::AcceptAll,
            Confirm::RejectAll { .. } => Command::RejectAll,
        };
        vec![Effect::Command(cmd)]
    }

    // ------------------------------------------------------------------
    // Mouse
    // ------------------------------------------------------------------

    /// Mouse wheel scrolls whatever is under the pointer; an overlay takes every wheel event.
    pub fn handle_mouse(&mut self, mouse: MouseEvent) -> Vec<Effect> {
        if self.feedback.too_small.get() {
            return Vec::new();
        }
        let delta = match mouse.kind {
            MouseEventKind::ScrollDown => WHEEL_ROWS,
            MouseEventKind::ScrollUp => -WHEEL_ROWS,
            _ => return Vec::new(),
        };
        let pos = Position::new(mouse.column, mouse.row);
        if self.overlay.is_some() {
            if let Some(Overlay::LinkPicker(p)) = &mut self.overlay {
                p.cursor = p.cursor.saturating_add_signed(delta.signum());
            } else {
                self.scroll_overlay(delta);
            }
            return Vec::new();
        }
        if self.tab == Tab::Summary {
            self.scroll_summary(delta);
        } else if self
            .feedback
            .details_area
            .get()
            .is_some_and(|a| a.contains(pos))
        {
            self.scroll_details(delta);
        } else if self.feedback.list_area.get().contains(pos) {
            if matches!(&self.input, Some(a) if a.purpose == InputPurpose::Search) {
                self.input = None;
            }
            self.move_cursor(delta);
        }
        Vec::new()
    }
}

fn todo_key(t: &Todo) -> String {
    t.id.clone().unwrap_or_else(|| format!("title:{}", t.title))
}

/// Display text and link of a suggestion.
// TODO(integration): once `SuggestionPayload::text()/link()` land, delegate to them.
#[allow(unreachable_patterns)]
pub fn suggestion_text(s: &Suggestion) -> (&str, Option<&str>) {
    match &s.payload {
        SuggestionPayload::AddTodo { title, link, .. } => (title.as_str(), link.as_deref()),
        SuggestionPayload::KeepOrAbandon { link, message, .. } => {
            (message.as_str(), Some(link.as_str()))
        }
        _ => ("(suggestion)", None),
    }
}

/// Whether a suggestion needs a keep / abandon choice (`?`) instead of a plain accept (`+`).
pub fn needs_choice(s: &Suggestion) -> bool {
    matches!(s.payload, SuggestionPayload::KeepOrAbandon { .. })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::sample;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ch(c: char) -> KeyEvent {
        key(KeyCode::Char(c))
    }

    fn press(app: &mut App, keys: &str) -> Vec<Effect> {
        let mut out = Vec::new();
        for c in keys.chars() {
            out.extend(app.handle_key(ch(c)));
        }
        out
    }

    fn commands(effects: Vec<Effect>) -> Vec<Command> {
        effects
            .into_iter()
            .filter_map(|e| match e {
                Effect::Command(c) => Some(c),
                _ => None,
            })
            .collect()
    }

    fn todo_ops(effects: Vec<Effect>) -> Vec<TodoOp> {
        commands(effects)
            .into_iter()
            .filter_map(|c| match c {
                Command::Todo(op) => Some(op),
                _ => None,
            })
            .collect()
    }

    fn select_todo(app: &mut App, id: &str) {
        let pos = app
            .visible_todos()
            .iter()
            .position(|&i| app.todos[i].id.as_deref() == Some(id))
            .unwrap();
        app.cursor[1] = pos;
    }

    #[test]
    fn shell_quote_escapes_single_quotes() {
        assert_eq!(shell_quote("/a b"), "'/a b'");
        assert_eq!(shell_quote("it's"), "'it'\\''s'");
        assert_eq!(
            resume_command(Path::new("/w/it's here"), "abc-123"),
            "cd '/w/it'\\''s here' && claude --resume abc-123"
        );
        assert_eq!(
            resume_command(Path::new("/w"), "x;rm -rf"),
            "cd '/w' && claude --resume 'x;rm -rf'"
        );
    }

    #[test]
    fn digits_tab_and_backtab_switch_tabs() {
        let mut app = sample::app();
        press(&mut app, "3");
        assert_eq!(app.tab, Tab::Sessions);
        app.handle_key(key(KeyCode::Tab));
        assert_eq!(app.tab, Tab::Summary);
        app.handle_key(key(KeyCode::Tab));
        app.handle_key(key(KeyCode::Tab));
        assert_eq!(app.tab, Tab::Work);
        app.handle_key(key(KeyCode::BackTab));
        assert_eq!(app.tab, Tab::Inbox);
        press(&mut app, "1");
        assert_eq!(app.tab, Tab::Work);
    }

    #[test]
    fn q_quits_and_ctrl_c_quits() {
        let mut app = sample::app();
        assert!(matches!(press(&mut app, "q").as_slice(), [Effect::Quit]));
        let e = app.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert!(matches!(e.as_slice(), [Effect::Quit]));
    }

    #[test]
    fn f_cycles_work_and_todo_filters() {
        let mut app = sample::app();
        let seen: Vec<WorkFilter> = (0..5)
            .map(|_| {
                press(&mut app, "f");
                app.work_filter
            })
            .collect();
        assert_eq!(
            seen,
            vec![
                WorkFilter::NeedsAttention,
                WorkFilter::Cleanup,
                WorkFilter::Done,
                WorkFilter::All,
                WorkFilter::Unfinished
            ]
        );
        press(&mut app, "2f");
        assert_eq!(app.todo_filter, TodoFilter::Done);
        press(&mut app, "ff");
        assert_eq!(app.todo_filter, TodoFilter::Active);
    }

    #[test]
    fn filters_select_the_right_items() {
        let mut app = sample::app();
        let branches = |app: &App| -> Vec<String> {
            app.work_rows()
                .into_iter()
                .filter_map(|r| match r {
                    WorkRow::Item(i) => Some(app.work_items[i].branch.clone()),
                    _ => None,
                })
                .collect()
        };
        let unfinished = branches(&app);
        assert!(unfinished.contains(&"feature/PROJ-123-retry".to_string()));
        assert!(!unfinished.contains(&"bugfix/flaky".to_string()));
        assert!(!unfinished.iter().any(|b| b == "main"));
        app.work_filter = WorkFilter::NeedsAttention;
        let attention = branches(&app);
        assert!(attention.contains(&"feature/PROJ-123-retry".to_string()));
        assert!(attention.contains(&"spike/s3-multipart".to_string()));
        assert!(!attention.contains(&"chore/bump-deps".to_string()));
        app.work_filter = WorkFilter::Cleanup;
        assert_eq!(branches(&app), vec!["bugfix/flaky".to_string()]);
    }

    #[test]
    fn search_with_prefixes_narrows_the_work_list() {
        let mut app = sample::app();
        press(&mut app, "/@repo:auth");
        assert!(app.input.is_some());
        let rows = app.work_rows();
        assert!(rows.iter().all(|r| match r {
            WorkRow::Group { name, .. } => name.contains("auth"),
            WorkRow::Item(i) => app.work_items[*i].repo_name.contains("auth"),
        }));
        app.handle_key(key(KeyCode::Enter));
        assert!(app.input.is_none());
        assert_eq!(app.search[0], "@repo:auth");

        app.search[0] = "@state:needs".into();
        let items: Vec<_> = app
            .work_rows()
            .into_iter()
            .filter_map(|r| match r {
                WorkRow::Item(i) => Some(app.work_items[i].state),
                _ => None,
            })
            .collect();
        assert_eq!(items, vec![WorkState::PrNeedsWork]);

        app.search[0] = "chunk".into();
        assert_eq!(app.work_item_count(), 1);

        // Esc clears the search.
        app.handle_key(key(KeyCode::Esc));
        assert!(app.search[0].is_empty());
    }

    #[test]
    fn esc_while_typing_a_search_clears_it() {
        let mut app = sample::app();
        press(&mut app, "/retry");
        assert_eq!(app.search[0], "retry");
        app.handle_key(key(KeyCode::Esc));
        assert!(app.search[0].is_empty());
        assert!(app.input.is_none());
    }

    #[test]
    fn enter_on_a_group_collapses_it() {
        let mut app = sample::app();
        app.cursor[0] = 0;
        let before = app.work_rows().len();
        app.handle_key(key(KeyCode::Enter));
        let after = app.work_rows().len();
        assert!(after < before);
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(app.work_rows().len(), before);
    }

    #[test]
    fn adding_a_todo_emits_add() {
        let mut app = sample::app();
        press(&mut app, "2a");
        press(&mut app, "Buy milk");
        let ops = todo_ops(app.handle_key(key(KeyCode::Enter)));
        match ops.as_slice() {
            [TodoOp::Add(t)] => {
                assert_eq!(t.title, "Buy milk");
                assert_eq!(t.link, None);
                assert_eq!(t.status, TodoStatus::Todo);
                assert_eq!(t.source.as_deref(), Some("manual"));
            }
            other => panic!("{other:?}"),
        }
        assert!(app.input.is_none());
    }

    #[test]
    fn esc_cancels_inline_input() {
        let mut app = sample::app();
        press(&mut app, "2a");
        press(&mut app, "nope");
        assert!(app.handle_key(key(KeyCode::Esc)).is_empty());
        assert!(app.input.is_none());
    }

    #[test]
    fn t_in_work_adds_a_linked_todo() {
        let mut app = sample::app();
        app.cursor[0] = 1; // first item of the first group
        let link = app.selected_work_item().unwrap().link_ref();
        press(&mut app, "tFollow up");
        let ops = todo_ops(app.handle_key(key(KeyCode::Enter)));
        match ops.as_slice() {
            [TodoOp::Add(t)] => assert_eq!(t.link.as_deref(), Some(link.as_str())),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn editing_a_todo_title_emits_update() {
        let mut app = sample::app();
        press(&mut app, "2");
        select_todo(&mut app, "t_2");
        press(&mut app, "e");
        app.handle_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
        press(&mut app, "New title");
        let ops = todo_ops(app.handle_key(key(KeyCode::Enter)));
        assert_eq!(
            ops,
            vec![TodoOp::Update {
                id: "t_2".into(),
                patch: TodoPatch {
                    title: Some("New title".into()),
                    ..Default::default()
                }
            }]
        );
    }

    #[test]
    fn todo_single_keys_emit_the_right_ops() {
        let mut app = sample::app();
        let today = app.today();
        press(&mut app, "2");
        select_todo(&mut app, "t_2");
        // t_2 is status todo, priority B
        assert_eq!(
            todo_ops(press(&mut app, " ")),
            vec![TodoOp::Update {
                id: "t_2".into(),
                patch: TodoPatch {
                    status: Some(TodoStatus::Done),
                    done: Some(Some(today)),
                    ..Default::default()
                }
            }]
        );
        assert_eq!(
            todo_ops(press(&mut app, "s")),
            vec![TodoOp::Update {
                id: "t_2".into(),
                patch: TodoPatch {
                    status: Some(TodoStatus::Doing),
                    ..Default::default()
                }
            }]
        );
        assert_eq!(
            todo_ops(press(&mut app, "p")),
            vec![TodoOp::Update {
                id: "t_2".into(),
                patch: TodoPatch {
                    priority: Some(Some(Priority::C)),
                    ..Default::default()
                }
            }]
        );
        assert_eq!(
            todo_ops(press(&mut app, "D")),
            vec![TodoOp::Update {
                id: "t_2".into(),
                patch: TodoPatch {
                    status: Some(TodoStatus::Dropped),
                    done: Some(Some(today)),
                    ..Default::default()
                }
            }]
        );
        assert!(matches!(
            press(&mut app, "E").as_slice(),
            [Effect::OpenEditor { kind: EditorTarget::NotesOf(id) }] if id == "t_2"
        ));
        assert!(matches!(
            press(&mut app, "O").as_slice(),
            [Effect::OpenEditor {
                kind: EditorTarget::WholeFile
            }]
        ));
    }

    #[test]
    fn delete_needs_confirmation() {
        let mut app = sample::app();
        press(&mut app, "2");
        select_todo(&mut app, "t_2");
        assert!(press(&mut app, "d").is_empty());
        assert!(matches!(
            app.overlay,
            Some(Overlay::Confirm(Confirm::DeleteTodo { .. }))
        ));
        // n cancels
        assert!(press(&mut app, "n").is_empty());
        assert!(app.overlay.is_none());
        press(&mut app, "d");
        assert_eq!(
            todo_ops(press(&mut app, "y")),
            vec![TodoOp::Delete { id: "t_2".into() }]
        );
    }

    #[test]
    fn plus_and_minus_move_within_active() {
        let mut app = sample::app();
        press(&mut app, "2");
        // Active order: t_1, t_2, t_3
        select_todo(&mut app, "t_3");
        assert_eq!(
            todo_ops(press(&mut app, "+")),
            vec![TodoOp::Move {
                id: "t_3".into(),
                after: Some("t_1".into())
            }]
        );
        select_todo(&mut app, "t_2");
        assert_eq!(
            todo_ops(press(&mut app, "+")),
            vec![TodoOp::Move {
                id: "t_2".into(),
                after: None
            }]
        );
        select_todo(&mut app, "t_1");
        assert_eq!(
            todo_ops(press(&mut app, "-")),
            vec![TodoOp::Move {
                id: "t_1".into(),
                after: Some("t_2".into())
            }]
        );
        select_todo(&mut app, "t_1");
        assert!(todo_ops(press(&mut app, "+")).is_empty());
    }

    #[test]
    fn link_picker_sets_the_link() {
        let mut app = sample::app();
        press(&mut app, "2");
        select_todo(&mut app, "t_3");
        press(&mut app, "L");
        assert!(matches!(app.overlay, Some(Overlay::LinkPicker(_))));
        press(&mut app, "chunk");
        let ops = todo_ops(app.handle_key(key(KeyCode::Enter)));
        assert_eq!(
            ops,
            vec![TodoOp::Update {
                id: "t_3".into(),
                patch: TodoPatch {
                    link: Some(Some("API/upload-svc:feature/PROJ-130-chunking".into())),
                    ..Default::default()
                }
            }]
        );
    }

    #[test]
    fn parse_error_blocks_todo_edits() {
        let mut app = sample::app();
        app.handle_event(AppEvent::TodosReloaded(Err("line 3: bad".into())));
        assert_eq!(app.todos.len(), sample::app().todos.len());
        press(&mut app, "2");
        assert!(press(&mut app, "s").is_empty());
        assert!(press(&mut app, "a").is_empty());
        assert!(app.input.is_none());
        assert!(app.status.as_ref().unwrap().text.contains("parse error"));
    }

    #[test]
    fn inbox_actions_emit_commands() {
        let mut app = sample::app();
        press(&mut app, "5");
        // Newest first: id 4 (pr_needs_work), 3 (session_unfinished), 2 (pr_declined), 1
        assert_eq!(app.selected_suggestion().unwrap().id, 4);
        let c = commands(app.handle_key(key(KeyCode::Enter)));
        assert!(matches!(
            c.as_slice(),
            [Command::AcceptSuggestion {
                id: 4,
                edited_title: None
            }]
        ));
        let c = commands(press(&mut app, "x"));
        assert!(matches!(
            c.as_slice(),
            [Command::RejectSuggestion { id: 4 }]
        ));

        // e edits then accepts
        press(&mut app, "e");
        app.handle_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
        press(&mut app, "Fix review");
        let c = commands(app.handle_key(key(KeyCode::Enter)));
        assert!(matches!(
            c.as_slice(),
            [Command::AcceptSuggestion { id: 4, edited_title: Some(t) }] if t == "Fix review"
        ));

        // keep / abandon on the choice suggestion
        app.handle_key(key(KeyCode::Down));
        app.handle_key(key(KeyCode::Down));
        assert_eq!(app.selected_suggestion().unwrap().id, 2);
        assert!(commands(app.handle_key(key(KeyCode::Enter))).is_empty());
        let c = commands(press(&mut app, "k"));
        assert!(matches!(
            c.as_slice(),
            [Command::DecideSuggestion {
                id: 2,
                abandon: false
            }]
        ));
        let c = commands(press(&mut app, "b"));
        assert!(matches!(
            c.as_slice(),
            [Command::DecideSuggestion {
                id: 2,
                abandon: true
            }]
        ));

        // A / X ask first
        assert!(press(&mut app, "A").is_empty());
        let c = commands(press(&mut app, "y"));
        assert!(matches!(c.as_slice(), [Command::AcceptAll]));
        press(&mut app, "X");
        let c = commands(press(&mut app, "y"));
        assert!(matches!(c.as_slice(), [Command::RejectAll]));
    }

    #[test]
    fn session_c_copies_a_quoted_resume_command() {
        let mut app = sample::app();
        press(&mut app, "3");
        assert_eq!(app.selected_session().unwrap().id, "s-live");
        let e = press(&mut app, "c");
        match e.as_slice() {
            [Effect::CopyToClipboard(cmd)] => assert_eq!(
                cmd,
                "cd '/home/dev/work/upload-svc-wt/retry' && claude --resume s-live"
            ),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn resume_without_a_worktree_copies_nothing() {
        let mut app = sample::app();
        press(&mut app, "3");
        let pos = app
            .visible_sessions()
            .iter()
            .position(|&i| app.sessions[i].id == "s-nobranch")
            .unwrap();
        app.cursor[2] = pos;
        assert!(press(&mut app, "c").is_empty());
        assert!(app
            .status
            .as_ref()
            .unwrap()
            .text
            .contains("no worktree known"));
    }

    #[test]
    fn work_c_copies_the_latest_session_resume() {
        let mut app = sample::app();
        app.cursor[0] = 1;
        assert_eq!(
            app.selected_work_item().unwrap().branch,
            "feature/PROJ-123-retry"
        );
        match press(&mut app, "c").as_slice() {
            [Effect::CopyToClipboard(cmd)] => assert!(cmd.ends_with("--resume s-live")),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn sessions_enter_jumps_to_the_work_item() {
        let mut app = sample::app();
        press(&mut app, "3");
        let pos = app
            .visible_sessions()
            .iter()
            .position(|&i| app.sessions[i].id == "s-sso")
            .unwrap();
        app.cursor[2] = pos;
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(app.tab, Tab::Work);
        assert_eq!(
            app.selected_work_item().unwrap().branch,
            "feature/PROJ-118-sso"
        );
    }

    #[test]
    fn work_o_opens_pr_then_jira() {
        let mut app = sample::app();
        app.cursor[0] = 1;
        let c = commands(press(&mut app, "o"));
        assert!(matches!(c.as_slice(), [Command::OpenUrl(u)] if u.ends_with("/pull-requests/44")));
        // spike/s3-multipart has no PR and no Jira key
        app.search[0] = "spike".into();
        app.cursor[0] = 1;
        assert!(commands(press(&mut app, "o")).is_empty());
        // chunking has PR; find an item with Jira key and no PR
        app.search[0] = "PROJ-140".into();
        app.cursor[0] = 1;
        let c = commands(press(&mut app, "o"));
        assert!(
            matches!(c.as_slice(), [Command::OpenUrl(u)] if u == "https://jira.example.com/browse/PROJ-140")
        );
    }

    #[test]
    fn work_x_on_cleanup_lists_both_commands() {
        let mut app = sample::app();
        app.work_filter = WorkFilter::Cleanup;
        app.cursor[0] = 1;
        press(&mut app, "X");
        match &app.overlay {
            Some(Overlay::Confirm(Confirm::RemoveWorktree { commands, .. })) => {
                assert_eq!(
                    commands,
                    &vec![
                        "git worktree remove '/home/dev/work/auth-wt/flaky'".to_string(),
                        "git branch -d 'bugfix/flaky'".to_string()
                    ]
                );
            }
            other => panic!("{other:?}"),
        }
        let c = commands(press(&mut app, "y"));
        assert!(
            matches!(c.as_slice(), [Command::RemoveWorktree { branch, .. }] if branch == "bugfix/flaky")
        );
    }

    #[test]
    fn work_x_refuses_non_cleanup_and_dirty() {
        let mut app = sample::app();
        app.cursor[0] = 1;
        press(&mut app, "X");
        assert!(app.overlay.is_none());
        app.work_filter = WorkFilter::Cleanup;
        let i = app
            .work_items
            .iter()
            .position(|w| w.branch == "bugfix/flaky")
            .unwrap();
        app.work_items[i].local.dirty = true;
        app.cursor[0] = 1;
        press(&mut app, "X");
        assert!(app.overlay.is_none());
        assert_eq!(app.status.as_ref().unwrap().kind, StatusKind::Error);
    }

    #[test]
    fn abandon_confirms_and_unmark_needs_a_mark() {
        let mut app = sample::app();
        app.cursor[0] = 1;
        press(&mut app, "A");
        let c = commands(press(&mut app, "y"));
        assert!(matches!(
            c.as_slice(),
            [Command::SetAbandoned {
                abandoned: true,
                ..
            }]
        ));
        assert!(commands(press(&mut app, "U")).is_empty());
    }

    #[test]
    fn summary_brackets_shift_by_workday() {
        let mut app = sample::app();
        press(&mut app, "4");
        // now = Wed 2026-11-25; since = Tue 2026-11-24
        assert_eq!(app.summary_since, NaiveDate::from_ymd_opt(2026, 11, 24));
        let c = commands(press(&mut app, "[["));
        let mon = NaiveDate::from_ymd_opt(2026, 11, 23).unwrap();
        let fri = NaiveDate::from_ymd_opt(2026, 11, 20).unwrap();
        assert!(
            matches!(c.as_slice(), [Command::Summary { since: a }, Command::Summary { since: b }] if *a == mon && *b == fri)
        );
        let c = commands(press(&mut app, "]"));
        assert!(matches!(c.as_slice(), [Command::Summary { since }] if *since == mon));
        let c = commands(press(&mut app, "w"));
        assert!(matches!(c.as_slice(), [Command::WriteSummary { since }] if *since == mon));
        assert!(
            matches!(press(&mut app, "y").as_slice(), [Effect::CopyToClipboard(md)] if md.starts_with("# Summary"))
        );
        // Cannot start after today
        app.summary_since = Some(app.today());
        assert!(commands(press(&mut app, "]")).is_empty());
    }

    #[test]
    fn workday_helpers_skip_weekends() {
        use chrono::Weekday::*;
        let days = [Mon, Tue, Wed, Thu, Fri];
        let mon = NaiveDate::from_ymd_opt(2026, 11, 23).unwrap();
        assert_eq!(
            previous_workday(mon, &days),
            NaiveDate::from_ymd_opt(2026, 11, 20).unwrap()
        );
        assert_eq!(
            next_workday(NaiveDate::from_ymd_opt(2026, 11, 20).unwrap(), &days),
            mon
        );
        assert_eq!(previous_workday(mon, &[]), mon.pred_opt().unwrap());
    }

    #[test]
    fn status_clears_after_five_seconds() {
        let mut app = sample::app();
        app.set_status("hello");
        let t0 = app.now;
        app.tick(t0 + chrono::Duration::seconds(4));
        assert!(app.status.is_some());
        app.tick(t0 + chrono::Duration::seconds(5));
        assert!(app.status.is_none());
    }

    #[test]
    fn selection_survives_a_reload() {
        let mut app = sample::app();
        app.cursor[0] = 2;
        let before = app.selected_work_item().unwrap().branch.clone();
        let mut items = app.work_items.clone();
        items.reverse();
        app.handle_event(AppEvent::WorkItemsUpdated(items));
        assert_eq!(app.selected_work_item().unwrap().branch, before);
    }

    #[test]
    fn progress_and_errors_update_state() {
        let mut app = sample::app();
        app.handle_event(AppEvent::Progress {
            task: "sync".into(),
            done: 1,
            total: 3,
        });
        assert!(app.progress.is_some());
        app.handle_event(AppEvent::Progress {
            task: "sync".into(),
            done: 3,
            total: 3,
        });
        assert!(app.progress.is_none());
        app.handle_event(AppEvent::Error {
            scope: "API/auth".into(),
            message: "fetch failed".into(),
        });
        assert_eq!(app.errors.last().unwrap(), "API/auth: fetch failed");
        assert_eq!(app.status.as_ref().unwrap().kind, StatusKind::Error);
    }

    #[test]
    fn list_only_enter_opens_details_and_esc_closes() {
        let mut app = sample::app();
        app.feedback.arrangement.set(Some(Arrangement::ListOnly));
        press(&mut app, "2");
        app.handle_key(key(KeyCode::Enter));
        assert!(app.details_open);
        app.handle_key(key(KeyCode::Esc));
        assert!(!app.details_open);
    }

    #[test]
    fn v_cycles_layout_and_overlays_open_and_close() {
        let mut app = sample::app();
        press(&mut app, "v");
        assert_eq!(app.layout_mode, LayoutMode::SideBySide);
        press(&mut app, "?");
        assert_eq!(app.overlay, Some(Overlay::Help));
        app.handle_key(key(KeyCode::Esc));
        assert!(app.overlay.is_none());
        press(&mut app, "i");
        assert_eq!(app.overlay, Some(Overlay::Diagnostics));
        press(&mut app, "x");
        assert!(app.overlay.is_none());
    }

    #[test]
    fn r_and_capital_r_reload_and_sync() {
        let mut app = sample::app();
        assert!(matches!(
            commands(press(&mut app, "r")).as_slice(),
            [Command::LocalReload]
        ));
        assert!(matches!(
            commands(press(&mut app, "R")).as_slice(),
            [Command::FullSync]
        ));
    }

    #[test]
    fn fuzzy_score_prefers_tight_matches() {
        assert!(fuzzy_score("chk", "chunking").is_some());
        assert!(fuzzy_score("xyz", "chunking").is_none());
        assert!(fuzzy_score("abc", "abc").unwrap() < fuzzy_score("abc", "a_b_c").unwrap());
    }

    #[test]
    fn mouse_wheel_moves_the_list_cursor() {
        let mut app = sample::app();
        app.feedback.list_area.set(Rect::new(0, 2, 50, 10));
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: 5,
            row: 5,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(app.cursor[0], 3);
    }
}
