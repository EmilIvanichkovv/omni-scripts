// Application state management

use crate::cache::PrCache;
use crate::git::{self, BranchInfo, BranchStatus};
use crate::pr::PrProviderKind;
use crate::refresh::{JobResult, RefreshKind, Refresher};
use crate::worktree_view::{WorktreeFilter, WorktreeInfo, WorktreeView};
use std::collections::HashSet;
use std::path::Path;
use std::time::{Duration, Instant};

/// How long after a first Esc press a second one quits the app
pub const ESC_QUIT_WINDOW: Duration = Duration::from_secs(1);

/// How long the footer shows "updated HH:MM:SS" after a reload
pub const RELOAD_NOTICE_DURATION: Duration = Duration::from_secs(5);

/// Sort mode for branch list
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SortMode {
    /// Sort by status (current default) - protected/current first, then by status type
    #[default]
    Status,
    /// Sort alphabetically by branch name
    Name,
    /// Sort by last activity (last commit), newest first
    ActivityNewest,
    /// Sort by last activity (last commit), oldest first
    ActivityOldest,
    /// Sort by branch creation date, newest first
    CreatedNewest,
    /// Sort by branch creation date, oldest first
    CreatedOldest,
}

impl SortMode {
    /// Get the label for the sort mode
    pub fn label(&self) -> &'static str {
        match self {
            SortMode::Status => "Status",
            SortMode::Name => "Name",
            SortMode::ActivityNewest => "Active ↓",
            SortMode::ActivityOldest => "Active ↑",
            SortMode::CreatedNewest => "Created ↓",
            SortMode::CreatedOldest => "Created ↑",
        }
    }

    /// Cycle to the next sort mode
    pub fn next(&self) -> Self {
        match self {
            SortMode::Status => SortMode::Name,
            SortMode::Name => SortMode::ActivityNewest,
            SortMode::ActivityNewest => SortMode::ActivityOldest,
            SortMode::ActivityOldest => SortMode::CreatedNewest,
            SortMode::CreatedNewest => SortMode::CreatedOldest,
            SortMode::CreatedOldest => SortMode::Status,
        }
    }
}

/// Filter mode for branch list
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterMode {
    All,
    SafeMerged,
    GoneUpstream,
    Unmerged,
}

impl FilterMode {
    /// Get the label for the filter
    pub fn label(&self) -> &'static str {
        match self {
            FilterMode::All => "ALL",
            FilterMode::SafeMerged => "SAFE MERGED",
            FilterMode::GoneUpstream => "UPSTREAM GONE",
            FilterMode::Unmerged => "UNMERGED",
        }
    }

    /// Cycle to the next filter mode
    pub fn next(&self) -> Self {
        match self {
            FilterMode::All => FilterMode::SafeMerged,
            FilterMode::SafeMerged => FilterMode::GoneUpstream,
            FilterMode::GoneUpstream => FilterMode::Unmerged,
            FilterMode::Unmerged => FilterMode::All,
        }
    }

    /// Get filter by number (1-4)
    #[allow(dead_code)]
    pub fn from_number(n: u8) -> Option<Self> {
        match n {
            1 => Some(FilterMode::SafeMerged),
            2 => Some(FilterMode::GoneUpstream),
            3 => Some(FilterMode::Unmerged),
            4 => Some(FilterMode::All),
            _ => None,
        }
    }
}

/// Which list the TUI shows
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ViewMode {
    #[default]
    Branches,
    Worktrees,
}

/// Cursor, scroll and search state of the view that is not on screen
#[derive(Debug, Clone, Default)]
struct NavState {
    selected_index: usize,
    scroll_offset: usize,
    search_query: String,
    search_cursor_pos: usize,
}

/// Whether deleting a branch uses `git branch -D`: force mode, force-only
/// statuses (unmerged, local, pr-diverged) and gone branches (squash/rebase
/// merges that `-d` would refuse). Shared by the branch and worktree views.
pub fn branch_delete_needs_force(status: &BranchStatus, force_mode: bool) -> bool {
    force_mode || status.requires_force() || *status == BranchStatus::GoneUpstream
}

/// Unique branch authors, sorted case-insensitively, for `@author:` completion
fn collect_authors(branches: &[BranchInfo]) -> Vec<String> {
    let mut authors: Vec<String> = branches
        .iter()
        .map(|b| b.branch_author.clone())
        .filter(|a| !a.is_empty())
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    authors.sort_by_key(|a| a.to_lowercase());
    authors
}

/// Log entry for deletion actions
#[derive(Debug, Clone)]
pub struct ActionLogEntry {
    pub branch_name: String,
    pub success: bool,
    pub message: String,
}

/// Main application state
pub struct App {
    /// All discovered branches
    pub branches: Vec<BranchInfo>,
    /// Currently selected branch index (cursor) - relative to filtered list
    pub selected_index: usize,
    /// Names of the branches selected for deletion. Stored by name, so the
    /// selection follows its branches when the list is re-sorted or reloaded.
    pub selected_branches: HashSet<String>,
    /// Whether the app should quit
    pub should_quit: bool,
    /// When Esc was pressed in normal mode; a second Esc within `ESC_QUIT_WINDOW` quits
    pub esc_quit_armed_at: Option<Instant>,
    /// Repository path
    pub repo_path: String,
    /// Trunk branch name
    pub trunk: String,
    /// Whether to show confirmation modal
    pub show_confirmation: bool,
    /// Action log entries
    pub action_log: Vec<ActionLogEntry>,
    /// Force mode for deleting unmerged branches
    pub force_mode: bool,
    /// Current filter mode
    pub current_filter: FilterMode,
    /// Whether to show help modal
    pub show_help: bool,
    /// Whether to show info modal
    pub show_info: bool,
    /// Dry run mode - preview actions without executing
    pub dry_run: bool,
    /// Whether to show filter tabs (hidden by default)
    pub show_filter: bool,
    /// Whether search mode is active (input focused)
    pub search_active: bool,
    /// Current search query string
    pub search_query: String,
    /// Cursor position in search query (0-indexed)
    pub search_cursor_pos: usize,
    /// Scroll offset for the branch list viewport
    pub scroll_offset: usize,
    /// Visible height of the branch list (set during render)
    pub visible_height: usize,
    /// Current sort mode
    pub sort_mode: SortMode,
    /// Current git user name (for @author:me filter)
    pub current_git_user: String,
    /// Unique branch authors for autocomplete
    pub unique_authors: Vec<String>,
    /// Current autocomplete suggestions
    pub suggestions: Vec<String>,
    /// Selected suggestion index (None if no suggestion selected)
    pub suggestion_index: Option<usize>,
    /// Whether to show suggestions dropdown
    pub show_suggestions: bool,
    /// Which PR provider is enabled (None = PR integration disabled)
    pub pr_provider: Option<PrProviderKind>,
    /// PR cache opened at startup; deleting a branch drops its entry
    pub pr_cache: Option<PrCache>,
    /// "Update available" line for the footer, when a newer release is known
    pub update_notice: Option<String>,
    /// Which list is on screen
    pub view: ViewMode,
    /// Worktree view state, loaded the first time the view opens
    pub worktrees: Option<WorktreeView>,
    /// Navigation state of the view that is not on screen
    other_nav: NavState,
    /// One-off message shown in the footer until the next key press
    pub flash: Option<String>,
    /// Whether the worktree view can select and remove worktrees (`x`).
    /// Off on every launch: the view starts as a read-only explorer.
    pub worktree_remove_mode: bool,
    /// Background reloads of the branch list
    pub refresher: Refresher,
    /// When the last reload changed the list, and that time as HH:MM:SS
    pub last_reload: Option<(Instant, String)>,
}

/// Footer hint shown when a removal key is pressed while removal is off
pub const REMOVAL_OFF_HINT: &str = "Removal is off (press x to turn it on)";

impl App {
    pub fn new(
        branches: Vec<BranchInfo>,
        repo_path: String,
        trunk: String,
        current_git_user: String,
    ) -> Self {
        let unique_authors = collect_authors(&branches);

        Self {
            branches,
            selected_index: 0,
            selected_branches: HashSet::new(),
            should_quit: false,
            esc_quit_armed_at: None,
            repo_path,
            trunk,
            show_confirmation: false,
            action_log: Vec::new(),
            force_mode: false,
            current_filter: FilterMode::All,
            show_help: false,
            show_info: false,
            dry_run: false,
            show_filter: false,
            search_active: false,
            search_query: String::new(),
            search_cursor_pos: 0,
            scroll_offset: 0,
            visible_height: 0,
            sort_mode: SortMode::Status,
            current_git_user,
            unique_authors,
            suggestions: Vec::new(),
            suggestion_index: None,
            show_suggestions: false,
            pr_provider: None,
            pr_cache: None,
            update_notice: None,
            view: ViewMode::Branches,
            worktrees: None,
            other_nav: NavState::default(),
            flash: None,
            worktree_remove_mode: false,
            refresher: Refresher::default(),
            last_reload: None,
        }
    }

    pub fn quit(&mut self) {
        self.should_quit = true;
    }

    /// Esc in normal mode: the first press arms quit, a second press within
    /// `ESC_QUIT_WINDOW` quits (a single Esc is too easy to hit after closing a popup)
    pub fn press_esc_to_quit(&mut self, now: Instant) {
        match self.esc_quit_armed_at {
            Some(armed_at) if now.duration_since(armed_at) <= ESC_QUIT_WINDOW => self.quit(),
            _ => self.esc_quit_armed_at = Some(now),
        }
    }

    /// Disarm a pending Esc quit once its window has passed
    pub fn expire_esc_quit(&mut self, now: Instant) {
        if self
            .esc_quit_armed_at
            .is_some_and(|armed_at| now.duration_since(armed_at) > ESC_QUIT_WINDOW)
        {
            self.esc_quit_armed_at = None;
        }
    }

    pub fn select_next(&mut self) {
        let len = self.visible_len();
        if len > 0 && self.selected_index < len - 1 {
            self.selected_index += 1;
            // Adjust scroll offset if cursor goes below visible area
            self.adjust_scroll_for_selection();
        }
    }

