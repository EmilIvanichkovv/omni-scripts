// Application state management

use crate::git::{self, BranchInfo, BranchStatus};
use std::collections::HashSet;

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
    /// Set of selected branch indices (for deletion) - relative to all branches
    pub selected_branches: HashSet<usize>,
    /// Whether the app should quit
    pub should_quit: bool,
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
    /// Whether GitHub PR integration is enabled
    pub github_enabled: bool,
}

impl App {
    pub fn new(
        branches: Vec<BranchInfo>,
        repo_path: String,
        trunk: String,
        current_git_user: String,
    ) -> Self {
        // Collect unique branch authors for autocomplete
        let mut unique_authors: Vec<String> = branches
            .iter()
            .map(|b| b.branch_author.clone())
            .filter(|a| !a.is_empty())
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        unique_authors.sort_by_key(|a| a.to_lowercase());

        Self {
            branches,
            selected_index: 0,
            selected_branches: HashSet::new(),
            should_quit: false,
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
            scroll_offset: 0,
            visible_height: 0,
            sort_mode: SortMode::Status,
            current_git_user,
            unique_authors,
            suggestions: Vec::new(),
            suggestion_index: None,
            show_suggestions: false,
            github_enabled: false,
        }
    }

    pub fn quit(&mut self) {
        self.should_quit = true;
    }

    pub fn select_next(&mut self) {
        let filtered = self.filtered_branches();
        if !filtered.is_empty() && self.selected_index < filtered.len() - 1 {
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
        let filtered = self.filtered_branches();
        if !filtered.is_empty() {
            self.selected_index = filtered.len() - 1;
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
        let filtered = self.filtered_branches();
        if filtered.is_empty() || self.visible_height == 0 {
            return;
        }
        let page_size = self.visible_height.saturating_sub(1).max(1);
        self.selected_index = (self.selected_index + page_size).min(filtered.len() - 1);
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
                .filter(|b| b.status == BranchStatus::SafeMerged)
                .collect(),
            FilterMode::GoneUpstream => self
                .branches
                .iter()
                .filter(|b| b.status == BranchStatus::GoneUpstream)
                .collect(),
            FilterMode::Unmerged => self
                .branches
                .iter()
                .filter(|b| b.status == BranchStatus::Unmerged)
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
                .filter(|b| b.status == BranchStatus::SafeMerged)
                .count(),
            FilterMode::GoneUpstream => self
                .branches
                .iter()
                .filter(|b| b.status == BranchStatus::GoneUpstream)
                .count(),
            FilterMode::Unmerged => self
                .branches
                .iter()
                .filter(|b| b.status == BranchStatus::Unmerged)
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
                self.branches.sort_by(|a, b| {
                    let order = |s: &BranchStatus| match s {
                        BranchStatus::Current => 0,
                        BranchStatus::Protected => 1,
                        BranchStatus::SafeMerged => 2,
                        BranchStatus::GoneUpstream => 3,
                        BranchStatus::Unmerged => 4,
                    };
                    order(&a.status).cmp(&order(&b.status))
                });
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

            if let Some(at_pos) = query.rfind('@') {
                let after_at = &query[at_pos + 1..];

                // Accepting a command (no colon yet)
                if !after_at.contains(':') {
                    // Replace everything after @ with the command and add colon
                    self.search_query = format!("{}@{}:", &query[..at_pos], suggestion);
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

            // Update suggestions after accepting
            self.update_suggestions();
            return true;
        }

        false
    }

    /// Hide suggestions dropdown
    pub fn hide_suggestions(&mut self) {
        self.show_suggestions = false;
        self.suggestion_index = None;
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
            // Skip unmerged branches unless force mode is enabled
            if branch.status == BranchStatus::Unmerged && !self.force_mode {
                return;
            }

            if self.selected_branches.contains(&index) {
                self.selected_branches.remove(&index);
            } else {
                self.selected_branches.insert(index);
            }
        }
    }

    /// Select all safe (deletable) branches
    pub fn select_all_safe(&mut self) {
        // If all safe branches are selected, deselect all
        let safe_indices: Vec<usize> = self
            .branches
            .iter()
            .enumerate()
            .filter(|(_, b)| {
                b.status.is_deletable() && (self.force_mode || b.status != BranchStatus::Unmerged)
            })
            .map(|(i, _)| i)
            .collect();

        let all_selected = safe_indices
            .iter()
            .all(|i| self.selected_branches.contains(i));

        if all_selected {
            // Deselect all
            self.selected_branches.clear();
        } else {
            // Select all safe branches
            for i in safe_indices {
                self.selected_branches.insert(i);
            }
        }
    }

    /// Clear all selections
    pub fn clear_selection(&mut self) {
        self.selected_branches.clear();
    }

    /// Get the branches that are selected for deletion
    pub fn get_selected_branches(&self) -> Vec<&BranchInfo> {
        self.selected_branches
            .iter()
            .filter_map(|&i| self.branches.get(i))
            .collect()
    }

    /// Count of selected branches
    pub fn selected_count(&self) -> usize {
        self.selected_branches.len()
    }

    /// Check if a branch index is selected
    pub fn is_branch_selected(&self, index: usize) -> bool {
        self.selected_branches.contains(&index)
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
        let selected: Vec<(usize, String, BranchStatus)> = self
            .selected_branches
            .iter()
            .filter_map(|&i| {
                self.branches
                    .get(i)
                    .map(|b| (i, b.name.clone(), b.status.clone()))
            })
            .collect();

        for (_, branch_name, status) in &selected {
            // Auto-force for "gone" branches (squash/rebase merges) and "unmerged" branches
            // Also respect user's force_mode setting
            let use_force = self.force_mode
                || *status == BranchStatus::Unmerged
                || *status == BranchStatus::GoneUpstream;

            match git::delete_branch_with_mode(branch_name, use_force) {
                Ok(_) => {
                    let method = if use_force { "-D" } else { "-d" };
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

    /// Refresh the branch list (after deletion)
    pub fn refresh_branches(&mut self) {
        if let Ok(branches) = git::get_branches_with_classification(None) {
            self.branches = branches;
            // Reset selection index if out of bounds
            if self.selected_index >= self.branches.len() {
                self.selected_index = self.branches.len().saturating_sub(1);
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

    /// Open the PR URL for the currently selected branch in the default browser
    /// Returns true if a PR URL was opened, false if no PR is associated
    pub fn open_selected_pr(&self) -> bool {
        if let Some(branch) = self.selected_branch() {
            if let Some(pr_info) = &branch.pr_info {
                if git::open_url_in_browser(&pr_info.url).is_ok() {
                    return true;
                }
            }
        }
        false
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
}
