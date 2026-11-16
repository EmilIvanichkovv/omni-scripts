# TUI Usage Guide

This guide covers how to use the interactive terminal interface for Local Git Branch Cleanup.

## Interface Overview

The TUI is divided into several sections:

```
┌─────────────────────────────────────────────────────────────────────────┐
│ Header: Repository path, trunk branch, selection count, mode indicators │
├─────────────────────────────────────────────────────────────────────────┤
│ Filter Tabs: [Safe Merged] [Upstream Gone] [Unmerged] [All]             │
├───────────────────────────────────────────┬─────────────────────────────┤
│                                           │                             │
│            Branch List (70%)              │    Details Pane (30%)       │
│   (PR column with --github/--bitbucket)   │    (includes PR info)       │
│                                           │                             │
├───────────────────────────────────────────┴─────────────────────────────┤
│ Action Log: Deletion results (appears after deletions)                  │
├─────────────────────────────────────────────────────────────────────────┤
│ Footer: Status legend and keyboard shortcuts                            │
└──────────────────────────────── Update available: X.Y.Z (when there is) ┘
```

---

## Keyboard Controls

### Navigation

| Key               | Action                                         |
| ----------------- | ---------------------------------------------- |
| `↑` / `k`         | Move cursor up                                 |
| `↓` / `j`         | Move cursor down                               |
| `g` / `Home`      | Go to first item                               |
| `G` / `End`       | Go to last item                                |
| `Ctrl+U`          | Go to first item                               |
| `Ctrl+D`          | Go to last item                                |
| `Page Up`         | Move up by one page                            |
| `Page Down`       | Move down by one page                          |
| `q` / `Esc` `Esc` | Quit application (press `Esc` twice within 1s) |

### Filtering Branches

| Key        | Action                                                       |
| ---------- | ------------------------------------------------------------ |
| `1` / `F1` | Show only safe merged branches (includes pr-merged)          |
| `2` / `F2` | Show only upstream gone branches                             |
| `3` / `F3` | Show only unmerged branches (includes pr-diverged and local) |
| `4` / `F4` | Show all branches                                            |
| `Tab`      | Cycle through filters                                        |

The active filter tab is highlighted in cyan, and each tab displays the branch count for that
category.

### Search

| Key             | Action                                                   |
| --------------- | -------------------------------------------------------- |
| `/`             | Start search mode                                        |
| `Tab` / `Enter` | Accept autocomplete suggestion                           |
| `↑` / `↓`       | Navigate autocomplete suggestions                        |
| `Esc`           | Exit search and clear query                              |
| `Enter`         | Exit search and keep filter active (when no suggestions) |

**Search Syntax:**

| Syntax                 | Description                                  |
| ---------------------- | -------------------------------------------- |
| `feature`              | Filter branches containing "feature" in name |
| `@author:john`         | Filter by branch author containing "john"    |
| `@author:me`           | Filter by your branches (current git user)   |
| `@author:"John Doe"`   | Filter by author with spaces (use quotes)    |
| `feature @author:john` | Combine name and author filters              |

**Autocomplete:**

When you type `@`, the search box shows available commands:

- `@author:` - Filter by branch author

After typing `@author:`, you'll see suggestions for:

- `me` - Your branches (based on git config user.name)
- All unique branch authors in the repository

**Autocomplete Features:**

- Use `Tab` or `Enter` to accept a suggestion
- Use `↑`/`↓` to navigate through suggestions
- **Scrollable dropdown** - When many authors exist, the dropdown shows scroll indicators
  (`↑ X more above`, `↓ X more below`)
- **Auto-quoting** - Author names with spaces are automatically wrapped in quotes when selected
- Accepting an author closes the dropdown, so `↑`/`↓` move straight into the filtered list

When search is active, the search box appears below the filters. The search is case-insensitive.

### Sorting Branches

| Key | Action                   |
| --- | ------------------------ |
| `s` | Cycle through sort modes |

**Available Sort Modes:**

