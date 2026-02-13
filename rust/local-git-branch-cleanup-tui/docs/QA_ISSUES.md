# QA Issues & Bug Tracking

**Last Updated:** 2026-02-13 17:40

---

## Overview

This document tracks bugs, issues, and problems discovered during QA testing of the local-git-branch-cleanup-tui application.

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

### Issue #1: Force delete mode not using `git branch -D`

- **Status:** 🟢 Resolved
- **Reported:** 2026-02-13
- **Resolved:** 2026-02-13
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

---

## UI/UX Issues

### Issue #2: Branch list scrolling not working with many branches

- **Status:** 🟢 Resolved
- **Reported:** 2026-02-13
- **Resolved:** 2026-02-13
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

### Issue #3: Highlight active mode options in footer

- **Status:** 🟢 Resolved
- **Reported:** 2026-02-13
- **Resolved:** 2026-02-13
- **Type:** Enhancement
- **Description:**
  - When user toggles force mode (f) or dry run mode (d), the selected option should be visually highlighted in the footer
- **Expected Behavior:** Active mode options should be highlighted/styled differently in the footer to provide clear visual feedback
- **Fix:**
  - When active, `f force` gets black text on red background
  - When active, `d dry` gets black text on amber background
  - Entire label is highlighted, not just the key

### Issue #4: Add info modal (shortcut: i)

- **Status:** 🟢 Resolved
- **Reported:** 2026-02-13
- **Resolved:** 2026-02-13
- **Type:** Enhancement
- **Description:**
  - Add an info modal accessible via the 'i' shortcut that provides users with brief information about what the tool does
- **Expected Behavior:** Pressing 'i' opens a modal with a brief description of the tool's purpose and functionality
- **Fix:**
  - Added `show_info` state to App
  - Added 'i' key handler in main.rs
  - Created `render_info_modal` function in ui.rs with tool description and branch status explanations
  - Added `i info` hint in footer

### Issue #5: Confirmation modal Y/N not visible with many branches selected

- **Status:** � Resolved
- **Reported:** 2026-02-13
- **Resolved:** 2026-02-13
- **Type:** Bug + Enhancement
- **Description:**
  - When many branches are selected for deletion, the confirmation prompt (Y/N) is cut off and not visible in the modal
  - Additionally, users should be able to confirm with Enter and cancel with Esc (not just y/n)
- **Fix:**
  - Dynamic modal height based on content (branches + warnings)
  - Added Enter key to confirm deletion (in addition to y/Y)
  - Esc already supported for cancel
  - Confirmation hints now centered: `y/Enter confirm    n/Esc cancel`

### Issue #6: Improve search focus behavior

- **Status:** 🔴 Open
- **Reported:** 2026-02-13
- **Type:** Enhancement
- **Description:**
  - Improve the search workflow to allow seamless switching between search input and branch selection
- **Expected Behavior:**
  1. User presses `/` to activate search and types a query
  2. User presses `↓` (arrow down) to move focus to the branch list (search input loses focus but query remains)
  3. User can navigate and select branches with arrow keys and Space
  4. User presses `/` again to return focus to search bar and continue editing the query
- **Actual Behavior:** Current behavior needs verification - may already partially work
- **Notes:**
  - Search query should persist when focus moves to branch list
  - This creates a smooth workflow: search → navigate → select → search more

---

## Performance Issues

### Issue #: [Title]

- **Status:** 🔴 Open
- **Reported:** 2026-02-13
- **Description:**
  - [Describe the issue]
- **Notes:**

---

## Minor/Cosmetic Issues

### Issue #: [Title]

- **Status:** 🔴 Open
- **Reported:** 2026-02-13
- **Description:**
  - [Describe the issue]
- **Notes:**

---

## Resolved Issues

_Move resolved issues here for tracking purposes._

---

## Change Log

| Date       | Changes          |
| ---------- | ---------------- |
| 2026-02-13 | Document created |
