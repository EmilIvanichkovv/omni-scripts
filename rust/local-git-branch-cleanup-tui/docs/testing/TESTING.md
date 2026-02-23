# Manual Testing Checklist for local-git-branch-cleanup-tui

This document provides a comprehensive checklist for manual testing of the Rust TUI branch cleanup tool. Use this to verify functionality before release.

## 🎯 Testing Environment Setup

- [ ] Test on Linux (primary development environment)
- [ ] Test on macOS (if available)
- [ ] Test on Windows/WSL (if available)
- [ ] Test with various terminal emulators:
  - [ ] GNOME Terminal
  - [ ] Alacritty
  - [ ] iTerm2 (macOS)
  - [ ] Windows Terminal

## 📋 Test Categories

### 1. Basic Functionality Tests

#### 1.1 Repository Detection
- [ ] Run in a Git repository - should work
- [ ] Run in a non-Git directory - should show clear error message
- [ ] Run in a subdirectory of a Git repo - should work (find repo root)

#### 1.2 Branch Discovery
- [ ] Empty repository (only main/master) - should show protected branch only
- [ ] Repository with merged branches - should classify as "merged"
- [ ] Repository with unmerged branches - should classify as "unmerged"
- [ ] Repository with mix of branch types - should classify correctly
- [ ] Repository with 50+ branches - should handle performance well

#### 1.3 Branch Classification
- [ ] Main branch marked as "protected" ⊘
- [ ] Master branch marked as "protected" ⊘
- [ ] Develop branch marked as "protected" ⊘
- [ ] Current branch marked as "current" ◉
- [ ] Merged branches marked as "merged" ✓
- [ ] Unmerged branches marked as "unmerged" !

### 2. Bash Script Parity Tests

Compare behavior with the original bash script:

#### 2.1 Branch Detection
- [ ] Both find the same branches without remote tracking
- [ ] Both show the same last commit times
- [ ] Both prevent deletion of protected branches

#### 2.2 Safety Features
- [ ] Rust version safer: uses `-d` by default (vs bash always `-D`)
- [ ] Rust version prevents deletion of main/master/develop
- [ ] Rust version prevents deletion of current branch
- [ ] Both require confirmation before deletion

#### 2.3 Output Format
- [ ] Both use box-drawing characters
- [ ] Both show branch status clearly
- [ ] Both provide summary of actions taken

### 3. TUI Mode Tests

#### 3.1 Navigation
- [ ] `j` / Down arrow - move selection down
- [ ] `k` / Up arrow - move selection up
- [ ] Selection stops at top/bottom (no wrap)
- [ ] Selection indicator (highlight) visible
- [ ] `g` / Home - go to first item
- [ ] `G` / End - go to last item
- [ ] `Ctrl+U` - go to first item
- [ ] `Ctrl+D` - go to last item
- [ ] `Page Up` - move up by one page
- [ ] `Page Down` - move down by one page

#### 3.2 Selection
- [ ] `Space` - toggle selection of current branch
- [ ] Checkbox appears when branch selected `[x]`
- [ ] Cannot select protected branches (main/master/develop)
- [ ] Cannot select current branch
- [ ] Cannot select unmerged branches without force mode
- [ ] `a` - select all safe branches
- [ ] `a` again - deselect all
- [ ] `c` - clear all selections

#### 3.3 Filtering
- [ ] `1` or F1 - Show only merged branches
- [ ] `2` or F2 - Show only gone upstream branches
- [ ] `3` or F3 - Show only unmerged branches
- [ ] `4` or F4 - Show all branches
- [ ] Tab - cycle through filters
- [ ] Filter counts update correctly
- [ ] Selection index resets when changing filters

#### 3.4 Details Pane
- [ ] Details pane shows information for selected branch
- [ ] Shows: commit SHA, author, message, ahead/behind counts
- [ ] Updates when selection changes
- [ ] Text wraps properly in pane

#### 3.5 Deletion Flow
- [ ] `Enter` or `d` - show confirmation modal
- [ ] Confirmation modal shows count of branches to delete
- [ ] Confirmation modal lists branch names
- [ ] `y` in confirmation - delete branches
- [ ] `n` or Esc in confirmation - cancel
- [ ] Action log shows results after deletion
- [ ] Branch list refreshes after deletion
- [ ] Selected branches removed from UI after successful deletion

#### 3.6 Force Mode
- [ ] `f` - toggle force mode
- [ ] Header shows "⚠️ FORCE" when enabled
- [ ] Unmerged branches become selectable in force mode
- [ ] Force delete uses `git branch -D`
- [ ] Safe delete uses `git branch -d`

#### 3.7 Dry Run Mode
- [ ] `d` - toggle dry run mode
- [ ] Header shows "🔍 DRY RUN" when enabled
- [ ] Confirmation modal title changes to "Preview"
- [ ] Action log shows "[DRY RUN] Would delete: ..." messages
- [ ] No branches actually deleted in dry run mode
- [ ] Branch list not refreshed in dry run mode

#### 3.8 Help Modal
- [ ] `?` - show help modal
- [ ] Help modal shows all keyboard shortcuts
- [ ] Help organized by category (Navigation, Filters, Selection, Actions, Other)
- [ ] Any key closes help modal
- [ ] Help modal centered and readable

#### 3.9 Exit
- [ ] `q` - quit application
- [ ] Esc - quit application (when not in modal)
- [ ] Terminal restored properly on exit
- [ ] No artifacts left on screen

### 4. CLI Mode Tests

