// Application state management

use crate::git::BranchInfo;

/// Main application state
pub struct App {
    pub branches: Vec<BranchInfo>,
    pub selected_index: usize,
    pub should_quit: bool,
}

impl App {
    pub fn new(branches: Vec<BranchInfo>) -> Self {
        Self {
            branches,
            selected_index: 0,
            should_quit: false,
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
}