    pub fn select_prev(&mut self) {
        if self.selected_index > 0 {
            self.selected_index -= 1;
            // Adjust scroll offset if cursor goes above visible area
            self.adjust_scroll_for_selection();
        }
    }

    /// Go to the first item in the list
    pub fn go_to_top(&mut self) {
        self.selected_index = 0;
        self.scroll_offset = 0;
    }

    /// Go to the last item in the list
    pub fn go_to_bottom(&mut self) {
        let len = self.visible_len();
        if len > 0 {
            self.selected_index = len - 1;
            self.adjust_scroll_for_selection();
        }
    }

    /// Move up by one page (viewport height)
    pub fn page_up(&mut self) {
        if self.visible_height == 0 {
            return;
        }
        let page_size = self.visible_height.saturating_sub(1).max(1);
        self.selected_index = self.selected_index.saturating_sub(page_size);
        self.adjust_scroll_for_selection();
    }

    /// Move down by one page (viewport height)
    pub fn page_down(&mut self) {
        let len = self.visible_len();
        if len == 0 || self.visible_height == 0 {
            return;
        }
        let page_size = self.visible_height.saturating_sub(1).max(1);
        self.selected_index = (self.selected_index + page_size).min(len - 1);
        self.adjust_scroll_for_selection();
    }

    /// Adjust scroll offset to keep the selected item visible
    /// Only scrolls when cursor would go outside the visible bounds
    pub fn adjust_scroll_for_selection(&mut self) {
        if self.visible_height == 0 {
            return;
        }

        // If cursor is above the visible area, scroll up
        if self.selected_index < self.scroll_offset {
            self.scroll_offset = self.selected_index;
        }

        // If cursor is below the visible area, scroll down
        // visible_height - 1 because we need to account for header row
        let visible_rows = self.visible_height.saturating_sub(1);
        if visible_rows > 0 && self.selected_index >= self.scroll_offset + visible_rows {
            self.scroll_offset = self.selected_index - visible_rows + 1;
        }
    }

    /// Get the currently selected branch, if any
    pub fn selected_branch(&self) -> Option<&BranchInfo> {
        let filtered = self.filtered_branches();
        filtered.get(self.selected_index).copied()
    }

    /// Get filtered branches based on current filter mode and search query
    pub fn filtered_branches(&self) -> Vec<&BranchInfo> {
        let status_filtered: Vec<&BranchInfo> = match self.current_filter {
            FilterMode::All => self.branches.iter().collect(),
            FilterMode::SafeMerged => self
                .branches
                .iter()
                .filter(|b| {
                    b.status == BranchStatus::SafeMerged || b.status == BranchStatus::PrMerged
                })
                .collect(),
            FilterMode::GoneUpstream => self
                .branches
                .iter()
                .filter(|b| b.status == BranchStatus::GoneUpstream)
                .collect(),
            FilterMode::Unmerged => self
                .branches
                .iter()
                .filter(|b| b.status.requires_force())
                .collect(),
        };

        // Apply search filter if query is not empty
        if self.search_query.is_empty() {
            status_filtered
        } else {
            // Parse search query for @author: prefix
            let (name_query, author_query) = self.parse_search_query();

            status_filtered
                .into_iter()
                .filter(|b| {
                    // Filter by branch name if name query exists
                    let name_matches =
                        name_query.is_empty() || b.name.to_lowercase().contains(&name_query);

                    // Filter by author if author query exists
                    let author_matches = match &author_query {
                        Some(author) => b
                            .branch_author
                            .to_lowercase()
                            .contains(&author.to_lowercase()),
                        None => true,
                    };

                    name_matches && author_matches
                })
                .collect()
        }
    }

    /// Parse search query into name filter and author filter
    /// Returns (name_query, Option<author_query>)
    /// Supports quoted author names: @author:"Emil Ivanichkov"
    fn parse_search_query(&self) -> (String, Option<String>) {
        let query = self.search_query.to_lowercase();

        // Check for @author: prefix
        if let Some(author_idx) = query.find("@author:") {
            let before_author = query[..author_idx].trim().to_string();
            let after_author = &query[author_idx + 8..]; // Skip "@author:"

            // Check if author value is quoted
            let (author_value, author_end) = if let Some(content) = after_author.strip_prefix('"') {
                // Find closing quote
                if let Some(close_quote) = content.find('"') {
                    (content[..close_quote].to_string(), close_quote + 2) // +2 for both quotes
                } else {
                    // No closing quote, treat rest as author (user still typing)
                    (content.to_string(), after_author.len())
                }
            } else {
                // Find the end of author query (next space or end of string)
                let end = after_author.find(' ').unwrap_or(after_author.len());
                (after_author[..end].trim().to_string(), end)
            };

            // Get any remaining name query after author
            let remaining = if author_end < after_author.len() {
                after_author[author_end..].trim().to_string()
            } else {
                String::new()
            };

            // Combine name parts
            let name_query = if !before_author.is_empty() && !remaining.is_empty() {
                format!("{} {}", before_author, remaining)
            } else if !before_author.is_empty() {
                before_author
            } else {
                remaining
            };

            // Handle @author:me special case
            let author_filter = if author_value == "me" {
                Some(self.current_git_user.clone())
            } else if !author_value.is_empty() {
                Some(author_value)
            } else {
                None
            };

            (name_query, author_filter)
        } else {
            (query, None)
        }
    }

    /// Get count of branches for a specific filter
    pub fn filter_count(&self, filter: FilterMode) -> usize {
        match filter {
            FilterMode::All => self.branches.len(),
            FilterMode::SafeMerged => self
                .branches
                .iter()
                .filter(|b| {
                    b.status == BranchStatus::SafeMerged || b.status == BranchStatus::PrMerged
                })
                .count(),
            FilterMode::GoneUpstream => self
                .branches
                .iter()
                .filter(|b| b.status == BranchStatus::GoneUpstream)
                .count(),
            FilterMode::Unmerged => self
                .branches
                .iter()
                .filter(|b| b.status.requires_force())
                .count(),
        }
    }

    /// Set the current filter mode
    pub fn set_filter(&mut self, filter: FilterMode) {
        self.current_filter = filter;
        // Reset selection and scroll when filter changes
        self.selected_index = 0;
        self.scroll_offset = 0;
    }

    /// Cycle to next filter
    pub fn next_filter(&mut self) {
        self.current_filter = self.current_filter.next();
        self.selected_index = 0;
        self.scroll_offset = 0;
    }

    /// Cycle to next sort mode and re-sort branches
    pub fn cycle_sort_mode(&mut self) {
        self.sort_mode = self.sort_mode.next();
        self.sort_branches();
        // Reset selection and scroll when sort changes
        self.selected_index = 0;
        self.scroll_offset = 0;
    }

    /// Sort branches based on current sort mode
    pub fn sort_branches(&mut self) {
        match self.sort_mode {
            SortMode::Status => {
                // Original sort: protected/current first, then by status
                self.branches.sort_by_key(|b| b.status.sort_rank());
            }
            SortMode::Name => {
                // Sort alphabetically by branch name (case-insensitive)
                self.branches
                    .sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
            }
            SortMode::ActivityNewest => {
                // Sort by last activity timestamp, newest first (descending)
                self.branches
                    .sort_by(|a, b| b.last_activity_timestamp.cmp(&a.last_activity_timestamp));
            }
            SortMode::ActivityOldest => {
                // Sort by last activity timestamp, oldest first (ascending)
                self.branches
                    .sort_by(|a, b| a.last_activity_timestamp.cmp(&b.last_activity_timestamp));
            }
            SortMode::CreatedNewest => {
                // Sort by branch creation timestamp, newest first (descending)
                self.branches
                    .sort_by(|a, b| b.branch_created_timestamp.cmp(&a.branch_created_timestamp));
            }
            SortMode::CreatedOldest => {
                // Sort by branch creation timestamp, oldest first (ascending)
                self.branches
                    .sort_by(|a, b| a.branch_created_timestamp.cmp(&b.branch_created_timestamp));
            }
        }
    }

    /// Update autocomplete suggestions based on current search query
    pub fn update_suggestions(&mut self) {
        // @author: commands only apply to branches
        if self.view == ViewMode::Worktrees {
            self.hide_suggestions();
            return;
        }
        let query = &self.search_query;

        // Available search commands
        const SEARCH_COMMANDS: &[&str] = &["author"];

        // Check if we're at a point where we should show suggestions
        if let Some(at_pos) = query.rfind('@') {
            let after_at = &query[at_pos + 1..];

            // Check if we're typing a command (no colon yet)
            if !after_at.contains(':') {
                // Show command suggestions that match what's typed after @
                self.suggestions = SEARCH_COMMANDS
                    .iter()
                    .filter(|cmd| cmd.to_lowercase().starts_with(&after_at.to_lowercase()))
                    .map(|s| s.to_string())
                    .collect();
                self.show_suggestions = !self.suggestions.is_empty();
                self.suggestion_index = if self.show_suggestions { Some(0) } else { None };
                return;
            }

            // Check if we have @author: and need to show author suggestions
            if after_at.to_lowercase().starts_with("author:") {
                let author_query = &after_at[7..]; // Skip "author:"

                // Build author suggestions including "me" as first option
                let mut author_suggestions: Vec<String> = Vec::new();

                // Add "me" if it matches the query
                if "me".starts_with(&author_query.to_lowercase()) {
                    author_suggestions.push("me".to_string());
                }

                // Add matching authors from unique_authors
                for author in &self.unique_authors {
                    if author
                        .to_lowercase()
                        .starts_with(&author_query.to_lowercase())
                        || author.to_lowercase().contains(&author_query.to_lowercase())
                    {
                        author_suggestions.push(author.clone());
                    }
                }

                self.suggestions = author_suggestions;
                self.show_suggestions = !self.suggestions.is_empty();
                self.suggestion_index = if self.show_suggestions { Some(0) } else { None };
                return;
            }
        }

        // No suggestions needed
        self.suggestions.clear();
        self.show_suggestions = false;
        self.suggestion_index = None;
    }