| Mode       | Label     | Description                                                                                                       |
| ---------- | --------- | ----------------------------------------------------------------------------------------------------------------- |
| Status     | (default) | Groups by branch status (current, protected, protected-wt, merged, pr-merged, pr-diverged, gone, unmerged, local) |
| Name       | Name      | Alphabetical by branch name (case-insensitive)                                                                    |
| Activity ↓ | Active ↓  | Most recently active branches first (by last commit)                                                              |
| Activity ↑ | Active ↑  | Least recently active branches first                                                                              |
| Created ↓  | Created ↓ | Most recently created branches first                                                                              |
| Created ↑  | Created ↑ | Oldest branches first (by creation date)                                                                          |

When a non-default sort mode is active, the header displays a sort indicator: `🔀 Active ↓`

### Selection

| Key     | Action                              |
| ------- | ----------------------------------- |
| `Space` | Toggle selection for current branch |
| `a`     | Select/deselect all safe branches   |
| `c`     | Clear all selections                |

### Actions

| Key     | Action                                           |
| ------- | ------------------------------------------------ |
| `Enter` | Delete selected branches (opens confirmation)    |
| `f`     | Toggle force mode                                |
| `d`     | Toggle dry run mode                              |
| `s`     | Cycle sort mode                                  |
| `o`     | Open PR in browser (when PR integration enabled) |
| `w`     | Switch to the worktree view                      |
| `y`     | Copy the branch name to the clipboard            |
| `r`     | Reload the branch list (see Live Refresh)        |
| `R`     | Fetch the remote, reload, re-fetch PR data       |
| `i`     | Show info modal (about the tool)                 |
| `?`     | Show help modal                                  |

---

## PR Integration (GitHub & Bitbucket)

When started with the `--github` or `--bitbucket` flag, the TUI shows Pull Request information for
each branch. The PR column, the details pane, and the `o` (open in browser) shortcut behave the same
for both providers. The two flags are mutually exclusive.

### GitHub Requirements

- **GitHub CLI (`gh`)** must be installed and authenticated
- Repository must have a GitHub remote

If `gh` is not installed, the tool prints a warning and continues without PR integration.

### Enabling GitHub Integration

```bash
# Start with GitHub PR integration
local-git-branch-cleanup-tui --github

# Or with cargo
cargo run -- --github
```

### Bitbucket Requirements

- A **Bitbucket Data Center** instance (Bitbucket Cloud / `bitbucket.org` is not supported)
- A Bitbucket **HTTP access token** with repository read permission, exported as `BITBUCKET_TOKEN`
- Repository must have an `origin` remote pointing at the Bitbucket instance (or use the explicit
  overrides below)

### Enabling Bitbucket Integration

```bash
# Prefer a hidden prompt so the token never lands in your shell history
read -rs BITBUCKET_TOKEN && export BITBUCKET_TOKEN

# Start with Bitbucket PR integration
local-git-branch-cleanup-tui --bitbucket
```

The Bitbucket base URL, project key, and repository slug are derived from the `origin` remote. When
derivation fails (nonstandard remote, context path, custom HTTP port), supply explicit overrides:

```bash
local-git-branch-cleanup-tui --bitbucket \
  --bitbucket-base-url 'https://bitbucket.example.com' \
  --bitbucket-project 'PROJ' \
  --bitbucket-repo 'my-repo'
```

Each override also has an environment-variable fallback (`BITBUCKET_BASE_URL`, `BITBUCKET_PROJECT`,
`BITBUCKET_REPO`). Precedence: CLI flag > environment variable > value derived from `origin`.

**Token guidance:**

- The token is read from the environment only — it is never accepted as a CLI argument and is never
  written to the cache, logs, or error messages.
- Bitbucket configuration, authentication, or repository-access failures are fatal and reported
  before the TUI opens.

### PR Status Icons

| Icon | Status | Description                   |
| ---- | ------ | ----------------------------- |
| 🟢   | Merged | PR was merged                 |
| 🟡   | Open   | PR is still open              |
| 🔴   | Closed | PR was closed without merging |
| ⚪   | None   | No associated PR              |

### PR Column

When enabled, an additional "PR" column appears in the branch list showing:

- PR state icon (🟢/🟡/🔴/⚪)
- PR number (e.g., `#165`)

### Details Pane

The details pane shows additional PR information:

- **PR Title** - The pull request title
- **PR Number** - The PR number (e.g., #165)
- **PR State** - Open, Merged, or Closed
- **Hint** - Press `o` to open in browser

### Opening PRs in Browser

Press `o` when a branch with an associated PR is selected to open the PR URL in your default
browser.

### Cache Behaviour

PR data is cached locally in a SQLite database for **1 hour** by default. The database is at
`$XDG_CACHE_HOME/omni-scripts/pr-cache.db` when `XDG_CACHE_HOME` is set (on every platform),
otherwise in the platform cache directory:

- **Linux:** `~/.cache/omni-scripts/pr-cache.db`
- **macOS:** `~/Library/Caches/omni-scripts/pr-cache.db`
- **Windows:** `%LOCALAPPDATA%\omni-scripts\pr-cache.db`

To print the resolved path, run:
`just cache-db --bail 2>/dev/null || echo "$XDG_CACHE_HOME/omni-scripts/pr-cache.db"` This means:

- **Second run within 1h**: instant — zero provider calls (`gh` subprocesses or Bitbucket API
  requests)
- **New PR opened**: will appear after the cache entry expires (up to 1h)
- **PR closed or merged**: old state shown until cache expires

GitHub and Bitbucket entries never collide: Bitbucket entries are namespaced by provider, host,
project, and repository, while GitHub keeps its `owner/repo` key. Failed lookups are never cached —
only successful results (including "no PR") are stored.

At startup the cache reports how many branches were served from cache vs fetched live:

```
🔗 Fetching GitHub PR info...
   42 from cache, 3 fetched, 0 failed
```

**Inspecting the cache** (from the project dev shell):

```bash
just cache-show    # pretty-print all cached entries
just cache-db      # open interactive sqlite3 REPL
just cache-clear   # delete all cached rows (forces fresh fetch on next run)
```

The same directory holds `local-git-branch-cleanup-tui-update-check.json`, the result of the daily
"update available" check (see the README, section Updating): when it last ran and the newest version
it found. Delete it to check again on the next run.

> **Note:** PR integration requires additional API calls at startup for cache misses, which may slow
> down the initial load with many branches. Subsequent runs within the TTL are instant.

## Understanding Branch Status

Each branch displays a status icon indicating its state:

| Icon | Status       | Description                                           | Deletable?             |
| ---- | ------------ | ----------------------------------------------------- | ---------------------- |
| ✓    | Merged       | Fully merged into trunk                               | ✅ Safe (`-d`)         |
| ↑    | PR-Merged    | PR merged (squash/rebase), remote branch still exists | ✅ Safe (`-d`)         |
| ↕   | PR-Diverged  | PR merged, local has commits the remote doesn't       | ⚠️ Requires force mode |
| ↗   | Gone         | Remote tracking branch was deleted                    | ✅ Safe (`-d`)         |
| !    | Unmerged     | Has commits not in trunk                              | ⚠️ Requires force mode |
| ○    | Local        | Never pushed to the remote; exists only locally       | ⚠️ Requires force mode |
| ⊘    | Protected    | main/master/develop branches                          | ❌ Never               |
| ⑂    | Protected-WT | Checked out in another worktree (main or linked)      | ❌ Never               |
| ◉    | Current      | Currently checked out branch                          | ❌ Never               |

---

## Understanding Checkboxes

The checkbox state indicates whether a branch can be selected:

| Checkbox      | Meaning                                                                   |
| ------------- | ------------------------------------------------------------------------- |
| `[✓]`         | Selected for deletion                                                     |
| `[ ]`         | Not selected (can be toggled with `Space`)                                |
| `-`           | Disabled (unmerged/pr-diverged/local branch, enable force mode to select) |
| _No checkbox_ | Protected or current branch (cannot be deleted)                           |

---

## Selecting Branches

### Individual Selection

1. Navigate to the branch using `↑`/`↓` or `j`/`k`
2. Press `Space` to toggle selection
3. The checkbox changes from `[ ]` to `[✓]`
4. Header updates to show selection count: "📦 X selected"

### Bulk Selection

- Press `a` to select **all safe branches** (merged, pr-merged, and gone)
- Press `a` again to deselect all
- Press `c` to clear all selections

> **Note:** Bulk selection only affects branches that are currently selectable based on force mode.

---

## Force Mode

By default, unmerged, pr-diverged, and local branches cannot be selected. This protects you from
accidentally deleting work that hasn't been merged or pushed.

### Enabling Force Mode

1. Press `f` to toggle force mode
2. Header displays "⚠️ FORCE" indicator
3. Unmerged, pr-diverged, and local branches now show `[ ]` instead of `-`
4. You can now select them

Toggling force mode keeps your selection. Turning it on keeps everything selected; turning it off
deselects only what needs force (unmerged, pr-diverged and local branches, and dirty worktrees in
the worktree view), in both views.

### Force Mode Behavior

- While force mode is on, every selected branch is deleted with `git branch -D` (force delete),
  including merged ones selected before `f` was pressed
- Without force mode, merged branches use the safe `git branch -d`; gone branches always use `-D`
  (`-d` refuses squash/rebase merges)

> **⚠️ Warning:** Force mode allows deletion of branches with unmerged commits. These commits may be
> lost if not pushed or merged elsewhere. Use with caution!

---

## Dry Run Mode

Preview deletions without actually executing them.

### Using Dry Run

1. Press `d` to toggle dry run mode
2. Header displays "🔍 DRY RUN" indicator
3. Select branches as normal
4. Press `Enter` to preview
5. Confirmation modal shows "Preview (Dry Run)" title
6. Press `y` to see preview results
7. Action log shows "[DRY RUN] Would delete: branch-name"

No branches are actually deleted in dry run mode. This is useful for:

- Testing your selection before committing
- Understanding what the tool will do
- Verifying filter behavior

---

## Deleting Branches

### Step-by-Step

1. **Select branches** using `Space` (individual) or `a` (bulk)
2. **Review selection** in the header ("📦 X selected")
3. **Press `Enter`** to open confirmation modal
4. **Review the modal** which shows:
   - Number of branches to delete
   - Branch names (up to 3, then "and X more")
   - Warning if any branches are unmerged
   - Delete command that will be used (`-d` or `-D`)
5. **Press `y`** to confirm, or `n` to cancel

### After Deletion

- Action log appears showing results
- ✓ indicates successful deletion
- ✗ indicates failure with error message
- Branch list automatically refreshes

---

## Live Refresh

The branch list follows the repository while the TUI is open.

- **Automatically:** every 2 s the TUI checks the branch and remote-tracking refs, the checked-out
  branch and the worktree list (3 cheap git calls). When something changed in another terminal (a
  branch created, deleted, committed to, merged or checked out), it re-reads the branches in the
  background. Switching back to the terminal window checks at once in terminals that report focus.
  `--no-auto-refresh` turns this off.
- **`r`:** reload the branch list now.
- **`R`:** full sync: `git fetch --prune` on the primary remote, reload, and with `--github` /
  `--bitbucket` re-fetch PR data for every branch (skipping the cache, then updating it). This is
  the only way the TUI touches the network after startup; it is never automatic.

What stays put across a reload: the selection (by branch name; a branch that is gone or no longer
selectable leaves it), the cursor (same branch, or the same row if that branch is gone), search,
filter tab, sort mode, and in the worktree view its selection and the statuses already read.

While the delete confirmation is open, nothing is applied: a finished reload waits until the modal
closes, so the list never changes under a pending delete.

The bottom-left corner of the footer shows `⟳ reloading…`, or the steps of a full sync
(`⟳ fetching origin…`, `⟳ reloading…`, `⟳ fetching PRs…`), then `updated HH:MM:SS` for 5 s.
Automatic checks that find nothing new show nothing.

`R` never prompts for credentials: prompts would draw over the TUI. HTTPS prompts, Git Credential
Manager dialogs (also the Windows one under WSL) and SSH passphrase or host-key questions are turned
off, so a remote that needs them fails with
`Fetch failed: authentication required (run git fetch in a terminal)`; stored credentials and
ssh-agent keep working. A fetch running for more than 60 s is stopped. The reload still runs on
local data when the fetch fails.

---

## Details Pane

The right panel (30% of screen) shows detailed information about the highlighted branch:

- **Branch name** — Full branch name
- **Status** — Explanation (e.g., "Merged into main")
- **Worktree** — For `protected-wt` branches: the worktree holding the branch as `<name> (<path>)`,
  a note when its directory is missing (prunable), and the status the branch would otherwise have
- **Upstream** — Remote tracking branch, or "None"
- **Ahead/Behind** — Commit differences with upstream
- **Last Commit** — SHA, author, and message

The details pane updates as you navigate through the branch list.

---

## Help Modal

Press `?` at any time to display a comprehensive help modal with all keyboard shortcuts organized by
category:

- **Navigation** — Arrow keys, j/k
- **Filters** — 1-4, F1-F4, Tab
- **Selection** — Space, a (all), c (clear), and which statuses are not selectable (protected,
  protected-wt, current)
- **Actions** — Enter (delete), f (force), d (dry run)
- **Other** — w (worktree view), i (info), ? (help), q (quit)

In the worktree view the help modal lists that view's keys instead (filters, `y` copy path, `b` in
the confirmation, `w` back to branches).

