# QA Issues & Bug Tracking

**Last Updated:** 2026-02-13 16:00

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

- **Status:** 🔴 Open
- **Reported:** 2026-02-13
- **Description:**
  - When there are many branches in the repository, the branch list box does not scroll properly
- **Steps to Reproduce:**
  1. Open the TUI in a repository with many branches (more than fit in the visible area)
  2. Attempt to scroll through the branch list
- **Expected Behavior:** Branch list should scroll to show all branches
- **Actual Behavior:** Scrolling does not work
- **Notes:**

### Issue #3: Highlight active mode options in footer

- **Status:** 🔴 Open
- **Reported:** 2026-02-13
- **Type:** Enhancement
- **Description:**
  - When user toggles force mode (f) or dry run mode (d), the selected option should be visually highlighted in the footer
- **Expected Behavior:** Active mode options should be highlighted/styled differently in the footer to provide clear visual feedback
- **Actual Behavior:** No visual distinction when modes are active
- **Notes:**

### Issue #4: Add info modal (shortcut: i)

- **Status:** 🔴 Open
- **Reported:** 2026-02-13
- **Type:** Enhancement
- **Description:**
  - Add an info modal accessible via the 'i' shortcut that provides users with brief information about what the tool does
- **Expected Behavior:** Pressing 'i' opens a modal with a brief description of the tool's purpose and functionality
- **Notes:**
  - Should explain the tool helps clean up local git branches
  - Could include info about merged/gone branches detection

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