    /// Move to next suggestion
    pub fn suggestion_next(&mut self) {
        if self.show_suggestions && !self.suggestions.is_empty() {
            self.suggestion_index = Some(
                self.suggestion_index
                    .map(|i| (i + 1) % self.suggestions.len())
                    .unwrap_or(0),
            );
        }
    }

    /// Move to previous suggestion
    pub fn suggestion_prev(&mut self) {
        if self.show_suggestions && !self.suggestions.is_empty() {
            self.suggestion_index = Some(
                self.suggestion_index
                    .map(|i| {
                        if i == 0 {
                            self.suggestions.len() - 1
                        } else {
                            i - 1
                        }
                    })
                    .unwrap_or(0),
            );
        }
    }

    /// Accept the currently selected suggestion
    pub fn accept_suggestion(&mut self) -> bool {
        if !self.show_suggestions || self.suggestions.is_empty() {
            return false;
        }

        let suggestion_idx = self.suggestion_index.unwrap_or(0);
        if let Some(suggestion) = self.suggestions.get(suggestion_idx).cloned() {
            let query = self.search_query.clone();
            let mut accepted_command = false;

            if let Some(at_pos) = query.rfind('@') {
                let after_at = &query[at_pos + 1..];

                // Accepting a command (no colon yet)
                if !after_at.contains(':') {
                    // Replace everything after @ with the command and add colon
                    self.search_query = format!("{}@{}:", &query[..at_pos], suggestion);
                    accepted_command = true;
                } else if after_at.to_lowercase().starts_with("author:") {
                    // Accepting an author name - wrap in quotes if contains space
                    let formatted_author = if suggestion.contains(' ') {
                        format!("\"{}\"", suggestion)
                    } else {
                        suggestion
                    };
                    self.search_query = format!("{}@author:{}", &query[..at_pos], formatted_author);
                }
            }

            // The query was replaced: put the cursor at its end so it stays
            // on a char boundary
            self.search_cursor_pos = self.search_query.len();

            if accepted_command {
                // Show the command's value suggestions (e.g. authors after @author:)
                self.update_suggestions();
            } else {
                // The accepted value still matches its own suggestion (e.g. "me"),
                // so recomputing would keep the dropdown open and trap Up/Down
                self.hide_suggestions();
            }
            return true;
        }

        false
    }

    /// Hide suggestions dropdown
    pub fn hide_suggestions(&mut self) {
        self.show_suggestions = false;
        self.suggestion_index = None;
    }

    /// Byte index of the char before the cursor (cursor stays on char
    /// boundaries — the query can contain multi-byte characters)
    fn search_prev_char_pos(&self) -> Option<usize> {
        self.search_query[..self.search_cursor_pos]
            .char_indices()
            .next_back()
            .map(|(i, _)| i)
    }

    /// Move search cursor left
    pub fn search_cursor_left(&mut self) {
        if let Some(prev) = self.search_prev_char_pos() {
            self.search_cursor_pos = prev;
        }
    }

    /// Move search cursor right
    pub fn search_cursor_right(&mut self) {
        if let Some(c) = self.search_query[self.search_cursor_pos..].chars().next() {
            self.search_cursor_pos += c.len_utf8();
        }
    }

    /// Insert character at cursor position in search query
    pub fn search_insert_char(&mut self, c: char) {
        self.search_query.insert(self.search_cursor_pos, c);
        self.search_cursor_pos += c.len_utf8();
        self.selected_index = 0;
        self.scroll_offset = 0;
        self.update_suggestions();
    }

    /// Delete character before cursor in search query (backspace behavior)
    pub fn search_backspace(&mut self) {
        if let Some(prev) = self.search_prev_char_pos() {
            self.search_query.remove(prev);
            self.search_cursor_pos = prev;
            self.selected_index = 0;
            self.scroll_offset = 0;
            self.update_suggestions();
        }
    }

    /// Delete character at cursor position in search query
    pub fn search_delete(&mut self) {
        if self.search_cursor_pos < self.search_query.len() {
            self.search_query.remove(self.search_cursor_pos);
            self.selected_index = 0;
            self.scroll_offset = 0;
            self.update_suggestions();
        }
    }

    /// Move cursor to start of search query
    pub fn search_cursor_start(&mut self) {
        self.search_cursor_pos = 0;
    }

    /// Move cursor to end of search query
    pub fn search_cursor_end(&mut self) {
        self.search_cursor_pos = self.search_query.len();
    }

    /// Count of deletable branches
    #[allow(dead_code)]
    pub fn deletable_count(&self) -> usize {
        self.branches
            .iter()
            .filter(|b| b.status.is_deletable())
            .count()
    }

    /// Count of protected branches
    #[allow(dead_code)]
    pub fn protected_count(&self) -> usize {
        self.branches.len() - self.deletable_count()
    }

    /// Toggle selection of a branch by filtered index
    pub fn toggle_selection_at_cursor(&mut self) {
        let filtered = self.filtered_branches();
        if let Some(&branch) = filtered.get(self.selected_index) {
            // Find the original index in all branches
            if let Some(original_idx) = self.branches.iter().position(|b| b.name == branch.name) {
                self.toggle_selection(original_idx);
            }
        }
    }

    /// Toggle selection of a branch by index
    pub fn toggle_selection(&mut self, index: usize) {
        if let Some(branch) = self.branches.get(index) {
            // Only allow selection of deletable branches
            if !branch.status.is_deletable() {
                return;
            }
            // Skip force-only branches unless force mode is enabled
            if branch.status.requires_force() && !self.force_mode {
                return;
            }

            if !self.selected_branches.remove(&branch.name) {
                self.selected_branches.insert(branch.name.clone());
            }
        }
    }

    /// Select all safe (deletable) branches
    pub fn select_all_safe(&mut self) {
        // If all safe branches are selected, deselect all
        let safe_names: Vec<String> = self
            .branches
            .iter()
            .filter(|b| b.status.is_deletable() && (self.force_mode || !b.status.requires_force()))
            .map(|b| b.name.clone())
            .collect();

        let all_selected = safe_names
            .iter()
            .all(|name| self.selected_branches.contains(name));

        if all_selected {
            // Deselect all
            self.selected_branches.clear();
        } else {
            // Select all safe branches
            self.selected_branches.extend(safe_names);
        }
    }

    /// Clear the selection of the view on screen
    pub fn clear_selection(&mut self) {
        match self.view {
            ViewMode::Branches => self.selected_branches.clear(),
            ViewMode::Worktrees => {
                if let Some(wt) = self.worktrees.as_mut() {
                    wt.selected.clear();
                }
            }
        }
    }

    /// Toggle force mode (`f`) in either view. Turning it on only makes more
    /// rows selectable, so both selections are kept. Turning it off drops
    /// just the selections that need force: branches whose status
    /// `requires_force()` and dirty worktrees, in both views.
    pub fn toggle_force_mode(&mut self) {
        self.force_mode = !self.force_mode;
        if self.force_mode {
            return;
        }
        let branches = &self.branches;
        self.selected_branches.retain(|name| {
            branches
                .iter()
                .any(|b| &b.name == name && !b.status.requires_force())
        });
        if let Some(wt) = self.worktrees.as_mut() {
            wt.retain_selectable(false);
        }
    }

    /// Get the branches that are selected for deletion, in list order
    pub fn get_selected_branches(&self) -> Vec<&BranchInfo> {
        self.branches
            .iter()
            .filter(|b| self.selected_branches.contains(&b.name))
            .collect()
    }

    /// Count of selected items in the view on screen
    pub fn selected_count(&self) -> usize {
        match self.view {
            ViewMode::Branches => self.selected_branches.len(),
            ViewMode::Worktrees => self.worktrees.as_ref().map_or(0, |wt| wt.selected.len()),
        }
    }

    /// Check if the branch at `index` (in all branches) is selected
    pub fn is_branch_selected(&self, index: usize) -> bool {
        self.branches
            .get(index)
            .is_some_and(|b| self.selected_branches.contains(&b.name))
    }

    /// Show the confirmation modal
    #[allow(dead_code)]
    pub fn show_confirm_modal(&mut self) {
        if !self.selected_branches.is_empty() {
            self.show_confirmation = true;
        }
    }

    /// Hide the confirmation modal
    #[allow(dead_code)]
    pub fn hide_confirm_modal(&mut self) {
        self.show_confirmation = false;
    }

    /// Execute deletion of selected branches
    pub fn delete_selected_branches(&mut self) {
        let selected: Vec<(String, BranchStatus)> = self
            .get_selected_branches()
            .into_iter()
            .map(|b| (b.name.clone(), b.status.clone()))
            .collect();

        for (branch_name, status) in &selected {
            // Auto-force for "gone" branches (squash/rebase merges) and
            // force-only branches (unmerged, pr-diverged)
            // Also respect user's force_mode setting
            let use_force = branch_delete_needs_force(status, self.force_mode);

            match git::delete_branch_with_mode(branch_name, use_force) {
                Ok(_) => {
                    let method = if use_force { "-D" } else { "-d" };
                    // Best effort: a stale entry only costs a re-fetch later.
                    if let Some(cache) = &self.pr_cache {
                        cache.invalidate(branch_name).ok();
                    }
                    self.action_log.push(ActionLogEntry {
                        branch_name: branch_name.clone(),
                        success: true,
                        message: format!("Deleted ({})", method),
                    });
                }
                Err(e) => {
                    self.action_log.push(ActionLogEntry {
                        branch_name: branch_name.clone(),
                        success: false,
                        message: e.to_string(),
                    });
                }
            }
        }

        // Clear selection
        self.selected_branches.clear();
        self.show_confirmation = false;
    }

    /// Refresh the branch list (after deletion). Classifies against the
    /// trunk resolved at startup, so a `--trunk` override is kept.
    pub fn refresh_branches(&mut self) {
        // A background reload that read git before this point is out of date
        self.refresher.invalidate();
        if let Ok(branches) = git::get_branches_with_classification(Some(&self.trunk)) {
            self.replace_branches(branches);
            self.reload_worktrees_if_loaded();
        }
    }

    /// Start a background reload (`r`); one running already queues it
    pub fn request_refresh(&mut self, kind: RefreshKind) {
        self.refresher.request(kind, &self.trunk);
    }

