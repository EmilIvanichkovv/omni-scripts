// Application state management

use crate::git::{self, BranchInfo, BranchStatus};
use std::collections::HashSet;

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
    /// Dry run mode - preview actions without executing
    pub dry_run: bool,
    /// Whether to show filter tabs (hidden by default)
    pub show_filter: bool,
    /// Whether search mode is active (input focused)
    pub search_active: bool,
    /// Current search query string
    pub search_query: String,
}

impl App {
    pub fn new(branches: Vec<BranchInfo>, repo_path: String, trunk: String) -> Self {
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
            dry_run: false,
            show_filter: false,
            search_active: false,
            search_query: String::new(),
        }
    }

    pub fn quit(&mut self) {
        self.should_quit = true;
    }

    pub fn select_next(&mut self) {
        let filtered = self.filtered_branches();
        if !filtered.is_empty() {
            self.selected_index = (self.selected_index + 1) % filtered.len();
        }
    }

    pub fn select_prev(&mut self) {
        let filtered = self.filtered_branches();
        if !filtered.is_empty() {
            if self.selected_index == 0 {
                self.selected_index = filtered.len() - 1;
            } else {
                self.selected_index -= 1;
            }
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
            FilterMode::SafeMerged => self.branches
                .iter()
                .filter(|b| b.status == BranchStatus::SafeMerged)
                .collect(),
            FilterMode::GoneUpstream => self.branches
                .iter()
                .filter(|b| b.status == BranchStatus::GoneUpstream)
                .collect(),
            FilterMode::Unmerged => self.branches
                .iter()
                .filter(|b| b.status == BranchStatus::Unmerged)
                .collect(),
        };
        
        // Apply search filter if query is not empty
        if self.search_query.is_empty() {
            status_filtered
        } else {
            let query = self.search_query.to_lowercase();
            status_filtered
                .into_iter()
                .filter(|b| b.name.to_lowercase().contains(&query))
                .collect()
        }
    }

    /// Get count of branches for a specific filter
    pub fn filter_count(&self, filter: FilterMode) -> usize {
        match filter {
            FilterMode::All => self.branches.len(),
            FilterMode::SafeMerged => self.branches
                .iter()
                .filter(|b| b.status == BranchStatus::SafeMerged)
                .count(),
            FilterMode::GoneUpstream => self.branches
                .iter()
                .filter(|b| b.status == BranchStatus::GoneUpstream)
                .count(),
            FilterMode::Unmerged => self.branches
                .iter()
                .filter(|b| b.status == BranchStatus::Unmerged)
                .count(),
        }
    }

    /// Set the current filter mode
    pub fn set_filter(&mut self, filter: FilterMode) {
        self.current_filter = filter;
        // Reset selection when filter changes
        self.selected_index = 0;
    }

    /// Cycle to next filter
    pub fn next_filter(&mut self) {
        self.current_filter = self.current_filter.next();
        self.selected_index = 0;
    }

    /// Count of deletable branches
    #[allow(dead_code)]
    pub fn deletable_count(&self) -> usize {
        self.branches.iter().filter(|b| b.status.is_deletable()).count()
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
        let safe_indices: Vec<usize> = self.branches
            .iter()
            .enumerate()
            .filter(|(_, b)| {
                b.status.is_deletable() && 
                (self.force_mode || b.status != BranchStatus::Unmerged)
            })
            .map(|(i, _)| i)
            .collect();

        let all_selected = safe_indices.iter().all(|i| self.selected_branches.contains(i));

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
        let selected: Vec<(usize, String, BranchStatus)> = self.selected_branches
            .iter()
            .filter_map(|&i| {
                self.branches.get(i).map(|b| (i, b.name.clone(), b.status.clone()))
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
        App::new(branches, "/test/repo".to_string(), "main".to_string())
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
        
        // Wrap around at end
        app.selected_index = 4;
        app.select_next();
        assert_eq!(app.selected_index, 0);
        
        // Wrap around at beginning
        app.selected_index = 0;
        app.select_prev();
        assert_eq!(app.selected_index, 4);
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
        assert!(app.is_branch_selected(1));  // SafeMerged
        assert!(app.is_branch_selected(2));  // GoneUpstream
        assert!(!app.is_branch_selected(3)); // Unmerged (no force)
        assert!(!app.is_branch_selected(4)); // Current
        
        // Toggle again to deselect all
        app.select_all_safe();
        assert_eq!(app.selected_count(), 0);
        
        // With force mode, unmerged branches should be selectable
        app.force_mode = true;
        app.select_all_safe();
        assert!(app.is_branch_selected(1));  // SafeMerged
        assert!(app.is_branch_selected(2));  // GoneUpstream
        assert!(app.is_branch_selected(3));  // Unmerged (with force)
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
}
