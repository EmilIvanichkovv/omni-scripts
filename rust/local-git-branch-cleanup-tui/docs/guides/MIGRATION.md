# Migration Guide: Bash Script → Rust TUI

This guide helps you migrate from the legacy bash script (`bash/local-git-branch-cleanup.sh`) to the
new Rust-based TUI (`rust/local-git-branch-cleanup-tui`).

## Quick Start

### Option 1: Side-by-Side Testing (Recommended)

Keep both versions and test the Rust TUI on non-critical repositories first:

```bash
# Build the Rust version
cd rust
cargo build --release

# Test it on a test repository
cd /path/to/test/repo
/path/to/rust/target/release/local-git-branch-cleanup-tui

# If satisfied, install it
cp /path/to/rust/target/release/local-git-branch-cleanup-tui ~/.local/bin/
```

### Option 2: Replace Bash Script

If you're confident, replace the bash script entirely:

```bash
# Build the Rust version
cd rust
cargo build --release

# Replace bash script (backup first!)
cp bash/local-git-branch-cleanup.sh bash/local-git-branch-cleanup.sh.backup
cp rust/target/release/local-git-branch-cleanup-tui ~/.local/bin/local-git-branch-cleanup

# Create alias for backward compatibility
alias local-git-branch-cleanup='local-git-branch-cleanup-tui --cli'
```

## Feature Comparison

| Feature                   | Bash Script       | Rust TUI               | Notes                                              |
| ------------------------- | ----------------- | ---------------------- | -------------------------------------------------- |
| **Interface**             | Static list       | Interactive TUI        | TUI is default, use `--cli` for bash-like behavior |
| **Branch Selection**      | All-or-nothing    | Individual selection   | Select with Space, confirm with Enter              |
| **Delete Mode**           | Force (`-D`) only | Safe (`-d`) by default | Use `--force` for unmerged branches                |
| **Branch Classification** | No remote only    | 5 status types         | Merged, gone, unmerged, protected, current         |
| **Safety**                | No protection     | Protected branches     | Never deletes main/master/develop/current          |
| **Filtering**             | None              | 4 filter modes         | Safe merged, upstream gone, unmerged, all          |
| **Details**               | Last commit time  | Full branch info       | Commit SHA, author, message, ahead/behind          |
| **Confirmation**          | Yes               | Yes (with preview)     | Shows which branches will be deleted               |
| **Dry Run**               | No                | Yes (`--dry-run`)      | Preview deletions without executing                |
| **Trunk Override**        | No                | Yes (`--trunk`)        | Override default branch detection                  |
| **Action Log**            | Summary only      | Per-branch log         | Shows success/failure for each branch              |
| **Help**                  | None              | Press `?`              | Comprehensive keyboard shortcuts                   |
| **Navigation**            | N/A               | Vim-style or arrows    | j/k or ↑/↓                                         |
| **Performance**           | Fast              | Fast                   | Both handle <200 branches easily                   |
| **Dependencies**          | bash, git         | git only               | Statically linked binary                           |

## Command Equivalents

### Basic Usage

```bash
# Bash script
./bash/local-git-branch-cleanup.sh

# Rust TUI (interactive mode)
./rust/target/release/local-git-branch-cleanup-tui

# Rust TUI (CLI mode - closest to bash script)
./rust/target/release/local-git-branch-cleanup-tui --cli
```

### With Trunk Override

```bash
# Bash script (no support)
# Manual workaround needed

# Rust TUI
./rust/target/release/local-git-branch-cleanup-tui --trunk develop
```

### Force Delete

```bash
# Bash script (always force deletes)
./bash/local-git-branch-cleanup.sh

# Rust TUI (safe delete by default)
./rust/target/release/local-git-branch-cleanup-tui --force
```

### Dry Run / Preview

```bash
# Bash script (no support)
# Manual review needed

# Rust TUI
./rust/target/release/local-git-branch-cleanup-tui --dry-run
```

## Behavior Changes

### 1. Delete Mode (IMPORTANT!)

**Bash Script:**

- Always uses `git branch -D` (force delete)
- Can accidentally delete branches with unmerged commits

**Rust TUI:**

- Uses `git branch -d` (safe delete) by default
- Requires `--force` flag to delete unmerged branches
- Clearly indicates unmerged branches in UI

**Migration Impact:**

- ✅ **Safer**: Won't accidentally lose unmerged work
- ⚠️ **Breaking Change**: Unmerged branches won't be deleted without `--force`

**Action:** If you relied on force deleting unmerged branches, add `--force` flag or press `f` in
TUI mode.

### 2. Branch Protection

**Bash Script:**

- No explicit protection
- Relies on "no remote" check (which protects main/master indirectly)

**Rust TUI:**

