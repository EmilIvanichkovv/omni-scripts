// Application state management

use crate::git::{self, BranchInfo, BranchStatus};
use std::collections::HashSet;

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
    /// Currently selected branch index (cursor)
    pub selected_index: usize,
    /// Set of selected branch indices (for deletion)
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
        }
    }

    pub fn quit(&mut self) {
        self.should_quit = true;
    }

    pub fn select_next(&mut self) {
        if !self.branches.is_empty() {
            self.selected_index = (self.selected_index + 1) % self.branches.len();
        }
    }

    pub fn select_prev(&mut self) {
        if !self.branches.is_empty() {
            if self.selected_index == 0 {
                self.selected_index = self.branches.len() - 1;
            } else {
                self.selected_index -= 1;
            }
        }
    }

    /// Get the currently selected branch, if any
    pub fn selected_branch(&self) -> Option<&BranchInfo> {
        self.branches.get(self.selected_index)
    }

    /// Count of deletable branches
    pub fn deletable_count(&self) -> usize {
        self.branches.iter().filter(|b| b.status.is_deletable()).count()
    }

    /// Count of protected branches
    pub fn protected_count(&self) -> usize {
        self.branches.len() - self.deletable_count()
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
    pub fn show_confirm_modal(&mut self) {
        if !self.selected_branches.is_empty() {
            self.show_confirmation = true;
        }
    }

    /// Hide the confirmation modal
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
            let use_force = *status == BranchStatus::Unmerged;
            
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
