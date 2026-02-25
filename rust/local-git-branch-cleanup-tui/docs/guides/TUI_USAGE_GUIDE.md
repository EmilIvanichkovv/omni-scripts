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
│                                           │                             │
├───────────────────────────────────────────┴─────────────────────────────┤
│ Action Log: Deletion results (appears after deletions)                  │
├─────────────────────────────────────────────────────────────────────────┤
│ Footer: Status legend and keyboard shortcuts                            │
└─────────────────────────────────────────────────────────────────────────┘
```

---

## Keyboard Controls

### Navigation

| Key | Action |
|-----|--------|
| `↑` / `k` | Move cursor up |
| `↓` / `j` | Move cursor down |
| `g` / `Home` | Go to first item |
| `G` / `End` | Go to last item |
| `Ctrl+U` | Go to first item |
| `Ctrl+D` | Go to last item |
| `Page Up` | Move up by one page |
| `Page Down` | Move down by one page |
| `q` / `Esc` | Quit application |

### Filtering Branches

| Key | Action |
|-----|--------|
| `1` / `F1` | Show only safe merged branches |
| `2` / `F2` | Show only upstream gone branches |
| `3` / `F3` | Show only unmerged branches |
| `4` / `F4` | Show all branches |
| `Tab` | Cycle through filters |

The active filter tab is highlighted in cyan, and each tab displays the branch count for that category.

### Search

| Key | Action |
|-----|--------|
| `/` | Start search mode |
| `Esc` | Exit search and clear query |
| `Enter` | Exit search and keep filter active |

**Search Syntax:**

| Syntax | Description |
|--------|-------------|
| `feature` | Filter branches containing "feature" in name |
| `@author:john` | Filter by branch author containing "john" |
| `@author:me` | Filter by your branches (current git user) |
| `feature @author:john` | Combine name and author filters |

When search is active, the search box appears below the filters. The search is case-insensitive.

### Sorting Branches

| Key | Action |
|-----|--------|
| `s` | Cycle through sort modes |

**Available Sort Modes:**

| Mode | Label | Description |
|------|-------|-------------|
| Status | (default) | Groups by branch status (current, protected, merged, gone, unmerged) |
| Name | Name | Alphabetical by branch name (case-insensitive) |
| Activity ↓ | Active ↓ | Most recently active branches first (by last commit) |
| Activity ↑ | Active ↑ | Least recently active branches first |
| Created ↓ | Created ↓ | Most recently created branches first |
| Created ↑ | Created ↑ | Oldest branches first (by creation date) |

When a non-default sort mode is active, the header displays a sort indicator: `🔀 Active ↓`

### Selection

| Key | Action |
|-----|--------|
| `Space` | Toggle selection for current branch |
| `a` | Select/deselect all safe branches |
| `c` | Clear all selections |

### Actions

| Key | Action |
|-----|--------|
| `Enter` | Delete selected branches (opens confirmation) |
| `f` | Toggle force mode |
| `d` | Toggle dry run mode |
| `s` | Cycle sort mode |
| `i` | Show info modal (about the tool) |
| `?` | Show help modal |

---

## Understanding Branch Status

Each branch displays a status icon indicating its state:

| Icon | Status | Description | Deletable? |
|------|--------|-------------|------------|
| ✓ | Merged | Fully merged into trunk | ✅ Safe (`-d`) |
| ↗ | Gone | Remote tracking branch was deleted | ✅ Safe (`-d`) |
| ! | Unmerged | Has commits not in trunk | ⚠️ Requires force mode |
| ⊘ | Protected | main/master/develop branches | ❌ Never |
| ◉ | Current | Currently checked out branch | ❌ Never |

---

## Understanding Checkboxes

The checkbox state indicates whether a branch can be selected:

| Checkbox | Meaning |
|----------|---------|
| `[✓]` | Selected for deletion |
| `[ ]` | Not selected (can be toggled with `Space`) |
| ` - ` | Disabled (unmerged branch, enable force mode to select) |
| *No checkbox* | Protected or current branch (cannot be deleted) |

---

## Selecting Branches

### Individual Selection

1. Navigate to the branch using `↑`/`↓` or `j`/`k`
2. Press `Space` to toggle selection
3. The checkbox changes from `[ ]` to `[✓]`
4. Header updates to show selection count: "📦 X selected"

### Bulk Selection

- Press `a` to select **all safe branches** (merged and gone)
- Press `a` again to deselect all
- Press `c` to clear all selections

> **Note:** Bulk selection only affects branches that are currently selectable based on force mode.

---

## Force Mode

By default, unmerged branches cannot be selected. This protects you from accidentally deleting work that hasn't been merged.

### Enabling Force Mode

1. Press `f` to toggle force mode
2. Header displays "⚠️ FORCE" indicator
3. Unmerged branches now show `[ ]` instead of ` - `
4. You can now select unmerged branches

### Force Mode Behavior

- Selected unmerged branches will be deleted with `git branch -D` (force delete)
- Merged/gone branches still use safe delete `git branch -d`

> **⚠️ Warning:** Force mode allows deletion of branches with unmerged commits. These commits may be lost if not pushed or merged elsewhere. Use with caution!

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

## Details Pane

The right panel (30% of screen) shows detailed information about the highlighted branch:

- **Branch name** — Full branch name
- **Status** — Explanation (e.g., "Merged into main")
- **Upstream** — Remote tracking branch, or "None"
- **Ahead/Behind** — Commit differences with upstream
- **Last Commit** — SHA, author, and message

The details pane updates as you navigate through the branch list.

---

## Help Modal

Press `?` at any time to display a comprehensive help modal with all keyboard shortcuts organized by category:

- **Navigation** — Arrow keys, j/k
- **Filters** — 1-4, F1-F4, Tab
- **Selection** — Space, a (all), c (clear)
- **Actions** — Enter (delete), f (force), d (dry run)
- **Other** — i (info), ? (help), q (quit)

Press any key to close the help modal.

---

## Info Modal

Press `i` at any time to display information about the tool:

- Tool name and description
- Explanation of all branch status types (merged, gone, unmerged, protected, current)
- Hint to press `?` for keyboard shortcuts

Press any key to close the info modal.

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
- Check if it's unmerged (! icon) — enable force mode with `f`

### Can't select a branch

- Unmerged branches show ` - ` checkbox — enable force mode with `f`
- Protected and current branches have no checkbox — they cannot be deleted

### Deletion failed

- Check the action log for the error message
- Common causes: branch checked out, permission issues, or branch no longer exists