- Explicitly protects: `main`, `master`, `develop`, and current branch
- Cannot be selected for deletion in TUI mode
- Won't appear in CLI mode output

**Migration Impact:**

- ✅ **Safer**: Prevents accidental deletion of critical branches
- No breaking changes

### 3. Selection Method

**Bash Script:**

- Finds all branches without remotes
- Requires confirmation to delete ALL found branches
- All-or-nothing approach

**Rust TUI:**

- Shows all branches without remotes
- Individual selection with checkboxes (Space key)
- Bulk selection with `a` key
- Delete only selected branches

**Migration Impact:**

- ✅ **More Control**: Delete specific branches instead of all
- ⚠️ **Workflow Change**: Need to explicitly select branches (or press `a` for all)

**Action:** In TUI mode, press `a` to select all safe branches (similar to bash script behavior).

### 4. Branch Classification

**Bash Script:**

- Only checks: "has remote" vs "no remote"
- Shows last commit time

**Rust TUI:**

- Classifies: merged, gone, unmerged, protected, current
- Shows status icon, last commit time, commit details
- Filters by status type

**Migration Impact:**

- ✅ **Better Information**: Know why branches can/can't be deleted
- No breaking changes

## Migration Scenarios

### Scenario 1: "I just want it to work like the bash script"

Use CLI mode with force flag (not recommended):

```bash
local-git-branch-cleanup-tui --cli --force
```

This matches bash script behavior:

- ✅ Lists branches without remotes
- ✅ Asks for confirmation
- ✅ Deletes all confirmed branches
- ⚠️ Uses force delete (less safe)

**Better alternative:** Use TUI mode and press `a` then `Enter`, which:

- ✅ Lists branches without remotes
- ✅ Pre-selects all safe branches
- ✅ Shows detailed confirmation
- ✅ Uses safe delete (protects unmerged work)

### Scenario 2: "I use this in CI/CD scripts"

**Option A:** Keep using bash script for automation

```bash
# In CI/CD pipeline
./bash/local-git-branch-cleanup.sh
```

**Option B:** Use Rust TUI CLI mode with automation-friendly flags

```bash
# Non-interactive mode (requires implementation)
# For now, use bash script for automation
./bash/local-git-branch-cleanup.sh
```

**Note:** The Rust TUI currently requires interactive confirmation. For full automation, continue
using the bash script or add `--yes` flag to Rust TUI (future enhancement).

### Scenario 3: "I want the best of both worlds"

Keep both installed with different names:

```bash
# Install bash version
cp bash/local-git-branch-cleanup.sh ~/.local/bin/git-branch-cleanup-legacy

# Install Rust version
cp rust/target/release/local-git-branch-cleanup-tui ~/.local/bin/git-branch-cleanup

# Use based on preference
git-branch-cleanup          # Interactive TUI (default)
git-branch-cleanup --cli    # CLI mode
git-branch-cleanup-legacy   # Original bash script
```

### Scenario 4: "I have custom modifications to the bash script"

Review your changes and determine if Rust TUI supports them:

1. **Custom branch patterns:** Not supported - file an issue
2. **Custom remote names:** Use `--trunk` flag or wait for `--remote` flag
3. **Different protection rules:** Not configurable yet - file an issue
4. **Output formatting:** Use CLI mode or TUI mode based on preference

If Rust TUI doesn't support your workflow, keep using the bash script and file a feature request.

## Testing Checklist

Before fully migrating, test these scenarios:

### Basic Functionality

- [ ] Run in a test repository
- [ ] Verify branches are listed correctly
- [ ] Try selecting individual branches
- [ ] Confirm deletion works
- [ ] Check action log shows results

### Edge Cases

- [ ] Test with no branches to clean
- [ ] Test with only protected branches
- [ ] Test with current branch in list (should not be deletable)
- [ ] Test with unmerged branches (should require force mode)
- [ ] Test trunk override: `--trunk develop`

### Safety Features

- [ ] Verify main/master/develop cannot be selected
- [ ] Verify current branch cannot be selected
- [ ] Verify unmerged branches show warning
- [ ] Test dry run mode: `--dry-run`
- [ ] Cancel deletion and verify nothing deleted

### Filters & Navigation

- [ ] Try filter tabs (1-4 or F1-F4)
- [ ] Navigate with j/k and arrow keys
- [ ] Select all safe branches with `a`
- [ ] Clear selection with `c`
- [ ] Toggle force mode with `f`

### CLI Mode

- [ ] Run with `--cli` flag
- [ ] Verify behavior matches bash script
- [ ] Test with `--force` flag
- [ ] Test trunk override in CLI mode

## Rollback Instructions

If you encounter issues, rollback to the bash script:

### If You Replaced the Script

