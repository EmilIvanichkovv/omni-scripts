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

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Datelike, FixedOffset, NaiveDate, TimeZone, Weekday};
use crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::{Position, Rect};

use crate::config::Config;
use crate::lgbc::responsive::{Arrangement, LayoutMode};
use crate::model::{CiState, Repo, WorkItem, WorkState, Worktree};
use crate::refresh::{AppEvent, Command, PrStatus};
use crate::sessions::{Session, SessionLink};
use crate::summary::{Entry, Summary, Target};
use crate::todos::rules::{normalize_title, Suggestion, SuggestionPayload, SuggestionStatus};
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

    fn next(self) -> Self {
        Self::ALL[(self.index() + 1) % Self::ALL.len()]
    }

    fn prev(self) -> Self {
        Self::ALL[(self.index() + Self::ALL.len() - 1) % Self::ALL.len()]
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

/// Todos view status filters (spec 9.3), shown as an always-visible bar; `h`/`l`, `←`/`→`
/// move between them and `f` cycles.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TodoFilter {
    /// `todo` and `doing`.
    #[default]
    Active,
    Todo,
    Doing,
    Done,
    Dropped,
    All,
}

impl TodoFilter {
    pub const ALL: [TodoFilter; 6] = [
        Self::Active,
        Self::Todo,
        Self::Doing,
        Self::Done,
        Self::Dropped,
        Self::All,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Active => "Active",
            Self::Todo => "Todo",
            Self::Doing => "Doing",
            Self::Done => "Done",
            Self::Dropped => "Dropped",
            Self::All => "All",
        }
    }

    /// Label for narrow filter bars.
    pub fn short(self) -> &'static str {
        match self {
            Self::Active => "Act",
            Self::Dropped => "Drop",
            other => other.label(),
        }
    }

    pub fn matches(self, status: TodoStatus) -> bool {
        match self {
            Self::Active => status.is_active(),
            Self::Todo => status == TodoStatus::Todo,
            Self::Doing => status == TodoStatus::Doing,
            Self::Done => status == TodoStatus::Done,
            Self::Dropped => status == TodoStatus::Dropped,
            Self::All => true,
        }
    }

    fn step(self, delta: isize) -> Self {
        let i = Self::ALL.iter().position(|f| *f == self).unwrap_or(0);
        Self::ALL[(i as isize + delta).rem_euclid(Self::ALL.len() as isize) as usize]
    }
}

/// Sections of the Summary tab, in bar order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SummarySection {
    Done,
    #[default]
    InProgress,
    Todo,
    Blockers,
    Cleanup,
}

impl SummarySection {
    pub const ALL: [SummarySection; 5] = [
        Self::Done,
        Self::InProgress,
        Self::Todo,
        Self::Blockers,
        Self::Cleanup,
    ];

    /// Order in which the first non-empty section is picked when the summary arrives.
    const INITIAL_ORDER: [SummarySection; 5] = [
        Self::InProgress,
        Self::Done,
        Self::Todo,
        Self::Blockers,
        Self::Cleanup,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Done => "Done",
            Self::InProgress => "In progress",
            Self::Todo => "Todo",
            Self::Blockers => "Blockers",
            Self::Cleanup => "Cleanup",
        }
    }

    /// Label for narrow section bars.
    pub fn short(self) -> &'static str {
        match self {
            Self::InProgress => "Prog",
            Self::Blockers => "Block",
            Self::Cleanup => "Clean",
            other => other.label(),
        }
    }

    pub fn entries(self, s: &Summary) -> &[Entry] {
        match self {
            Self::Done => &s.done,
            Self::InProgress => &s.in_progress,
            Self::Todo => &s.todo,
            Self::Blockers => &s.blockers,
            Self::Cleanup => &s.cleanup,
        }
    }

    fn step(self, delta: isize) -> Self {
        let i = Self::ALL.iter().position(|f| *f == self).unwrap_or(0);
        Self::ALL[(i as isize + delta).rem_euclid(Self::ALL.len() as isize) as usize]
    }
}

/// A clickable label of the tab bar or a filter/section bar, recorded by the renderer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BarHit {
    Tab(Tab),
    Todo(TodoFilter),
    Summary(SummarySection),
    /// A suggestion row of the New todo dialog.
    Seed(usize),
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
    /// New unlinked todo (`a` in Todos; Work and Sessions use the New todo dialog).
    AddTodo,
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
    /// `Space` on an open todo: marking done waits for Enter (or `y`).
    MarkDone {
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

/// Which list of a session summary a New todo suggestion comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeedKind {
    Unfinished,
    /// A summary todo or an explicit todo of the transcript.
    Todo,
    Blocker,
}

impl SeedKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Unfinished => "unfinished",
            Self::Todo => "todo",
            Self::Blocker => "blocker",
        }
    }
}

/// A suggestion of the New todo dialog, taken from a session summary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TodoSeed {
    /// Sanitized single-line text.
    pub text: String,
    pub kind: SeedKind,
    /// The session it came from (the todo's `source` when it is used).
    pub session_id: String,
    /// A todo with the same normalized title exists already.
    pub exists: bool,
}

/// The New todo dialog (`t` in Work and Sessions): a title input, a priority and
/// suggestions from the linked sessions' summaries. The input keeps the focus; `↑`/`↓`
/// highlight a suggestion and `Enter` copies it into the input, a second `Enter` adds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewTodo {
    /// `PROJECT/repo:branch` the new todo links to.
    pub link: Option<String>,
    pub input: LineInput,
    pub priority: Option<Priority>,
    pub seeds: Vec<TodoSeed>,
    /// Highlighted suggestion.
    pub selected: Option<usize>,
    /// The suggestion the input text was copied from; the todo's source is its session
    /// until the input is cleared.
    pub picked: Option<usize>,
}

/// Next priority of the cycle none → A → B → C → none (backwards with `back`).
pub fn cycle_priority(p: Option<Priority>, back: bool) -> Option<Priority> {
    const ORDER: [Option<Priority>; 4] = [
        None,
        Some(Priority::A),
        Some(Priority::B),
        Some(Priority::C),
    ];
    let i = ORDER.iter().position(|o| *o == p).unwrap_or(0);
    ORDER[if back { (i + 3) % 4 } else { (i + 1) % 4 }]
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Overlay {
    Help,
    Diagnostics,
    Confirm(Confirm),
    LinkPicker(LinkPicker),
    NewTodo(NewTodo),
}

/// A group header of a grouped list (a Work repo, a Todos repo, a Sessions day, a Summary
/// working directory). Every grouped list folds the same way: `Enter` / `Space` on a header,
/// `z` from any row of the group, `Z` for all groups; the collapse state lives in
/// [`App::collapsed`] under [`GroupHeader::key`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupHeader {
    /// Identity of the group, stable across data refreshes (repo id, day, directory).
    pub key: String,
    /// Display name.
    pub label: String,
    /// Items of the group (indices as in [`ListRow::Item`]), also when it is collapsed.
    pub members: Vec<usize>,
    pub collapsed: bool,
}

impl GroupHeader {
    pub fn count(&self) -> usize {
        self.members.len()
    }
}

/// A row of a list. The cursor stops on both kinds. `Item` indexes the tab's data:
/// `App::work_items`, `App::todos`, `App::sessions`, the current Summary section's entries,
/// `App::suggestions`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ListRow {
    Group(GroupHeader),
    Item(usize),
}

/// Identity of a list row for keeping the selection across list changes: the row's own key
/// and, for an item inside a group, the key of its group header (the fallback when the item
/// gets folded away).
#[derive(Debug, Clone, PartialEq, Eq)]
struct RowKey {
    key: String,
    group: Option<String>,
}

impl RowKey {
    fn item(key: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            group: None,
        }
    }
}

/// The working directory a Summary entry is grouped under.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum SummaryDir {
    Path(PathBuf),
    NoDir,
    Other,
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
    pub too_small: Cell<bool>,
    /// Screen areas of the tabs and filter/section bar labels drawn in the last frame
    /// (mouse clicks).
    pub bar_hits: RefCell<Vec<(Rect, BarHit)>>,
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
    /// The structured summary behind `summary_markdown` (the Summary tab's lists).
    pub summary: Option<Summary>,
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
    pub summary_section: SummarySection,
    pub todo_grouped: bool,
    /// Keys ([`GroupHeader::key`]) of the collapsed groups of each tab's list. Kept across
    /// data refreshes; ignored while the tab has a search.
    pub collapsed: [HashSet<String>; 5],
    pub search: [String; 5],
    pub cursor: [usize; 5],
    pub details_scroll: usize,
    /// List-only layout: details shown full screen.
    pub details_open: bool,
    pub input: Option<ActiveInput>,
    pub overlay: Option<Overlay>,
    pub overlay_scroll: usize,
    pub feedback: RenderFeedback,
}

pub use crate::sessions::resume::shell_quote;

/// `cd '<path>' && claude --resume <id>` (spec 6.5).
pub fn resume_command(path: &Path, session_id: &str) -> String {
    crate::sessions::resume::resume_command_text(path, session_id)
}

/// Collapse key of a Sessions day group.
fn day_key(day: Option<NaiveDate>) -> String {
    day.map_or_else(|| "none".to_string(), |d| d.to_string())
}