Press any key to close the help modal.

---

## Info Modal

Press `i` at any time to display information about the tool:

- Tool name and description
- Explanation of all branch status types (merged, pr-merged, pr-diverged, gone, unmerged, local,
  protected, protected-wt, current)
- Hint to press `?` for keyboard shortcuts

In the worktree view it explains the worktree states and the Changes column instead.

Press any key to close the info modal.

---

## Worktree View

Start with `--worktrees` (`-w`), or press `w` in the branch view; `w` switches back. Each view keeps
its own cursor, search query and selection. The header shows `🌳 Worktrees` while the view is open.

```
  Name              Branch                     State        Changes  Last Commit
▶ ◆ omni-scripts    main                       ◉ current    clean    2 hours ago
 [ ] qa-session     qa/tui-session-2026-11-19  ✎ dirty      3M 1N    5 minutes ago
 [ ] fix-login      fix/login                  ✓ merged     clean    3 weeks ago
 [ ] review-pr-41   (detached @ 1a2b3c4)       ○ detached   clean    1 month ago
 [ ] old-experiment feat/old                   ✗ prunable   -        -
  🔒 release-hotfix  release/1.2                🔒 locked    clean    4 days ago
```

- **Name** — the worktree directory name; `◆` marks the main worktree
- **Branch** — the checked-out branch, or `(detached @ <sha>)`
- **State** — the first that applies: `current` (the worktree the tool runs in), `prunable`
  (directory missing), `locked` (`git worktree lock`), `dirty` (uncommitted or untracked changes),
  `detached`, otherwise the branch's own status from the branch view (`merged`, `pr-merged`, `gone`,
  `unmerged`, `local`, `protected`, ...). The branch view shows branches held by other worktrees as
  `⑂ protected-wt`; this view shows the status they would have otherwise
