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

## Keyboard Controls (TUI Mode)

| Key | Action |
|-----|--------|
| `↑` / `k` | Move selection up |
| `↓` / `j` | Move selection down |
| `q` / `Esc` | Quit |

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
