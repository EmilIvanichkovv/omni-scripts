# Issues, Bugs & Feature Requests

**Last Updated:** 19-11-2026 13:55

---

## Overview

This document tracks bugs, issues, and feature requests for the local-git-branch-cleanup-tui
application.

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

### Issue #20: Add a worktree view (`--worktrees`)

- **Status:** 🔴 Open
- **Reported:** 19-11-2026
- **Category:** UI/UX / Enhancement
- **GitHub:** [#61](https://github.com/EmilIvanichkovv/omni-scripts/issues/61)
- **Description:**
  - The tool only knows about branches. Users who work with many `git worktree`s have no way to see
    which worktrees exist, what branch each one holds, whether it has uncommitted work, or whether
    it is stale (branch merged/gone, directory deleted)
  - There is no worktree code at all: `git.rs` never calls `git worktree list`, and `App`/`ui.rs`
    are built around a single branch table (`render_branch_list()` + `render_details_pane()`)
- **Expected Behavior:**
  - **Entry points:** a new `--worktrees` / `-w` flag starts the TUI in the worktree view; `w`
    toggles between the branch view and the worktree view in both modes; `--worktrees --cli` prints
    the worktree table and exits (respects `--dry-run`)
  - **Header:** same header as the branch view, with a `🌳 Worktrees` mode indicator
  - **Worktree table** (left, 70%), one row per worktree, main worktree first, then by last
    activity:

    ```text
      Name                Branch                    State        Changes  Last commit
    ▶ ◆ omni-scripts      main                      ◉ current    clean    2 hours ago
    [ ] qa-tui-session    qa/tui-session-2026-11-19 ✎ dirty      3M 1?    5 minutes ago
    [ ] fix-login         fix/login                 ✓ merged     clean    3 weeks ago
    [ ] spike-cache       feat/cache                ↗ gone       clean    2 months ago
    [ ] review-pr-41      (detached @ 1a2b3c4)      ○ detached   clean    1 month ago
    [ ] old-experiment    feat/old                  ✗ prunable   -        -
     🔒 release-hotfix    release/1.2               🔒 locked    clean    4 days ago
    ```

    - Name = worktree directory name; `◆` marks the main worktree (never removable)
    - Branch = checked-out branch, or `(detached @ <sha>)`
    - State, in priority order: `current` (the worktree the tool runs in), `prunable` (directory
      missing), `locked` (`git worktree lock`), `dirty` (uncommitted or untracked changes), then the
      branch's own status reused from the branch classifier (`merged`, `pr-merged`, `gone`,
      `unmerged`, `local`), or `detached`
    - Changes = compact `git status --porcelain` counts (`M` modified, `S` staged, `?` untracked)
    - PR column when `--github`/`--bitbucket` is on, same as the branch view

  - **Details pane** (right, 30%): name, full path, branch + upstream + ahead/behind, HEAD sha,
    author and subject, change counts with the first few changed paths, lock reason, prunable
    reason, PR info
  - **Actions** (cleanup-focused, all behind the existing confirmation modal and `--dry-run`):
    - `Space` selects, `a` selects all clean stale ones, `c` clears; Enter removes the selected ones
      with `git worktree remove <path>`
    - Dirty worktrees need force mode (`f`) and are removed with `--force`; locked worktrees, the
      main worktree and the current worktree are never selectable
    - Prunable worktrees are removed with `git worktree prune` instead
    - In the confirmation modal, an opt-in toggle (`b`) also deletes each removed worktree's branch,
      using the branch view's existing `-d`/`-D` rules
    - `o` opens the PR in the browser, `y` copies the worktree path to the clipboard (so the user
      can `cd` into it)
  - Search (`/`) filters by name, branch or path; filter tabs: All / Clean / Dirty / Stale (merged,
    gone, prunable)
  - Help (`?`) and info (`i`) modals get a worktree section; README and usage guide document the
    flag

- **Actual Behavior:** No worktree support; users have to run `git worktree list` and
  `git -C <path> status` by hand for each worktree