- **Changes** — counts from `git status --porcelain`: `M` modified, `S` staged, `N` new (untracked).
  `git status` runs in the background for every worktree, so the column shows `…` until it arrives
- **PR** — with `--github` / `--bitbucket`, the PR of the worktree's branch

The details pane shows the full path, branch, branch status, upstream and ahead/behind, HEAD, the
last commit, the change counts, the changed paths grouped under Modified, Staged and New (up to 5
per group, then `… and N more`; a file both staged and modified appears in both), the lock or
prunable reason, and the PR.

### Worktree Keys

| Key        | Action                                                           |
| ---------- | ---------------------------------------------------------------- |
| `x`        | Turn remove mode on/off (off at every start)                     |
| `Space`    | Toggle selection for the worktree under the cursor               |
| `a`        | Select/deselect all clean stale worktrees                        |
| `c`        | Clear the selection                                              |
| `1` / `F1` | Show clean worktrees                                             |
| `2` / `F2` | Show dirty worktrees                                             |
| `3` / `F3` | Show stale worktrees (merged, pr-merged, gone, prunable)         |
| `4` / `F4` | Show all worktrees                                               |
| `Tab`      | Cycle through filters                                            |
| `/`        | Search by worktree name, branch or path                          |
| `Enter`    | Remove the selected worktrees (opens confirmation)               |
| `b`        | In the confirmation: also delete the removed worktrees' branches |
| `f`        | Toggle force mode (dirty worktrees become selectable)            |
| `d`        | Toggle dry run mode                                              |
| `y`        | Copy the worktree path to the clipboard                          |
| `o`        | Open the branch's PR in the browser (PR integration enabled)     |
| `r`        | Reload worktrees and branches                                    |
| `R`        | Fetch the remote, reload, re-fetch PR data                       |
| `w`        | Switch back to the branch view                                   |