#### 4.1 Basic CLI
- [ ] `--cli` flag activates CLI mode
- [ ] Shows trunk branch
- [ ] Lists all branches with status
- [ ] Shows legend for status icons
- [ ] Shows count of deletable vs protected branches

#### 4.2 CLI with Flags
- [ ] `--trunk develop` - uses develop as trunk branch
- [ ] `--force` - shows "FORCE MODE" indicator
- [ ] `--dry-run` - shows "DRY RUN" indicator
- [ ] `--help` - shows comprehensive help
- [ ] `--version` - shows version number

#### 4.3 CLI Deletion Flow
- [ ] Prompts for confirmation before deletion
- [ ] Accepts 'y' or 'yes' to confirm
- [ ] Accepts 'n' or 'no' to cancel
- [ ] Shows summary of deleted branches
- [ ] Handles errors gracefully

### 5. Edge Cases

#### 5.1 Unusual Repository States
- [ ] Detached HEAD state - doesn't crash
- [ ] Repository with submodules - works correctly
- [ ] Repository with worktrees - works correctly
- [ ] Shallow clone - works correctly
- [ ] Bare repository - handles gracefully

#### 5.2 Branch Name Edge Cases
- [ ] Branch names with spaces - handled correctly
- [ ] Branch names with special characters (/, -, _) - handled correctly
- [ ] Very long branch names - displayed correctly
- [ ] Unicode characters in branch names - handled correctly

#### 5.3 Concurrent Changes
- [ ] Someone else deletes a branch while TUI open - handles gracefully on refresh
- [ ] New commits made while TUI open - shows correct info on refresh
- [ ] Remote branch deleted while TUI open - detects on refresh

#### 5.4 Error Scenarios
- [ ] Git command fails - shows clear error message
- [ ] Insufficient permissions to delete branch - shows error
- [ ] Network issues (if applicable) - handles gracefully
- [ ] Ctrl+C during operation - cleans up properly

### 6. Terminal Compatibility

#### 6.1 Terminal Sizes
- [ ] 80x24 (minimum) - functional
- [ ] 120x40 (medium) - comfortable
- [ ] 200x60 (large) - utilizes space well
- [ ] Resize during operation - adapts correctly

#### 6.2 Color Support
- [ ] 256 color terminals - full colors
- [ ] 16 color terminals - fallback colors work
- [ ] Monochrome terminals - readable without colors

#### 6.3 Special Characters
- [ ] Box drawing characters render correctly
- [ ] Emoji icons display correctly (or fallback gracefully)
- [ ] UTF-8 encoding handled properly

### 7. Performance Tests

#### 7.1 Repository Size
- [ ] 10 branches - instant (<0.1s)
- [ ] 50 branches - very fast (<0.5s)
- [ ] 100 branches - fast (<1s)
- [ ] 200+ branches - acceptable (<2s)

#### 7.2 Operations
- [ ] Navigation smooth (no lag)
- [ ] Filter switching instant
- [ ] Selection toggle instant
- [ ] Deletion with 10 branches fast
- [ ] Deletion with 50 branches acceptable

### 8. Safety Verification

#### 8.1 Protection Mechanisms
- [ ] Cannot delete main/master/develop even with force mode
- [ ] Cannot delete current branch
- [ ] Confirmation required before any deletion
- [ ] Safe delete (-d) fails for unmerged branches (expected)
- [ ] Force delete only used when explicitly enabled

#### 8.2 Data Loss Prevention
- [ ] Accidental 'a' (select all) + Enter requires explicit confirmation
- [ ] Force mode requires explicit toggle, not default
- [ ] Clear visual indicators for destructive actions
- [ ] Action log records all operations for auditing

### 9. User Experience

#### 9.1 Visual Clarity
- [ ] Status icons clearly distinguishable
- [ ] Selected branches clearly marked
- [ ] Current selection clearly highlighted
- [ ] Modal dialogs stand out from background

#### 9.2 Feedback
- [ ] Immediate visual feedback for all actions
- [ ] Action log shows success/failure clearly
- [ ] Error messages helpful and actionable
- [ ] Success messages informative

#### 9.3 Intuitiveness
- [ ] Key bindings follow common conventions (vim-like)
- [ ] Footer shows relevant key hints
- [ ] Help accessible and comprehensive
- [ ] Workflows logical and predictable

## ✅ Acceptance Criteria Summary

For Milestone 8 completion, verify:

- [ ] **All automated tests pass** (unit + integration)
  - 31 unit tests passing
  - 12 integration tests passing
  
- [ ] **Bash script parity achieved**
  - Same branch detection logic
  - Better safety features (uses -d by default)
  - Better user control (selective deletion)
  
- [ ] **No regressions introduced**
  - All M1-M7 features still working
  - No crashes on normal operations
  - Terminal cleanup on all exit paths
  
- [ ] **Edge cases handled**
  - Non-git directories
  - Empty repositories
  - Various terminal sizes
  - Error conditions
  
- [ ] **Performance acceptable**
  - <1s startup for repos with <50 branches
  - Smooth navigation
  - No UI lag

## 📝 Testing Notes

Use this section to record observations during testing:

### Issues Found
- (Record any bugs or issues discovered)

### Improvement Opportunities
- (Note any UX improvements or enhancements)

### Performance Observations
- (Record startup times, operation speeds)

### Compatibility Notes
- (Note any terminal or OS-specific behavior)

---

**Testing Date**: _______________  
**Tested By**: _______________  
**Environment**: _______________  
**Result**: ☐ Pass  ☐ Fail  ☐ Pass with minor issues