    /// Apply a finished background reload (called every tick). Results wait
    /// while the delete confirmation is open, so the list and the selection
    /// never change under a pending delete.
    pub fn poll_refresh(&mut self) {
        let hold = self.show_confirmation;
        match self.refresher.poll(hold, &self.trunk) {
            Some(JobResult::Reloaded { branches, .. }) => {
                self.replace_branches(branches);
                self.reload_worktrees_if_loaded();
                self.last_reload = Some((
                    Instant::now(),
                    chrono::Local::now().format("%H:%M:%S").to_string(),
                ));
            }
            Some(JobResult::Failed { error, .. }) => {
                self.flash = Some(format!("Reload failed: {}", error));
            }
            None => {}
        }
    }

    /// Footer status of background reloads: running, or recently finished
    pub fn refresh_status(&self, now: Instant) -> Option<String> {
        if self.refresher.running().is_some() {
            return Some("⟳ reloading…".to_string());
        }
        match &self.last_reload {
            Some((at, time)) if now.duration_since(*at) <= RELOAD_NOTICE_DURATION => {
                Some(format!("updated {}", time))
            }
            _ => None,
        }
    }

    /// Swap in a freshly read branch list. Keeps the PR info already known,
    /// the sort order, the cursor (by branch name) and the selection of
    /// branches that still exist and are still selectable.
    pub fn replace_branches(&mut self, mut branches: Vec<BranchInfo>) {
        let cursor_name = match self.view {
            ViewMode::Branches => self.selected_branch().map(|b| b.name.clone()),
            ViewMode::Worktrees => None,
        };

        // Carry over PR info fetched at startup; a refresh only re-reads
        // git, so without this PrMerged branches revert to Unmerged.
        for branch in branches.iter_mut() {
            if let Some(old) = self.branches.iter().find(|b| b.name == branch.name) {
                branch.pr_info = old.pr_info.clone();
            }
        }
        git::apply_pr_merge_status(&mut branches);
        self.branches = branches;
        self.sort_branches();
        self.unique_authors = collect_authors(&self.branches);

        // A branch that is gone, or whose new status makes it unselectable
        // (e.g. a new commit turned it unmerged), leaves the selection
        let force_mode = self.force_mode;
        let branches = &self.branches;
        self.selected_branches.retain(|name| {
            branches.iter().any(|b| {
                &b.name == name
                    && b.status.is_deletable()
                    && (force_mode || !b.status.requires_force())
            })
        });

        match self.view {
            ViewMode::Branches => self.restore_branch_cursor(cursor_name.as_deref()),
            ViewMode::Worktrees => {
                let last = self.branches.len().saturating_sub(1);
                self.other_nav.selected_index = self.other_nav.selected_index.min(last);
            }
        }
    }

    /// Put the cursor back on `name` in the filtered list, or keep its row
    /// (clamped) when that branch is no longer listed
    fn restore_branch_cursor(&mut self, name: Option<&str>) {
        let filtered = self.filtered_branches();
        let position = name.and_then(|name| filtered.iter().position(|b| b.name == name));
        self.selected_index = match position {
            Some(position) => position,
            None => self.selected_index.min(filtered.len().saturating_sub(1)),
        };
        self.scroll_offset = self.scroll_offset.min(self.selected_index);
        self.adjust_scroll_for_selection();
    }

    /// Re-read the worktree list after the branches changed, when the
    /// worktree view has been opened
    pub fn reload_worktrees_if_loaded(&mut self) {
        if self.worktrees.is_some() {
            if let Err(e) = self.load_worktrees() {
                self.flash = Some(format!("Cannot list worktrees: {}", e));
            }
        }
    }

    /// Get success count from action log
    pub fn deletion_success_count(&self) -> usize {
        self.action_log.iter().filter(|e| e.success).count()
    }

    /// Get failure count from action log
    pub fn deletion_failure_count(&self) -> usize {
        self.action_log.iter().filter(|e| !e.success).count()
    }

    /// Open the PR URL for the item under the cursor in the default browser
    /// Returns true if a PR URL was opened, false if no PR is associated
    pub fn open_selected_pr(&self) -> bool {
        let pr_info = match self.view {
            ViewMode::Branches => self.selected_branch().and_then(|b| b.pr_info.as_ref()),
            ViewMode::Worktrees => self.selected_worktree().and_then(|w| w.pr_info.as_ref()),
        };
        pr_info.is_some_and(|pr| git::open_url_in_browser(&pr.url).is_ok())
    }

    /// Number of rows in the list on screen (after filter and search)
    pub fn visible_len(&self) -> usize {
        match self.view {
            ViewMode::Branches => self.filtered_branches().len(),
            ViewMode::Worktrees => self.filtered_worktrees().len(),
        }
    }

    /// Switch between the branch and worktree views. Each view keeps its own
    /// cursor, scroll position and search query. The worktree list is loaded
    /// the first time the view opens.
    pub fn toggle_view(&mut self) {
        let target = match self.view {
            ViewMode::Branches => ViewMode::Worktrees,
            ViewMode::Worktrees => ViewMode::Branches,
        };
        if target == ViewMode::Worktrees && self.worktrees.is_none() {
            if let Err(e) = self.load_worktrees() {
                self.flash = Some(format!("Cannot list worktrees: {}", e));
                return;
            }
        }
        let current = NavState {
            selected_index: self.selected_index,
            scroll_offset: self.scroll_offset,
            search_query: std::mem::take(&mut self.search_query),
            search_cursor_pos: self.search_cursor_pos,
        };
        let other = std::mem::replace(&mut self.other_nav, current);
        self.selected_index = other.selected_index;
        self.scroll_offset = other.scroll_offset;
        self.search_query = other.search_query;
        self.search_cursor_pos = other.search_cursor_pos;
        self.search_active = false;
        self.hide_suggestions();
        self.view = target;
    }

    /// (Re)load the worktree list and start reading each worktree's status.
    /// On a reload, the filter, the statuses already read, the selection and
    /// the cursor (by path) carry over.
    pub fn load_worktrees(&mut self) -> color_eyre::Result<()> {
        let view = WorktreeView::load(&self.branches, Path::new(&self.repo_path))?;
        self.set_worktree_view(view);
        Ok(())
    }

    /// Install a freshly loaded worktree view (see `load_worktrees`)
    fn set_worktree_view(&mut self, mut view: WorktreeView) {
        let cursor_path = match self.view {
            ViewMode::Worktrees => self.selected_worktree().map(|w| w.entry.path.clone()),
            ViewMode::Branches => None,
        };
        if let Some(old) = &self.worktrees {
            view.carry_over_from(old, self.force_mode);
        }
        self.worktrees = Some(view);
        if self.view == ViewMode::Worktrees {
            let filtered = self.filtered_worktrees();
            let position =
                cursor_path.and_then(|path| filtered.iter().position(|w| w.entry.path == path));
            self.selected_index = match position {
                Some(position) => position,
                None => self.selected_index.min(filtered.len().saturating_sub(1)),
            };
            self.scroll_offset = self.scroll_offset.min(self.selected_index);
            self.adjust_scroll_for_selection();
        }
    }

    /// Apply background `git status` results (called every tick)
    pub fn poll_worktree_status(&mut self) {
        if let Some(wt) = self.worktrees.as_mut() {
            wt.poll_status();
        }
    }

    /// Worktrees matching the worktree filter tab and the search query
    pub fn filtered_worktrees(&self) -> Vec<&WorktreeInfo> {
        // Each view keeps its own search query
        let query = match self.view {
            ViewMode::Worktrees => self.search_query.as_str(),
            ViewMode::Branches => self.other_nav.search_query.as_str(),
        };
        self.worktrees
            .as_ref()
            .map(|wt| wt.filtered(query))
            .unwrap_or_default()
    }

    /// The worktree under the cursor
    pub fn selected_worktree(&self) -> Option<&WorktreeInfo> {
        self.filtered_worktrees().get(self.selected_index).copied()
    }

    /// Set the worktree filter tab
    pub fn set_worktree_filter(&mut self, filter: WorktreeFilter) {
        if let Some(wt) = self.worktrees.as_mut() {
            wt.filter = filter;
        }
        self.selected_index = 0;
        self.scroll_offset = 0;
    }

    /// Cycle the worktree filter tab
    pub fn next_worktree_filter(&mut self) {
        if let Some(filter) = self.worktrees.as_ref().map(|wt| wt.filter.next()) {
            self.set_worktree_filter(filter);
        }
    }

    /// Turn worktree remove mode on or off (`x`). Turning it off clears the
    /// worktree selection, so nothing stays armed for removal.
    pub fn toggle_worktree_remove_mode(&mut self) {
        self.worktree_remove_mode = !self.worktree_remove_mode;
        if !self.worktree_remove_mode {
            if let Some(wt) = self.worktrees.as_mut() {
                wt.selected.clear();
                wt.delete_branches = false;
            }
        }
    }

    /// Whether worktree removal is on; when it is off, show the footer hint
    /// explaining how to turn it on
    pub fn require_worktree_remove_mode(&mut self) -> bool {
        if !self.worktree_remove_mode {
            self.flash = Some(REMOVAL_OFF_HINT.to_string());
        }
        self.worktree_remove_mode
    }

    /// Toggle selection of the worktree under the cursor
    pub fn toggle_worktree_at_cursor(&mut self) {
        if !self.require_worktree_remove_mode() {
            return;
        }
        let path = self.selected_worktree().map(|w| w.entry.path.clone());
        let force = self.force_mode;
        if let (Some(path), Some(wt)) = (path, self.worktrees.as_mut()) {
            wt.toggle(&path, force);
        }
    }

    /// Select (or deselect) all clean stale worktrees
    pub fn select_all_stale_worktrees(&mut self) {
        if !self.require_worktree_remove_mode() {
            return;
        }
        let force = self.force_mode;
        if let Some(wt) = self.worktrees.as_mut() {
            wt.select_all_clean_stale(force);
        }
    }

    /// Enter in the worktree view: open the removal confirmation for the
    /// selected worktrees
    pub fn request_worktree_removal(&mut self) {
        if !self.require_worktree_remove_mode() || self.selected_count() == 0 {
            return;
        }
        if let Some(wt) = self.worktrees.as_mut() {
            wt.delete_branches = false;
        }
        self.show_confirmation = true;
    }

