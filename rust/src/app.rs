// Application state management

use crate::git::BranchInfo;

/// Main application state
pub struct App {
    /// All discovered branches
    pub branches: Vec<BranchInfo>,
    /// Currently selected branch index
    pub selected_index: usize,
    /// Whether the app should quit
    pub should_quit: bool,
    /// Repository path
    pub repo_path: String,
    /// Trunk branch name
    pub trunk: String,
}

impl App {
    pub fn new(branches: Vec<BranchInfo>, repo_path: String, trunk: String) -> Self {
        Self {
            branches,
            selected_index: 0,
            should_quit: false,
            repo_path,
            trunk,
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
}
