# Issues, Bugs & Feature Requests

**Last Updated:** 2026-02-13 18:55

---

## Overview

This document tracks bugs, issues, and feature requests for the local-git-branch-cleanup-tui application.

---

## Status Legend

| Status         | Description                         |
| -------------- | ----------------------------------- |
| 🔴 Open        | Issue identified, not yet addressed |
| 🟡 In Progress | Currently being worked on           |
| 🟢 Resolved    | Fix implemented and verified        |
| ⚪ Won't Fix   | Decided not to address              |

---

## Critical Issues

_No open critical issues._

---

## UI/UX Issues

### Issue #7: Scrolling behavior causes unnecessary viewport movement

- **GitHub:** [#19](https://github.com/EmilIvanichkovv/omni-scripts/issues/19)
- **Status:** � Resolved
- **Reported:** 2026-02-13
- **Resolved:** 2026-02-13
- **Category:** UI/UX / Bug
- **Description:**
  - When navigating up from the bottom of the visible area, the viewport scrolls/re-renders even when there's room for the cursor to move within the current view
  - The viewport should remain stable while the cursor moves within the visible area
- **Steps to Reproduce:**
  1. Open the TUI in a repository with enough branches to require scrolling
  2. Scroll down to the bottom of the list
  3. Press up arrow to move selection upward
- **Expected Behavior:**
  - The cursor/selection moves up within the visible area
  - The viewport stays in place until the cursor reaches the top edge of the visible area
  - Only then should the viewport scroll to reveal more items above
- **Actual Behavior:**
  - The viewport scrolls/re-renders immediately when moving up, even when cursor is not at the top of visible area
- **Fix:**
  - Added `scroll_offset` and `visible_height` fields to App state
  - Implemented `adjust_scroll_for_selection()` method for "edge-only" scrolling
  - Viewport only adjusts when cursor would move outside visible bounds
  - Used `TableState::with_offset()` to control scroll position manually

---

### Issue #8: Missing keyboard shortcuts for list navigation

- **GitHub:** [#20](https://github.com/EmilIvanichkovv/omni-scripts/issues/20)
- **Status:** 🔴 Open
- **Reported:** 2026-02-13
- **Category:** UI/UX / Enhancement
- **Description:**
  - The TUI is missing common keyboard shortcuts for efficient list navigation
  - Users expect vim-like and standard terminal navigation keys to work
- **Expected Behavior:**
  - **Go to top of list:**
    - `Home` key
    - `Ctrl+U`
    - `g` (vim-style)
  - **Go to bottom of list:**
    - `End` key
    - `Ctrl+D`
    - `G` (vim-style, Shift+g)
  - **Page navigation:**
    - `Page Up` - move up by one page/viewport height
    - `Page Down` - move down by one page/viewport height
- **Actual Behavior:**
  - These keyboard shortcuts are not implemented
  - Users can only navigate one item at a time with arrow keys
- **Suggested Fix:**
  - Add key handlers in `main.rs` for:
    - `KeyCode::Home`, `Ctrl+U`, `g` → `app.selected_index = 0`
    - `KeyCode::End`, `Ctrl+D`, `G` → `app.selected_index = app.branches.len() - 1`
    - `KeyCode::PageUp` → move up by visible area height
    - `KeyCode::PageDown` → move down by visible area height

---

### Issue #9: Add ability to sort branches by creation date

- **GitHub:** [#21](https://github.com/EmilIvanichkovv/omni-scripts/issues/21)
- **Status:** 🔴 Open
- **Reported:** 2026-02-13
- **Category:** UI/UX / Enhancement
- **Description:**
  - Users should be able to sort branches by their creation date/time
  - This helps identify old branches that may need cleanup
- **Expected Behavior:**
  - Add a keyboard shortcut (e.g., `s` or `o` for sort/order) to toggle sorting mode
  - Sorting options:
    - By name (alphabetical) - current default
    - By creation date (newest first)
    - By creation date (oldest first)
  - Visual indicator in the UI showing current sort order
- **Actual Behavior:**
  - Branches are only displayed in default order (alphabetical or git's default)
  - No sorting options available
- **Suggested Fix:**
  - Add `sort_mode` field to App state (enum: Name, NewestFirst, OldestFirst)
  - Fetch branch creation date using `git log -1 --format=%ci <branch>`
  - Add key handler to cycle through sort modes
  - Display current sort mode in footer or header
  - Re-sort branch list when mode changes

---

### Issue #10: Add filter by branch creator/author

- **GitHub:** [#22](https://github.com/EmilIvanichkovv/omni-scripts/issues/22)
- **Status:** 🔴 Open
- **Reported:** 2026-02-13
- **Category:** UI/UX / Enhancement
- **Description:**
  - Users should be able to filter branches by their creator (the author of the first commit on the branch)
  - Useful in team environments to quickly find and manage your own branches
- **Expected Behavior:**
  - Add a keyboard shortcut (e.g., `a` for author) to open author filter
  - Display a list of unique branch authors to select from, or allow typing author name
  - Option to filter by "my branches" (current git user)
  - Show filtered results with indicator of active filter
  - Clear filter option (e.g., press shortcut again or `Esc`)
- **Actual Behavior:**
  - No filtering by author/creator available
  - Users must manually scan through all branches
- **Suggested Fix:**
  - Fetch branch author using `git log -1 --format=%an <branch>` or `git log -1 --format=%ae <branch>` for email
  - Add `author_filter` field to App state (Option<String>)
  - Collect unique authors when loading branches
  - Add key handler to open author selection or toggle "my branches" filter
  - Filter displayed branches based on selected author
  - Show active filter in header/footer (e.g., "Filtered by: john@example.com")

---

### Issue #11: Show GitHub PR association for branches

- **GitHub:** [#23](https://github.com/EmilIvanichkovv/omni-scripts/issues/23)
- **Status:** 🔴 Open
- **Reported:** 2026-02-13
- **Category:** UI/UX / Enhancement
- **Description:**
  - Users should be able to see if a branch has an associated Pull Request on GitHub
  - This helps identify branches that are part of active/merged/closed PRs
- **Expected Behavior:**
  - Display PR status next to branch name (e.g., PR #123, merged/open/closed)
  - Show PR title on hover or in a details panel
  - Optional: Link to open PR in browser (e.g., `o` key to open PR URL)
  - Visual indicators:
    - 🟢 PR merged
    - 🟡 PR open
    - 🔴 PR closed (not merged)
    - ⚪ No PR associated
- **Actual Behavior:**
  - No GitHub integration
  - Users must manually check GitHub for PR status
- **Suggested Fix:**
  - Use GitHub CLI (`gh pr list --head <branch>`) or GitHub API to fetch PR info
  - Add optional `--github` flag to enable PR fetching (may slow startup)
  - Cache PR data to avoid repeated API calls
  - Add `pr_info` field to Branch struct: `Option<PrInfo>` with number, state, title, url
  - Display PR indicator in branch list table
  - Add key handler to open PR URL in default browser
- **Notes:**
  - Requires `gh` CLI or GitHub API token for authentication
  - Consider rate limiting and error handling for API calls
  - Could be optional feature enabled via flag or config

---

## Performance Issues

_No open performance issues._

---

## Minor/Cosmetic Issues

_No open minor/cosmetic issues._

---

## Resolved Issues

### Issue #1: Force delete mode not using `git branch -D`

- **GitHub:** [#11](https://github.com/EmilIvanichkovv/omni-scripts/issues/11)
- **Status:** 🟢 Resolved
- **Reported:** 2026-02-13
- **Resolved:** 2026-02-13
- **Category:** Critical / Bug
- **Description:**
  - When attempting to delete a merged branch (marked as "gone") with force mode enabled, the deletion fails with an error suggesting the branch is not fully merged.
- **Steps to Reproduce:**
  1. Have a branch marked as "gone" (remote deleted)
  2. Enable force delete mode
  3. Attempt to delete the branch
- **Expected Behavior:** Branch should be force deleted using `git branch -D`
- **Actual Behavior:**
  - Shows error: `✗ feat/TUI - Failed to delete branch: error: the branch 'feat/TUI' is not fully merged`
  - Hint suggests running `git branch -D feat/TUI`
- **Fix:**
  - In `app.rs`, `delete_selected_branches()` now respects `self.force_mode`
  - Also auto-forces deletion for "gone" branches (handles squash/rebase merges)
  - Changed: `let use_force = self.force_mode || *status == BranchStatus::Unmerged || *status == BranchStatus::GoneUpstream;`
- **Commit:** `🐛(rust/local-git-branch-cleanup-tui): Fix force delete mode to properly use git branch -D`

---

### Issue #2: Branch list scrolling not working with many branches

- **GitHub:** [#12](https://github.com/EmilIvanichkovv/omni-scripts/issues/12)
- **Status:** 🟢 Resolved
- **Reported:** 2026-02-13
- **Resolved:** 2026-02-13
- **Category:** UI/UX / Bug
- **Description:**
  - When there are many branches in the repository, the branch list box does not scroll properly
- **Steps to Reproduce:**
  1. Open the TUI in a repository with many branches (more than fit in the visible area)
  2. Attempt to scroll through the branch list
- **Expected Behavior:** Branch list should scroll to show all branches
- **Actual Behavior:** Scrolling does not work
- **Fix:**
  - Used `TableState` with `render_stateful_widget` instead of `render_widget`
  - `TableState::default().with_selected(Some(app.selected_index))` enables automatic scrolling to keep cursor visible
- **Commit:** `🐛(rust/local-git-branch-cleanup-tui): Fix branch list scrolling with TableState`

---

### Issue #3: Highlight active mode options in footer

- **GitHub:** [#13](https://github.com/EmilIvanichkovv/omni-scripts/issues/13)
- **Status:** 🟢 Resolved
- **Reported:** 2026-02-13
- **Resolved:** 2026-02-13
- **Category:** UI/UX / Enhancement
- **Description:**
  - When user toggles force mode (f) or dry run mode (d), the selected option should be visually highlighted in the footer
- **Expected Behavior:** Active mode options should be highlighted/styled differently in the footer to provide clear visual feedback
- **Fix:**
  - When active, `f force` gets black text on red background
  - When active, `d dry` gets black text on amber background
  - Entire label is highlighted, not just the key
- **Commit:** `✨(rust/local-git-branch-cleanup-tui): Highlight active mode options in footer`

---

### Issue #4: Add info modal (shortcut: i)

- **GitHub:** [#14](https://github.com/EmilIvanichkovv/omni-scripts/issues/14)
- **Status:** 🟢 Resolved
- **Reported:** 2026-02-13
- **Resolved:** 2026-02-13
- **Category:** UI/UX / Enhancement
- **Description:**
  - Add an info modal accessible via the 'i' shortcut that provides users with brief information about what the tool does
- **Expected Behavior:** Pressing 'i' opens a modal with a brief description of the tool's purpose and functionality
- **Fix:**
  - Added `show_info` state to App
  - Added 'i' key handler in main.rs
  - Created `render_info_modal` function in ui.rs with tool description and branch status explanations
  - Added `i info` hint in footer
- **Commit:** `✨(rust/local-git-branch-cleanup-tui): Add info modal with tool description`

---

### Issue #5: Confirmation modal Y/N not visible with many branches selected

- **GitHub:** [#15](https://github.com/EmilIvanichkovv/omni-scripts/issues/15)
- **Status:** 🟢 Resolved
- **Reported:** 2026-02-13
- **Resolved:** 2026-02-13
- **Category:** UI/UX / Bug + Enhancement
- **Description:**
  - When many branches are selected for deletion, the confirmation prompt (Y/N) is cut off and not visible in the modal
  - Additionally, users should be able to confirm with Enter and cancel with Esc (not just y/n)
- **Fix:**
  - Dynamic modal height based on content (branches + warnings)
  - Added Enter key to confirm deletion (in addition to y/Y)
  - Esc already supported for cancel
  - Confirmation hints now centered: `y/Enter confirm    n/Esc cancel`
- **Commit:** `✨(rust/local-git-branch-cleanup-tui): Improve confirmation modal UX`

---

### Issue #6: Improve search focus behavior

- **GitHub:** [#16](https://github.com/EmilIvanichkovv/omni-scripts/issues/16)
- **Status:** 🟢 Resolved
- **Reported:** 2026-02-13
- **Resolved:** 2026-02-13
- **Category:** UI/UX / Enhancement
- **Description:**
  - Improve the search workflow to allow seamless switching between search input and branch selection
- **Expected Behavior:**
  1. User presses `/` to activate search and types a query
  2. User presses `↓` (arrow down) to move focus to the branch list (search input loses focus but query remains)
  3. User can navigate and select branches with arrow keys and Space
  4. User presses `/` again to return focus to search bar and continue editing the query
- **Fix:**
  - Arrow down/up in search mode now exits search but keeps query
  - Pressing `/` re-enters search mode to continue editing
  - Esc clears query and exits search
- **Commit:** `✨(rust/local-git-branch-cleanup-tui): Improve search focus behavior`

---

## Change Log

| Date       | Time  | Issue | Change                                                                 |
| ---------- | ----- | ----- | ---------------------------------------------------------------------- |
| 2026-02-13 | 14:30 | -     | Document created                                                       |
| 2026-02-13 | 14:45 | #1    | Reported: Force delete mode not using `git branch -D`                  |
| 2026-02-13 | 15:00 | #1    | Resolved: Fixed force delete logic in `app.rs`                         |
| 2026-02-13 | 15:15 | #2    | Reported: Branch list scrolling not working                            |
| 2026-02-13 | 15:30 | #2    | Resolved: Implemented `TableState` for proper scrolling                |
| 2026-02-13 | 15:45 | #3    | Reported: No visual feedback for active modes                          |
| 2026-02-13 | 16:00 | #3    | Resolved: Added background highlighting for force/dry modes            |
| 2026-02-13 | 16:15 | #4    | Reported: Missing info modal                                           |
| 2026-02-13 | 16:30 | #4    | Resolved: Added info modal with 'i' shortcut                           |
| 2026-02-13 | 16:45 | #5    | Reported: Confirmation modal issues                                    |
| 2026-02-13 | 17:00 | #5    | Resolved: Dynamic modal height, Enter/Esc support, centered hints      |
| 2026-02-13 | 17:15 | #6    | Reported: Search focus behavior needs improvement                      |
| 2026-02-13 | 17:50 | #6    | Resolved: Arrow keys exit search but keep query, `/` re-enters search  |
| 2026-02-13 | 18:30 | #7    | Reported: Scrolling causes unnecessary viewport movement               |
| 2026-02-13 | 18:35 | #8    | Reported: Missing keyboard shortcuts (Home/End/g/G/PgUp/PgDn/Ctrl+U/D) |
| 2026-02-13 | 18:40 | #9    | Reported: Add ability to sort branches by creation date                |
| 2026-02-13 | 18:45 | #10   | Reported: Add filter by branch creator/author                          |
| 2026-02-13 | 18:50 | #11   | Reported: Show GitHub PR association for branches                      |
| 2026-02-13 | 19:10 | #7    | Resolved: Implemented edge-only scrolling with scroll_offset           |
