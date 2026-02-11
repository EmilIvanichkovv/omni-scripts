# 🧹 Local Git Branch Cleanup TUI

An interactive terminal user interface (TUI) for cleaning up local Git branches that no longer have remote counterparts.

## Features

- **Interactive TUI** - Navigate and select branches with keyboard controls
- **Smart Classification** - Automatically categorizes branches:
  - ● **Merged** - Safely merged into trunk (safe to delete)
  - ◆ **Gone** - Remote tracking branch was deleted
  - ▲ **Unmerged** - Has commits not in trunk (requires `--force`)
  - ⛔ **Protected** - main/master/develop (cannot be deleted)
  - ★ **Current** - Currently checked out branch
- **Safe by Default** - Uses `git branch -d` for safe deletion, protecting unmerged work
- **Trunk Detection** - Automatically detects your default branch (main/master)
- **CLI Mode** - Traditional command-line mode available with `--cli`

## Installation

### Using Nix (Recommended)

```bash
# Enter the development shell
nix develop .#rust-tui

# Build the project
cd rust
cargo build --release

# The binary is at ./target/release/local-git-branch-cleanup-tui
```

### Using Cargo

```bash
cd rust
cargo build --release
```

## Usage

### TUI Mode (Default)

Run the tool in any Git repository:

```bash
# From the rust/ directory after building
./target/release/local-git-branch-cleanup-tui

# Or using cargo
cargo run --release
```

This opens an interactive interface where you can:
- Browse all local branches without remote counterparts
- See branch status and last commit time
- Navigate with keyboard controls

### CLI Mode

For scripting or if you prefer the classic interface:

```bash
./target/release/local-git-branch-cleanup-tui --cli

# Or using cargo
cargo run --release -- --cli
```

### Command Line Options

```
Usage: local-git-branch-cleanup-tui [OPTIONS]

Options:
      --trunk <TRUNK>  Override the default trunk branch
  -f, --force          Force delete unmerged branches (use with caution!)
      --cli            Use CLI mode instead of TUI
  -h, --help           Print help
```

### Examples

```bash
# Use TUI mode (default)
./target/release/local-git-branch-cleanup-tui

# Use CLI mode
./target/release/local-git-branch-cleanup-tui --cli

# Override trunk branch detection
./target/release/local-git-branch-cleanup-tui --trunk develop

# Force delete unmerged branches (dangerous!)
./target/release/local-git-branch-cleanup-tui --force

# Using cargo run (from rust/ directory)
cargo run --release -- --cli
cargo run --release -- --trunk develop
```

### Adding to PATH (Optional)

To use `local-git-branch-cleanup-tui` from anywhere:

```bash
# Copy to a directory in your PATH
cp ./target/release/local-git-branch-cleanup-tui ~/.local/bin/

# Or create a symlink
ln -s $(pwd)/target/release/local-git-branch-cleanup-tui ~/.local/bin/
```

## TUI Guide

### Getting Started

When you launch the TUI, you'll see:
- **Header** - Repository path, trunk branch, selection count, and force mode indicator
- **Filter Tabs** - Filter branches by status (Safe Merged, Upstream Gone, Unmerged, All)
- **Branch List** - Filtered branches with status icons, checkboxes, and metadata (70% width)
- **Details Pane** - Detailed information about selected branch (30% width, right side)
- **Action Log** - Deletion results (appears after deletions)
- **Footer** - Status legend and keyboard shortcuts

### Navigation

| Key | Action |
|-----|--------|
| `↑` / `k` | Move cursor up |
| `↓` / `j` | Move cursor down |
| `q` / `Esc` | Quit application |

### Filters

| Key | Action |
|-----|--------|
| `1` / `F1` | Show only safe merged branches |
| `2` / `F2` | Show only upstream gone branches |
| `3` / `F3` | Show only unmerged branches |
| `4` / `F4` | Show all branches |
| `Tab` | Cycle through filters |

The active filter tab is highlighted in cyan, and each tab shows the branch count for that category.

### Branch Selection

| Key | Action |
|-----|--------|
| `Space` | Toggle selection for current branch |
| `a` | Select/deselect all safe branches |
| `c` | Clear all selections |
| `f` | Toggle force mode (enables unmerged branches) |

**Understanding Checkboxes:**
- `[✓]` - Selected for deletion
- `[ ]` - Not selected (can be toggled)
- ` - ` - Disabled (unmerged branch without force mode)
- No checkbox - Protected/current branch (cannot be deleted)

### Deleting Branches