- **Proposed Fix:**
  - `git.rs` (or a new `worktree.rs`): `list_worktrees()` parses `git worktree list --porcelain -z`
    into
    `WorktreeInfo { name, path, head, branch, is_main, is_current, locked: Option<String>, prunable: Option<String>, changes, branch_status, pr_info }`.
    Shares the parser with Issue #19, so #19 should land first
  - Per-worktree `git -C <path> status --porcelain=v1` is the slow part on large repos: run it
    concurrently (one thread per worktree, like PR fetching) and show `…` in the Changes column
    until it arrives
  - Branch status: reuse `get_branches_with_classification()` output and look up each worktree's
    branch, so states match the branch view exactly
  - `app.rs`: add `ViewMode { Branches, Worktrees }` and worktree list state (cursor, selection,
    scroll, filter) next to the branch state; key handling in `main.rs` dispatches on the view
  - `ui.rs`: `render_worktree_list()` and `render_worktree_details()` alongside the branch versions,
    reusing the header, search box, action log, footer and confirmation modal
  - `--cli` mode: `run_worktree_cli_mode()` prints the table with the same legend style as the
    branch CLI output
  - Scope tradeoff: the list + details pane alone is a useful first step; removal actions can follow
    in a second commit if the issue is split

---

### Issue #19: Protect branches checked out in other worktrees

