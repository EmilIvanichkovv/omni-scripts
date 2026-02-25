# 🧹 Local Git Branch Cleanup TUI

> **Modern Rust-based replacement for the bash script** – An interactive terminal user interface
> (TUI) for cleaning up local Git branches that no longer have remote counterparts.

Over time, local Git branches accumulate – feature branches get merged, remote branches get deleted,
but the local copies remain. This tool helps you **safely identify and remove** these stale branches
with an intuitive, keyboard-driven interface.

## Demo

<!-- TODO: Add video/GIF demo here -->
<!--
![Demo](./assets/demo.gif)

Or embed a video:
[![Demo Video](https://img.youtube.com/vi/VIDEO_ID/0.jpg)](https://www.youtube.com/watch?v=VIDEO_ID)
-->

_Coming soon: Video demonstration of the TUI in action_

---

## Features

- **Interactive TUI** - Navigate and select branches with keyboard controls
- **Smart Classification** - Automatically categorizes branches:
  - ✓ **Merged** - Safely merged into trunk (safe to delete)
  - ↗ **Gone** - Remote tracking branch was deleted
  - ! **Unmerged** - Has commits not in trunk (requires `--force`)
  - ⊘ **Protected** - main/master/develop (cannot be deleted)
  - ◉ **Current** - Currently checked out branch
- **Flexible Sorting** - Sort branches by status, name, activity, or creation date
- **Powerful Search** - Filter by name or author (`@author:name` / `@author:me` /
  `@author:"Name With Spaces"`)
- **Smart Autocomplete** - Suggestions for `@author:` with scrollable dropdown and auto-quoting
- **GitHub PR Integration** - See PR status for each branch with `--github` flag (requires GitHub
  CLI)
- **Safe by Default** - Uses `git branch -d` for safe deletion, protecting unmerged work
- **Trunk Detection** - Automatically detects your default branch (main/master)
- **CLI Mode** - Traditional command-line mode available with `--cli`

## Installation

### Using Nix (Recommended)

```bash
# Run directly (no build needed)
nix run github:EmilIvanichkovv/omni-scripts#local-git-branch-cleanup-tui

# Or from local checkout
nix run .#local-git-branch-cleanup-tui
```

### Using Cargo

```bash
# Enter development shell (from repo root)
nix develop

# Build from workspace
cd rust
cargo build -p local-git-branch-cleanup-tui --release

# Binary is at ./target/release/local-git-branch-cleanup-tui
```

## Usage

### TUI Mode (Default)

Run the tool in any Git repository:

```bash
# Using Nix (recommended)
nix run .#local-git-branch-cleanup-tui

# Or using cargo from rust/ directory
cargo run -p local-git-branch-cleanup-tui --release
```

This opens an interactive interface where you can:

- Browse all local branches without remote counterparts
- See branch status and last commit time
- Navigate with keyboard controls

### CLI Mode

For scripting or if you prefer the classic interface:

```bash
nix run .#local-git-branch-cleanup-tui -- --cli

# Or using cargo
cargo run -p local-git-branch-cleanup-tui --release -- --cli
```

### Command Line Options

```
Usage: local-git-branch-cleanup-tui [OPTIONS]

Options:
      --trunk <TRUNK>  Override the default trunk branch
  -f, --force          Force delete unmerged branches (use with caution!)
      --cli            Use CLI mode instead of TUI
      --dry-run        Dry run mode - preview actions without executing
  -g, --github         Enable GitHub PR integration (requires gh CLI)
  -h, --help           Print help
```

### Examples

```bash
# Use TUI mode (default)
./target/release/local-git-branch-cleanup-tui

# Use CLI mode
./target/release/local-git-branch-cleanup-tui --cli

# Preview deletions without executing (dry run)
./target/release/local-git-branch-cleanup-tui --dry-run

# Override trunk branch detection
./target/release/local-git-branch-cleanup-tui --trunk develop

# Force delete unmerged branches (dangerous!)
./target/release/local-git-branch-cleanup-tui --force

# Enable GitHub PR integration
./target/release/local-git-branch-cleanup-tui --github

# Using cargo run (from rust/ directory)
cargo run --release -- --cli
cargo run --release -- --trunk develop
cargo run --release -- --github
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

📖 **[Full TUI Usage Guide](docs/guides/TUI_USAGE_GUIDE.md)** — Complete documentation for the
interactive interface.

### Quick Reference

| Key                | Action                              |
| ------------------ | ----------------------------------- |
| `↑`/`↓` or `j`/`k` | Navigate branches                   |
| `Space`            | Toggle selection                    |
| `a`                | Select all safe branches            |
| `c`                | Clear selections                    |
| `1`-`4` or `Tab`   | Switch filter tabs                  |
| `/`                | Search branches                     |
| `s`                | Cycle sort mode                     |
| `f`                | Toggle force mode                   |
| `d`                | Toggle dry run mode                 |
| `Enter`            | Delete selected (with confirmation) |
| `o`                | Open PR in browser (with --github)  |
| `?`                | Show help                           |
| `q` / `Esc`        | Quit                                |

### Search Syntax

| Syntax         | Description             |
| -------------- | ----------------------- |
| `feature`      | Filter by branch name   |
| `@author:john` | Filter by branch author |
| `@author:me`   | Filter by your branches |

## Branch Status Legend

| Icon | Status    | Description             | Deletable             |
| ---- | --------- | ----------------------- | --------------------- |
| ✓    | merged    | Fully merged into trunk | ✅ Safe (`-d`)        |
| ↗   | gone      | Remote was deleted      | ✅ Safe (`-d`)        |
| !    | unmerged  | Has unmerged commits    | ⚠️ Requires `--force` |
| ⊘    | protected | main/master/develop     | ❌ Never              |
| ◉    | current   | Currently checked out   | ❌ Never              |

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
local-git-branch-cleanup-tui/
├── Cargo.toml          # Dependencies and project config
├── README.md           # This file
├── docs/               # Documentation
│   ├── README.md       # Documentation index
│   ├── guides/         # User guides
│   │   ├── TUI_USAGE_GUIDE.md
│   │   └── MIGRATION.md
│   ├── specs/          # Technical specifications
│   │   ├── ARCHITECTURE.md
│   │   ├── ROADMAP.md
│   │   └── SEARCH_FEATURE.md
│   └── testing/        # Testing docs
│       ├── TESTING.md
│       └── TEST_SUMMARY.md
├── src/
│   ├── main.rs         # Entry point, CLI parsing, event loop
│   ├── app.rs          # Application state management
│   ├── git.rs          # Git integration and branch classification
│   └── ui.rs           # TUI rendering with Ratatui
└── tests/
    └── integration_test.rs
```

## Documentation

📚 **[Full Documentation](docs/README.md)** — Index of all documentation.

| Document                                          | Description                                 |
| ------------------------------------------------- | ------------------------------------------- |
| [TUI Usage Guide](docs/guides/TUI_USAGE_GUIDE.md) | Complete guide to the interactive interface |
| [Migration Guide](docs/guides/MIGRATION.md)       | Migrate from the bash script                |
| [Architecture](docs/specs/ARCHITECTURE.md)        | System design and module responsibilities   |
| [Roadmap](docs/specs/ROADMAP.md)                  | Project milestones and history              |

## Comparison with Bash Script

This tool replaces `bash/local-git-branch-cleanup.sh` with improvements:

| Feature          | Bash Script       | Rust TUI                    |
| ---------------- | ----------------- | --------------------------- |
| Interface        | Static list       | Interactive TUI             |
| Selection        | All-or-nothing    | Per-branch selection        |
| Delete mode      | Force (`-D`) only | Safe (`-d`) by default      |
| Branch info      | Last commit time  | + Status classification     |
| Protection       | None              | main/master/develop/current |
| Unmerged warning | No                | Yes, requires `--force`     |

See the **[Migration Guide](docs/guides/MIGRATION.md)** for detailed comparison and migration steps.

## License

Part of the omni-scripts repository.