```bash
# Restore from backup
cp bash/local-git-branch-cleanup.sh.backup bash/local-git-branch-cleanup.sh

# If installed to PATH
cp bash/local-git-branch-cleanup.sh ~/.local/bin/local-git-branch-cleanup
```

### If You Installed Side-by-Side

```bash
# Just remove the Rust version
rm ~/.local/bin/local-git-branch-cleanup-tui

# Continue using bash version
./bash/local-git-branch-cleanup.sh
```

### If You Find Bugs

1. Document the issue (steps to reproduce)
2. Rollback to bash script
3. File an issue with:
   - Git version
   - Repository characteristics (branch count, structure)
   - Expected vs actual behavior
   - Error messages or logs

## Performance Comparison

### Small Repos (<10 branches)

- **Bash:** < 0.5s
- **Rust TUI:** < 0.5s
- **Winner:** Tie

### Medium Repos (10-50 branches)

- **Bash:** < 1s
- **Rust TUI:** < 1s
- **Winner:** Tie

### Large Repos (50-200 branches)

- **Bash:** 1-3s
- **Rust TUI:** 1-3s
- **Winner:** Tie

Both versions have similar performance for typical repositories.

## Common Issues

### Issue 1: "Rust binary not found"

```bash
# Solution: Build the binary first
cd rust
cargo build --release

# Or ensure it's in PATH
export PATH="$PATH:$(pwd)/target/release"
```

### Issue 2: "Unmerged branches not showing as deletable"

This is expected! Unmerged branches require force mode:

```bash
# Press 'f' in TUI mode to toggle force mode
# Or use CLI with --force flag
local-git-branch-cleanup-tui --force
```

### Issue 3: "TUI looks weird in my terminal"

Try a different terminal emulator or use CLI mode:

```bash
# Use CLI mode instead
local-git-branch-cleanup-tui --cli
```

Supported terminals:

- ✅ gnome-terminal
- ✅ kitty
- ✅ alacritty
- ✅ iTerm2 (macOS)
- ✅ Windows Terminal
- ⚠️ Basic terminal emulators may have limited color support

### Issue 4: "I want the old force-delete behavior"

Use force mode:

```bash
# TUI mode: press 'f' to enable force mode
local-git-branch-cleanup-tui

# CLI mode: use --force flag
local-git-branch-cleanup-tui --cli --force
```

⚠️ **Warning:** Force mode deletes branches with unmerged commits. Use with caution!

## FAQ

### Q: Can I run both versions simultaneously?

**A:** Yes! They don't interfere with each other. Install them with different names.

### Q: Which version is recommended?

**A:** The Rust TUI is recommended for interactive use due to:

- Better safety (protected branches, safe delete by default)
- More control (individual selection)
- Better information (branch classification, details pane)

Use the bash script for:

- CI/CD automation (until Rust TUI supports `--yes` flag)
- Simple scripts without interactive prompts

### Q: Will the bash script be deprecated?

**A:** Not immediately. The bash script will remain available for backward compatibility and
automation use cases. Future updates will focus on the Rust TUI.

### Q: Can I customize the protected branch list?

**A:** Not yet. Currently hardcoded: `main`, `master`, `develop`. File an issue if you need
different protection rules.

### Q: Does Rust TUI support different remotes (not just origin)?

**A:** Not yet. Currently assumes `origin` remote. Use `--trunk` to override the trunk branch. Full
`--remote` flag support is planned.

### Q: How do I use this in automated scripts?

**A:** For full automation, continue using the bash script. The Rust TUI requires interactive
confirmation (for safety). A `--yes` flag for automation is planned.

## Getting Help

- **Documentation:** See [README.md](README.md) for detailed usage
- **Architecture:** See [ARCHITECTURE.md](ARCHITECTURE.md) for technical details
- **Issues:** File bugs or feature requests on the repository
- **Testing:** See [TESTING.md](TESTING.md) for testing guidelines

## Summary

**Recommended Migration Path:**

1. **Week 1:** Test Rust TUI side-by-side with bash script
2. **Week 2:** Use Rust TUI as primary tool, keep bash script as backup
3. **Week 3:** If satisfied, replace bash script or keep both

**Key Advantages of Rust TUI:**

- ✅ Safer (protected branches, safe delete default)
- ✅ More control (individual selection)
- ✅ Better information (status classification, details pane)
- ✅ Modern interface (interactive TUI, filters, help modal)

**When to Keep Using Bash Script:**

- CI/CD automation (until `--yes` flag added)
- Custom modifications not supported by Rust TUI
- Preference for simple, static output

**Breaking Changes to Watch:**

- Unmerged branches require `--force` flag
- Protected branches cannot be deleted
- Individual selection required (use `a` for all)

Good luck with your migration! 🚀