- **Status:** 🔴 Open
- **Reported:** 19-11-2026
- **Category:** UI/UX / Enhancement
- **GitHub:** [#60](https://github.com/EmilIvanichkovv/omni-scripts/issues/60)
- **Description:**
  - Branches checked out in another worktree (linked or main) are listed like any other branch:
    `classify_branch()` only treats the branch of the current working tree (`current_branch`) as
    undeletable, so a worktree branch gets `merged`/`unmerged`/`gone`/`local` and a checkbox
  - Git refuses to delete such a branch even with `-D`
    (`error: cannot delete branch 'x' used by worktree at '<path>'`), so the user can select it,
    confirm, and only learn from the action log that the delete failed
  - Nothing in the list or details pane says which worktree holds the branch
- **Steps to Reproduce:**
  1. In a repo, run `git worktree add ../wt-feature feature` (any branch other than the current one)
  2. Start the TUI from the main checkout
  3. Find `feature` in the list: it shows a merge status and a selectable checkbox
  4. Select it with Space, press Enter, confirm: the action log shows the `used by worktree` error
- **Expected Behavior:**
  - A branch checked out in another worktree gets a new status `protected-wt`: not selectable and
    not deletable, like `protected`/`current`
  - The details pane (right column) names the worktree that holds it (worktree name plus path), e.g.
    `Worktree: wt-feature (/home/me/code/wt-feature)`
  - The status shows in the legend, the help/info modals, and `--cli` output
- **Actual Behavior:**
  - The branch is classified by merge state, can be selected, and deletion fails with a git error
- **Proposed Fix:**
  - `git.rs`: parse `git worktree list --porcelain` once per load into a map
    `branch -> worktree path` (`branch refs/heads/<name>` lines), skipping the current worktree (it
    is already `Current`) and detached/bare entries
  - Add `BranchStatus::ProtectedWorktree` (label `protected-wt`, icon e.g. `⊞`) and a
    `worktree: Option<PathBuf>` field on `BranchInfo`; `classify_branch()` returns it right after
    the `Current` check; `is_deletable()` returns false for it; sort it next to `Protected`
  - `ui.rs`: details pane shows `Worktree: <name> (<path>)` where the name is the path's last
    component; status explanation "Checked out in another worktree"; widen the status column from 12
    to 15 so `⊞ protected-wt` fits
  - `main.rs`: add the status to the CLI legend and protected count
  - Edge cases: a worktree whose directory was removed without `git worktree prune` still blocks
    deletion in git (`prunable` in porcelain output) and should still be protected, with the details
    pane noting it is prunable

---

## Performance Issues

_No open performance issues._

---

## Minor/Cosmetic Issues

_No open minor/cosmetic issues._

---

## Resolved Issues

### Issue #18: Require a double Esc press to quit

- **Status:** 🟢 Resolved
- **Reported:** 09-10-2026
- **Resolved:** 09-10-2026
- **Category:** UI/UX / Enhancement
- **Description:**
  - In normal mode a single `Esc` with an empty search query called `app.quit()` (the `KeyCode::Esc`
    arm in `main.rs`). `Esc` is also the key for closing modals, clearing the search, and cancelling
    the confirmation, so an extra press meant for a popup closed the whole app.
- **Steps to Reproduce:**
  1. Open a popup (e.g. `?` help) or clear a search with `Esc`
  2. Press `Esc` once more
  3. The app exited immediately
- **Expected Behavior:**
  - The first `Esc` in normal mode (no modal, no search query) only arms quit and shows a hint
  - A second `Esc` within 1s quits; any other key or the timeout disarms it
  - `q` still quits immediately
- **Actual Behavior:** A single `Esc` quit the app
- **Fix:**
  - Added `esc_quit_armed_at: Option<Instant>` and `ESC_QUIT_WINDOW` (1s) to `App`.
    `press_esc_to_quit` arms on the first press and quits on a second press inside the window;
    `expire_esc_quit` runs every loop tick, and any non-Esc key disarms it
  - While armed, the footer shows `Press Esc again to quit` in amber
  - Help modal, footer, `README.md`, `TUI_USAGE_GUIDE.md`, and `TESTING.md` now show `q` / `Esc Esc`
  - Added `test_double_esc_quits` and `test_esc_quit_expires_after_window`
- **Commit:** `feat(tui): Require a double Esc press to quit`

---

### Issue #17: Down arrow does not browse branches after accepting `@author:me`

- **Status:** 🟢 Resolved
- **Reported:** 09-10-2026
- **Resolved:** 09-10-2026
- **Category:** UI/UX / Bug
- **Description:**
  - After accepting the `me` author suggestion (Enter or Tab), the list was filtered correctly but
    Down/Up did not move into the branch list.
  - Root cause: `accept_suggestion` called `update_suggestions` again. For `@author:me` the author
    query `me` still prefix-matched the `"me"` suggestion, so `show_suggestions` stayed true and the
    Down/Up handlers in `main.rs` cycled suggestions instead of leaving search. Single-word author
    names (`@author:Alice`) had the same problem; names with spaces worked only because the added
    quotes stopped them matching.
- **Steps to Reproduce:**
  1. Press `/` to open search
  2. Type `@` and accept `author`, then accept the first suggestion `me` with Enter or Tab
  3. Press Down
- **Expected Behavior:** The dropdown closes after accepting an author, and Down moves into the
  filtered branch list
- **Actual Behavior:** The suggestions dropdown stayed open and Down cycled through it; the branch
  list could not be browsed
- **Fix:**
  - `accept_suggestion` recomputes suggestions only after accepting a command (so `@author:` still
    opens the author list); after accepting an author value it calls `hide_suggestions`
  - Added `test_accept_author_suggestion_closes_dropdown` covering `me` and a single-word name
- **Commit:** `fix(tui): Close autocomplete dropdown after accepting an author`

---

### Issue #16: Search input panics on multi-byte characters

- **Status:** 🟢 Resolved
- **Reported:** 23-08-2026
- **Resolved:** 23-08-2026
- **Category:** Critical / Bug
- **Description:**
  - Typing a multi-byte character in search (e.g. a Cyrillic author name after `@author:`) crashed
    the app: `assertion failed: self.is_char_boundary(idx)` in `search_insert_char`.
    `search_cursor_pos` is a byte index but the cursor moved by ±1 byte, landing mid-character.
- **Steps to Reproduce:**
  1. Press `/` to open search
  2. Type any non-ASCII character (e.g. `Е`), then type another character
  3. The app panicked
- **Expected Behavior:** Search accepts any text; the cursor moves by whole characters
- **Actual Behavior:** Panic at `String::insert` char-boundary assertion
- **Fix:**
  - Cursor movement, insert, and backspace now step by whole characters (`char_indices`/`len_utf8`),
    keeping the cursor on char boundaries
  - `accept_suggestion` repositions the cursor to the end of the replaced query
- **Commit:** `🐛(tui): Keep search cursor on char boundaries with multi-byte input`

---

### Issue #15: Branch pushed without tracking config shows as local

- **Status:** 🟢 Resolved
- **Reported:** 18-08-2026
- **Resolved:** 19-08-2026
- **Category:** Critical / Bug
- **Description:**
  - A branch with a remote counterpart but no upstream tracking config (pushed without `-u`, or
    tracking lost) was misclassified as `○ local` ("never pushed"). The pr-merged/pr-diverged
    upgrade also required the tracking config, so a merged PR could not correct it either.
- **Steps to Reproduce:**
  1. Push a branch with `git push origin <branch>` (no `-u`)
  2. Run the tool — the branch showed `○ local` although `origin/<branch>` exists
- **Expected Behavior:** The branch is classified against its remote counterpart (unmerged,
  pr-merged, or pr-diverged as appropriate)
- **Actual Behavior:** Showed `○ local` with no ahead/behind information
- **Fix:**
  - When `rev-parse <branch>@{u}` fails, fall back to `refs/remotes/origin/<branch>` as the
    effective upstream: the branch is then not local, ahead/behind are computed against it, and a
    merged PR classifies it pr-merged/pr-diverged as usual
- **Commit:** `🐛(tui): Use origin/<branch> as effective upstream when tracking is unset`

---

### Issue #14: pr-merged status lost when the list refreshes after deletion

- **Status:** 🟢 Resolved
- **Reported:** 14-08-2026
- **Resolved:** 15-08-2026
- **Category:** Critical / Bug
- **Description:**
  - After deleting any branch, the branch list re-rendered and the remaining branches marked
    `pr-merged` reverted to `! unmerged` and lost their PR badges.
- **Steps to Reproduce:**
  1. Run with `--github` or `--bitbucket` so some branches show `↑ pr-merged`
  2. Delete any branch
  3. The remaining pr-merged branches showed `! unmerged` with no PR column
- **Expected Behavior:** pr-merged branches keep their status and PR info after a refresh
- **Actual Behavior:** They reverted to `! unmerged` and the PR badges disappeared
- **Fix:**
  - `App::refresh_branches` rebuilt the list from git classification alone, dropping the fetched
    `pr_info`; `apply_pr_merge_status` only ever ran at startup
  - The refresh now carries `pr_info` over by branch name and re-applies `apply_pr_merge_status`
- **Commit:** `🐛(tui): Preserve pr-merged status when refreshing after deletion`

---

### Issue #13: Squash-merged branches with live upstream show as unmerged

- **Status:** 🟢 Resolved
- **Reported:** 13-08-2026
- **Resolved:** 14-08-2026
- **Category:** Critical / Bug
- **Description:**
  - QA found branches whose PR was already merged showing as `! unmerged`. Squash/rebase merges
    produce a new commit on trunk, so git ancestry cannot see the merge; a stale local trunk hides
    normal merges the same way. The branch remained force-gated even though its content is merged.
- **Steps to Reproduce:**
  1. Merge a branch's PR with squash (remote branch kept)
  2. Run the tool with `--github` or `--bitbucket`
  3. The branch shows `! unmerged` although the PR is merged
- **Expected Behavior:** The branch should be recognized as merged and safe to delete
- **Actual Behavior:** Showed `! unmerged`, required force mode
- **Fix:**
  - New `BranchStatus::PrMerged` (label `pr-merged`, icon ↑): applied after PR fetch when the newest
    PR is merged, the upstream still exists, and `ahead == 0`, so `git branch -d` is safe
  - Merged-ancestry check now also considers `origin/<trunk>` (works without a PR provider)
- **Commit:** `✨(tui): Add pr-merged status for squash-merged branches with live upstream`

---

### Issue #12: Cannot edit search query text with left/right arrow keys

- **GitHub:** [#27](https://github.com/EmilIvanichkovv/omni-scripts/issues/27)
- **Status:** 🟢 Resolved
- **Reported:** 23-03-2026
- **Resolved:** 23-03-2026
- **Category:** UI/UX / Bug
- **Description:**
  - When user is in search mode (typing a search query), they should be able to use Left and Right
    arrow keys to move the cursor within the search input text for editing
  - Previously, Left/Right arrow keys were not handled during search input, preventing text editing
- **Expected Behavior:**
  - User presses `/` to activate search
  - User types a search query (e.g., "feature/abc")
  - User can use Left/Right arrow keys to move cursor within the text
  - User can use Backspace/Delete to edit characters
  - Standard text editing experience like in a terminal input field
  - Also support Home/End keys for cursor positioning
- **Actual Behavior (Before Fix):**
  - Left/Right arrow keys didn't work during search input
  - Cannot position cursor within the search query text
  - Had to delete entire query with Backspace and retype if there was a typo
- **Fix:**
  - Added `search_cursor_pos: usize` field to App state to track cursor position in search query
  - Implemented helper methods in `app.rs`:
    - `search_cursor_left()` - move cursor left (if not at start)
    - `search_cursor_right()` - move cursor right (if not at end)
    - `search_cursor_start()` - move cursor to start of query
    - `search_cursor_end()` - move cursor to end of query
    - `search_insert_char(c)` - insert character at cursor position and advance cursor
    - `search_backspace()` - delete character before cursor (respects cursor position)
    - `search_delete()` - delete character at cursor (Delete key)
  - Updated search input handler in `main.rs` to:
    - Handle `KeyCode::Left` to move cursor left
    - Handle `KeyCode::Right` to move cursor right
    - Handle `KeyCode::Home` to move cursor to start
    - Handle `KeyCode::End` to move cursor to end
    - Handle `KeyCode::Delete` to delete char at cursor
    - Use `search_insert_char()` instead of `push()` for character input
    - Use `search_backspace()` instead of `pop()` for backspace
    - Reset `search_cursor_pos` when clearing search
  - Updated search box rendering in `ui.rs` to:
    - Display cursor position (\_) at correct location within text
    - Cursor uses accent color for visibility
    - Supports cursor display within regular text, @author commands, and quoted values
    - Visual cursor feedback shows position clearly for all text segments
    - Cursor character chosen for minimal spacing impact on text rendering
- **Implementation Notes:**
  - Cursor position is character-based (0-indexed), not byte-based (handles multi-byte chars
    correctly)
  - Suggestions update when text is modified (char insertion/deletion)
  - All existing search features (autocomplete, @author filtering) continue to work
  - Cursor resets to 0 when typing new characters after clearing query
- **Testing:**
  - Manually tested cursor movement within search query
  - Tested text insertion and deletion at various cursor positions
  - Verified cursor display at start, middle, and end of text
  - Tested with @author: commands and quoted names
  - Verified suggestions still update correctly when editing text

---

### Issue #11: Show GitHub PR association for branches

- **GitHub:** [#23](https://github.com/EmilIvanichkovv/omni-scripts/issues/23)
- **Status:** 🟢 Resolved
- **Reported:** 13-02-2026
- **Resolved:** 25-02-2026
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
- **Fix:**
  - Added `--github` / `-g` CLI flag to enable GitHub PR integration
  - Added `PrState` enum with variants: `Open`, `Merged`, `Closed`
  - Added `PrInfo` struct with fields: `number`, `state`, `title`, `url`
  - Added `pr_info: Option<PrInfo>` field to `BranchInfo` struct
  - Implemented `get_pr_info_for_branch()` using GitHub CLI (`gh pr list --head <branch> --json`)
  - Implemented `fetch_pr_info_for_branches()` to batch fetch PR info for all branches
  - **Branch list table:**
    - Added PR column showing: 🟢 merged, 🟡 open, 🔴 closed, ⚪ none
    - Only shown when `--github` flag is enabled
  - **Details pane:**
    - Shows PR number, state, and title when branch has associated PR
    - Shows "Press `o` to open in browser" hint
    - Shows "Pull Request: none" when no PR exists
  - **Keyboard shortcut:**
    - `o` key opens PR URL in default browser (Linux: xdg-open, macOS: open, Windows: start)
  - **Legend updated:**
    - Shows PR status icons when GitHub integration enabled
  - **Help modal updated:**
    - Added "GitHub Integration" section with `o` shortcut documentation
  - **Footer updated:**
    - Shows `o pr` hint when GitHub integration is enabled
  - **CLI mode:**
    - Shows PR indicator next to branch status when `--github` flag is enabled
- **Notes:**
  - Requires GitHub CLI (`gh`) to be installed and authenticated
  - Shows warning if `gh` CLI is not available when `--github` flag is used
  - PR fetching happens at startup; may slow startup with many branches

---

### Issue #10: Add filter by branch creator/author

- **GitHub:** [#22](https://github.com/EmilIvanichkovv/omni-scripts/issues/22)
- **Status:** 🟢 Resolved
- **Reported:** 13-02-2026
- **Resolved:** 25-02-2026
- **Category:** UI/UX / Enhancement
- **Description:**
  - Users should be able to filter branches by their creator (the author of the first commit on the
    branch)
  - Useful in team environments to quickly find and manage your own branches
- **Expected Behavior:**
  - Integrate author filtering into existing search functionality using `@author:` prefix
  - User presses `/` to open search, then types `@author:` to initiate author search
  - Search syntax examples:
    - `@author:john` - filter by author name containing "john"
    - `@author:me` - special keyword to filter by current git user
  - Can combine with branch name search: `feature @author:john`
  - Show filtered results with indicator of active author filter
  - Clear filter by removing `@author:` from search or pressing `Esc`
- **Actual Behavior:**
  - No filtering by author/creator available
  - Users must manually scan through all branches
- **Fix:**
  - Added `branch_author` field to `BranchInfo` struct in `git.rs`
  - Fetched branch author via `git log --format=%ct|%an --reverse trunk..branch` (first commit
    author)
  - Added `get_current_git_user()` function to get user from `git config user.name`
  - Added `current_git_user` field to App state for `@author:me` support
  - Implemented `parse_search_query()` method to extract `@author:` prefix from search
  - Updated `filtered_branches()` to filter by author name (case-insensitive partial match)
  - Support combining name search with author filter: `feature @author:john`
  - **Quoted author names:** Support `@author:"John Doe"` for names with spaces
  - **Autocomplete suggestions:**
    - Typing `@` shows available commands (currently: `author`)
    - Typing `@author:` shows list of unique branch authors + `me` keyword
    - Use `Tab` or `Enter` to accept suggestion, `↑/↓` to navigate
    - Collects unique authors from all branches at startup
    - **Auto-quoting:** Author names with spaces are automatically wrapped in quotes
  - **Scrollable dropdown:**
    - Dropdown adapts to available screen space
    - Shows `↑ X more above` and `↓ X more below` indicators when scrolling
    - Auto-scrolls to keep selected item visible
  - Added help modal documentation for `@author:` syntax
  - Added unit tests for author filtering, quoted names, and autocomplete functionality

---

### Issue #9: Add ability to sort branches by creation date

- **GitHub:** [#21](https://github.com/EmilIvanichkovv/omni-scripts/issues/21)
- **Status:** 🟢 Resolved
- **Reported:** 13-02-2026
- **Resolved:** 25-02-2026
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
- **Fix:**
  - Added `SortMode` enum to `app.rs` with 6 variants:
    - `Status` (default) - sort by branch status type
    - `Name` - alphabetical by branch name
    - `ActivityNewest` / `ActivityOldest` - sort by last commit date (Active ↓/↑)
    - `CreatedNewest` / `CreatedOldest` - sort by branch creation date (Created ↓/↑)
  - Added `last_activity_timestamp` field (last commit on branch)
  - Added `branch_created_timestamp` field (first unique commit, fetched via
    `git log --format=%ct --reverse trunk..branch`)
  - Added `sort_mode` field to App state
  - Added `cycle_sort_mode()` and `sort_branches()` methods to App
  - Added `s` key handler to cycle through sort modes
  - Footer shows `s sort` shortcut (no highlight on activation)
  - Header shows sort indicator (🔀) with current mode when not using default
  - Help modal updated with sorting section

---

### Issue #8: Missing keyboard shortcuts for list navigation

- **GitHub:** [#20](https://github.com/EmilIvanichkovv/omni-scripts/issues/20)
- **Status:** 🟢 Resolved
- **Reported:** 13-02-2026
- **Resolved:** 13-02-2026
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
- **Fix:**
  - Added `go_to_top()`, `go_to_bottom()`, `page_up()`, `page_down()` methods to App
  - Added key handlers for Home/g, End/G, Ctrl+U, Ctrl+D, PageUp, PageDown
  - All navigation methods properly adjust scroll_offset

---

### Issue #7: Scrolling behavior causes unnecessary viewport movement

- **GitHub:** [#19](https://github.com/EmilIvanichkovv/omni-scripts/issues/19)
- **Status:** 🟢 Resolved
- **Reported:** 13-02-2026
- **Resolved:** 13-02-2026
- **Category:** UI/UX / Bug
- **Description:**
  - When navigating up from the bottom of the visible area, the viewport scrolls/re-renders even
    when there's room for the cursor to move within the current view
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
  - The viewport scrolls/re-renders immediately when moving up, even when cursor is not at the top
    of visible area
- **Fix:**
  - Added `scroll_offset` and `visible_height` fields to App state
  - Implemented `adjust_scroll_for_selection()` method for "edge-only" scrolling
  - Viewport only adjusts when cursor would move outside visible bounds
  - Used `TableState::with_offset()` to control scroll position manually

---

### Issue #6: Improve search focus behavior

- **GitHub:** [#16](https://github.com/EmilIvanichkovv/omni-scripts/issues/16)
- **Status:** 🟢 Resolved
- **Reported:** 13-02-2026
- **Resolved:** 13-02-2026
- **Category:** UI/UX / Enhancement
- **Description:**
  - Improve the search workflow to allow seamless switching between search input and branch
    selection
- **Expected Behavior:**
  1. User presses `/` to activate search and types a query
  2. User presses `↓` (arrow down) to move focus to the branch list (search input loses focus but
     query remains)
  3. User can navigate and select branches with arrow keys and Space
  4. User presses `/` again to return focus to search bar and continue editing the query
- **Fix:**
  - Arrow down/up in search mode now exits search but keeps query
  - Pressing `/` re-enters search mode to continue editing
  - Esc clears query and exits search
- **Commit:** `✨(rust/local-git-branch-cleanup-tui): Improve search focus behavior`

---

### Issue #5: Confirmation modal Y/N not visible with many branches selected

- **GitHub:** [#15](https://github.com/EmilIvanichkovv/omni-scripts/issues/15)
- **Status:** 🟢 Resolved
- **Reported:** 13-02-2026
- **Resolved:** 13-02-2026
- **Category:** UI/UX / Bug + Enhancement
- **Description:**
  - When many branches are selected for deletion, the confirmation prompt (Y/N) is cut off and not
    visible in the modal
  - Additionally, users should be able to confirm with Enter and cancel with Esc (not just y/n)
- **Fix:**
  - Dynamic modal height based on content (branches + warnings)
  - Added Enter key to confirm deletion (in addition to y/Y)
  - Esc already supported for cancel
  - Confirmation hints now centered: `y/Enter confirm    n/Esc cancel`
- **Commit:** `✨(rust/local-git-branch-cleanup-tui): Improve confirmation modal UX`

---

### Issue #4: Add info modal (shortcut: i)

- **GitHub:** [#14](https://github.com/EmilIvanichkovv/omni-scripts/issues/14)
- **Status:** 🟢 Resolved
- **Reported:** 13-02-2026
- **Resolved:** 13-02-2026
- **Category:** UI/UX / Enhancement
- **Description:**
  - Add an info modal accessible via the 'i' shortcut that provides users with brief information
    about what the tool does
- **Expected Behavior:** Pressing 'i' opens a modal with a brief description of the tool's purpose
  and functionality
- **Fix:**
  - Added `show_info` state to App
  - Added 'i' key handler in main.rs
  - Created `render_info_modal` function in ui.rs with tool description and branch status
    explanations
  - Added `i info` hint in footer
- **Commit:** `✨(rust/local-git-branch-cleanup-tui): Add info modal with tool description`

---

### Issue #3: Highlight active mode options in footer

- **GitHub:** [#13](https://github.com/EmilIvanichkovv/omni-scripts/issues/13)
- **Status:** 🟢 Resolved
- **Reported:** 13-02-2026
- **Resolved:** 13-02-2026
- **Category:** UI/UX / Enhancement
- **Description:**
  - When user toggles force mode (f) or dry run mode (d), the selected option should be visually
    highlighted in the footer
- **Expected Behavior:** Active mode options should be highlighted/styled differently in the footer
  to provide clear visual feedback
- **Fix:**
  - When active, `f force` gets black text on red background
  - When active, `d dry` gets black text on amber background
  - Entire label is highlighted, not just the key
- **Commit:** `✨(rust/local-git-branch-cleanup-tui): Highlight active mode options in footer`

---

### Issue #2: Branch list scrolling not working with many branches

- **GitHub:** [#12](https://github.com/EmilIvanichkovv/omni-scripts/issues/12)
- **Status:** 🟢 Resolved
- **Reported:** 13-02-2026
- **Resolved:** 13-02-2026
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
  - `TableState::default().with_selected(Some(app.selected_index))` enables automatic scrolling to
    keep cursor visible
- **Commit:** `🐛(rust/local-git-branch-cleanup-tui): Fix branch list scrolling with TableState`

---

### Issue #1: Force delete mode not using `git branch -D`

- **GitHub:** [#11](https://github.com/EmilIvanichkovv/omni-scripts/issues/11)
- **Status:** 🟢 Resolved
- **Reported:** 13-02-2026
- **Resolved:** 13-02-2026
- **Category:** Critical / Bug
- **Description:**
  - When attempting to delete a merged branch (marked as "gone") with force mode enabled, the
    deletion fails with an error suggesting the branch is not fully merged.
- **Steps to Reproduce:**
  1. Have a branch marked as "gone" (remote deleted)
  2. Enable force delete mode
  3. Attempt to delete the branch
- **Expected Behavior:** Branch should be force deleted using `git branch -D`
- **Actual Behavior:**
  - Shows error:
    `✗ feat/TUI - Failed to delete branch: error: the branch 'feat/TUI' is not fully merged`
  - Hint suggests running `git branch -D feat/TUI`
- **Fix:**
  - In `app.rs`, `delete_selected_branches()` now respects `self.force_mode`
  - Also auto-forces deletion for "gone" branches (handles squash/rebase merges)
  - Changed:
    `let use_force = self.force_mode || *status == BranchStatus::Unmerged || *status == BranchStatus::GoneUpstream;`
- **Commit:**
  `🐛(rust/local-git-branch-cleanup-tui): Fix force delete mode to properly use git branch -D`

---

## Change Log

| Date       | Time  | Issue | Change                                                                 |
| ---------- | ----- | ----- | ---------------------------------------------------------------------- |
| 13-02-2026 | 14:30 | -     | Document created                                                       |
| 13-02-2026 | 14:45 | #1    | Reported: Force delete mode not using `git branch -D`                  |
| 13-02-2026 | 15:00 | #1    | Resolved: Fixed force delete logic in `app.rs`                         |
| 13-02-2026 | 15:15 | #2    | Reported: Branch list scrolling not working                            |
| 13-02-2026 | 15:30 | #2    | Resolved: Implemented `TableState` for proper scrolling                |
| 13-02-2026 | 15:45 | #3    | Reported: No visual feedback for active modes                          |
| 13-02-2026 | 16:00 | #3    | Resolved: Added background highlighting for force/dry modes            |
| 13-02-2026 | 16:15 | #4    | Reported: Missing info modal                                           |
| 13-02-2026 | 16:30 | #4    | Resolved: Added info modal with 'i' shortcut                           |
| 13-02-2026 | 16:45 | #5    | Reported: Confirmation modal issues                                    |
| 13-02-2026 | 17:00 | #5    | Resolved: Dynamic modal height, Enter/Esc support, centered hints      |
| 13-02-2026 | 17:15 | #6    | Reported: Search focus behavior needs improvement                      |
| 13-02-2026 | 17:50 | #6    | Resolved: Arrow keys exit search but keep query, `/` re-enters search  |
| 13-02-2026 | 18:30 | #7    | Reported: Scrolling causes unnecessary viewport movement               |
| 13-02-2026 | 18:35 | #8    | Reported: Missing keyboard shortcuts (Home/End/g/G/PgUp/PgDn/Ctrl+U/D) |
| 13-02-2026 | 18:40 | #9    | Reported: Add ability to sort branches by creation date                |
| 13-02-2026 | 18:45 | #10   | Reported: Add filter by branch creator/author                          |
| 13-02-2026 | 18:50 | #11   | Reported: Show GitHub PR association for branches                      |
| 13-02-2026 | 19:10 | #7    | Resolved: Implemented edge-only scrolling with scroll_offset           |
| 13-02-2026 | 21:00 | #8    | Resolved: Added keyboard shortcuts (Home/End/g/G/PgUp/PgDn/Ctrl+U/D)   |
| 25-02-2026 | 13:15 | #9    | Resolved: Added sort by date with 's' key (Status/Name/Newest/Oldest)  |
| 25-02-2026 | 14:00 | #10   | Resolved: Added @author: filter with autocomplete suggestions          |
| 25-02-2026 | 14:30 | #11   | Resolved: Added GitHub PR integration with --github flag               |
| 25-02-2026 | 16:05 | -     | Migrated resolved issues #7-#11 to Resolved Issues section             |
| 23-03-2026 | 17:40 | #12   | Reported: Cannot edit search query with left/right arrow keys          |
| 23-03-2026 | 18:30 | #12   | Resolved: Added cursor position tracking and text editing support      |
| 09-10-2026 | 12:58 | #17   | Reported: Down arrow does not browse branches after `@author:me`       |
| 09-10-2026 | 13:06 | #18   | Reported: Require a double Esc press to quit                           |
| 09-10-2026 | 13:35 | #17   | Resolved: Close autocomplete dropdown after accepting an author        |
| 09-10-2026 | 13:36 | #18   | Resolved: Esc must be pressed twice within 1s to quit                  |
| 19-11-2026 | 13:40 | #19   | Reported: Protect branches checked out in other worktrees              |
| 19-11-2026 | 13:55 | #20   | Reported: Add a worktree view (`--worktrees`)                          |