`Space`, `a`, `c` and `Enter` need remove mode (`x`); while it is off they only show
`Removal is off (press x to turn it on)` in the footer.

With remove mode off the footer reads
`↑↓ nav  x remove mode  y copy path  / search  w branches  ? help  q quit`; with it on,
`↑↓ nav  Space sel  a stale  c clr  f force  d dry  Enter remove  x remove mode  y copy path  / search  w branches  ? help  q quit`
(plus `o pr` with PR integration).

### Removing Worktrees

- Removal is off when the view opens: it is a read-only explorer with no checkboxes. Press `x` to
  turn remove mode on (header badge `🗑 REMOVE`); pressing `x` again turns it off and clears the
  selection
- Clean worktrees are removed with `git worktree remove <path>`
- Dirty worktrees are selectable only in force mode and are removed with
  `git worktree remove --force <path>` — their uncommitted changes are lost
- Prunable worktrees (directory already gone) are cleaned up with `git worktree prune`, which prunes
  every prunable worktree at once
- The main, current and locked worktrees are never selectable; a worktree whose `git status` is
  still loading becomes selectable once it arrives
- In the confirmation modal, `b` toggles **also delete branches** (off by default). Each removed
  worktree's branch is then deleted with the branch view's rules: `-d`, or `-D` for gone branches
  and in force mode; protected branches are kept, and unmerged / local / pr-diverged branches are
  kept unless force mode is on
- Dry run mode logs the commands instead of running them
- `y` copies the path with the first clipboard tool that works (`wl-copy`, `xclip`, `xsel`,
  `pbcopy`, `clip`, or `clip.exe` under WSL); the footer shows the result

### CLI

`--worktrees --cli` prints the worktree table and exits without removing anything. With `--dry-run`
it also prints the command the TUI would run for each clean stale worktree (with `--force`, dirty
stale ones too).

---

## Example Workflows

### Quick Cleanup of Safe Branches

```
1. Press `a` to select all safe branches
2. Review the count in header
3. Press `Enter`
4. Press `y` to confirm
5. Press `q` to quit
```

### Targeted Cleanup with Preview

```
1. Press `d` to enable dry run mode
2. Press `1` to filter to merged branches
3. Navigate and select specific branches with `Space`
4. Press `Enter` then `y` to preview
5. Review action log
6. Press `d` to disable dry run
7. Press `Enter` then `y` to actually delete
```

### Removing Merged Worktrees and Their Branches

```
1. Press `w` (or start with --worktrees)
2. Press `a` to select all clean stale worktrees
3. Press `Enter`
4. Press `b` to also delete their branches
5. Press `y` to confirm
```

### Deleting an Unmerged Branch

```
1. Press `3` to filter to unmerged branches
2. Navigate to the branch
3. Press `f` to enable force mode
4. Press `Space` to select
5. Press `Enter`
6. Review the warning in the modal
7. Press `y` to confirm (or `n` to cancel)
```

---

## Tips

- **Start without force mode** — Review merged and gone branches first, they're always safe
- **Use filters** — Narrow down the list to focus on specific branch types
- **Check the details pane** — Verify branch info before selecting
- **Use dry run for complex cleanup** — Preview actions before executing
- **Watch the action log** — Verify deletions succeeded after confirming
- **Read confirmation modals** — They show exactly what will happen

---

## Troubleshooting

### Branch won't delete

- Check if it's the current branch (◉ icon) — switch to another branch first
- Check if it's protected (⊘ icon) — main/master/develop cannot be deleted
- Check if it's checked out in another worktree (⑂ icon) — the details pane shows
  `Worktree: <name> (<path>)`; switch that worktree to another branch or remove it
  (`git worktree remove <path>`, or `git worktree prune` when it is marked prunable) first
- Check if it's unmerged (! icon) — enable force mode with `f`

### Can't select a branch

- Unmerged branches show `-` checkbox — enable force mode with `f`
- Protected and current branches have no checkbox — they cannot be deleted

### Can't select a worktree

- `◆` (main), `◉ current` and `🔒 locked` worktrees can never be removed
- Dirty worktrees show `-` — enable force mode with `f`
- `…` means `git status` is still running for that worktree

### Deletion failed

- Check the action log for the error message
- Common causes: branch checked out, permission issues, or branch no longer exists