    /// Remove the selected worktrees (or describe it in dry-run mode),
    /// optionally deleting their branches, then reload both lists
    pub fn remove_selected_worktrees(&mut self) {
        if !self.worktree_remove_mode {
            return;
        }
        let Some(wt) = self.worktrees.as_mut() else {
            return;
        };
        let plans = crate::worktree_view::plan_removals(
            &wt.selected_infos(),
            self.force_mode,
            wt.delete_branches,
        );
        let log =
            crate::worktree_view::execute_removals(&plans, self.dry_run, self.pr_cache.as_ref());
        wt.selected.clear();
        wt.delete_branches = false;
        self.action_log.extend(log);
        self.show_confirmation = false;
        if !self.dry_run {
            self.refresh_branches();
        }
    }

    /// Copy the path of the worktree under the cursor to the clipboard
    pub fn copy_selected_worktree_path(&mut self) {
        let Some(path) = self.selected_worktree().map(|w| w.entry.path.clone()) else {
            return;
        };
        let path = path.to_string_lossy().into_owned();
        self.copy_with(&path, crate::clipboard::copy_to_clipboard);
    }

    /// Copy the name of the branch under the cursor to the clipboard
    pub fn copy_selected_branch_name(&mut self) {
        self.copy_selected_branch_name_with(crate::clipboard::copy_to_clipboard);
    }

    /// `copy_selected_branch_name` with an injectable copy function (tests)
    fn copy_selected_branch_name_with(
        &mut self,
        copy: impl FnOnce(&str) -> std::result::Result<&'static str, String>,
    ) {
        let Some(name) = self.selected_branch().map(|b| b.name.clone()) else {
            return;
        };
        self.copy_with(&name, copy);
    }

    /// Copy `text` with `copy` and report the outcome in the footer
    fn copy_with(
        &mut self,
        text: &str,
        copy: impl FnOnce(&str) -> std::result::Result<&'static str, String>,
    ) {
        self.flash = Some(match copy(text) {
            Ok(tool) => format!("Copied {} to the clipboard ({})", text, tool),
            Err(e) => format!("Cannot copy {}: {}", text, e),
        });
    }

    /// Check if the currently selected branch has a PR
    #[allow(dead_code)]
    pub fn selected_branch_has_pr(&self) -> bool {
        self.selected_branch()
            .map(|b| b.pr_info.is_some())
            .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_branch(name: &str, status: BranchStatus) -> BranchInfo {
        BranchInfo {
            name: name.to_string(),
            upstream: None,
            last_commit_relative: "2 days ago".to_string(),
            status,
            last_commit_sha: "abc123".to_string(),
            last_commit_author: "Test Author".to_string(),
            last_commit_message: "Test commit".to_string(),
            ahead: None,
            behind: None,
            last_activity_timestamp: 0,
            branch_created_timestamp: 0,
            branch_author: "Test Author".to_string(),
            pr_info: None,
            worktree: None,
            worktree_shadowed_status: None,
        }
    }

    fn create_test_app() -> App {
        let branches = vec![
            create_test_branch("main", BranchStatus::Protected),
            create_test_branch("feature/merged", BranchStatus::SafeMerged),
            create_test_branch("feature/gone", BranchStatus::GoneUpstream),
            create_test_branch("feature/unmerged", BranchStatus::Unmerged),
            create_test_branch("current-branch", BranchStatus::Current),
        ];
        App::new(
            branches,
            "/test/repo".to_string(),
            "main".to_string(),
            "Test Author".to_string(),
        )
    }

    #[test]
    fn test_search_editing_with_multibyte_chars() {
        // Cursor is a byte index but must always stay on a char boundary —
        // Cyrillic (2 bytes/char) used to panic String::insert/remove.
        let mut app = create_test_app();
        for c in "Емил".chars() {
            app.search_insert_char(c);
        }
        assert_eq!(app.search_query, "Емил");
        assert_eq!(app.search_cursor_pos, app.search_query.len());

        // Walk left through multi-byte chars, then insert mid-string
        app.search_cursor_left();
        app.search_cursor_left();
        app.search_insert_char('X');
        assert_eq!(app.search_query, "ЕмXил");

        // Backspace removes the whole char before the cursor
        app.search_backspace(); // remove 'X'
        app.search_backspace(); // remove 'м' (2 bytes)
        assert_eq!(app.search_query, "Еил");

        // Delete removes the whole char at the cursor
        app.search_delete(); // remove 'и'
        assert_eq!(app.search_query, "Ел");

        // Right steps over the full 2-byte 'л' and lands on the end boundary
        assert_eq!(app.search_cursor_pos, "Е".len());
        app.search_cursor_right();
        assert_eq!(app.search_cursor_pos, app.search_query.len());
    }

    #[test]
    fn test_accept_suggestion_resets_cursor_to_query_end() {
        let mut app = create_test_app();
        for c in "@author:Test".chars() {
            app.search_insert_char(c);
        }
        app.update_suggestions();
        if app.accept_suggestion() {
            assert_eq!(app.search_cursor_pos, app.search_query.len());
        }
    }

    #[test]
    fn test_filter_mode_label() {
        assert_eq!(FilterMode::All.label(), "ALL");
        assert_eq!(FilterMode::SafeMerged.label(), "SAFE MERGED");
        assert_eq!(FilterMode::GoneUpstream.label(), "UPSTREAM GONE");
        assert_eq!(FilterMode::Unmerged.label(), "UNMERGED");
    }

    #[test]
    fn test_filter_mode_next() {
        assert_eq!(FilterMode::All.next(), FilterMode::SafeMerged);
        assert_eq!(FilterMode::SafeMerged.next(), FilterMode::GoneUpstream);
        assert_eq!(FilterMode::GoneUpstream.next(), FilterMode::Unmerged);
        assert_eq!(FilterMode::Unmerged.next(), FilterMode::All);
    }

    #[test]
    fn test_filter_mode_from_number() {
        assert_eq!(FilterMode::from_number(1), Some(FilterMode::SafeMerged));
        assert_eq!(FilterMode::from_number(2), Some(FilterMode::GoneUpstream));
        assert_eq!(FilterMode::from_number(3), Some(FilterMode::Unmerged));
        assert_eq!(FilterMode::from_number(4), Some(FilterMode::All));
        assert_eq!(FilterMode::from_number(5), None);
        assert_eq!(FilterMode::from_number(0), None);
    }

    #[test]
    fn copy_branch_name_copies_the_row_under_the_cursor() {
        let mut app = create_test_app();
        app.selected_index = 1;
        let mut copied = String::new();
        app.copy_selected_branch_name_with(|text| {
            copied = text.to_string();
            Ok("fake-clip")
        });
        assert_eq!(copied, "feature/merged");
        assert_eq!(
            app.flash.as_deref(),
            Some("Copied feature/merged to the clipboard (fake-clip)")
        );
    }

    #[test]
    fn copy_branch_name_works_on_undeletable_rows_and_reports_errors() {
        let mut app = create_test_app();
        app.selected_index = 0; // main (protected)
        app.copy_selected_branch_name_with(|_| Err("no clipboard tool found".to_string()));
        assert_eq!(
            app.flash.as_deref(),
            Some("Cannot copy main: no clipboard tool found")
        );
    }

    #[test]
    fn copy_branch_name_does_nothing_on_an_empty_list() {
        let mut app = App::new(
            vec![],
            "/test/repo".to_string(),
            "main".to_string(),
            "Test Author".to_string(),
        );
        let mut called = false;
        app.copy_selected_branch_name_with(|_| {
            called = true;
            Ok("fake-clip")
        });
        assert!(!called);
        assert_eq!(app.flash, None);
    }

    #[test]
    fn test_app_creation() {
        let app = create_test_app();
        assert_eq!(app.branches.len(), 5);
        assert_eq!(app.selected_index, 0);
        assert_eq!(app.selected_branches.len(), 0);
        assert!(!app.should_quit);
        assert!(!app.show_confirmation);
        assert!(!app.force_mode);
        assert!(!app.show_help);
        assert!(!app.dry_run);
    }

    #[test]
    fn test_select_next_prev() {
        let mut app = create_test_app();

        // Initially at index 0
        assert_eq!(app.selected_index, 0);

        // Move next
        app.select_next();
        assert_eq!(app.selected_index, 1);

        app.select_next();
        assert_eq!(app.selected_index, 2);

        // Move prev
        app.select_prev();
        assert_eq!(app.selected_index, 1);

        // Stop at end (no wrap)
        app.selected_index = 4;
        app.select_next();
        assert_eq!(app.selected_index, 4); // Should stay at 4

        // Stop at beginning (no wrap)
        app.selected_index = 0;
        app.select_prev();
        assert_eq!(app.selected_index, 0); // Should stay at 0
    }

    #[test]
    fn test_filtered_branches() {
        let app = create_test_app();

        // All filter
        let all = app.filtered_branches();
        assert_eq!(all.len(), 5);

        // Safe merged filter
        let mut app = create_test_app();
        app.current_filter = FilterMode::SafeMerged;
        let safe = app.filtered_branches();
        assert_eq!(safe.len(), 1);
        assert_eq!(safe[0].name, "feature/merged");

        // Gone upstream filter
        let mut app = create_test_app();
        app.current_filter = FilterMode::GoneUpstream;
        let gone = app.filtered_branches();
        assert_eq!(gone.len(), 1);
        assert_eq!(gone[0].name, "feature/gone");

        // Unmerged filter
        let mut app = create_test_app();
        app.current_filter = FilterMode::Unmerged;
        let unmerged = app.filtered_branches();
        assert_eq!(unmerged.len(), 1);
        assert_eq!(unmerged[0].name, "feature/unmerged");
    }

    #[test]
    fn test_filter_count() {
        let app = create_test_app();

        assert_eq!(app.filter_count(FilterMode::All), 5);
        assert_eq!(app.filter_count(FilterMode::SafeMerged), 1);
        assert_eq!(app.filter_count(FilterMode::GoneUpstream), 1);
        assert_eq!(app.filter_count(FilterMode::Unmerged), 1);
    }

    #[test]
    fn test_set_filter() {
        let mut app = create_test_app();
        app.selected_index = 3;

        app.set_filter(FilterMode::SafeMerged);
        assert_eq!(app.current_filter, FilterMode::SafeMerged);
        assert_eq!(app.selected_index, 0); // Reset on filter change
    }

    #[test]
    fn test_next_filter() {
        let mut app = create_test_app();

        assert_eq!(app.current_filter, FilterMode::All);
        app.next_filter();
        assert_eq!(app.current_filter, FilterMode::SafeMerged);
        app.next_filter();
        assert_eq!(app.current_filter, FilterMode::GoneUpstream);
        app.next_filter();
        assert_eq!(app.current_filter, FilterMode::Unmerged);
        app.next_filter();
        assert_eq!(app.current_filter, FilterMode::All);
    }

    #[test]
    fn test_deletable_count() {
        let app = create_test_app();
        // SafeMerged, GoneUpstream, Unmerged are deletable (3)
        // Protected and Current are not (2)
        assert_eq!(app.deletable_count(), 3);
        assert_eq!(app.protected_count(), 2);
    }

    #[test]
    fn test_toggle_selection() {
        let mut app = create_test_app();

        // Cannot select protected branch (index 0)
        app.toggle_selection(0);
        assert!(!app.is_branch_selected(0));

        // Can select safe merged branch (index 1)
        app.toggle_selection(1);
        assert!(app.is_branch_selected(1));

        // Toggle again to deselect
        app.toggle_selection(1);
        assert!(!app.is_branch_selected(1));

        // Can select gone upstream (index 2)
        app.toggle_selection(2);
        assert!(app.is_branch_selected(2));

        // Cannot select unmerged without force mode (index 3)
        app.toggle_selection(3);
        assert!(!app.is_branch_selected(3));

        // Can select unmerged with force mode
        app.force_mode = true;
        app.toggle_selection(3);
        assert!(app.is_branch_selected(3));
    }

    #[test]
    fn test_select_all_safe() {
        let mut app = create_test_app();

        // Select all safe branches (without force mode)
        app.select_all_safe();
        assert!(!app.is_branch_selected(0)); // Protected
        assert!(app.is_branch_selected(1)); // SafeMerged
        assert!(app.is_branch_selected(2)); // GoneUpstream
        assert!(!app.is_branch_selected(3)); // Unmerged (no force)
        assert!(!app.is_branch_selected(4)); // Current

        // Toggle again to deselect all
        app.select_all_safe();
        assert_eq!(app.selected_count(), 0);

        // With force mode, unmerged branches should be selectable
        app.force_mode = true;
        app.select_all_safe();
        assert!(app.is_branch_selected(1)); // SafeMerged
        assert!(app.is_branch_selected(2)); // GoneUpstream
        assert!(app.is_branch_selected(3)); // Unmerged (with force)
    }

    #[test]
    fn test_clear_selection() {
        let mut app = create_test_app();

        app.toggle_selection(1);
        app.toggle_selection(2);
        assert_eq!(app.selected_count(), 2);

        app.clear_selection();
        assert_eq!(app.selected_count(), 0);
    }

    #[test]
    fn test_get_selected_branches() {
        let mut app = create_test_app();

        app.toggle_selection(1);
        app.toggle_selection(2);

        let selected = app.get_selected_branches();
        assert_eq!(selected.len(), 2);

        // Check that both branches are present (order not guaranteed from HashSet)
        let names: Vec<&str> = selected.iter().map(|b| b.name.as_str()).collect();
        assert!(names.contains(&"feature/merged"));
        assert!(names.contains(&"feature/gone"));
    }

    #[test]
    fn test_selected_branch() {
        let mut app = create_test_app();

        let branch = app.selected_branch();
        assert!(branch.is_some());
        assert_eq!(branch.unwrap().name, "main");

        app.selected_index = 1;
        let branch = app.selected_branch();
        assert_eq!(branch.unwrap().name, "feature/merged");
    }

    #[test]
    fn test_quit() {
        let mut app = create_test_app();
        assert!(!app.should_quit);

        app.quit();
        assert!(app.should_quit);
    }

    #[test]
    fn test_confirmation_modal() {
        let mut app = create_test_app();

        // Cannot show confirmation with no selection
        app.show_confirm_modal();
        assert!(!app.show_confirmation);

        // Can show with selection
        app.toggle_selection(1);
        app.show_confirm_modal();
        assert!(app.show_confirmation);

        // Can hide
        app.hide_confirm_modal();
        assert!(!app.show_confirmation);
    }

    #[test]
    fn test_action_log() {
        let mut app = create_test_app();

        app.action_log.push(ActionLogEntry {
            branch_name: "test1".to_string(),
            success: true,
            message: "Deleted (-d)".to_string(),
        });

        app.action_log.push(ActionLogEntry {
            branch_name: "test2".to_string(),
            success: false,
            message: "Failed to delete".to_string(),
        });

        assert_eq!(app.deletion_success_count(), 1);
        assert_eq!(app.deletion_failure_count(), 1);
    }

    #[test]
    fn test_toggle_selection_at_cursor() {
        let mut app = create_test_app();

        // Move to safe merged branch
        app.selected_index = 1;
        app.toggle_selection_at_cursor();
        assert!(app.is_branch_selected(1));

        // Move to another branch
        app.selected_index = 2;
        app.toggle_selection_at_cursor();
        assert!(app.is_branch_selected(2));
    }

    #[test]
    fn test_filtered_selection() {
        let mut app = create_test_app();

        // Switch to SafeMerged filter
        app.set_filter(FilterMode::SafeMerged);

        // Should have only 1 item in filtered list
        let filtered = app.filtered_branches();
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].name, "feature/merged");