/// `Today`, `Yesterday`, `Tue 24 Nov` (with the year when it is not this year).
pub fn day_label(now: &Now, day: Option<NaiveDate>) -> String {
    let Some(d) = day else {
        return "No date".to_string();
    };
    let today = now.date_naive();
    match (today - d).num_days() {
        0 => "Today".to_string(),
        1 => "Yesterday".to_string(),
        _ if d.year() == today.year() => d.format("%a %-d %b").to_string(),
        _ => d.format("%a %-d %b %Y").to_string(),
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

/// How `App::todo_session` found a todo's session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TodoSessionKind {
    /// The todo was created from this session (`source=session:<id>`).
    CreatedFrom,
    /// The newest session on the todo's linked branch.
    LatestOnBranch,
}

/// Last branch seen in the session's transcript: the branch it was working on when it ended.
pub fn working_branch(s: &Session) -> Option<&str> {
    s.branches.last().map(String::as_str)
}

/// Link of the working branch; else the last own link (sessions without recorded branches);
/// else the last link (sessions linked only through work in other worktrees).
pub fn working_link(s: &Session) -> Option<&SessionLink> {
    working_branch(s)
        .and_then(|b| s.links.iter().rev().find(|l| l.branch == b))
        .or_else(|| s.own_links().last())
        .or_else(|| s.links.last())
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
            summary: None,
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
            summary_section: SummarySection::default(),
            todo_grouped: false,
            collapsed: Default::default(),
            search: Default::default(),
            cursor: [0; 5],
            details_scroll: 0,
            details_open: false,
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
        let keys: Vec<Option<RowKey>> = Tab::ALL.iter().map(|t| self.selected_key(*t)).collect();
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
            AppEvent::SummaryUpdated {
                since,
                summary,
                markdown,
            } => {
                if self.summary.is_none() {
                    // First summary: open on the first section with something in it.
                    if let Some(sec) = SummarySection::INITIAL_ORDER
                        .into_iter()
                        .find(|sec| !sec.entries(&summary).is_empty())
                    {
                        self.summary_section = sec;
                    }
                }
                self.summary_since = Some(since);
                self.summary = Some(summary);
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
    pub fn work_rows(&self) -> Vec<ListRow> {
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
        self.grouped_rows(Tab::Work, groups)
    }

    /// Whether a tab's groups are all shown open regardless of [`Self::collapsed`]: while it
    /// has a search, so every match shows.
    pub fn groups_forced_open(&self, tab: Tab) -> bool {
        !self.query(tab).trim().is_empty()
    }

    /// Rows of `(key, label, members)` groups: each header, then its members unless the
    /// group is collapsed.
    fn grouped_rows(&self, tab: Tab, groups: Vec<(String, String, Vec<usize>)>) -> Vec<ListRow> {
        let open = self.groups_forced_open(tab);
        let mut rows = Vec::new();
        for (key, label, members) in groups {
            let collapsed = !open && self.collapsed[tab.index()].contains(&key);
            let items: Vec<ListRow> = if collapsed {
                Vec::new()
            } else {
                members.iter().copied().map(ListRow::Item).collect()
            };
            rows.push(ListRow::Group(GroupHeader {
                key,
                label,
                members,
                collapsed,
            }));
            rows.extend(items);
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

    pub fn selected_work_row(&self) -> Option<ListRow> {
        self.selected_row(Tab::Work)
    }

    pub fn selected_work_item(&self) -> Option<&WorkItem> {
        self.work_items.get(self.selected_item(Tab::Work)?)
    }

    /// Rows of a tab's list.
    pub fn list_rows(&self, tab: Tab) -> Vec<ListRow> {
        match tab {
            Tab::Work => self.work_rows(),
            Tab::Todos => self.todo_rows(),
            Tab::Sessions => self.session_rows(),
            Tab::Summary => self.summary_rows(),
            Tab::Inbox => self
                .pending_suggestions()
                .into_iter()
                .map(ListRow::Item)
                .collect(),
        }
    }

    /// The row under a tab's cursor.
    pub fn selected_row(&self, tab: Tab) -> Option<ListRow> {
        self.list_rows(tab).get(self.cursor[tab.index()]).cloned()
    }

    /// The item under a tab's cursor (`None` on a group header).
    pub fn selected_item(&self, tab: Tab) -> Option<usize> {
        match self.selected_row(tab)? {
            ListRow::Item(i) => Some(i),
            ListRow::Group(_) => None,
        }
    }

    /// Whether a tab's cursor is on a group header.
    pub fn on_group_header(&self, tab: Tab) -> bool {
        matches!(self.selected_row(tab), Some(ListRow::Group(_)))
    }

    /// The group the cursor is in (the header itself, or the header above the item) and the
    /// header's row.
    fn current_group(&self, tab: Tab) -> Option<(usize, GroupHeader)> {
        let rows = self.list_rows(tab);
        let cursor = self.cursor[tab.index()].min(rows.len().checked_sub(1)?);
        rows[..=cursor]
            .iter()
            .enumerate()
            .rev()
            .find_map(|(n, r)| match r {
                ListRow::Group(g) => Some((n, g.clone())),
                ListRow::Item(_) => None,
            })
    }

    /// Row of the first item of a tab's list (where a fresh list puts the cursor), 0 when
    /// it has none.
    pub fn first_item_row(&self, tab: Tab) -> usize {
        self.list_rows(tab)
            .iter()
            .position(|r| matches!(r, ListRow::Item(_)))
            .unwrap_or(0)
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
                self.todo_filter.matches(t.status)
                    && q.split_whitespace().all(|w| {
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

    /// Todos matching status filter `f`, ignoring the search (the filter bar's counts).
    pub fn todo_filter_count(&self, f: TodoFilter) -> usize {
        self.todos.iter().filter(|t| f.matches(t.status)).count()
    }

    pub fn todo_group_name(&self, t: &Todo) -> String {
        t.link
            .as_deref()
            .map(|l| Self::link_repo(l).to_string())
            .unwrap_or_else(|| "~(no link)".to_string())
    }

    /// Display rows of the Todos list, grouped by repo with `G`.
    pub fn todo_rows(&self) -> Vec<ListRow> {
        let visible = self.visible_todos();
        if !self.todo_grouped {
            return visible.into_iter().map(ListRow::Item).collect();
        }
        // `visible_todos` is sorted by group name, so groups are contiguous.
        let mut groups: Vec<(String, String, Vec<usize>)> = Vec::new();
        for i in visible {
            let g = self.todo_group_name(&self.todos[i]);
            match groups.last_mut() {
                Some(last) if last.0 == g => last.2.push(i),
                _ => {
                    let label = g.trim_start_matches('~').to_string();
                    groups.push((g, label, vec![i]));
                }
            }
        }
        self.grouped_rows(Tab::Todos, groups)
    }

    pub fn selected_todo(&self) -> Option<&Todo> {
        self.todos.get(self.selected_item(Tab::Todos)?)
    }

    /// Sessions passing the search, newest first.
    pub fn visible_sessions(&self) -> Vec<usize> {
        let q = self.query(Tab::Sessions).trim().to_lowercase();
        let mut out: Vec<usize> = (0..self.sessions.len())
            .filter(|&i| {
                let s = &self.sessions[i];
                q.split_whitespace().all(|w| {
                    s.id.to_lowercase().contains(w)
                        || self.session_title(s).to_lowercase().contains(w)
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

    /// Day a session is listed under (its last activity, else its start), local time.
    pub fn session_day(&self, s: &Session) -> Option<NaiveDate> {
        s.last_activity_at
            .or(s.started_at)
            .and_then(|t| self.now.offset().timestamp_opt(t, 0).single())
            .map(|dt| dt.date_naive())
    }

    /// Rows of the Sessions list: visible sessions under day groups, newest first.
    pub fn session_rows(&self) -> Vec<ListRow> {
        let mut groups: Vec<(String, String, Vec<usize>)> = Vec::new();
        for i in self.visible_sessions() {
            let day = self.session_day(&self.sessions[i]);
            let key = day_key(day);
            match groups.last_mut() {
                Some(last) if last.0 == key => last.2.push(i),
                _ => groups.push((key, day_label(&self.now, day), vec![i])),
            }
        }
        self.grouped_rows(Tab::Sessions, groups)
    }

    pub fn selected_session(&self) -> Option<&Session> {
        self.sessions.get(self.selected_item(Tab::Sessions)?)
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
        working_link(s).and_then(|l| self.repo_name(&l.repo_id))
    }

    /// The branch the session ended up working on: the last branch seen in its transcript
    /// (a session can start on one branch and switch to another).
    pub fn session_branch(&self, s: &Session) -> Option<String> {
        working_branch(s)
            .map(str::to_string)
            .or_else(|| working_link(s).map(|l| l.branch.clone()))
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

    /// The resume command for a session (spec 6.5), or `None` when its directory is unknown.
    ///
    /// Claude Code stores sessions per project directory, derived from the directory `claude`
    /// was started in, so the `cd` target is the session's recorded `cwd`, not the worktree
    /// root (they differ when `claude` ran in a subdirectory) and not dependent on worktrack
    /// having discovered the repo. Falls back to the linked worktree for sessions without a
    /// recorded `cwd`.
    pub fn session_resume(&self, s: &Session) -> Option<String> {
        s.cwd
            .as_deref()
            .filter(|c| !c.trim().is_empty())
            .map(PathBuf::from)
            .or_else(|| self.session_worktree(s))
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

    /// Path with the home directory shown as `~`.
    pub fn display_path(&self, p: &Path) -> String {
        if let Some(home) = &self.home {
            if let Ok(rest) = p.strip_prefix(home) {
                return if rest.as_os_str().is_empty() {
                    "~".into()
                } else {
                    format!("~/{}", rest.display())
                };
            }
        }
        p.display().to_string()
    }

    /// Directory of a repo: its main worktree, else the parent of its common git dir.
    fn repo_dir(&self, repo_id: &str) -> Option<PathBuf> {
        self.worktrees
            .iter()
            .find(|w| w.repo_id == repo_id && w.is_main)
            .map(|w| w.path.clone())
            .or_else(|| {
                let repo = self.repos.iter().find(|r| r.id == repo_id)?;
                repo.common_dir.parent().map(Path::to_path_buf)
            })
    }

    /// Directory of a WorkItem: its worktree, else its repo's directory.
    fn item_dir(&self, item: &WorkItem) -> Option<PathBuf> {
        item.local
            .worktree
            .clone()
            .or_else(|| self.repo_dir(&item.repo_id))
    }

    /// Working directory a Summary entry is grouped under, and its latest activity.
    fn summary_dir(&self, target: &Target) -> (SummaryDir, Option<i64>) {
        let path = |p: Option<PathBuf>| p.map_or(SummaryDir::NoDir, SummaryDir::Path);
        match target {
            Target::Item { repo_id, branch } => match self.work_item(repo_id, branch) {
                Some(item) => (path(self.item_dir(item)), item.last_activity),
                None => (path(self.repo_dir(repo_id)), None),
            },
            Target::Session { id } => match self.find_session(id) {
                Some(s) => (
                    path(s.cwd.as_deref().map(PathBuf::from)),
                    s.last_activity_at.or(s.started_at),
                ),
                None => (SummaryDir::NoDir, None),
            },
            Target::Todo { .. } => match self.target_item(target) {
                Some(item) => (path(self.item_dir(item)), item.last_activity),
                None => (SummaryDir::NoDir, None),
            },
            Target::Inbox | Target::None => (SummaryDir::Other, None),
        }
    }

    /// Rows of the current Summary section: entries grouped under their working directory
    /// (directories with the most recent activity first, then `(no directory)`, then
    /// `(other)`), entries in section order inside a group.
    pub fn summary_rows(&self) -> Vec<ListRow> {
        let entries = self
            .summary
            .as_ref()
            .map_or(&[][..], |s| self.summary_section.entries(s));
        // (dir, newest activity, members) in first-seen order.
        let mut groups: Vec<(SummaryDir, Option<i64>, Vec<usize>)> = Vec::new();
        for (i, e) in entries.iter().enumerate() {
            let (dir, at) = self.summary_dir(&e.target);
            match groups.iter_mut().find(|g| g.0 == dir) {
                Some(g) => {
                    g.1 = g.1.max(at);
                    g.2.push(i);
                }
                None => groups.push((dir, at, vec![i])),
            }
        }
        // Stable: paths by newest activity (unknown last), then the two catch-alls.
        groups.sort_by_key(|g| match g.0 {
            SummaryDir::Path(_) => (0, std::cmp::Reverse(g.1.unwrap_or(i64::MIN))),
            SummaryDir::NoDir => (1, std::cmp::Reverse(0)),
            SummaryDir::Other => (2, std::cmp::Reverse(0)),
        });
        let groups = groups
            .into_iter()
            .map(|(dir, _, members)| {
                let (key, label) = match &dir {
                    SummaryDir::Path(p) => (format!("p:{}", p.display()), self.display_path(p)),
                    SummaryDir::NoDir => ("nodir".to_string(), "(no directory)".to_string()),
                    SummaryDir::Other => ("other".to_string(), "(other)".to_string()),
                };
                (key, label, members)
            })
            .collect();
        self.grouped_rows(Tab::Summary, groups)
    }

    /// Latest activity behind a Summary entry's target (what its directory group is
    /// ordered by).
    pub fn summary_activity(&self, target: &Target) -> Option<i64> {
        self.summary_dir(target).1
    }

    /// Entries of the current Summary section (indexed by the section's [`ListRow::Item`]s).
    pub fn summary_section_entries(&self) -> &[Entry] {
        self.summary
            .as_ref()
            .map_or(&[][..], |s| self.summary_section.entries(s))
    }

    pub fn summary_section_count(&self, sec: SummarySection) -> usize {
        self.summary.as_ref().map_or(0, |s| sec.entries(s).len())
    }

    pub fn selected_summary_entry(&self) -> Option<&Entry> {
        self.summary_section_entries()
            .get(self.selected_item(Tab::Summary)?)
    }

    pub fn work_item(&self, repo_id: &str, branch: &str) -> Option<&WorkItem> {
        self.work_items
            .iter()
            .find(|w| w.repo_id == repo_id && w.branch == branch)
    }

    /// A todo by id, or by title when it has no id yet.
    pub fn find_todo(&self, id: Option<&str>, title: &str) -> Option<&Todo> {
        match id {
            Some(id) => self.todos.iter().find(|t| t.id.as_deref() == Some(id)),
            None => self
                .todos
                .iter()
                .find(|t| t.id.is_none() && t.title == title),
        }
    }

    pub fn find_session(&self, id: &str) -> Option<&Session> {
        self.sessions.iter().find(|s| s.id == id)
    }

    /// The session a todo leads to: the one it was created from (`source=session:<id>`),
    /// else the newest session on its linked branch. The flag says which.
    pub fn todo_session(&self, t: &Todo) -> Option<(&Session, TodoSessionKind)> {
        if let Some(s) = t
            .source
            .as_deref()
            .and_then(|src| src.strip_prefix("session:"))
            .and_then(|id| self.find_session(id))
        {
            return Some((s, TodoSessionKind::CreatedFrom));
        }
        let item = self.item_for_link(t.link.as_deref()?)?;
        let latest = *self.item_sessions(item).first()?;
        Some((latest, TodoSessionKind::LatestOnBranch))
    }

    /// Enter in Todos: show the todo's session (see `todo_session`) in the Sessions tab.
    fn jump_to_todo_session(&mut self) -> Vec<Effect> {
        let Some(t) = self.selected_todo() else {
            return Vec::new();
        };
        match self.todo_session(t).map(|(s, kind)| (s.id.clone(), kind)) {
            Some((id, kind)) => {
                self.select_session(id);
                self.set_status(match kind {
                    TodoSessionKind::CreatedFrom => "the session this todo came from",
                    TodoSessionKind::LatestOnBranch => "the latest session on this todo's branch",
                });
            }
            None if t.link.is_none() => {
                self.set_status("this todo has no session and no branch link (L links one)")
            }
            None => self.set_status("no session on this todo's branch yet"),
        }
        Vec::new()
    }

    /// The pending suggestion an Inbox summary entry stands for (same message), if any.
    pub fn suggestion_for_entry(&self, entry: &Entry) -> Option<&Suggestion> {
        self.suggestions.iter().find(|s| {
            s.status == SuggestionStatus::Pending
                && s.payload.needs_choice()
                && crate::sanitize::clean_line(s.payload.text()) == entry.line
        })
    }

    /// The WorkItem a summary target refers to: the item itself, or the item a todo or
    /// session is linked to.
    pub fn target_item(&self, target: &Target) -> Option<&WorkItem> {
        match target {
            Target::Item { repo_id, branch } => self.work_item(repo_id, branch),
            Target::Todo { id, title } => {
                let link = self.find_todo(id.as_deref(), title)?.link.as_deref()?;
                self.item_for_link(link)
            }
            Target::Session { id } => {
                let link = working_link(self.find_session(id)?)?;
                self.work_item(&link.repo_id, &link.branch)
            }
            Target::Inbox | Target::None => None,
        }
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

    /// Number of cursor positions in a tab's list (group headers included).
    pub fn row_count(&self, tab: Tab) -> usize {
        self.list_rows(tab).len()
    }

    /// Selection key of item `i` of a tab's list. Summary keys are numbered by `seen` so
    /// duplicate targets stay apart.
    fn item_key(&self, tab: Tab, i: usize, seen: &mut Vec<String>) -> String {
        match tab {
            Tab::Work => {
                let w = &self.work_items[i];
                format!("i:{}:{}", w.repo_id, w.branch)
            }
            Tab::Todos => todo_key(&self.todos[i]),
            Tab::Sessions => self.sessions[i].id.clone(),
            Tab::Summary => {
                // Target first, so a selection survives text changes; duplicates numbered.
                let Some(e) = self.summary_section_entries().get(i) else {
                    return String::new();
                };
                let base = match &e.target {
                    Target::None => format!("l:{}", e.line),
                    t => format!("{t:?}"),
                };
                let n = seen.iter().filter(|k| **k == base).count();
                seen.push(base.clone());
                format!("{base}#{n}")
            }
            Tab::Inbox => self.suggestions[i].id.to_string(),
        }
    }

    fn row_keys(&self, tab: Tab) -> Vec<RowKey> {
        let mut seen = Vec::new();
        let mut group: Option<String> = None;
        self.list_rows(tab)
            .into_iter()
            .map(|r| match r {
                ListRow::Group(g) => {
                    let key = format!("g:{}", g.key);
                    group = Some(key.clone());
                    RowKey { key, group: None }
                }
                ListRow::Item(i) => RowKey {
                    key: self.item_key(tab, i, &mut seen),
                    group: group.clone(),
                },
            })
            .collect()
    }

    fn has_key(&self, tab: Tab, key: &str) -> bool {
        self.row_keys(tab).iter().any(|k| k.key == key)
    }

    fn selected_key(&self, tab: Tab) -> Option<RowKey> {
        self.row_keys(tab).get(self.cursor[tab.index()]).cloned()
    }

    /// Put the cursor back on `key` after the list changed: on the row itself, else on its
    /// group's header (it was folded away), else clamped. With no previous selection (the
    /// list was empty) the cursor goes to the first item.
    fn reselect(&mut self, tab: Tab, key: Option<RowKey>) {
        let keys = self.row_keys(tab);
        let idx = tab.index();
        let Some(key) = key else {
            self.cursor[idx] = self.first_item_row(tab);
            return;
        };
        let find = |k: &str| keys.iter().position(|x| x.key == k);
        if let Some(pos) = find(&key.key).or_else(|| key.group.as_deref().and_then(find)) {
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
                KeyCode::Enter if self.tab == Tab::Summary => return self.jump_to_summary_target(),
                KeyCode::Enter if self.tab == Tab::Todos => return self.jump_to_todo_session(),
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
            KeyCode::Char('g') | KeyCode::Home => self.set_cursor(0),
            KeyCode::Char('G') | KeyCode::End => self.set_cursor(usize::MAX),
            KeyCode::Char('J') => self.scroll_details(1),
            KeyCode::Char('K') => self.scroll_details(-1),
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
                if self.arrangement() == Arrangement::ListOnly && self.row_count(self.tab) > 0 {
                    self.details_open = true;
                    self.details_scroll = 0;
                }
            }
            _ => {}
        }
        Vec::new()
    }

    fn move_or_scroll(&mut self, delta: isize) {
        self.move_cursor(delta);
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
                self.set_todo_filter(self.todo_filter.step(1));
                return;
            }
            Tab::Summary => {
                self.set_summary_section(self.summary_section.step(1));
                return;
            }
            _ => self.set_status("no filters in this view"),
        }
        self.reselect(self.tab, key);
    }

    /// Select a Todos status filter, keeping the selected todo when it still shows.
    fn set_todo_filter(&mut self, f: TodoFilter) {
        let key = self.selected_key(Tab::Todos);
        self.todo_filter = f;
        self.reselect(Tab::Todos, key);
        self.details_scroll = 0;
    }

    /// Switch the Summary section; the cursor goes to its first entry.
    fn set_summary_section(&mut self, sec: SummarySection) {
        self.summary_section = sec;
        self.cursor[Tab::Summary.index()] = self.first_item_row(Tab::Summary);
        self.details_scroll = 0;
        self.details_open = false;
    }

    /// Keys specific to the current tab. `None` = not handled, fall through to global keys.
    fn tab_key(&mut self, key: KeyEvent) -> Option<Vec<Effect>> {
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            return None;
        }
        if let Some(effects) = self.group_key(key) {
            return Some(effects);
        }
        match self.tab {
            Tab::Work => self.work_key(key),
            Tab::Todos => self.todos_key(key),
            Tab::Sessions => self.sessions_key(key),
            Tab::Summary => self.summary_key(key),
            Tab::Inbox => self.inbox_key(key),
        }
    }

    /// Folding keys of the grouped lists (Work, Todos, Sessions, Summary): `Enter` / `Space`
    /// on a group header toggle it, `z` toggles the cursor's group from any row, `Z` folds
    /// all groups (unfolds all when every group is folded).
    fn group_key(&mut self, key: KeyEvent) -> Option<Vec<Effect>> {
        if self.tab == Tab::Inbox {
            return None;
        }
        match key.code {
            KeyCode::Enter | KeyCode::Char(' ') if self.on_group_header(self.tab) => {
                self.toggle_group(self.tab);
            }
            KeyCode::Char('z') => self.toggle_group(self.tab),
            KeyCode::Char('Z') => self.fold_all(self.tab),
            _ => return None,
        }
        Some(Vec::new())
    }

    /// Why a tab's groups cannot be folded right now, if they cannot.
    fn fold_refusal(&self, tab: Tab) -> Option<&'static str> {
        if self.groups_forced_open(tab) {
            Some("groups stay open while searching")
        } else if tab == Tab::Todos && !self.todo_grouped {
            Some("no groups; G groups todos by repo")
        } else {
            None
        }
    }

    /// `z` / `Enter` / `Space` on a header: fold or unfold the cursor's group; the cursor
    /// goes to its header.
    fn toggle_group(&mut self, tab: Tab) {
        if let Some(why) = self.fold_refusal(tab) {
            self.set_status(why);
            return;
        }
        let Some((row, group)) = self.current_group(tab) else {
            return;
        };
        let set = &mut self.collapsed[tab.index()];
        if group.collapsed {
            set.remove(&group.key);
        } else {
            set.insert(group.key);
        }
        // Rows above the header do not change.
        self.cursor[tab.index()] = row;
        self.details_scroll = 0;
    }

    /// `Z`: fold every group of the list, or unfold all when they are all folded already.
    fn fold_all(&mut self, tab: Tab) {
        if let Some(why) = self.fold_refusal(tab) {
            self.set_status(why);
            return;
        }
        let groups: Vec<GroupHeader> = self
            .list_rows(tab)
            .into_iter()
            .filter_map(|r| match r {
                ListRow::Group(g) => Some(g),
                ListRow::Item(_) => None,
            })
            .collect();
        if groups.is_empty() {
            return;
        }
        let key = self.selected_key(tab);
        let idx = tab.index();
        if groups.iter().any(|g| !g.collapsed) {
            self.collapsed[idx].extend(groups.into_iter().map(|g| g.key));
            self.set_status("folded all groups");
        } else {
            self.collapsed[idx].clear();
            self.set_status("unfolded all groups");
        }
        self.reselect(tab, key);
        self.details_scroll = 0;
    }

    /// Item-only key pressed on a group header: a short hint instead of doing anything.
    /// `None` (not handled) when the cursor is not on a header.
    fn needs_item(&mut self) -> Option<Vec<Effect>> {
        if !self.on_group_header(self.tab) {
            return None;
        }
        self.set_status("select an item; enter/space folds this group");
        Some(Vec::new())
    }

    fn work_key(&mut self, key: KeyEvent) -> Option<Vec<Effect>> {
        let item = self.selected_work_item().cloned();
        let none = Some(Vec::new());
        if matches!(
            key.code,
            KeyCode::Char('o' | 'y' | 'Y' | 't' | 'c' | 'A' | 'U' | 'X')
        ) && item.is_none()
        {
            return self.needs_item();
        }
        match key.code {
            KeyCode::Char('o') => {
                let item = item?;
                Some(self.open_item_url(&item))
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
                let sessions: Vec<Session> =
                    self.item_sessions(&item).into_iter().cloned().collect();
                self.open_new_todo(Some(item.link_ref()), &sessions);
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

    /// `o`: open the item's PR, else its Jira issue.
    fn open_item_url(&mut self, item: &WorkItem) -> Vec<Effect> {
        if let Some(pr) = &item.pr {
            return vec![Effect::Command(Command::OpenUrl(pr.url.clone()))];
        }
        match self.jira_key(&item.branch).and_then(|k| self.jira_url(&k)) {
            Some(url) => vec![Effect::Command(Command::OpenUrl(url))],
            None => {
                self.set_status("no PR or Jira link for this branch");
                Vec::new()
            }
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
                self.set_error("this session has no recorded directory; nothing copied");
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

    /// `PROJECT/repo:branch` of a session's working branch, when its repo is known.
    pub fn session_link_ref(&self, s: &Session) -> Option<String> {
        let link = working_link(s)?;
        let repo = self.repo_name(&link.repo_id)?;
        Some(format!("{repo}:{}", link.branch))
    }

    /// New todo suggestions from the sessions' summaries (in the order given): unfinished
    /// work, summary todos, blockers, then the transcript's explicit todos. Deduplicated by
    /// normalized text (the first one wins) and marked when a todo has that title already.
    pub fn todo_seeds(&self, sessions: &[Session]) -> Vec<TodoSeed> {
        let existing: HashSet<String> = self
            .todos
            .iter()
            .map(|t| normalize_title(&t.title))
            .collect();
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for s in sessions {
            let summary = s.summary.as_ref();
            let lists = [
                (SeedKind::Unfinished, summary.map(|x| &x.unfinished)),
                (SeedKind::Todo, summary.map(|x| &x.todos)),
                (SeedKind::Blocker, summary.map(|x| &x.blockers)),
                (SeedKind::Todo, Some(&s.explicit_todos)),
            ];
            for (kind, list) in lists {
                for text in list.into_iter().flatten() {
                    let text = crate::sanitize::clean_line(text).trim().to_string();
                    let norm = normalize_title(&text);
                    if norm.is_empty() || !seen.insert(norm.clone()) {
                        continue;
                    }
                    out.push(TodoSeed {
                        exists: existing.contains(&norm),
                        text,
                        kind,
                        session_id: s.id.clone(),
                    });
                }
            }
        }
        out
    }

    /// Open the New todo dialog for a todo linked to `link`, with suggestions from
    /// `sessions` (newest first). Refused while the todo file does not parse.
    fn open_new_todo(&mut self, link: Option<String>, sessions: &[Session]) {
        if self.refuse_todo_edit() {
            return;
        }
        let seeds = self.todo_seeds(sessions);
        self.open_overlay(Overlay::NewTodo(NewTodo {
            link,
            input: LineInput::default(),
            priority: None,
            seeds,
            selected: None,
            picked: None,
        }));
    }

    /// The `TodoOp::Add` effect of a new todo created today.
    fn add_todo(
        &self,
        title: String,
        priority: Option<Priority>,
        link: Option<String>,
        session: Option<&str>,
    ) -> Effect {
        let mut todo = Todo::new(title);
        todo.priority = priority;
        todo.link = link;
        todo.created = Some(self.today());
        todo.source = Some(match session {
            Some(id) => format!("session:{id}"),
            None => "manual".into(),
        });
        Effect::Command(Command::Todo(TodoOp::Add(todo)))
    }

    /// Keys of the New todo dialog. The title input keeps the focus: editing keys type,
    /// `↑`/`↓` (`Ctrl-n`/`Ctrl-p`) highlight a suggestion, `Tab`/`Shift-Tab` cycle the
    /// priority, `Enter` picks the highlighted suggestion or adds the todo, `Alt-Enter` /
    /// `Ctrl-Enter` / `Ctrl-s` add it and keep the dialog open for the next one.
    ///
    /// Without the kitty keyboard protocol most terminals send plain `Enter` for
    /// `Ctrl-Enter`, and some take `Alt-Enter` for themselves (Windows Terminal: full
    /// screen), so `Ctrl-s` is the binding that always works.
    fn new_todo_key(&mut self, mut d: NewTodo, key: KeyEvent) -> Vec<Effect> {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let n = d.seeds.len();
        match key.code {
            KeyCode::Esc => {
                self.set_status("cancelled");
                return Vec::new();
            }
            KeyCode::Enter => return self.new_todo_enter(d, ctrl || alt),
            // Ctrl-Enter arrives as Ctrl-j in terminals that send a line feed for it.
            KeyCode::Char('s' | 'j') if ctrl => return self.new_todo_enter(d, true),
            KeyCode::Tab => d.priority = cycle_priority(d.priority, false),
            KeyCode::BackTab => d.priority = cycle_priority(d.priority, true),
            KeyCode::Down if n > 0 => {
                d.selected = Some(d.selected.map_or(0, |i| (i + 1).min(n - 1)));
            }
            KeyCode::Char('n') if ctrl && n > 0 => {
                d.selected = Some(d.selected.map_or(0, |i| (i + 1).min(n - 1)));
            }
            // Up from the first suggestion leaves the list.
            KeyCode::Up => d.selected = d.selected.and_then(|i| i.checked_sub(1)),
            KeyCode::Char('p') if ctrl => d.selected = d.selected.and_then(|i| i.checked_sub(1)),
            _ => {
                d.input.edit(key);
                if d.input.text.trim().is_empty() {
                    d.picked = None;
                }
            }
        }
        self.overlay = Some(Overlay::NewTodo(d));
        Vec::new()
    }

    /// `Enter` in the New todo dialog. With a highlighted suggestion and an input that is
    /// empty or still holds an earlier pick, the suggestion is copied into the input (a
    /// second `Enter` adds it). Otherwise a non-empty title is added; `keep_open` clears
    /// the input for the next todo instead of closing.
    fn new_todo_enter(&mut self, mut d: NewTodo, keep_open: bool) -> Vec<Effect> {
        let untouched = d.input.text.trim().is_empty()
            || d.picked.is_some_and(|p| d.seeds[p].text == d.input.text);
        if let Some(sel) = d.selected.filter(|_| untouched) {
            if d.picked != Some(sel) {
                d.input = LineInput::with_text(&d.seeds[sel].text);
                d.picked = Some(sel);
                self.overlay = Some(Overlay::NewTodo(d));
                return Vec::new();
            }
        }
        let title = d.input.text.trim().to_string();
        if title.is_empty() {
            self.set_status("type a title or pick a suggestion (↑↓, enter)");
            self.overlay = Some(Overlay::NewTodo(d));
            return Vec::new();
        }
        let session = d.picked.map(|p| d.seeds[p].session_id.clone());
        let effect = self.add_todo(
            title.clone(),
            d.priority,
            d.link.clone(),
            session.as_deref(),
        );
        self.set_status(TODO_ADDED);
        if keep_open {
            let norm = normalize_title(&title);
            for s in &mut d.seeds {
                if normalize_title(&s.text) == norm {
                    s.exists = true;
                }
            }
            d.input = LineInput::default();
            d.picked = None;
            d.selected = None;
            self.overlay = Some(Overlay::NewTodo(d));
        }
        vec![effect]
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

    /// `Space`: an open todo asks for confirmation before it is marked done (Enter/`y`);
    /// a done one reopens right away.
    fn toggle_done(&mut self, id: String, todo: &Todo) -> Vec<Effect> {
        if todo.status == TodoStatus::Done {
            self.set_status("marked not done");
            let patch = TodoPatch {
                status: Some(TodoStatus::Todo),
                done: Some(None),
                ..Default::default()
            };
            return self.todo_update(id, patch);
        }
        self.open_overlay(Overlay::Confirm(Confirm::MarkDone {
            id,
            title: todo.title.clone(),
        }));
        Vec::new()
    }

    fn todos_key(&mut self, key: KeyEvent) -> Option<Vec<Effect>> {
        let none = Some(Vec::new());
        let today = self.today();
        match key.code {
            KeyCode::Enter if self.arrangement() != Arrangement::ListOnly => {
                Some(self.jump_to_todo_session())
            }
            KeyCode::Char('O') => Some(vec![Effect::OpenEditor {
                kind: EditorTarget::WholeFile,
            }]),
            KeyCode::Char('G') => {
                let key = self.selected_key(Tab::Todos);
                self.todo_grouped = !self.todo_grouped;
                self.reselect(Tab::Todos, key);
                none
            }
            KeyCode::Char('h') | KeyCode::Left => {
                self.set_todo_filter(self.todo_filter.step(-1));
                none
            }
            KeyCode::Char('l') | KeyCode::Right => {
                self.set_todo_filter(self.todo_filter.step(1));
                none
            }
            KeyCode::Char('a') => {
                if !self.refuse_todo_edit() {
                    self.input = Some(ActiveInput {
                        purpose: InputPurpose::AddTodo,
                        input: LineInput::default(),
                    });
                }
                none
            }
            KeyCode::Char('e' | 'E' | ' ' | 's' | 'p' | 'L' | '+' | '-' | 'D' | 'd') => {
                if self.selected_todo().is_none() {
                    return self.needs_item().or(none);
                }
                if self.refuse_todo_edit() {
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
                    KeyCode::Char(' ') => self.toggle_done(id, &todo),
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
        if matches!(key.code, KeyCode::Char('c' | 's' | 't')) && self.selected_session().is_none() {
            return self.needs_item();
        }
        match key.code {
            KeyCode::Char('t') => {
                let s = self.selected_session()?.clone();
                let link = self.session_link_ref(&s);
                self.open_new_todo(link, std::slice::from_ref(&s));
                Some(Vec::new())
            }
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
        let Some(link) = working_link(s).cloned() else {
            self.set_status("this session is not linked to a branch");
            return Vec::new();
        };
        if self.work_item(&link.repo_id, &link.branch).is_none() {
            self.set_status(format!("{} is not a tracked work item", link.branch));
            return Vec::new();
        }
        self.select_work_item(&link.repo_id, &link.branch);
        Vec::new()
    }

    /// Switch to Work and select an item. When the search, the filter or a collapsed group
    /// hides it: clear the search, then widen the filter to All, and expand its group.
    fn select_work_item(&mut self, repo_id: &str, branch: &str) {
        self.switch_tab(Tab::Work);
        self.collapsed[Tab::Work.index()].remove(repo_id);
        let key = format!("i:{repo_id}:{branch}");
        if !self.has_key(Tab::Work, &key) {
            self.search[Tab::Work.index()].clear();
        }
        if !self.has_key(Tab::Work, &key) {
            self.work_filter = WorkFilter::All;
        }
        self.reselect(Tab::Work, Some(RowKey::item(key)));
        self.details_scroll = 0;
    }

    /// Switch to Todos and select a todo, clearing the search / widening the filter to All
    /// when they hide it, and expanding its repo group.
    fn select_todo(&mut self, key: String) {
        self.switch_tab(Tab::Todos);
        if let Some(t) = self.todos.iter().find(|t| todo_key(t) == key) {
            let group = self.todo_group_name(t);
            self.collapsed[Tab::Todos.index()].remove(&group);
        }
        if !self.has_key(Tab::Todos, &key) {
            self.search[Tab::Todos.index()].clear();
        }
        if !self.has_key(Tab::Todos, &key) {
            self.todo_filter = TodoFilter::All;
        }
        self.reselect(Tab::Todos, Some(RowKey::item(key)));
        self.details_scroll = 0;
    }

    /// Switch to Sessions and select a session, clearing the search when it hides it and
    /// expanding its day.
    fn select_session(&mut self, id: String) {
        self.switch_tab(Tab::Sessions);
        if let Some(s) = self.find_session(&id) {
            let day = day_key(self.session_day(s));
            self.collapsed[Tab::Sessions.index()].remove(&day);
        }
        if !self.has_key(Tab::Sessions, &id) {
            self.search[Tab::Sessions.index()].clear();
        }
        self.reselect(Tab::Sessions, Some(RowKey::item(id)));
        self.details_scroll = 0;
    }

    /// Enter on a Summary entry: show what it points at in its own tab.
    fn jump_to_summary_target(&mut self) -> Vec<Effect> {
        let Some(target) = self.selected_summary_entry().map(|e| e.target.clone()) else {
            return Vec::new();
        };
        match target {
            Target::None => self.set_status("this entry does not point anywhere"),
            Target::Item { repo_id, branch } => {
                if self.work_item(&repo_id, &branch).is_some() {
                    self.select_work_item(&repo_id, &branch);
                } else {
                    self.set_status(format!("{} is no longer present", clean_status(&branch)));
                }
            }
            Target::Todo { id, title } => match self.find_todo(id.as_deref(), &title) {
                Some(t) => {
                    let key = todo_key(t);
                    self.select_todo(key);
                }
                None => self.set_status("this todo is no longer present"),
            },
            Target::Session { id } => {
                if self.find_session(&id).is_some() {
                    self.select_session(id);
                } else {
                    self.set_status("this session is no longer present");
                }
            }
            Target::Inbox => {
                let id = self
                    .selected_summary_entry()
                    .and_then(|e| self.suggestion_for_entry(e))
                    .map(|s| s.id.to_string());
                self.switch_tab(Tab::Inbox);
                if id.is_some() {
                    self.search[Tab::Inbox.index()].clear();
                    self.reselect(Tab::Inbox, id.map(RowKey::item));
                }
            }
        }
        Vec::new()
    }

    fn summary_key(&mut self, key: KeyEvent) -> Option<Vec<Effect>> {
        let since = self
            .summary_since
            .unwrap_or_else(|| previous_workday(self.today(), &self.workdays));
        let none = Some(Vec::new());
        match key.code {
            KeyCode::Char('h') | KeyCode::Left => {
                self.set_summary_section(self.summary_section.step(-1));
                none
            }
            KeyCode::Char('l') | KeyCode::Right => {
                self.set_summary_section(self.summary_section.step(1));
                none
            }
            KeyCode::Enter if self.arrangement() != Arrangement::ListOnly => {
                Some(self.jump_to_summary_target())
            }
            KeyCode::Char('o') => {
                let Some(target) = self.selected_summary_entry().map(|e| e.target.clone()) else {
                    return self.needs_item();
                };
                match self.target_item(&target).cloned() {
                    Some(item) => Some(self.open_item_url(&item)),
                    None => {
                        self.set_status("no PR or Jira link for this entry");
                        none
                    }
                }
            }
            KeyCode::Char(' ') => {
                let target = self.selected_summary_entry()?.target.clone();
                let Target::Todo { id, title } = target else {
                    self.set_status("space marks todos done; this entry is not a todo");
                    return none;
                };
                let Some(todo) = self.find_todo(id.as_deref(), &title).cloned() else {
                    self.set_status("this todo is no longer present");
                    return none;
                };
                if self.refuse_todo_edit() {
                    return none;
                }
                let Some(id) = todo.id.clone() else {
                    self.set_status("this todo has no id yet; it gets one on the next write");
                    return none;
                };
                Some(self.toggle_done(id, &todo))
            }
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
                        self.cursor[idx] = self.first_item_row(self.tab);
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
            InputPurpose::AddTodo => {
                self.set_status("todo added");
                vec![self.add_todo(text, None, None, None)]
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
            Overlay::NewTodo(d) => self.new_todo_key(d, key),
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
            Confirm::MarkDone { id, .. } => {
                self.set_status("marked done");
                Command::Todo(TodoOp::Update {
                    id,
                    patch: TodoPatch {
                        status: Some(TodoStatus::Done),
                        done: Some(Some(self.today())),
                        ..Default::default()
                    },
                })
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
        let pos = Position::new(mouse.column, mouse.row);
        let delta = match mouse.kind {
            MouseEventKind::ScrollDown => WHEEL_ROWS,
            MouseEventKind::ScrollUp => -WHEEL_ROWS,
            MouseEventKind::Down(MouseButton::Left) => {
                self.click(pos);
                return Vec::new();
            }
            _ => return Vec::new(),
        };
        if self.overlay.is_some() {
            if let Some(Overlay::LinkPicker(p)) = &mut self.overlay {
                p.cursor = p.cursor.saturating_add_signed(delta.signum());
            } else if let Some(Overlay::NewTodo(d)) = &mut self.overlay {
                let last = d.seeds.len().checked_sub(1);
                d.selected = match (d.selected, last) {
                    (_, None) => None,
                    (None, Some(_)) if delta > 0 => Some(0),
                    (None, _) => None,
                    (Some(i), Some(last)) if delta > 0 => Some((i + 1).min(last)),
                    (Some(i), _) => i.checked_sub(1),
                };
            } else {
                self.scroll_overlay(delta);
            }
            return Vec::new();
        }
        if self
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

    /// Left click: select the tab or the filter / section label under the pointer; in the
    /// New todo dialog, highlight the suggestion under it.
    fn click(&mut self, pos: Position) {
        let hits = self.feedback.bar_hits.borrow().clone();
        if let Some(Overlay::NewTodo(d)) = &mut self.overlay {
            // The dialog covers the bars; only its own rows count.
            let seed = hits.iter().find_map(|(r, h)| match h {
                BarHit::Seed(i) if r.contains(pos) => Some(*i),
                _ => None,
            });
            if let Some(i) = seed.filter(|i| *i < d.seeds.len()) {
                d.selected = Some(i);
            }
            return;
        }
        if self.overlay.is_some() || self.input.is_some() {
            return;
        }
        let hit = hits.iter().find(|(r, _)| r.contains(pos)).map(|(_, h)| *h);
        match hit {
            Some(BarHit::Tab(tab)) => self.switch_tab(tab),
            Some(BarHit::Todo(f)) if self.tab == Tab::Todos => self.set_todo_filter(f),
            Some(BarHit::Summary(sec)) if self.tab == Tab::Summary => self.set_summary_section(sec),
            _ => {}
        }
    }
}

/// Status line after a todo is added: where to find it.
const TODO_ADDED: &str = "todo added · 2 Todos";

/// Branch names in status messages, sanitized.
fn clean_status(s: &str) -> String {
    crate::sanitize::clean_line(s)
}

fn todo_key(t: &Todo) -> String {
    t.id.clone().unwrap_or_else(|| format!("title:{}", t.title))
}

/// Display text and link of a suggestion.
// TODO(integration): once `SuggestionPayload::text()/link()` land, delegate to them.
pub fn suggestion_text(s: &Suggestion) -> (&str, Option<&str>) {
    (s.payload.text(), s.payload.link())
}

/// Whether a suggestion needs a keep / abandon choice (`?`) instead of a plain accept (`+`).
pub fn needs_choice(s: &Suggestion) -> bool {
    s.payload.needs_choice()
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

    /// Put the Sessions cursor on the row of session `id`.
    fn select_session_row(app: &mut App, id: &str) {
        app.cursor[2] = app
            .session_rows()
            .iter()
            .position(|r| matches!(r, ListRow::Item(i) if app.sessions[*i].id == id))
            .unwrap();
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
        press(&mut app, "2");
        let seen: Vec<TodoFilter> = (0..6)
            .map(|_| {
                press(&mut app, "f");
                app.todo_filter
            })
            .collect();
        assert_eq!(
            seen,
            vec![
                TodoFilter::Todo,
                TodoFilter::Doing,
                TodoFilter::Done,
                TodoFilter::Dropped,
                TodoFilter::All,
                TodoFilter::Active
            ]
        );
    }

    #[test]
    fn filters_select_the_right_items() {
        let mut app = sample::app();
        let branches = |app: &App| -> Vec<String> {
            app.work_rows()
                .into_iter()
                .filter_map(|r| match r {
                    ListRow::Item(i) => Some(app.work_items[i].branch.clone()),
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
            ListRow::Group(g) => g.label.contains("auth"),
            ListRow::Item(i) => app.work_items[*i].repo_name.contains("auth"),
        }));
        app.handle_key(key(KeyCode::Enter));
        assert!(app.input.is_none());
        assert_eq!(app.search[0], "@repo:auth");

        app.search[0] = "@state:needs".into();
        let items: Vec<_> = app
            .work_rows()
            .into_iter()
            .filter_map(|r| match r {
                ListRow::Item(i) => Some(app.work_items[i].state),
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
    fn every_grouped_list_stops_on_its_headers() {
        let mut app = sample::app();
        press(&mut app, "2G");
        for (tab, first) in [
            (Tab::Work, "API/upload-svc"),
            (Tab::Todos, "API/auth"),
            (Tab::Sessions, "Today"),
            (Tab::Summary, "~/work/upload-svc-wt/retry"),
        ] {
            app.tab = tab;
            assert_eq!(app.first_item_row(tab), 1, "{tab:?}");
            app.cursor[tab.index()] = 0;
            match app.selected_row(tab) {
                Some(ListRow::Group(g)) => assert_eq!(g.label, first),
                other => panic!("{tab:?}: {other:?}"),
            }
            assert!(app.on_group_header(tab));
            press(&mut app, "j");
            assert_eq!(app.cursor[tab.index()], 1);
            assert!(app.selected_item(tab).is_some(), "{tab:?}");
            // `G` (bottom) is the Todos grouping key, so only the other tabs check the end.
            if tab != Tab::Todos {
                press(&mut app, "G");
                assert_eq!(app.cursor[tab.index()], app.row_count(tab) - 1);
                press(&mut app, "g");
                assert_eq!(app.cursor[tab.index()], 0);
            }
        }
        assert_eq!(
            group_labels(&app, Tab::Todos),
            ["API/auth", "API/upload-svc", "(no link)"]
        );
        assert_eq!(
            group_labels(&app, Tab::Sessions),
            ["Today", "Yesterday", "Mon 23 Nov"]
        );
        // Ungrouped todos have no headers.
        press(&mut app, "2G");
        assert!(!app
            .todo_rows()
            .iter()
            .any(|r| matches!(r, ListRow::Group(_))));
    }

    #[test]
    fn enter_space_and_z_fold_the_same_way_in_every_list() {
        let mut app = sample::app();
        press(&mut app, "2G");
        for tab in [Tab::Work, Tab::Todos, Tab::Sessions, Tab::Summary] {
            app.tab = tab;
            let idx = tab.index();
            let before = app.row_count(tab);
            // Enter on the header folds, Space unfolds.
            app.cursor[idx] = 0;
            app.handle_key(key(KeyCode::Enter));
            assert!(app.row_count(tab) < before, "{tab:?}");
            assert!(matches!(app.selected_row(tab), Some(ListRow::Group(g)) if g.collapsed));
            app.handle_key(key(KeyCode::Char(' ')));
            assert_eq!(app.row_count(tab), before, "{tab:?}");
            // z from an item folds its group and puts the cursor on the header.
            app.cursor[idx] = 1;
            press(&mut app, "z");
            assert_eq!(app.cursor[idx], 0, "{tab:?}");
            assert!(app.row_count(tab) < before);
            press(&mut app, "z");
            assert_eq!(app.row_count(tab), before);
            assert_eq!(app.cursor[idx], 0);
        }
    }

    #[test]
    fn capital_z_folds_all_then_unfolds_all() {
        let mut app = sample::app();
        press(&mut app, "3");
        let groups = group_labels(&app, Tab::Sessions).len();
        // From the second session of Today.
        press(&mut app, "j");
        press(&mut app, "Z");
        assert_eq!(app.row_count(Tab::Sessions), groups, "only headers left");
        assert_eq!(app.cursor[2], 0, "the cursor goes to its group's header");
        assert_eq!(app.status.as_ref().unwrap().text, "folded all groups");
        // One open group is enough for Z to fold all again.
        press(&mut app, "jz");
        assert!(app.row_count(Tab::Sessions) > groups);
        press(&mut app, "Z");
        assert_eq!(app.row_count(Tab::Sessions), groups);
        press(&mut app, "Z");
        assert_eq!(app.status.as_ref().unwrap().text, "unfolded all groups");
        assert_eq!(
            app.row_count(Tab::Sessions),
            app.visible_sessions().len() + groups
        );
        // Todos without grouping: nothing to fold, a hint instead.
        press(&mut app, "2Z");
        assert!(app.status.as_ref().unwrap().text.contains("G groups"));
        assert!(app.collapsed[1].is_empty());
    }

    #[test]
    fn folding_persists_across_refreshes_per_tab() {
        let mut app = sample::app();
        app.cursor[0] = 0;
        app.handle_key(key(KeyCode::Enter));
        press(&mut app, "3");
        app.cursor[2] = 0;
        press(&mut app, "z");
        press(&mut app, "4");
        press(&mut app, "z");
        let summary_folded = summary_headers(&app)[0].clone();
        let work_rows = app.work_rows();
        let session_rows = app.row_count(Tab::Sessions);
        // Fresh data from the background: the same groups stay folded.
        let mut items = app.work_items.clone();
        items.reverse();
        app.handle_event(AppEvent::WorkItemsUpdated(items));
        app.handle_event(AppEvent::SessionsUpdated(sample::sessions()));
        app.handle_event(AppEvent::SummaryUpdated {
            since: NaiveDate::from_ymd_opt(2026, 11, 24).unwrap(),
            summary: sample::summary(),
            markdown: String::new(),
        });
        assert!(
            matches!(&app.work_rows()[0], ListRow::Group(g) if g.collapsed && g.key == "r_upload")
        );
        assert_eq!(app.work_rows().len(), work_rows.len());
        assert_eq!(app.row_count(Tab::Sessions), session_rows);
        assert!(app.collapsed[2].contains("2026-11-25"));
        let folded: Vec<String> = app
            .summary_rows()
            .into_iter()
            .filter_map(|r| match r {
                ListRow::Group(g) if g.collapsed => Some(g.label),
                _ => None,
            })
            .collect();
        assert_eq!(folded, [summary_folded]);
        // Each tab keeps its own set; Todos is untouched.
        assert!(app.collapsed[1].is_empty());
        // The cursor stays on the folded header through the refresh.
        assert!(app.on_group_header(Tab::Summary));
    }

    #[test]
    fn searching_opens_every_group() {
        let mut app = sample::app();
        press(&mut app, "3");
        press(&mut app, "Z");
        let folded = app.row_count(Tab::Sessions);
        press(&mut app, "/upload");
        // While typing, the cursor goes to the first match, its group open.
        assert!(app.selected_session().is_some());
        assert!(app.row_count(Tab::Sessions) > folded);
        app.handle_key(key(KeyCode::Enter));
        press(&mut app, "z");
        assert!(app
            .status
            .as_ref()
            .unwrap()
            .text
            .contains("while searching"));
        app.handle_key(key(KeyCode::Esc));
        assert_eq!(app.row_count(Tab::Sessions), folded, "the folds come back");
    }

    #[test]
    fn item_keys_on_a_header_only_hint() {
        let hinted = |app: &App| {
            app.status
                .as_ref()
                .is_some_and(|s| s.text.starts_with("select an item"))
        };
        let mut app = sample::app();
        app.cursor[0] = 0;
        for k in ["o", "y", "Y", "t", "c", "A", "U", "X"] {
            app.status = None;
            assert!(press(&mut app, k).is_empty(), "work {k}");
            assert!(hinted(&app), "work {k}");
            assert!(app.overlay.is_none() && app.input.is_none(), "work {k}");
        }
        press(&mut app, "2G");
        app.cursor[1] = 0;
        for k in ["s", "p", "e", "E", "D", "d", "L", "+", "-"] {
            app.status = None;
            assert!(press(&mut app, k).is_empty(), "todos {k}");
            assert!(hinted(&app), "todos {k}");
            assert!(app.overlay.is_none() && app.input.is_none(), "todos {k}");
        }
        // Space on a todo header folds the group instead of toggling done.
        assert!(press(&mut app, " ").is_empty());
        assert!(matches!(app.selected_row(Tab::Todos), Some(ListRow::Group(g)) if g.collapsed));
        press(&mut app, "3");
        app.cursor[2] = 0;
        for k in ["c", "s"] {
            app.status = None;
            assert!(press(&mut app, k).is_empty(), "sessions {k}");
            assert!(hinted(&app), "sessions {k}");
        }
        press(&mut app, "4");
        app.cursor[3] = 0;
        app.status = None;
        assert!(press(&mut app, "o").is_empty());
        assert!(hinted(&app));
        // Enter folds instead of jumping; Space unfolds instead of marking done.
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(app.tab, Tab::Summary);
        assert!(press(&mut app, " ").is_empty());
        assert!(matches!(app.selected_row(Tab::Summary), Some(ListRow::Group(g)) if !g.collapsed));
        // List-only: Enter on a header still folds rather than opening the details.
        app.feedback.arrangement.set(Some(Arrangement::ListOnly));
        app.handle_key(key(KeyCode::Enter));
        assert!(!app.details_open);
        assert!(matches!(app.selected_row(Tab::Summary), Some(ListRow::Group(g)) if g.collapsed));
    }

    #[test]
    fn jumps_expand_the_target_group() {
        // Sessions → Work.
        let mut app = sample::app();
        app.collapsed[0].insert("r_auth".into());
        press(&mut app, "3");
        select_session_row(&mut app, "s-sso");
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(app.tab, Tab::Work);
        assert!(app.collapsed[0].is_empty());
        assert_eq!(
            app.selected_work_item().unwrap().branch,
            "feature/PROJ-118-sso"
        );
        // Summary → grouped Todos with the todo's repo folded.
        let mut app = summary_app();
        app.todo_grouped = true;
        app.collapsed[1].insert("API/auth".into());
        select_entry(&mut app, SummarySection::Todo, "Review SSO");
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(app.tab, Tab::Todos);
        assert!(!app.collapsed[1].contains("API/auth"));
        assert_eq!(app.selected_todo().unwrap().id.as_deref(), Some("t_6"));
        // Summary → Sessions with the session's day folded.
        let mut app = summary_app();
        app.collapsed[2].insert("2026-11-25".into());
        select_entry(&mut app, SummarySection::Blockers, "IdP metadata");
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(app.tab, Tab::Sessions);
        assert!(app.collapsed[2].is_empty());
        assert_eq!(app.selected_session().unwrap().id, "s-sso");
    }

    #[test]
    fn a_folded_away_selection_falls_back_to_its_header() {
        let mut app = sample::app();
        press(&mut app, "3");
        select_session_row(&mut app, "s-sso");
        // Today gets folded while the cursor is on one of its sessions (another tab's
        // jump would do this; here directly).
        let key = app.selected_key(Tab::Sessions);
        app.collapsed[2].insert("2026-11-25".into());
        app.reselect(Tab::Sessions, key);
        assert_eq!(app.cursor[2], 0);
        assert!(app.on_group_header(Tab::Sessions));
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

    /// Put the Work cursor on the item of `branch`.
    fn select_work_row(app: &mut App, branch: &str) {
        app.cursor[0] = app
            .list_rows(Tab::Work)
            .iter()
            .position(|r| matches!(r, ListRow::Item(i) if app.work_items[*i].branch == branch))
            .unwrap();
    }

    fn new_todo(app: &App) -> &NewTodo {
        match &app.overlay {
            Some(Overlay::NewTodo(d)) => d,
            other => panic!("no New todo dialog: {other:?}"),
        }
    }

    fn seed_texts(d: &NewTodo) -> Vec<(&str, SeedKind, &str, bool)> {
        d.seeds
            .iter()
            .map(|s| (s.text.as_str(), s.kind, s.session_id.as_str(), s.exists))
            .collect()
    }

    fn only_add(effects: Vec<Effect>) -> Todo {
        match todo_ops(effects).as_slice() {
            [TodoOp::Add(t)] => t.clone(),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn t_in_work_opens_the_new_todo_dialog_with_the_item_link() {
        let mut app = sample::app();
        select_work_row(&mut app, "feature/PROJ-118-sso");
        assert!(press(&mut app, "t").is_empty());
        let d = new_todo(&app);
        assert_eq!(d.link.as_deref(), Some("API/auth:feature/PROJ-118-sso"));
        assert_eq!(d.priority, None);
        assert_eq!(
            seed_texts(d),
            vec![
                ("Logout flow untested", SeedKind::Unfinished, "s-sso", false),
                ("Test logout on staging", SeedKind::Todo, "s-sso", false),
                (
                    "Waiting for IdP metadata from IT",
                    SeedKind::Blocker,
                    "s-sso",
                    false
                ),
            ]
        );
        assert!(app.input.is_none(), "no inline input any more");
    }

    #[test]
    fn typing_a_title_and_enter_adds_one_linked_todo() {
        let mut app = sample::app();
        select_work_row(&mut app, "feature/PROJ-118-sso");
        press(&mut app, "tFollow up with QA");
        app.handle_key(key(KeyCode::Tab)); // none → A
        app.handle_key(key(KeyCode::Tab)); // A → B
        let t = only_add(app.handle_key(key(KeyCode::Enter)));
        assert_eq!(t.title, "Follow up with QA");
        assert_eq!(t.priority, Some(Priority::B));
        assert_eq!(t.link.as_deref(), Some("API/auth:feature/PROJ-118-sso"));
        assert_eq!(t.created, Some(app.today()));
        assert_eq!(t.source.as_deref(), Some("manual"));
        assert_eq!(t.status, TodoStatus::Todo);
        assert!(t.id.is_none(), "the background assigns the id");
        assert!(app.overlay.is_none());
        assert_eq!(app.status.as_ref().unwrap().text, "todo added · 2 Todos");
    }

    #[test]
    fn tab_and_shift_tab_cycle_the_priority() {
        assert_eq!(cycle_priority(None, false), Some(Priority::A));
        assert_eq!(cycle_priority(Some(Priority::C), false), None);
        assert_eq!(cycle_priority(None, true), Some(Priority::C));
        let mut app = sample::app();
        press(&mut app, "3");
        select_session_row(&mut app, "s-sso");
        press(&mut app, "t");
        app.handle_key(key(KeyCode::BackTab));
        assert_eq!(new_todo(&app).priority, Some(Priority::C));
        // Typing letters never changes it.
        press(&mut app, "abc");
        assert_eq!(new_todo(&app).priority, Some(Priority::C));
        assert_eq!(new_todo(&app).input.text, "abc");
    }

    #[test]
    fn t_on_a_group_header_only_hints() {
        let mut app = sample::app();
        app.cursor[0] = 0;
        assert!(press(&mut app, "t").is_empty());
        assert!(app.overlay.is_none());
        assert!(app.status.as_ref().unwrap().text.contains("select an item"));

        let mut app = sample::app();
        press(&mut app, "3");
        app.cursor[2] = 0;
        assert!(app.on_group_header(Tab::Sessions));
        assert!(press(&mut app, "t").is_empty());
        assert!(app.overlay.is_none());
        assert!(app.status.as_ref().unwrap().text.contains("select an item"));
    }

    #[test]
    fn t_in_sessions_links_the_working_branch_and_a_suggestion_sets_the_source() {
        let mut app = sample::app();
        press(&mut app, "3");
        select_session_row(&mut app, "s-sso");
        press(&mut app, "t");
        assert_eq!(
            new_todo(&app).link.as_deref(),
            Some("API/auth:feature/PROJ-118-sso")
        );
        app.handle_key(key(KeyCode::Down));
        app.handle_key(key(KeyCode::Down));
        assert_eq!(new_todo(&app).selected, Some(1));
        // First Enter copies the suggestion into the input for editing.
        assert!(app.handle_key(key(KeyCode::Enter)).is_empty());
        assert_eq!(new_todo(&app).input.text, "Test logout on staging");
        // The second adds it, sourced from the session.
        let t = only_add(app.handle_key(key(KeyCode::Enter)));
        assert_eq!(t.title, "Test logout on staging");
        assert_eq!(t.source.as_deref(), Some("session:s-sso"));
        assert_eq!(t.link.as_deref(), Some("API/auth:feature/PROJ-118-sso"));
        assert!(app.overlay.is_none());
    }

    #[test]
    fn an_edited_suggestion_keeps_its_session_until_the_input_is_cleared() {
        let mut app = sample::app();
        press(&mut app, "3");
        select_session_row(&mut app, "s-sso");
        press(&mut app, "t");
        app.handle_key(key(KeyCode::Down));
        app.handle_key(key(KeyCode::Enter));
        press(&mut app, " today");
        let t = only_add(app.handle_key(key(KeyCode::Enter)));
        assert_eq!(t.title, "Logout flow untested today");
        assert_eq!(t.source.as_deref(), Some("session:s-sso"));

        press(&mut app, "t");
        app.handle_key(key(KeyCode::Down));
        app.handle_key(key(KeyCode::Enter));
        app.handle_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
        press(&mut app, "Something else");
        let t = only_add(app.handle_key(key(KeyCode::Enter)));
        assert_eq!(t.source.as_deref(), Some("manual"));
    }

    #[test]
    fn work_suggestions_come_from_every_linked_session_newest_first() {
        let mut app = sample::app();
        let mut older = app.find_session("s-sso").unwrap().clone();
        older.id = "s-old".into();
        older.last_activity_at = older.last_activity_at.map(|t| t - 86_400);
        older.summary = Some(crate::sessions::SessionSummary {
            unfinished: vec!["logout  FLOW untested".into(), "Rotate the SP cert".into()],
            ..Default::default()
        });
        app.sessions.push(older);
        select_work_row(&mut app, "feature/PROJ-118-sso");
        press(&mut app, "t");
        let d = new_todo(&app);
        let texts: Vec<(&str, &str)> = d
            .seeds
            .iter()
            .map(|s| (s.text.as_str(), s.session_id.as_str()))
            .collect();
        assert_eq!(
            texts,
            vec![
                ("Logout flow untested", "s-sso"),
                ("Test logout on staging", "s-sso"),
                ("Waiting for IdP metadata from IT", "s-sso"),
                ("Rotate the SP cert", "s-old"),
            ]
        );
        // Picking the older session's suggestion sources it from that session.
        for _ in 0..4 {
            app.handle_key(key(KeyCode::Down));
        }
        app.handle_key(key(KeyCode::Enter));
        let t = only_add(app.handle_key(key(KeyCode::Enter)));
        assert_eq!(t.source.as_deref(), Some("session:s-old"));
    }

    #[test]
    fn suggestions_are_deduplicated_and_marked_when_the_todo_exists() {
        let mut app = sample::app();
        let s = app.sessions.iter_mut().find(|s| s.id == "s-sso").unwrap();
        s.explicit_todos = vec![
            "test  logout ON staging".into(),
            "Review SSO rollout plan".into(),
            "   ".into(),
        ];
        press(&mut app, "3");
        select_session_row(&mut app, "s-sso");
        press(&mut app, "t");
        assert_eq!(
            seed_texts(new_todo(&app)),
            vec![
                ("Logout flow untested", SeedKind::Unfinished, "s-sso", false),
                ("Test logout on staging", SeedKind::Todo, "s-sso", false),
                (
                    "Waiting for IdP metadata from IT",
                    SeedKind::Blocker,
                    "s-sso",
                    false
                ),
                ("Review SSO rollout plan", SeedKind::Todo, "s-sso", true),
            ]
        );
    }

    #[test]
    fn esc_closes_the_dialog_without_effects() {
        let mut app = sample::app();
        select_work_row(&mut app, "feature/PROJ-118-sso");
        press(&mut app, "tnope");
        assert!(app.handle_key(key(KeyCode::Esc)).is_empty());
        assert!(app.overlay.is_none());
    }

    #[test]
    fn an_empty_title_only_hints() {
        let mut app = sample::app();
        select_work_row(&mut app, "feature/PROJ-118-sso");
        press(&mut app, "t   ");
        assert!(app.handle_key(key(KeyCode::Enter)).is_empty());
        assert!(matches!(app.overlay, Some(Overlay::NewTodo(_))));
        assert!(app.status.as_ref().unwrap().text.contains("type a title"));
    }

    #[test]
    fn the_new_todo_dialog_is_refused_while_the_todo_file_does_not_parse() {
        for tab in ["1", "3"] {
            let mut app = sample::app();
            app.todo_error = Some("line 3: bad".into());
            press(&mut app, tab);
            if tab == "1" {
                select_work_row(&mut app, "feature/PROJ-118-sso");
            } else {
                select_session_row(&mut app, "s-sso");
            }
            assert!(press(&mut app, "t").is_empty());
            assert!(app.overlay.is_none());
            assert!(app.status.as_ref().unwrap().text.contains("parse error"));
        }
    }

    #[test]
    fn an_unlinked_session_still_makes_a_todo_without_link() {
        let mut app = sample::app();
        press(&mut app, "3");
        select_session_row(&mut app, "s-nobranch");
        press(&mut app, "t");
        let d = new_todo(&app);
        assert_eq!(d.link, None);
        assert!(d.seeds.is_empty());
        press(&mut app, "Read the webpack docs");
        let t = only_add(app.handle_key(key(KeyCode::Enter)));
        assert_eq!(t.link, None);
        assert_eq!(t.source.as_deref(), Some("manual"));
    }

    #[test]
    fn ctrl_s_and_alt_enter_add_and_keep_the_dialog_open() {
        let mut app = sample::app();
        press(&mut app, "3");
        select_session_row(&mut app, "s-sso");
        press(&mut app, "t");
        app.handle_key(key(KeyCode::Down));
        app.handle_key(key(KeyCode::Enter));
        let t = only_add(app.handle_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL)));
        assert_eq!(t.title, "Logout flow untested");
        let d = new_todo(&app);
        assert_eq!(d.input.text, "");
        assert_eq!(d.selected, None);
        assert!(d.seeds[0].exists, "the added suggestion is marked");
        press(&mut app, "Second one");
        let t = only_add(app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT)));
        assert_eq!(t.title, "Second one");
        assert_eq!(t.source.as_deref(), Some("manual"));
        assert!(matches!(app.overlay, Some(Overlay::NewTodo(_))));
        assert_eq!(app.status.as_ref().unwrap().text, "todo added · 2 Todos");
    }

    #[test]
    fn up_from_the_first_suggestion_leaves_the_list() {
        let mut app = sample::app();
        press(&mut app, "3");
        select_session_row(&mut app, "s-sso");
        press(&mut app, "t");
        for _ in 0..9 {
            app.handle_key(key(KeyCode::Down));
        }
        assert_eq!(new_todo(&app).selected, Some(2));
        for _ in 0..3 {
            app.handle_key(key(KeyCode::Up));
        }
        assert_eq!(new_todo(&app).selected, None);
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
    fn session_search_matches_the_session_id() {
        let mut app = sample::app();
        press(&mut app, "3");
        let id = "8d4b3f35-49c6-43b1-9380-9d6d3780b077";
        let s = app.sessions.iter_mut().find(|s| s.id == "s-sso").unwrap();
        s.id = id.into();
        // Full id, a prefix (as `claude --resume` output shows), any case.
        for q in [id, "8d4b3f35", "8D4B3F35-49C6"] {
            app.search[Tab::Sessions.index()] = q.into();
            let found: Vec<&str> = app
                .visible_sessions()
                .into_iter()
                .map(|i| app.sessions[i].id.as_str())
                .collect();
            assert_eq!(found, vec![id], "{q}");
        }
    }

    #[test]
    fn enter_on_a_todo_goes_to_its_session() {
        let mut app = sample::app();
        press(&mut app, "2");
        // t_1 is linked to feature/PROJ-123-retry and was typed by hand: the latest session
        // on that branch.
        select_todo(&mut app, "t_1");
        let latest = {
            let t = app.selected_todo().unwrap();
            let (s, kind) = app.todo_session(t).unwrap();
            assert_eq!(kind, TodoSessionKind::LatestOnBranch);
            s.id.clone()
        };
        assert!(app.handle_key(key(KeyCode::Enter)).is_empty());
        assert_eq!(app.tab, Tab::Sessions);
        assert_eq!(app.selected_session().unwrap().id, latest);

        // A todo created from a session goes to that session, even if newer ones exist.
        press(&mut app, "2");
        let t = app
            .todos
            .iter_mut()
            .find(|t| t.id.as_deref() == Some("t_1"))
            .unwrap();
        t.source = Some("session:s-sso".into());
        select_todo(&mut app, "t_1");
        assert_eq!(
            app.todo_session(app.selected_todo().unwrap()).unwrap().1,
            TodoSessionKind::CreatedFrom
        );
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(app.tab, Tab::Sessions);
        assert_eq!(app.selected_session().unwrap().id, "s-sso");

        // No session and no link: stay, explain.
        press(&mut app, "2");
        select_todo(&mut app, "t_3");
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(app.tab, Tab::Todos);
        assert!(app.status.as_ref().unwrap().text.contains("no session"));
    }

    #[test]
    fn todo_single_keys_emit_the_right_ops() {
        let mut app = sample::app();
        let today = app.today();
        press(&mut app, "2");
        select_todo(&mut app, "t_2");
        // t_2 is status todo, priority B. Space asks first; Esc cancels, Enter confirms.
        assert!(press(&mut app, " ").is_empty());
        assert!(matches!(
            app.overlay,
            Some(Overlay::Confirm(Confirm::MarkDone { .. }))
        ));
        assert!(app.handle_key(key(KeyCode::Esc)).is_empty());
        assert!(app.overlay.is_none());
        press(&mut app, " ");
        assert_eq!(
            todo_ops(app.handle_key(key(KeyCode::Enter))),
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
    fn resume_uses_the_session_cwd_without_a_known_worktree() {
        let mut app = sample::app();
        press(&mut app, "3");
        select_session_row(&mut app, "s-nobranch");
        let cwd = app.selected_session().unwrap().cwd.clone().unwrap();
        match press(&mut app, "c").as_slice() {
            [Effect::CopyToClipboard(cmd)] => {
                assert_eq!(
                    cmd,
                    &format!("cd {} && claude --resume s-nobranch", shell_quote(&cwd))
                )
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn resume_prefers_the_session_cwd_over_the_worktree_root() {
        let mut app = sample::app();
        press(&mut app, "3");
        select_session_row(&mut app, "s-live");
        let s = app.sessions.iter_mut().find(|s| s.id == "s-live").unwrap();
        s.cwd = Some("/home/dev/work/upload-svc-wt/retry/src/api".into());
        match press(&mut app, "c").as_slice() {
            [Effect::CopyToClipboard(cmd)] => assert_eq!(
                cmd,
                "cd '/home/dev/work/upload-svc-wt/retry/src/api' && claude --resume s-live"
            ),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn resume_without_any_directory_copies_nothing() {
        let mut app = sample::app();
        press(&mut app, "3");
        select_session_row(&mut app, "s-nobranch");
        let s = app
            .sessions
            .iter_mut()
            .find(|s| s.id == "s-nobranch")
            .unwrap();
        s.cwd = None;
        assert!(press(&mut app, "c").is_empty());
        assert!(app
            .status
            .as_ref()
            .unwrap()
            .text
            .contains("no recorded directory"));
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
    fn work_c_finds_a_session_that_worked_in_the_branch_worktree() {
        let mut app = sample::app();
        // Started in another worktree on its own branch, edited files in the
        // spike/s3-multipart worktree: newer than the session started there.
        let mut s = app
            .sessions
            .iter()
            .find(|s| s.id == "s-failed")
            .unwrap()
            .clone();
        s.id = "s-elsewhere".into();
        s.cwd = Some("/home/dev/work/upload-svc-wt/bugfix".into());
        s.branches = vec!["bugfix/x".into()];
        s.last_activity_at = s.last_activity_at.map(|t| t + 3600);
        s.links = vec![
            SessionLink {
                repo_id: "r_upload".into(),
                branch: "bugfix/x".into(),
            },
            SessionLink {
                repo_id: "r_upload".into(),
                branch: "spike/s3-multipart".into(),
            },
        ];
        s.worked_in = vec![crate::sessions::WorkedIn {
            path: "/home/dev/work/upload-svc".into(),
            repo_id: "r_upload".into(),
            branch: "spike/s3-multipart".into(),
        }];
        app.sessions.push(s);
        app.search[0] = "spike".into();
        app.cursor[0] = 1;
        let item = app.selected_work_item().unwrap().clone();
        assert_eq!(item.branch, "spike/s3-multipart");
        let ids: Vec<&str> = app
            .item_sessions(&item)
            .iter()
            .map(|s| s.id.as_str())
            .collect();
        assert_eq!(ids, vec!["s-elsewhere", "s-failed"]);
        match press(&mut app, "c").as_slice() {
            [Effect::CopyToClipboard(cmd)] => assert_eq!(
                cmd,
                "cd '/home/dev/work/upload-svc-wt/bugfix' && claude --resume s-elsewhere"
            ),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn sessions_enter_jumps_to_the_work_item() {
        let mut app = sample::app();
        press(&mut app, "3");
        select_session_row(&mut app, "s-sso");
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

    fn summary_app() -> App {
        let mut app = sample::app();
        press(&mut app, "4");
        app
    }

    /// `(row, line)` of every entry row of the current Summary section, in display order.
    fn summary_entry_rows(app: &App) -> Vec<(usize, String)> {
        let entries = app.summary_section_entries();
        app.summary_rows()
            .into_iter()
            .enumerate()
            .filter_map(|(n, r)| match r {
                ListRow::Item(i) => Some((n, entries[i].line.clone())),
                ListRow::Group(_) => None,
            })
            .collect()
    }

    /// Put the Summary cursor on the first entry of `sec` whose line contains `text`.
    fn select_entry(app: &mut App, sec: SummarySection, text: &str) {
        app.summary_section = sec;
        app.cursor[3] = summary_entry_rows(app)
            .into_iter()
            .find(|(_, l)| l.contains(text))
            .unwrap_or_else(|| panic!("no {text} in {sec:?}"))
            .0;
    }

    /// Labels of the group headers of a tab's list.
    fn group_labels(app: &App, tab: Tab) -> Vec<String> {
        app.list_rows(tab)
            .into_iter()
            .filter_map(|r| match r {
                ListRow::Group(g) => Some(g.label),
                ListRow::Item(_) => None,
            })
            .collect()
    }

    fn summary_headers(app: &App) -> Vec<String> {
        group_labels(app, Tab::Summary)
    }

    #[test]
    fn summary_rows_group_entries_by_working_directory() {
        let mut app = summary_app();
        // Items by their worktree, the unlinked session by its cwd; newest activity first.
        assert_eq!(
            summary_headers(&app),
            [
                "~/work/upload-svc-wt/retry",
                "~/work/upload-svc",
                "~/work/auth-wt/sso",
                "/tmp/scratch"
            ]
        );
        // The cursor walks headers and entries in display order, starting on the first
        // entry.
        let lines: Vec<String> = summary_entry_rows(&app).into_iter().map(|e| e.1).collect();
        assert_eq!(app.row_count(Tab::Summary), lines.len() + 4);
        assert!(lines[0].contains("PROJ-123-retry"), "{lines:?}");
        assert!(lines[1].contains("PROJ-130-chunking"), "{lines:?}");
        assert_eq!(app.cursor[3], 1);
        press(&mut app, "j");
        assert!(app.on_group_header(Tab::Summary));
        assert!(app.selected_summary_entry().is_none());
        press(&mut app, "j");
        assert!(app
            .selected_summary_entry()
            .unwrap()
            .line
            .contains("PROJ-130-chunking"));
        // Todos: the linked item's directory, else `(no directory)`.
        app.summary_section = SummarySection::Todo;
        assert_eq!(
            summary_headers(&app),
            ["~/work/auth-wt/sso", "(no directory)"]
        );
        // Inbox choices go last, under `(other)`.
        app.summary_section = SummarySection::Blockers;
        assert_eq!(summary_headers(&app).last().unwrap(), "(other)");
        // The exported Markdown is not affected by the grouping.
        assert_eq!(
            app.summary_markdown.as_deref(),
            Some(crate::summary::to_markdown(app.summary.as_ref().unwrap()).as_str())
        );
    }

    #[test]
    fn summary_opens_on_the_first_non_empty_section() {
        let app = summary_app();
        assert_eq!(app.summary_section, SummarySection::InProgress);

        let mut app = App::new(&Config::default(), Tab::Summary);
        let mut s = sample::summary();
        s.in_progress.clear();
        s.done.clear();
        let since = NaiveDate::from_ymd_opt(2026, 11, 24).unwrap();
        app.handle_event(AppEvent::SummaryUpdated {
            since,
            summary: s.clone(),
            markdown: String::new(),
        });
        assert_eq!(app.summary_section, SummarySection::Todo);
        // Later updates keep the section the user is on.
        app.summary_section = SummarySection::Cleanup;
        app.handle_event(AppEvent::SummaryUpdated {
            since,
            summary: sample::summary(),
            markdown: String::new(),
        });
        assert_eq!(app.summary_section, SummarySection::Cleanup);
    }

    #[test]
    fn summary_sections_switch_with_h_l_arrows_and_f() {
        let mut app = summary_app();
        assert!(app.row_count(Tab::Summary) > summary_entry_rows(&app).len());
        press(&mut app, "jj");
        assert_eq!(app.cursor[3], 3);
        press(&mut app, "l");
        assert_eq!(app.summary_section, SummarySection::Todo);
        assert_eq!(app.cursor[3], 1, "a new section starts on its first entry");
        press(&mut app, "h");
        assert_eq!(app.summary_section, SummarySection::InProgress);
        app.handle_key(key(KeyCode::Right));
        assert_eq!(app.summary_section, SummarySection::Todo);
        app.handle_key(key(KeyCode::Left));
        app.handle_key(key(KeyCode::Left));
        assert_eq!(app.summary_section, SummarySection::Done);
        app.handle_key(key(KeyCode::Left));
        assert_eq!(app.summary_section, SummarySection::Cleanup, "wraps around");
        press(&mut app, "f");
        assert_eq!(app.summary_section, SummarySection::Done);
        press(&mut app, "ff");
        assert_eq!(app.summary_section, SummarySection::Todo);
        // j/k/G move within the section.
        press(&mut app, "G");
        assert_eq!(app.cursor[3], app.row_count(Tab::Summary) - 1);
        press(&mut app, "g");
        assert_eq!(app.cursor[3], 0);
    }

    #[test]
    fn summary_selection_survives_an_update() {
        let mut app = summary_app();
        select_entry(&mut app, SummarySection::InProgress, "chunking");
        let mut s = sample::summary();
        s.in_progress.reverse();
        app.handle_event(AppEvent::SummaryUpdated {
            since: NaiveDate::from_ymd_opt(2026, 11, 24).unwrap(),
            summary: s,
            markdown: String::new(),
        });
        assert!(app
            .selected_summary_entry()
            .unwrap()
            .line
            .contains("chunking"));
    }

    #[test]
    fn summary_enter_jumps_to_a_hidden_work_item() {
        let mut app = summary_app();
        // bugfix/flaky is merged: hidden by the Unfinished filter, the search and a
        // collapsed group.
        app.search[0] = "retry".into();
        app.collapsed[0].insert("r_auth".into());
        select_entry(&mut app, SummarySection::Cleanup, "bugfix/flaky");
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(app.tab, Tab::Work);
        assert_eq!(app.work_filter, WorkFilter::All);
        assert!(app.search[0].is_empty());
        assert!(!app.collapsed[0].contains("r_auth"));
        assert_eq!(app.selected_work_item().unwrap().branch, "bugfix/flaky");
    }

    #[test]
    fn summary_enter_keeps_the_work_filter_when_the_item_shows() {
        let mut app = summary_app();
        select_entry(&mut app, SummarySection::InProgress, "chunking");
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(app.tab, Tab::Work);
        assert_eq!(app.work_filter, WorkFilter::Unfinished);
        assert_eq!(
            app.selected_work_item().unwrap().branch,
            "feature/PROJ-130-chunking"
        );
    }

    #[test]
    fn summary_enter_jumps_to_todos_widening_the_filter() {
        let mut app = summary_app();
        app.todo_filter = TodoFilter::Done;
        select_entry(&mut app, SummarySection::Todo, "Call Ivan");
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(app.tab, Tab::Todos);
        assert_eq!(app.todo_filter, TodoFilter::All);
        assert_eq!(app.selected_todo().unwrap().id.as_deref(), Some("t_3"));

        // Visible under the current filter: the filter stays.
        let mut app = summary_app();
        app.todo_filter = TodoFilter::Todo;
        select_entry(&mut app, SummarySection::Todo, "Review SSO");
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(app.todo_filter, TodoFilter::Todo);
        assert_eq!(app.selected_todo().unwrap().id.as_deref(), Some("t_6"));
    }

    #[test]
    fn summary_enter_jumps_to_sessions_and_inbox() {
        let mut app = summary_app();
        app.search[2] = "nothing matches".into();
        select_entry(&mut app, SummarySection::Blockers, "IdP metadata");
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(app.tab, Tab::Sessions);
        assert!(app.search[2].is_empty());
        assert_eq!(app.selected_session().unwrap().id, "s-sso");

        let mut app = summary_app();
        select_entry(&mut app, SummarySection::Blockers, "PR #31 declined");
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(app.tab, Tab::Inbox);
        assert_eq!(app.selected_suggestion().unwrap().id, 2);
    }

    #[test]
    fn summary_enter_on_a_vanished_target_stays() {
        let mut app = summary_app();
        select_entry(&mut app, SummarySection::Cleanup, "bugfix/flaky");
        app.work_items.retain(|w| w.branch != "bugfix/flaky");
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(app.tab, Tab::Summary);
        assert!(app
            .status
            .as_ref()
            .unwrap()
            .text
            .contains("no longer present"));
    }

    #[test]
    fn summary_list_only_enter_opens_details_then_jumps() {
        let mut app = summary_app();
        app.feedback.arrangement.set(Some(Arrangement::ListOnly));
        select_entry(&mut app, SummarySection::InProgress, "chunking");
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(app.tab, Tab::Summary);
        assert!(app.details_open);
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(app.tab, Tab::Work);
        assert!(!app.details_open);
        assert_eq!(
            app.selected_work_item().unwrap().branch,
            "feature/PROJ-130-chunking"
        );
    }

    #[test]
    fn summary_space_toggles_a_todo_entry() {
        let mut app = summary_app();
        let today = app.today();
        select_entry(&mut app, SummarySection::Todo, "migration doc");
        assert!(press(&mut app, " ").is_empty());
        assert_eq!(
            todo_ops(app.handle_key(key(KeyCode::Enter))),
            vec![TodoOp::Update {
                id: "t_2".into(),
                patch: TodoPatch {
                    status: Some(TodoStatus::Done),
                    done: Some(Some(today)),
                    ..Default::default()
                }
            }]
        );
        // A done todo goes back to todo.
        select_entry(&mut app, SummarySection::Done, "bugfix/flaky");
        app.summary.as_mut().unwrap().done[0].target = Target::Todo {
            id: Some("t_4".into()),
            title: "Fix flaky auth test".into(),
        };
        assert_eq!(
            todo_ops(press(&mut app, " ")),
            vec![TodoOp::Update {
                id: "t_4".into(),
                patch: TodoPatch {
                    status: Some(TodoStatus::Todo),
                    done: Some(None),
                    ..Default::default()
                }
            }]
        );
        // Not a todo: nothing happens.
        select_entry(&mut app, SummarySection::InProgress, "retry");
        assert!(press(&mut app, " ").is_empty());
    }

    #[test]
    fn summary_o_opens_the_item_pr_or_jira() {
        let mut app = summary_app();
        select_entry(&mut app, SummarySection::InProgress, "retry");
        let c = commands(press(&mut app, "o"));
        assert!(matches!(c.as_slice(), [Command::OpenUrl(u)] if u.ends_with("/pull-requests/44")));
        // A todo entry opens its linked item's PR.
        select_entry(&mut app, SummarySection::Todo, "Review SSO");
        let c = commands(press(&mut app, "o"));
        assert!(matches!(c.as_slice(), [Command::OpenUrl(u)] if u.ends_with("/pull-requests/39")));
        // Unlinked todo: nothing to open.
        select_entry(&mut app, SummarySection::Todo, "migration doc");
        assert!(commands(press(&mut app, "o")).is_empty());
        // y and w still work on the Summary.
        assert!(matches!(
            press(&mut app, "y").as_slice(),
            [Effect::CopyToClipboard(md)] if md.starts_with("# Summary")
        ));
    }

    #[test]
    fn todo_filters_move_with_h_l_and_arrows_and_count() {
        let mut app = sample::app();
        press(&mut app, "2");
        let counts: Vec<usize> = TodoFilter::ALL
            .iter()
            .map(|f| app.todo_filter_count(*f))
            .collect();
        // Active, Todo, Doing, Done, Dropped, All
        assert_eq!(counts, vec![4, 3, 1, 1, 1, 6]);
        press(&mut app, "l");
        assert_eq!(app.todo_filter, TodoFilter::Todo);
        app.handle_key(key(KeyCode::Right));
        assert_eq!(app.todo_filter, TodoFilter::Doing);
        assert_eq!(app.selected_todo().unwrap().id.as_deref(), Some("t_1"));
        assert_eq!(app.visible_todos().len(), 1);
        press(&mut app, "h");
        assert_eq!(app.todo_filter, TodoFilter::Todo);
        app.handle_key(key(KeyCode::Left));
        assert_eq!(app.todo_filter, TodoFilter::Active);
        app.handle_key(key(KeyCode::Left));
        assert_eq!(app.todo_filter, TodoFilter::All, "wraps around");
        // Counts ignore the search; the list does not.
        app.search[1] = "flaky".into();
        assert_eq!(app.todo_filter_count(TodoFilter::All), 6);
        assert_eq!(app.visible_todos().len(), 1);
        app.search[1].clear();
        press(&mut app, "hh");
        assert_eq!(app.todo_filter, TodoFilter::Done);
        let ids: Vec<_> = app
            .visible_todos()
            .iter()
            .map(|&i| app.todos[i].id.clone().unwrap())
            .collect();
        assert_eq!(ids, vec!["t_4"]);
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
        assert_eq!(app.cursor[0], 4, "three rows down from the first item");
    }
}