1. **Select branches** - Use `Space` to select individual branches, or `a` to select all safe branches
2. **Review selection** - The header shows how many branches are selected
3. **Confirm deletion** - Press `Enter` to open the confirmation modal
4. **Approve or cancel** - Press `y` to delete, `n` to cancel

The confirmation modal shows:
- Number of branches to be deleted
- Names of selected branches (up to 3, with "and X more" if needed)
- Warning if any branches are unmerged
- The delete command that will be used (`-d` or `-D`)

### Force Mode

By default, unmerged branches show a `-` checkbox and cannot be selected. This protects you from accidentally losing work.

To delete unmerged branches:
1. Press `f` to toggle force mode
2. Header shows "⚠️ FORCE" indicator
3. Unmerged branches now show `[ ]` checkboxes
4. Selected unmerged branches will be deleted with `git branch -D` (force delete)

**⚠️ Warning:** Force mode allows deletion of branches with unmerged commits. Use with caution!

### Action Log

After deleting branches, the action log appears at the bottom showing:
- ✓ Successfully deleted branches
- ✗ Failed deletions with error messages
- Success/failure counts

The branch list automatically refreshes after deletion.

### Details Pane

The details pane (right side, 30% of screen) shows comprehensive information about the currently selected branch:
- **Branch name** - Full branch name
- **Status** - Status explanation (e.g., "Merged into main")
- **Upstream** - Remote tracking branch (if any), or "None"
- **Ahead/Behind** - Commit count differences with upstream (if applicable)
- **Last Commit** - SHA, author, and commit message

The details pane updates automatically as you navigate through the branch list.

### Example Workflow

```
1. Launch TUI
   $ ./target/release/local-git-branch-cleanup-tui

2. Navigate to an unmerged branch you want to delete
   → Press ↓ or j to move down

3. Enable force mode (if needed)
   → Press f
   → Header shows "⚠️ FORCE"

4. Select the branch
   → Press Space
   → Checkbox shows [✓]

5. Review your selection
   → Header shows "📦 1 selected"

6. Confirm deletion
   → Press Enter
   → Modal shows "Delete 1 branch(es)?"

7. Approve
   → Press y
   → Action log shows "✓ branch-name - Deleted (-D)"
   → Branch list refreshes

8. Quit when done
   → Press q
```

### Tips

- **Start without force mode** - Review merged/gone branches first
- **Use `a` for bulk cleanup** - Quickly select all safe branches
- **Check the status icons** - ● merged and ◆ gone are always safe to delete
- **Read the confirmation modal** - It shows exactly which branches will be deleted
- **Watch the action log** - Verify deletions succeeded

## Branch Status Legend

| Icon | Status | Description | Deletable |
|------|--------|-------------|-----------|
| ● | merged | Fully merged into trunk | ✅ Safe (`-d`) |
| ◆ | gone | Remote was deleted | ✅ Safe (`-d`) |
| ▲ | unmerged | Has unmerged commits | ⚠️ Requires `--force` |
| ⛔ | protected | main/master/develop | ❌ Never |
| ★ | current | Currently checked out | ❌ Never |

## Safety Features

1. **Protected Branches** - `main`, `master`, `develop`, and the current branch are never deleted
2. **Safe Delete by Default** - Uses `git branch -d` which fails if branch has unmerged commits
3. **Force Delete Opt-in** - Unmerged branches require explicit `--force` flag
4. **Confirmation Required** - CLI mode asks for confirmation before deletion

## Development

### Prerequisites

- Rust 1.75+ (or use Nix)
- Git

### Building

```bash
# Development build
cargo build

# Release build
cargo build --release

# Run directly
cargo run

# Run with arguments
cargo run -- --cli --trunk main
```

### Project Structure

```
rust/
├── Cargo.toml          # Dependencies and project config
├── README.md           # This file
└── src/
    ├── main.rs         # Entry point, CLI parsing, event loop
    ├── app.rs          # Application state management
    ├── git.rs          # Git integration and branch classification
    └── ui.rs           # TUI rendering with Ratatui
```

## Comparison with Bash Script

This tool replaces `bash/local-git-branch-cleanup.sh` with improvements:

| Feature | Bash Script | Rust TUI |
|---------|-------------|----------|
| Interface | Static list | Interactive TUI |
| Selection | All-or-nothing | Per-branch (M5) |
| Delete mode | Force (`-D`) only | Safe (`-d`) by default |
| Branch info | Last commit time | + Status classification |
| Protection | None | main/master/develop/current |
| Unmerged warning | No | Yes, requires `--force` |

## License

Part of the omni-scripts repository.