        // Select at cursor (should select the merged branch)
        app.selected_index = 0;
        app.toggle_selection_at_cursor();

        // Original index 1 should be selected
        assert!(app.is_branch_selected(1));
    }

    fn create_test_branch_with_author(
        name: &str,
        status: BranchStatus,
        author: &str,
    ) -> BranchInfo {
        BranchInfo {
            name: name.to_string(),
            upstream: None,
            last_commit_relative: "2 days ago".to_string(),
            status,
            last_commit_sha: "abc123".to_string(),
            last_commit_author: author.to_string(),
            last_commit_message: "Test commit".to_string(),
            ahead: None,
            behind: None,
            last_activity_timestamp: 0,
            branch_created_timestamp: 0,
            branch_author: author.to_string(),
            pr_info: None,
            worktree: None,
            worktree_shadowed_status: None,
        }
    }

    #[test]
    fn test_author_filter() {
        let branches = vec![
            create_test_branch_with_author("alice-feature", BranchStatus::SafeMerged, "Alice"),
            create_test_branch_with_author("bob-feature", BranchStatus::SafeMerged, "Bob"),
            create_test_branch_with_author("alice-fix", BranchStatus::GoneUpstream, "Alice"),
        ];
        let mut app = App::new(
            branches,
            "/test/repo".to_string(),
            "main".to_string(),
            "Alice".to_string(),
        );

        // Test @author:alice filter
        app.search_query = "@author:alice".to_string();
        let filtered = app.filtered_branches();
        assert_eq!(filtered.len(), 2);
        assert!(filtered
            .iter()
            .all(|b| b.branch_author.to_lowercase() == "alice"));

        // Test @author:bob filter
        app.search_query = "@author:bob".to_string();
        let filtered = app.filtered_branches();
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].branch_author, "Bob");

        // Test @author:me filter (should match current_git_user which is "Alice")
        app.search_query = "@author:me".to_string();
        let filtered = app.filtered_branches();
        assert_eq!(filtered.len(), 2);
        assert!(filtered
            .iter()
            .all(|b| b.branch_author.to_lowercase() == "alice"));
    }

    #[test]
    fn test_combined_name_and_author_filter() {
        let branches = vec![
            create_test_branch_with_author("alice-feature", BranchStatus::SafeMerged, "Alice"),
            create_test_branch_with_author("alice-bugfix", BranchStatus::SafeMerged, "Alice"),
            create_test_branch_with_author("bob-feature", BranchStatus::SafeMerged, "Bob"),
        ];
        let mut app = App::new(
            branches,
            "/test/repo".to_string(),
            "main".to_string(),
            "Alice".to_string(),
        );

        // Test combining name search with author filter
        app.search_query = "feature @author:alice".to_string();
        let filtered = app.filtered_branches();
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].name, "alice-feature");

        // Test author filter before name search
        app.search_query = "@author:alice feature".to_string();
        let filtered = app.filtered_branches();
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].name, "alice-feature");
    }

    #[test]
    fn test_unique_authors_collected() {
        let branches = vec![
            create_test_branch_with_author("branch1", BranchStatus::SafeMerged, "Alice"),
            create_test_branch_with_author("branch2", BranchStatus::SafeMerged, "Bob"),
            create_test_branch_with_author("branch3", BranchStatus::SafeMerged, "Alice"),
            create_test_branch_with_author("branch4", BranchStatus::SafeMerged, "Charlie"),
        ];
        let app = App::new(
            branches,
            "/test/repo".to_string(),
            "main".to_string(),
            "Test".to_string(),
        );

        // Should have 3 unique authors, sorted alphabetically
        assert_eq!(app.unique_authors.len(), 3);
        assert_eq!(app.unique_authors[0], "Alice");
        assert_eq!(app.unique_authors[1], "Bob");
        assert_eq!(app.unique_authors[2], "Charlie");
    }

    #[test]
    fn test_suggestions_for_at_symbol() {
        let branches = vec![create_test_branch_with_author(
            "branch1",
            BranchStatus::SafeMerged,
            "Alice",
        )];
        let mut app = App::new(
            branches,
            "/test/repo".to_string(),
            "main".to_string(),
            "Test".to_string(),
        );

        // Type @ - should show "author" command
        app.search_query = "@".to_string();
        app.update_suggestions();
        assert!(app.show_suggestions);
        assert_eq!(app.suggestions.len(), 1);
        assert_eq!(app.suggestions[0], "author");
    }

    #[test]
    fn test_suggestions_for_author_prefix() {
        let branches = vec![
            create_test_branch_with_author("branch1", BranchStatus::SafeMerged, "Alice"),
            create_test_branch_with_author("branch2", BranchStatus::SafeMerged, "Bob"),
        ];
        let mut app = App::new(
            branches,
            "/test/repo".to_string(),
            "main".to_string(),
            "Test".to_string(),
        );

        // Type @author: - should show "me" and authors
        app.search_query = "@author:".to_string();
        app.update_suggestions();
        assert!(app.show_suggestions);
        assert!(app.suggestions.contains(&"me".to_string()));
        assert!(app.suggestions.contains(&"Alice".to_string()));
        assert!(app.suggestions.contains(&"Bob".to_string()));
    }

    #[test]
    fn test_accept_command_suggestion() {
        let branches = vec![create_test_branch_with_author(
            "branch1",
            BranchStatus::SafeMerged,
            "Alice",
        )];
        let mut app = App::new(
            branches,
            "/test/repo".to_string(),
            "main".to_string(),
            "Test".to_string(),
        );

        // Type @ and accept suggestion
        app.search_query = "@".to_string();
        app.update_suggestions();
        app.accept_suggestion();

        // Should have @author: in query
        assert_eq!(app.search_query, "@author:");
    }

    #[test]
    fn test_accept_author_suggestion() {
        let branches = vec![create_test_branch_with_author(
            "branch1",
            BranchStatus::SafeMerged,
            "Alice",
        )];
        let mut app = App::new(
            branches,
            "/test/repo".to_string(),
            "main".to_string(),
            "Test".to_string(),
        );

        // Type @author:a and accept Alice suggestion
        app.search_query = "@author:a".to_string();
        app.update_suggestions();

        // First suggestion after "me" should be Alice
        if let Some(alice_idx) = app.suggestions.iter().position(|s| s == "Alice") {
            app.suggestion_index = Some(alice_idx);
            app.accept_suggestion();
            assert_eq!(app.search_query, "@author:Alice");
        }
    }

    #[test]
    fn test_suggestion_navigation() {
        let branches = vec![
            create_test_branch_with_author("branch1", BranchStatus::SafeMerged, "Alice"),
            create_test_branch_with_author("branch2", BranchStatus::SafeMerged, "Bob"),
        ];
        let mut app = App::new(
            branches,
            "/test/repo".to_string(),
            "main".to_string(),
            "Test".to_string(),
        );

        // Type @author: to get suggestions
        app.search_query = "@author:".to_string();
        app.update_suggestions();

        let initial_idx = app.suggestion_index;
        assert_eq!(initial_idx, Some(0));

        // Navigate next
        app.suggestion_next();
        assert_eq!(app.suggestion_index, Some(1));

        // Navigate prev
        app.suggestion_prev();
        assert_eq!(app.suggestion_index, Some(0));

        // Navigate prev from 0 should wrap to end
        app.suggestion_prev();
        assert_eq!(app.suggestion_index, Some(app.suggestions.len() - 1));
    }

    #[test]
    fn test_quoted_author_filter() {
        let branches = vec![
            create_test_branch_with_author("branch1", BranchStatus::SafeMerged, "Emil Ivanichkov"),
            create_test_branch_with_author("branch2", BranchStatus::SafeMerged, "Alice Smith"),
            create_test_branch_with_author(
                "branch3",
                BranchStatus::GoneUpstream,
                "Emil Ivanichkov",
            ),
        ];
        let mut app = App::new(
            branches,
            "/test/repo".to_string(),
            "main".to_string(),
            "Test".to_string(),
        );

        // Test quoted author filter with spaces
        app.search_query = "@author:\"emil ivanichkov\"".to_string();
        let filtered = app.filtered_branches();
        assert_eq!(filtered.len(), 2);
        assert!(filtered
            .iter()
            .all(|b| b.branch_author.to_lowercase() == "emil ivanichkov"));

        // Test that unquoted version only matches partial (Emil)
        app.search_query = "@author:emil ivanichkov".to_string();
        let filtered = app.filtered_branches();
        // "ivanichkov" is treated as branch name filter, not part of author
        // So it should match branches where author contains "emil" AND name contains "ivanichkov"
        assert_eq!(filtered.len(), 0); // No branch name contains "ivanichkov"
    }

    #[test]
    fn test_accept_suggestion_with_spaces() {
        let branches = vec![create_test_branch_with_author(
            "branch1",
            BranchStatus::SafeMerged,
            "Emil Ivanichkov",
        )];
        let mut app = App::new(
            branches,
            "/test/repo".to_string(),
            "main".to_string(),
            "Test".to_string(),
        );

        // Type @author: and accept suggestion with spaces
        app.search_query = "@author:".to_string();
        app.update_suggestions();

        // Find Emil Ivanichkov in suggestions
        if let Some(idx) = app.suggestions.iter().position(|s| s == "Emil Ivanichkov") {
            app.suggestion_index = Some(idx);
            app.accept_suggestion();
            // Should be wrapped in quotes
            assert_eq!(app.search_query, "@author:\"Emil Ivanichkov\"");
        }
    }

    #[test]
    fn test_accept_author_suggestion_closes_dropdown() {
        let branches = vec![create_test_branch_with_author(
            "branch1",
            BranchStatus::SafeMerged,
            "Alice",
        )];
        let mut app = App::new(
            branches,
            "/test/repo".to_string(),
            "main".to_string(),
            "Test".to_string(),
        );

        for author in ["me", "Alice"] {
            app.search_query = "@author:".to_string();
            app.update_suggestions();
            let idx = app.suggestions.iter().position(|s| s == author).unwrap();
            app.suggestion_index = Some(idx);
            assert!(app.accept_suggestion());

            // Dropdown must close so Up/Down browse the filtered branch list
            assert_eq!(app.search_query, format!("@author:{}", author));
            assert!(!app.show_suggestions);
            assert_eq!(app.suggestion_index, None);
        }
    }

    fn test_worktree(path: &str, branch: &str, status: BranchStatus) -> WorktreeInfo {
        use crate::worktree::WorktreeEntry;
        use crate::worktree_view::{ChangeCounts, Changes, CommitSummary};
        WorktreeInfo {
            entry: WorktreeEntry {
                path: std::path::PathBuf::from(path),
                head: "abc1234".to_string(),
                branch: Some(branch.to_string()),
                is_main: false,
                is_bare: false,
                is_detached: false,
                locked: None,
                prunable: None,
            },
            is_current: false,
            changes: Changes::Loaded(ChangeCounts::default()),
            branch_status: Some(status),
            upstream: None,
            ahead: None,
            behind: None,
            commit: CommitSummary::default(),
            pr_info: None,
        }
    }

    fn create_test_app_with_worktrees() -> App {
        let mut app = create_test_app();
        app.worktrees = Some(WorktreeView::from_infos(vec![
            test_worktree("/wt/a", "feature/merged", BranchStatus::SafeMerged),
            test_worktree("/wt/b", "feature/unmerged", BranchStatus::Unmerged),
        ]));
        app
    }

    #[test]
    fn reloading_worktrees_keeps_selection_status_and_cursor() {
        use crate::worktree_view::{parse_porcelain_status, Changes};

        let mut app = create_test_app_with_worktrees();
        app.toggle_view();
        app.worktree_remove_mode = true;
        app.selected_index = 1; // /wt/b
        app.worktrees
            .as_mut()
            .unwrap()
            .toggle(std::path::Path::new("/wt/a"), false);

        // The fresh list has a new worktree first and statuses not read yet
        let mut fresh = vec![
            test_worktree("/wt/0", "feature/zero", BranchStatus::SafeMerged),
            test_worktree("/wt/a", "feature/merged", BranchStatus::SafeMerged),
            test_worktree("/wt/b", "feature/unmerged", BranchStatus::Unmerged),
        ];
        for wt in fresh.iter_mut() {
            wt.changes = Changes::Loading;
        }
        app.set_worktree_view(WorktreeView::from_infos(fresh));

        let view = app.worktrees.as_ref().unwrap();
        assert_eq!(
            view.selected,
            HashSet::from([std::path::PathBuf::from("/wt/a")])
        );
        assert_eq!(
            view.worktrees[1].changes,
            Changes::Loaded(parse_porcelain_status(""))
        );
        assert_eq!(
            app.selected_worktree().unwrap().entry.path.to_str(),
            Some("/wt/b")
        );
    }

    #[test]
    fn test_toggle_view_keeps_cursor_and_search_per_view() {
        let mut app = create_test_app_with_worktrees();
        app.selected_index = 3;
        app.search_query = "feature".to_string();
        app.search_cursor_pos = 7;

        app.toggle_view();
        assert_eq!(app.view, ViewMode::Worktrees);
        assert_eq!(app.selected_index, 0);
        assert!(app.search_query.is_empty());
        assert_eq!(app.visible_len(), 2);

        // Navigation is bounded by the worktree list
        app.select_next();
        app.select_next();
        assert_eq!(app.selected_index, 1);
        app.search_query = "unmerged".to_string();
        assert_eq!(app.visible_len(), 1);

        app.toggle_view();
        assert_eq!(app.view, ViewMode::Branches);
        assert_eq!(app.selected_index, 3);
        assert_eq!(app.search_query, "feature");
        assert_eq!(app.search_cursor_pos, 7);
        // The worktree query is kept for when the view comes back
        assert_eq!(app.filtered_worktrees().len(), 1);
    }

    #[test]
    fn worktree_view_is_read_only_until_remove_mode() {
        let mut app = create_test_app_with_worktrees();
        app.toggle_view();
        assert!(!app.worktree_remove_mode);

        app.toggle_worktree_at_cursor();
        assert_eq!(app.selected_count(), 0);
        assert_eq!(app.flash.as_deref(), Some(REMOVAL_OFF_HINT));

        app.flash = None;
        app.select_all_stale_worktrees();
        assert_eq!(app.selected_count(), 0);
        assert_eq!(app.flash.as_deref(), Some(REMOVAL_OFF_HINT));

        app.flash = None;
        app.request_worktree_removal();
        assert!(!app.show_confirmation);
        assert_eq!(app.flash.as_deref(), Some(REMOVAL_OFF_HINT));
    }

    #[test]
    fn remove_mode_enables_selection_and_turning_it_off_clears_it() {
        let mut app = create_test_app_with_worktrees();
        app.toggle_view();
        app.toggle_worktree_remove_mode();
        assert!(app.require_worktree_remove_mode());
        assert_eq!(app.flash, None);

        app.select_all_stale_worktrees();
        assert_eq!(app.selected_count(), 1); // /wt/a is clean and merged
        app.request_worktree_removal();
        assert!(app.show_confirmation);
        app.show_confirmation = false;

        app.worktrees.as_mut().unwrap().delete_branches = true;
        app.toggle_worktree_remove_mode();
        assert!(!app.worktree_remove_mode);
        assert_eq!(app.selected_count(), 0);
        assert!(!app.worktrees.as_ref().unwrap().delete_branches);
    }

    #[test]
    fn remove_selected_worktrees_does_nothing_while_removal_is_off() {
        let mut app = create_test_app_with_worktrees();
        app.toggle_view();
        app.toggle_worktree_remove_mode();
        app.select_all_stale_worktrees();
        // Selection survives only while on; force it back to simulate a
        // stale confirmation after removal was turned off
        app.worktree_remove_mode = false;
        app.dry_run = true;
        app.remove_selected_worktrees();
        assert!(app.action_log.is_empty());
        assert_eq!(app.selected_count(), 1);
    }

    #[test]
    fn force_mode_on_keeps_the_branch_selection() {
        let mut app = create_test_app();
        app.toggle_selection(1); // feature/merged
        app.toggle_selection(2); // feature/gone
        app.toggle_force_mode();
        assert!(app.force_mode);
        assert_eq!(
            app.selected_branches,
            HashSet::from(["feature/merged".to_string(), "feature/gone".to_string()])
        );
    }

    #[test]
    fn selection_follows_its_branches_when_the_list_is_re_sorted() {
        let mut app = create_test_app();
        app.toggle_selection(1); // feature/merged
        app.toggle_selection(2); // feature/gone
        app.cycle_sort_mode(); // Status -> Name moves both branches
        assert_ne!(app.branches[1].name, "feature/merged");
        let names: Vec<&str> = app
            .get_selected_branches()
            .iter()
            .map(|b| b.name.as_str())
            .collect();
        assert_eq!(names.len(), 2);
        assert!(names.contains(&"feature/merged"));
        assert!(names.contains(&"feature/gone"));
    }

    #[test]
    fn refresh_status_shows_a_recent_reload_for_a_few_seconds() {
        let mut app = create_test_app();
        let now = Instant::now();
        assert_eq!(app.refresh_status(now), None);
        app.last_reload = Some((now, "14:32:05".to_string()));
        assert_eq!(
            app.refresh_status(now),
            Some("updated 14:32:05".to_string())
        );
        let later = now + RELOAD_NOTICE_DURATION + Duration::from_secs(1);
        assert_eq!(app.refresh_status(later), None);
    }

    #[test]
    fn replacing_branches_keeps_the_sort_mode() {
        let mut app = create_test_app();
        app.cycle_sort_mode(); // Status -> Name
        let fresh = vec![
            create_test_branch("zeta", BranchStatus::SafeMerged),
            create_test_branch("alpha", BranchStatus::SafeMerged),
        ];
        app.replace_branches(fresh);
        assert_eq!(app.branches[0].name, "alpha");
        assert_eq!(app.branches[1].name, "zeta");
    }

    #[test]
    fn replacing_branches_keeps_the_cursor_on_the_same_branch() {
        let mut app = create_test_app();
        app.selected_index = 2; // feature/gone
        let mut fresh: Vec<BranchInfo> = app.branches.clone();
        fresh.insert(
            1,
            create_test_branch("feature/new", BranchStatus::SafeMerged),
        );
        app.replace_branches(fresh);
        assert_eq!(app.selected_branch().unwrap().name, "feature/gone");
    }

    #[test]
    fn replacing_branches_keeps_the_row_when_the_cursor_branch_is_gone() {
        let mut app = create_test_app();
        app.selected_index = 4; // current-branch, last row
        let fresh: Vec<BranchInfo> = app.branches[..3].to_vec();
        app.replace_branches(fresh);
        assert_eq!(app.selected_index, 2);
    }

    #[test]
    fn replacing_branches_drops_vanished_and_unselectable_selections() {
        let mut app = create_test_app();
        app.toggle_selection(1); // feature/merged
        app.toggle_selection(2); // feature/gone
        let mut fresh: Vec<BranchInfo> = app.branches.clone();
        fresh.retain(|b| b.name != "feature/gone");
        // A new commit on feature/merged made it unmerged
        fresh
            .iter_mut()
            .find(|b| b.name == "feature/merged")
            .unwrap()
            .status = BranchStatus::Unmerged;
        fresh.push(create_test_branch(
            "feature/other",
            BranchStatus::SafeMerged,
        ));
        app.replace_branches(fresh);
        assert!(app.selected_branches.is_empty());
    }

    #[test]
    fn replacing_branches_updates_author_completion() {
        let mut app = create_test_app();
        let mut newcomer = create_test_branch("feature/new", BranchStatus::SafeMerged);
        newcomer.branch_author = "Newcomer".to_string();
        let mut fresh: Vec<BranchInfo> = app.branches.clone();
        fresh.push(newcomer);
        app.replace_branches(fresh);
        assert!(app.unique_authors.contains(&"Newcomer".to_string()));
    }

    #[test]
    fn force_mode_off_drops_only_force_only_branches() {
        let mut app = create_test_app();
        app.toggle_force_mode();
        app.toggle_selection(1); // merged
        app.toggle_selection(2); // gone
        app.toggle_selection(3); // unmerged (force only)
        assert_eq!(app.selected_count(), 3);

        app.toggle_force_mode();
        assert!(!app.force_mode);
        assert_eq!(
            app.selected_branches,
            HashSet::from(["feature/merged".to_string(), "feature/gone".to_string()])
        );
    }

    #[test]
    fn force_mode_off_drops_dirty_worktrees_in_both_views() {
        use crate::worktree_view::{parse_porcelain_status, Changes};

        let mut app = create_test_app();
        let mut dirty = test_worktree("/wt/dirty", "feature/unmerged", BranchStatus::Unmerged);
        dirty.changes = Changes::Loaded(parse_porcelain_status(" M src/a.rs\n"));
        app.worktrees = Some(WorktreeView::from_infos(vec![
            test_worktree("/wt/clean", "feature/merged", BranchStatus::SafeMerged),
            dirty,
        ]));
        app.toggle_force_mode();
        let wt = app.worktrees.as_mut().unwrap();
        wt.toggle(std::path::Path::new("/wt/clean"), true);
        wt.toggle(std::path::Path::new("/wt/dirty"), true);
        app.toggle_selection(1); // merged branch
        app.toggle_selection(3); // unmerged branch

        // Toggled from the branch view: the worktree selection is pruned too
        app.toggle_force_mode();
        let selected = &app.worktrees.as_ref().unwrap().selected;
        assert_eq!(
            *selected,
            HashSet::from([std::path::PathBuf::from("/wt/clean")])
        );
        assert_eq!(
            app.selected_branches,
            HashSet::from(["feature/merged".to_string()])
        );
    }

    #[test]
    fn test_selection_is_per_view() {
        let mut app = create_test_app_with_worktrees();
        app.toggle_selection(1);
        app.toggle_view();
        assert_eq!(app.selected_count(), 0);
        app.toggle_worktree_remove_mode();
        app.toggle_worktree_at_cursor();
        assert_eq!(app.selected_count(), 1);

        // c clears only the view on screen
        app.toggle_view();
        assert_eq!(app.selected_count(), 1);
        app.clear_selection();
        assert_eq!(app.selected_count(), 0);
        app.toggle_view();
        assert_eq!(app.selected_count(), 1);
    }

    #[test]
    fn test_worktree_filter_tabs() {
        let mut app = create_test_app_with_worktrees();
        app.toggle_view();
        app.set_worktree_filter(WorktreeFilter::Stale);
        assert_eq!(app.visible_len(), 1);
        app.next_worktree_filter();
        assert_eq!(app.worktrees.as_ref().unwrap().filter, WorktreeFilter::All);
        assert_eq!(app.visible_len(), 2);
    }

    #[test]
    fn test_no_author_suggestions_in_worktree_view() {
        let mut app = create_test_app_with_worktrees();
        app.toggle_view();
        app.search_insert_char('@');
        assert!(!app.show_suggestions);
    }

    #[test]
    fn test_branch_delete_needs_force() {
        assert!(!branch_delete_needs_force(&BranchStatus::SafeMerged, false));
        assert!(branch_delete_needs_force(&BranchStatus::SafeMerged, true));
        assert!(branch_delete_needs_force(
            &BranchStatus::GoneUpstream,
            false
        ));
        assert!(branch_delete_needs_force(&BranchStatus::Unmerged, false));
        assert!(branch_delete_needs_force(&BranchStatus::Local, false));
    }

    #[test]
    fn test_double_esc_quits() {
        let mut app = create_test_app();
        let start = Instant::now();

        app.press_esc_to_quit(start);
        assert!(!app.should_quit);
        assert_eq!(app.esc_quit_armed_at, Some(start));

        app.press_esc_to_quit(start + Duration::from_millis(500));
        assert!(app.should_quit);
    }

    #[test]
    fn test_esc_quit_expires_after_window() {
        let mut app = create_test_app();
        let start = Instant::now();

        app.press_esc_to_quit(start);
        app.expire_esc_quit(start + Duration::from_millis(500));
        assert!(app.esc_quit_armed_at.is_some());

        let late = start + ESC_QUIT_WINDOW + Duration::from_millis(1);
        app.expire_esc_quit(late);
        assert_eq!(app.esc_quit_armed_at, None);

        // A late second press only re-arms
        app.press_esc_to_quit(start);
        app.press_esc_to_quit(late);
        assert!(!app.should_quit);
        assert_eq!(app.esc_quit_armed_at, Some(late));
    }
}
