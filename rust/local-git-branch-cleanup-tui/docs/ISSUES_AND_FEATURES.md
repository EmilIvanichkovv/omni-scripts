# Issues, Bugs & Feature Requests

**Last Updated:** 25-11-2026 18:11

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

### Issue #29: Show errors from external commands in a popup instead of over the screen

- **Status:** 🔴 Open
- **Reported:** 25-11-2026
- **Category:** UI/UX / Bug
- **GitHub:** [#88](https://github.com/EmilIvanichkovv/omni-scripts/issues/88)
- **Description:**
  - When opening a PR in the browser (`o`) fails, the opener's error text is printed straight onto
    the terminal over the TUI, e.g. `/usr/bin/xdg-open: 1050: rundll32.exe: not found` under WSL. It
    stays there, breaking the layout, until something forces a full redraw.
  - Root cause: `git::open_url_in_browser` runs `Command::new("xdg-open").arg(url).spawn()` (and
    `open` / `cmd /C start` on macOS / Windows) without setting stdio, so the child inherits the
    TUI's stdout/stderr and writes into the alternate screen. `spawn` returns as soon as the process
    starts, so its exit status and error output are never checked, and `App::open_selected_pr`
    reports success.
  - There is no error popup to show such a failure in: errors go to the one-line `flash` message in
    the footer, which is cut off for long text and clears on the next key.
- **Steps to Reproduce:**
  1. In WSL without a working browser bridge (no `rundll32.exe` on `PATH`), start the TUI with PR
     integration on.
  2. Put the cursor on a branch with a PR and press `o`.
- **Expected Behavior:**
  - Nothing from a child process is written to the terminal while the TUI runs.
  - A failed open shows an error popup (title, the command that failed, its error output) that the
    user dismisses with `Esc`/`Enter`, and the error is written to the log file (TUI-30).
- **Actual Behavior:** The opener's stderr is drawn over the app, and the TUI behaves as if the PR
  had opened.
- **Proposed Fix:**
  - In `open_url_in_browser`, set `stdin`/`stdout` to `Stdio::null()` and `stderr` to
    `Stdio::piped()`, and wait for the opener on a background thread (`xdg-open` exits once the
    browser is handed the URL). On a non-zero exit, send the trimmed stderr to the app through the
    existing event channel.
  - Add an error modal to `App` (`error_popup: Option<ErrorPopup { title, message }>`), include it
    in `modal_open` so it takes keys and the mouse wheel, and render it like the other modals with
    wrapped, scrollable text. Use it for other user-facing failures that do not fit the footer
    `flash` (failed worktree listing, failed refresh).
  - Audit the other `Command`s: the `git` and `gh` calls use `.output()` (captured) and the
    clipboard nulls its stdio, so `open_url_in_browser` is the only one that inherits the terminal
    today. Add a comment there so new spawns follow the same rule.

---

## Performance Issues

_No open performance issues._

---

## Minor/Cosmetic Issues

### Issue #30: Write a log file for diagnosing errors

- **Status:** 🔴 Open
- **Reported:** 25-11-2026
- **Category:** Minor/Cosmetic / Enhancement
- **GitHub:** [#89](https://github.com/EmilIvanichkovv/omni-scripts/issues/89)
- **Description:**
  - The TUI keeps no log. Errors are shown once in the footer `flash`, the action log panel or
    (before the TUI starts) `eprintln!` in `main.rs`, and are gone after the session, so there is
    nothing to examine after something goes wrong (failed deletes, PR fetch errors, cache problems,
    a failing browser opener).
- **Expected Behavior:**
  - Each run appends to a log file in a fixed, documented folder: `$XDG_STATE_HOME` (or
    `~/.local/state`) `/local-git-branch-cleanup-tui/logs/` on Linux, the platform's local data
    folder on macOS and Windows.
  - Errors and warnings are always logged with a timestamp and context (command, branch, exit code,
    stderr); more detail is available with a log level setting (e.g. `--log-level debug` or an env
    var).
  - Old files are rotated (e.g. daily, keep the last 7) so the folder does not grow forever.
  - The info modal (`i`) and the error popup (TUI-29) show the log file path.
- **Proposed Fix:**
  - Add `tracing`, `tracing-subscriber` and `tracing-appender` (daily rolling, non-blocking writer)
    and initialise them in `main` before the terminal is set up; keep the worker guard alive until
    exit. Never log to stdout/stderr while the TUI owns the screen.
  - Resolve the folder like `cache::cache_root` does: an absolute `$XDG_STATE_HOME` first, then
    `dirs::state_dir()`, falling back to `dirs::data_local_dir()` (macOS/Windows have no state dir).
    If the folder cannot be created, run without a log instead of failing.
  - Log at the existing error sites: `flash` messages, failed `ActionLogEntry`s, PR provider and
    cache errors, refresh/fetch failures, the panic hook, and the opener failure from TUI-29.
  - Document the location and level setting in the README and usage guide.

---

## Resolved Issues

### Issue #28: Show the active filter in the header

- **Status:** 🟢 Resolved
- **Reported:** 25-11-2026
- **Resolved:** 25-11-2026
- **Category:** UI/UX / Enhancement
- **GitHub:** [#85](https://github.com/EmilIvanichkovv/omni-scripts/issues/85)
- **Description:**
  - Nothing on screen said which filter was active unless the filter bar was open. The bar is hidden
    by default (`show_filter: false`, toggled with `F`), but `Tab`, `1`-`4` and `F1`-`F4` change
    `current_filter` (branches) or `WorktreeFilter` (worktrees) with the bar hidden too.
  - `header_lines` in `ui.rs` showed indicators for force, dry run, sort (`🔀`) and layout (`▥`),
    but none for the filter, and the list titles (`Branches (N shown, M selected)`,
    `Worktrees (...)`) only gave a count. A filtered list looked like a repo with fewer branches.
- **Steps to Reproduce:**
  1. Start the TUI with the filter bar hidden (the default).
  2. Press `1` (or `Tab`) to switch to SAFE MERGED.
  3. Look for any sign that the list is filtered.
- **Expected Behavior:** While a filter other than ALL is active, the header shows an indicator with
  its name, like the sort and layout indicators, in both the branch and worktree views. It shows
  whether or not the filter bar is open.
- **Actual Behavior:** With the filter bar hidden, the only change was that fewer rows were listed.
- **Fix:**
  - `header_lines` pushes a `▽ <FILTER>` group (`COLOR_ACCENT`, before the sort indicator) when the
    active filter is not ALL: `app.current_filter` in the branch view, the worktree view's `filter`
    in the worktree view. It shows whether or not the filter bar is open, which keeps the header
    consistent. `▽` replaced the `⏷` proposed earlier: it is a narrow, widely supported glyph like
    the layout indicator's `▥`.
  - Added a render test for both views and the ALL case; documented the indicator in the usage guide
    and added a manual check to `TESTING.md`.
- **Commit:** `feat(tui): Show the active filter in the header`

---

### Issue #27: Move the legend to a full-width box above the footer

- **Status:** 🟢 Resolved
- **Reported:** 25-11-2026
- **Resolved:** 25-11-2026
- **Category:** UI/UX / Enhancement
- **GitHub:** [#84](https://github.com/EmilIvanichkovv/omni-scripts/issues/84)
- **Description:**
  - The legend sat inside the list column, not at the bottom of the screen. `render_branch_list` and
    `render_worktree_list` called `render_legend` on the list pane's `area`, which carved the legend
    box out of the bottom of that area. In side by side it was only as wide as the list (the details
    pane ran past it to the action log/footer), and in stacked it sat between the list and the
    details pane, in the middle of the screen.
  - Because the legend was squeezed into the list width, it wrapped sooner and fell back to
    `… i about` more often than a full-width row would.
- **Steps to Reproduce:**
  1. Start the TUI in a wide terminal (side by side): the legend ends where the details pane begins.
  2. Press `v` until the layout is stacked: the legend appears between the list and the details.
- **Expected Behavior:** The legend is a full-width box at the bottom of the screen, directly above
  the footer (the key-hint box), in every arrangement and in both the branch and worktree views.
- **Actual Behavior:** The legend was part of the list column: narrower than the screen in side by
  side, and above the details pane in stacked.
- **Fix:**
  - `render` in `ui.rs` builds the legend itself (new `legend_lines`, from `branch_legend_groups` or
    `worktree_legend_groups`) on the full inner width and gives it its own chunk between the action
    log and the footer; `render_legend` only draws that box. The list functions no longer touch the
    legend.
  - The space rule moved with it: the legend gets at most 2 lines, then 1, then none, before the
    content area (list and details) would get fewer than `CONTENT_MIN_HEIGHT` (5) rows. Compact mode
    keeps the borderless `Legend:` row.
  - Replaced the list-column legend tests with tests for the full-width position in all three
    arrangements, line flowing, and the space rule; updated the usage guide diagram,
    `RESPONSIVE_LAYOUT.md` and `TESTING.md`.
- **Commit:** `feat(tui): Move the legend to a full-width box above the footer`

---

### Issue #26: Make the action log scrollable

- **Status:** 🟢 Resolved
- **Reported:** 25-11-2026
- **Resolved:** 25-11-2026
- **Category:** UI/UX / Enhancement
- **GitHub:** [#83](https://github.com/EmilIvanichkovv/omni-scripts/issues/83)
- **Description:**
  - The action log panel showed only the newest entries that fit in its box, and nothing could
    scroll back to older ones. `draw` in `ui.rs` capped the panel at 4 rows (1 in compact mode) via
    `log_rows`, and `render_action_log` took `.rev().take(height)` of `app.action_log`, so after a
    bulk delete of more than 4 branches the earlier results (including failures) were unreachable.
  - The mouse wheel did nothing over the panel: `App::scroll_at` only routed wheel events to an open
    modal, the details pane (`details_area`), or the list (`list_area`), and the log area was not
    recorded.
- **Steps to Reproduce:**
  1. Select 6 or more branches and delete them (`Enter`, confirm).
  2. Look at the Action Log panel and try to see the first results (wheel over the panel, keys).
- **Expected Behavior:**
  - The mouse wheel over the action log scrolls through all its entries, newest first.
  - A keyboard way to scroll it exists too, and the panel shows its position (e.g. `3-6 of 12` in
    the title) when it holds more entries than fit.
  - A new action resets the scroll to the newest entry.
- **Actual Behavior:** Only the newest 4 entries (1 in compact mode) were ever visible; older ones
  were hidden with no indication that they existed.
- **Fix:**
  - `App` gained `action_log_scroll`, `action_log_viewport` and `action_log_area`.
    `render_action_log` records the area and viewport, clamps the scroll, skips that many entries
    from the newest, and, when the log overflows, shows `· A-B of N` in the title and a `[/] scroll`
    hint on the bottom border.
  - `scroll_at` routes the wheel over the log to the new `App::scroll_action_log`, and
    `handle_layout_key` binds `]` (older) / `[` (newer) in both views while the log is on screen.
  - Every write to the log goes through the new `App::log_actions`, which scrolls back to the newest
    entry.
  - Listed the keys in both help modals, the README and the usage guide; added a manual check to
    `TESTING.md` and render tests for the range, the hint, the wheel and the reset.
- **Commit:** `feat(tui): Scroll the action log with the mouse wheel and ] / [`

---

### Issue #25: Fetch and prune the remote when the TUI starts

- **Status:** 🟢 Resolved
- **Reported:** 24-11-2026
- **Resolved:** 24-11-2026
- **Category:** UI/UX / Enhancement
- **GitHub:** [#80](https://github.com/EmilIvanichkovv/omni-scripts/issues/80)
- **Description:**
  - The TUI opened on whatever remote-tracking refs the last `git fetch` left behind. Branches whose
    remote was deleted since then were not shown as `gone`, and ahead/behind counts were stale
  - `git fetch --prune` only ran as part of a full sync (`R`, or the automatic sync), and
    `Refresher::set_sync_interval()` deliberately made the first automatic sync wait a whole
    interval (5 minutes by default) because "startup has just read everything". Startup read only
    local refs, so until the user pressed `R` or the first interval passed, the list could be wrong
- **Expected Behavior:**
  - As soon as the TUI opens, `git fetch --prune` runs on the primary remote in the background
    (`git::fetch_remote()`, same timeout and non-interactive setup as `R`), then the branch list
    reloads; the list is usable immediately and the footer shows the fetch progress as for `R`
  - The startup fetch does not re-fetch PR data: startup has just loaded it (from the cache or the
    API), and a fetch does not change which PR belongs to a branch
  - A fetch error is shown in the footer without blocking the TUI
  - A flag (`--no-fetch-on-start`) turns it off, for offline use or slow remotes
- **Fix:**
  - `refresh.rs`: new `RefreshKind::Fetch` (between `Local` and `Full`), run by `run_full()` without
    a PR job: `Refresher::start()` only builds the PR job (and takes the PR cache) for `Full`, so
    the result has `prs_refetched: false` and `App::apply_refresh_result()` keeps the PR info. Only
    `Full` restarts the automatic sync clock, so the first sync is still one interval after startup.
    The footer shows the fetch stages (`⟳ fetching origin…`) as for `R`, and a fetch error flashes
    `Fetch failed`
  - `main.rs`: new `--no-fetch-on-start` flag; `run_tui_mode()` requests a `RefreshKind::Fetch`
    right after setting up the refresher unless it is set. `r`/`R` pressed meanwhile queue behind it
    (`R` still re-fetches PRs). `--cli` mode is unchanged
  - Tests: the startup fetch carries no PR job, leaves the PR cache with the app and does not move
    the sync clock; `R` during the startup fetch queues a full sync. Verified in tmux: a branch
    deleted on the remote before startup turned `gone` right away, and stayed `unmerged` with
    `--no-fetch-on-start`
  - Docs: README (features, flags), usage guide (live refresh), ARCHITECTURE and LIVE_REFRESH specs,
    TESTING (concurrent changes)
- **Commit:** `feat(tui): Fetch and prune the remote when the TUI starts`

---

### Issue #24: Keep the selection when toggling force mode

- **Status:** 🟢 Resolved
- **Reported:** 19-11-2026
- **Resolved:** 19-11-2026
- **Category:** UI/UX / Bug
- **GitHub:** [#70](https://github.com/EmilIvanichkovv/omni-scripts/issues/70)
- **Description:**
  - Pressing `f` threw away the whole selection. In the branch view the key handler in `main.rs`
    called `App::clear_selection()` after flipping `force_mode`; in the worktree view it called
    `App::clear_all_selections()`, which cleared both views
  - Turning force mode on only adds selectable rows, so nothing needed to be dropped. Turning it off
    only makes force-only rows unselectable, so only those needed to go
- **Steps to Reproduce:**
  1. In the branch view, select a few `merged`/`gone` branches with `Space`
  2. Press `f` to turn force mode on
  3. The selection is empty; select the branches again plus the `unmerged` ones
  4. Press `f` to turn force mode off: every branch is deselected again
- **Expected Behavior:**
  - Force mode on: the selection is kept as is
  - Force mode off: only selections that need force are dropped: branches whose
    `status.requires_force()` is true (`unmerged`, `pr-diverged`, `local`), and in the worktree view
    dirty worktrees; everything else stays selected, in both views
- **Actual Behavior:** Both directions cleared the selection (the worktree view cleared both views)
- **Fix:**
  - `app.rs`: new `App::toggle_force_mode()`, used by `f` in both views. Turning force on keeps both
    selections; turning it off keeps only branches that do not `requires_force()` and calls the new
    `WorktreeView::retain_selectable(false)`, which drops worktrees that are no longer selectable
    (dirty ones). `clear_all_selections()`, used only by the old handler, was removed
  - Selected branches are still deleted with `-D` while force mode is on
    (`branch_delete_needs_force()`), now including merged branches kept selected across `f`; the
    `⚠️ FORCE` header badge shows it. The usage guide wrongly said merged/gone branches keep `-d` in
    force mode, and now describes the actual rule
  - Tests: force on keeps the branch selection; force off drops only the unmerged branch; force off
    from the branch view also drops the dirty worktree and keeps the clean one
  - Docs: usage guide (Force Mode section), TESTING (force-mode selection checks)
- **Commit:** `fix(tui): Keep the selection when toggling force mode`

---

### Issue #23: Make the worktree view read-only until removal is switched on

- **Status:** 🟢 Resolved
- **Reported:** 19-11-2026
- **Resolved:** 19-11-2026
- **Category:** UI/UX / Enhancement
- **GitHub:** [#69](https://github.com/EmilIvanichkovv/omni-scripts/issues/69)
- **Description:**
  - The worktree view was used mostly as an explorer, but removal was always armed: every removable
    row showed a checkbox, and `Space`/`a` selected worktrees that Enter then removed after one
    confirmation. A stray `a` + Enter + `y` removed every clean stale worktree
  - Selection was gated only by `WorktreeInfo::is_selectable()` (`removal_action()`); there was no
    mode that turned removal off
- **Expected Behavior:**
  - The worktree view starts read-only on every launch (`--worktrees` or `w`): no checkboxes, and
    `Space`, `a`, `c` and Enter do nothing except a footer hint
    `Removal is off (press x to turn it on)`
  - `x` toggles remove mode in the worktree view. When on, the header shows a `🗑 REMOVE` badge,
    checkboxes and the selection keys work as before, and the footer lists them; turning it off
    clears the worktree selection
  - Browsing keys (navigation, search, filters, `o`, `y`, `w`, help/info) work in both modes
  - The branch view and `--worktrees --cli` are unchanged
- **Fix:**
  - `app.rs`: new `App::worktree_remove_mode` (false on every launch) and
    `toggle_worktree_remove_mode()`, which clears the worktree selection and the "also delete
    branches" toggle when turning it off. `require_worktree_remove_mode()` sets the footer hint
    (`REMOVAL_OFF_HINT`) and returns false while removal is off; `toggle_worktree_at_cursor()`,
    `select_all_stale_worktrees()`, the new `request_worktree_removal()` (Enter) and the `c` key go
    through it, and `remove_selected_worktrees()` also returns early while it is off
  - `main.rs`: `x` toggles the mode in the worktree view; Enter calls `request_worktree_removal()`
  - `ui.rs`: no checkboxes while removal is off (`◆` main and `🔒` locked markers stay); a
    `🗑 REMOVE` header badge while on; the footer shows only browse keys plus `x remove mode` while
    off, and the removal keys too while on; the help modal lists `x` under "Selection (removal is
    off until x)"; the info modal explains the read-only start (one row taller)
  - Tests: selection, select-all and Enter are no-ops with the hint while off; turning the mode on
    allows them and turning it off clears the selection; `remove_selected_worktrees()` does nothing
    while off; the render test checks the read-only screen, then the badge, footer and checkbox
    after `x`
  - Docs: README, usage guide (keys, footer, removal section), ARCHITECTURE, TESTING
- **Commit:** `feat(tui): Make the worktree view read-only until x turns removal on`

---

### Issue #22: Show new files as `N` and group changed files in the worktree details pane

- **Status:** 🟢 Resolved
- **Reported:** 19-11-2026
- **Resolved:** 19-11-2026
- **Category:** UI/UX / Enhancement
- **GitHub:** [#68](https://github.com/EmilIvanichkovv/omni-scripts/issues/68)
- **Description:**
  - The worktree view's Changes column marked untracked files with `?` (`ChangeCounts::compact()`,
    e.g. `3M 1S 2?`), which read like "unknown" next to the letter codes `M` and `S`
  - The details pane listed the first `MAX_CHANGED_PATHS` (5) changed paths as one flat list
    (`ChangeCounts::paths`), so the user could not tell which files were new, modified or staged
- **Expected Behavior:**
  - Untracked files use `N` (new): `3M 1S 2N` in the table, the details pane, the `--cli` table, and
    the `Changes:` legend line (`M modified  S staged  N new`)
  - The details pane groups the changed paths under Modified, Staged and New, in the same order as
    the counts, each with up to 5 paths and a `… and N more` line when there are more
  - A file both staged and modified (`MM`) appears in both groups; empty groups are not shown
- **Fix:**
  - `worktree_view.rs`: `ChangeCounts::paths` became `modified_paths`, `staged_paths` and
    `new_paths`, each capped at `MAX_CHANGED_PATHS` (now per group) and filled by
    `parse_porcelain_status()`; the `untracked` count keeps its name (it is git's term) but
    `compact()` shows it as `N`. New `ChangeCounts::groups()` returns the non-empty groups in
    display order
  - `ui.rs`: the details pane renders a `Modified (n)` / `Staged (n)` / `New (n)` heading per group
    with its paths and a per-group `… and N more`; the table legend and the info modal say `N new`
  - `main.rs`: the `--cli` legend says `N new`
  - Tests: the porcelain parser test checks the per-group paths (including an `MM` file in both
    groups), a new test covers the per-group cap and hidden empty groups, the render test checks
    `1M 1S 1N` and the group order in the details pane, and the `--worktrees --cli` integration test
    expects `1M 1N`
  - Docs: README, usage guide (table mockup, Changes column, details pane), TESTING
- **Commit:** `feat(tui): Show new files as N and group changes in the worktree details`

---

### Issue #21: Copy the branch name under the cursor with `y`

- **Status:** 🟢 Resolved
- **Reported:** 19-11-2026
- **Resolved:** 19-11-2026
- **Category:** UI/UX / Enhancement
- **GitHub:** [#67](https://github.com/EmilIvanichkovv/omni-scripts/issues/67)
- **Description:**
  - The worktree view copied the worktree path with `y` (`App::copy_selected_worktree_path()`), but
    the branch view had no copy key: users had to retype or mouse-select a branch name to use it in
    another command (`git switch`, `git log`, PR search)
- **Expected Behavior:**
  - In the branch view, `y` copies the name of the branch under the cursor (not the checked
    selection) to the clipboard, and the footer confirms it:
    `Copied feature/foo to the clipboard (wl-copy)`, or shows the error when no clipboard tool is
    found
  - Works on every row, including `protected`, `protected-wt` and `current` branches
  - The footer hints, help modal (`?`), README and usage guide list `y` for the branch view
- **Fix:**
  - Moved `copy_to_clipboard()` and `clipboard_commands()` out of `worktree_view.rs` into a new
    `clipboard.rs` module, now shared by both views
  - Added `App::copy_selected_branch_name()`, bound to `y` in the branch view's normal-mode key
    handler (`main.rs`); the confirmation modal's `y` is handled earlier, so it is unaffected. Both
    copy actions go through `App::copy_with()`, which sets the footer message (`flash`) from the
    copy result
  - Footer hints gained `y copy`; the help modal (`?`) lists `y` under Other and its height went
    from 50 to 52 rows so the Quit row still fits; the branch details pane shows
    `Press y to copy the branch name`, right under the `o` (open PR) hint when the branch has a PR
  - Tests: copying the row under the cursor, copying a protected row and reporting a clipboard
    error, and a no-op on an empty list (the copy function is injected, so no real clipboard is
    touched); the details pane shows the `y` hint under the `o` hint, and without PR integration
  - Docs: README and usage guide key tables, ARCHITECTURE (`clipboard.rs`), TESTING (manual check)
- **Commit:** `feat(tui): Copy the branch name under the cursor with y`

---

### Issue #20: Add a worktree view (`--worktrees`)

- **Status:** 🟢 Resolved
- **Reported:** 19-11-2026
- **Resolved:** 19-11-2026
- **Category:** UI/UX / Enhancement
- **GitHub:** [#61](https://github.com/EmilIvanichkovv/omni-scripts/issues/61)
- **Description:**
  - The tool only knew about branches. Users who work with many `git worktree`s had no way to see
    which worktrees existed, what branch each one held, whether it had uncommitted work, or whether
    it was stale (branch merged/gone, directory deleted)
  - There was no worktree code at all: `git.rs` never called `git worktree list`, and `App`/`ui.rs`
    were built around a single branch table (`render_branch_list()` + `render_details_pane()`)
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

- **Actual Behavior:** There was no worktree support; users had to run `git worktree list` and
  `git -C <path> status` by hand for each worktree
- **Fix:**
  - `worktree.rs` (shared with Issue #19) parses `git worktree list --porcelain -z`. The new
    `worktree_view.rs` holds the view: `WorktreeInfo` joins each entry with the branch classifier's
    `BranchInfo` (status, upstream, ahead/behind, last commit, PR) through one helper,
    `worktree_branch_status()`, so states match the branch view. `state()` applies the priority
    current → prunable → locked → dirty → detached → branch status
  - `git -C <path> status --porcelain=v1` runs on a pool of up to 8 threads (with
    `GIT_OPTIONAL_LOCKS=0`); results arrive over a channel that the event loop polls each tick, so
    the Changes column shows `…` until each one lands and the UI never blocks
  - `App` gained `ViewMode { Branches, Worktrees }` and an `Option<WorktreeView>` loaded on first
    use. Toggling swaps the cursor, scroll offset and search query with the other view's, so
    navigation, paging and search code is shared; selections are kept per view. `main.rs` dispatches
    normal-mode keys to `handle_worktree_view_key()` in the worktree view
  - `ui.rs` adds `render_worktree_list()` / `render_worktree_details()` and reuses the header (with
    the `🌳 Worktrees` indicator), search box, filter tabs, action log, footer (worktree key hints)
    and the confirmation modal frame (extracted as `render_confirmation_box()`). Help and info
    modals show worktree keys and states while the view is open
  - Removal: `removal_action()` picks `git worktree remove`, `remove --force` (dirty, force mode
    only) or `git worktree prune` (prunable, run once per batch). Main, current, locked and bare
    worktrees are never selectable, nor are ones whose status has not loaded yet. The `b` toggle in
    the confirmation modal deletes each removed worktree's branch with the branch view's rule, now
    shared as `app::branch_delete_needs_force()`; protected branches are kept, and force-only
    branches (unmerged, local, pr-diverged) are kept unless force mode is on. Dry run logs the
    commands instead
  - Integration with Issue #19: `worktree_branch_status()` reads `worktree_shadowed_status`, so no
    worktree row shows `protected-wt`; the `b` toggle's protection and `-d`/`-D` choice use that
    status too, and each branch is deleted only after its worktree is removed.
    `git::apply_pr_merge_status()` now also upgrades an `unmerged` shadowed status to `pr-merged` /
    `pr-diverged`, so a squash-merged branch held by a worktree is not shown as `unmerged`
  - A branch deleted with its worktree drops its cached PR entry, like a branch deleted from the
    branch view (`execute_removals()` takes the app's `PrCache`)
  - `y` pipes the path to the first clipboard tool that works (`wl-copy`, `xclip`, `xsel`, `pbcopy`,
    `clip`, `clip.exe`) and reports the result in the footer; no new dependency
  - `--worktrees --cli` prints the table; with `--dry-run` it also prints the commands for the clean
    stale worktrees (dirty stale ones too with `--force`)
  - Deviations from the proposal: filter keys follow the branch view's layout (`1` Clean, `2` Dirty,
    `3` Stale, `4` All, tabs in that order); `--worktrees --cli` never removes anything and without
    `--dry-run` only reports how many stale worktrees could be removed; `s` (sort) does nothing in
    the worktree view since the order is fixed (main first, then last activity); the `@author:`
    search syntax applies to branches only
  - Tests: state priority, porcelain parsing, filters/search, selectability, removal command choice,
    branch-deletion planning, dry-run output, row building/sorting, view toggling, a `TestBackend`
    render of the view and its modals, and an integration test for `--worktrees --cli --dry-run`
    with merged, dirty, locked and prunable worktrees
  - README, `TUI_USAGE_GUIDE.md`, `TESTING.md` and `ARCHITECTURE.md` document the view
- **Commit:** `feat(tui): Add a worktree view behind --worktrees`

---

### Issue #19: Protect branches checked out in other worktrees

- **Status:** 🟢 Resolved
- **Reported:** 19-11-2026
- **Resolved:** 19-11-2026
- **Category:** UI/UX / Enhancement
- **GitHub:** [#60](https://github.com/EmilIvanichkovv/omni-scripts/issues/60)
- **Description:**
  - Branches checked out in another worktree (linked or main) were listed like any other branch:
    `classify_branch()` only treated the branch of the current working tree (`current_branch`) as
    undeletable, so a worktree branch got `merged`/`unmerged`/`gone`/`local` and a checkbox
  - Git refuses to delete such a branch even with `-D`
    (`error: cannot delete branch 'x' used by worktree at '<path>'`), so the user could select it,
    confirm, and only learn from the action log that the delete failed
  - Nothing in the list or details pane said which worktree held the branch
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
  - The branch was classified by merge state, could be selected, and deletion failed with a git
    error
- **Fix:**
  - New `worktree.rs` parses `git worktree list --porcelain -z` into `WorktreeEntry` records (path,
    branch, bare/detached/locked/prunable); the worktree view (Issue #20) reuses it
  - Added `BranchStatus::ProtectedWorktree` (label `protected-wt`, icon `⑂`, red like `protected`):
    `is_deletable()` and `requires_force()` return false, and a shared `sort_rank()` puts it right
    after `protected` in both the loader and the Status sort
  - `BranchInfo` gained `worktree` (path of the other worktree holding the branch) and
    `worktree_shadowed_status` (the status it would have without the protection)
  - `git.rs`: `apply_worktree_protection()` runs as a post-pass after `classify_branch()` in
    `get_branches_with_classification()`, so `refresh_branches()` keeps it; it skips the worktree
    the tool runs in (via `WorktreeEntry::is_at()` against the repo toplevel, whose branch stays
    `current`) and bare/detached entries. Prunable worktrees still protect their branch, since git
    still blocks the delete. A failing `git worktree list` only drops the protection
  - `ui.rs`: the details pane shows `Worktree: <name> (<path>)`, a prunable note when the directory
    is missing, and the status the branch would otherwise have; the status explanation reads
    "Checked out in another worktree"; the status column widened from 12 to 15; the legend, info
    modal (`i`) and help modal (`?`, "Not selectable" line) list the status
  - `main.rs`: `--cli` legend lists `⑂ protected-wt`, rows show `(worktree: <name>)`, and the
    branches count as protected
  - Tests for the post-pass (other/main/prunable worktrees, current worktree and bare entries
    skipped), label/icon/deletability, sort rank, and the details pane rendering
  - `README.md`, `TUI_USAGE_GUIDE.md`, `MIGRATION.md`, `TESTING.md`, and `ROADMAP.md` list the new
    status
- **Commit:** `feat(tui): Protect branches checked out in other worktrees`

---

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
| 19-11-2026 | 14:02 | #19   | Resolved: New `protected-wt` status for branches in other worktrees    |
| 19-11-2026 | 14:20 | #20   | Resolved: Added the worktree view (`--worktrees` / `w`)                |
| 19-11-2026 | 16:35 | #21   | Reported: Copy the branch name under the cursor with `y`               |
| 19-11-2026 | 16:45 | #22   | Reported: Show new files as `N` and group changes in the details pane  |
| 19-11-2026 | 16:50 | #23   | Reported: Make the worktree view read-only until removal is on         |
| 19-11-2026 | 17:07 | #24   | Reported: Keep the selection when toggling force mode                  |
| 19-11-2026 | 17:13 | #21   | Resolved: `y` copies the branch name in the branch view                |
| 19-11-2026 | 17:17 | #22   | Resolved: `N` for new files, grouped changes in the details pane       |
| 19-11-2026 | 17:22 | #23   | Resolved: Worktree view read-only until `x` turns removal on           |
| 19-11-2026 | 17:24 | #24   | Resolved: `f` keeps the selection, dropping only force-only rows       |
| 24-11-2026 | 20:06 | #25   | Reported: Fetch and prune the remote when the TUI starts               |
| 24-11-2026 | 20:12 | #25   | Resolved: `git fetch --prune` in the background on startup             |
| 25-11-2026 | 15:40 | #26   | Reported: Make the action log scrollable                               |
| 25-11-2026 | 15:42 | #27   | Reported: Move the legend to a full-width box above the footer         |
| 25-11-2026 | 15:44 | #28   | Reported: Show the active filter in the header                         |
| 25-11-2026 | 17:52 | #29   | Reported: Show external command errors in a popup, not over the screen |
| 25-11-2026 | 17:52 | #30   | Reported: Write a log file for diagnosing errors                       |
| 25-11-2026 | 18:02 | #26   | Resolved: Action log scrolls with the wheel and `]` / `[`              |
| 25-11-2026 | 18:08 | #27   | Resolved: Legend is a full-width box above the footer                  |
| 25-11-2026 | 18:11 | #28   | Resolved: Header shows the active filter (`▽ SAFE MERGED`)             |
