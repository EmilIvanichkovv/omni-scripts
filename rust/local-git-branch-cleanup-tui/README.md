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
  - ↑ **PR-Merged** - Squash/rebase-merged via PR, remote branch still exists (safe to delete; needs
    `--github` or `--bitbucket`)
  - ↕ **PR-Diverged** - PR merged but the local branch has commits the remote doesn't (requires
    `--force`; needs `--github` or `--bitbucket`)
  - ↗ **Gone** - Remote tracking branch was deleted
  - ! **Unmerged** - Has commits not in trunk (requires `--force`)
  - ○ **Local** - Never pushed to the remote; exists only locally (requires `--force`)
  - ⊘ **Protected** - main/master/develop (cannot be deleted)
  - ◉ **Current** - Currently checked out branch
- **Flexible Sorting** - Sort branches by status, name, activity, or creation date
- **Powerful Search** - Filter by name or author (`@author:name` / `@author:me` /
  `@author:"Name With Spaces"`)
- **Smart Autocomplete** - Suggestions for `@author:` with scrollable dropdown and auto-quoting
- **PR Integration** - See PR status for each branch: GitHub with the `--github` flag (requires
  GitHub CLI) or Bitbucket Data Center with the `--bitbucket` flag (requires an HTTP access token in
  `BITBUCKET_TOKEN`); results cached in SQLite for 1 hour — concurrent tokio fetch gives **17.6×
  speedup** on cold runs
- **Safe by Default** - Uses `git branch -d` for safe deletion, protecting unmerged work
- **Trunk Detection** - Automatically detects your default branch (main/master)
- **CLI Mode** - Traditional command-line mode available with `--cli`

## Installation

Prebuilt binaries are published for every release: Linux (x86_64, ARM64), macOS (Intel, Apple
Silicon) and Windows (x86_64). You do not need Rust or Nix to use them.

### Install with one command (recommended)

Linux and macOS:

```bash
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/EmilIvanichkovv/omni-scripts/releases/latest/download/local-git-branch-cleanup-tui-installer.sh | sh
```

Windows (PowerShell):

```powershell
powershell -ExecutionPolicy Bypass -c "irm https://github.com/EmilIvanichkovv/omni-scripts/releases/latest/download/local-git-branch-cleanup-tui-installer.ps1 | iex"
```

The installer picks the build for your platform and puts `local-git-branch-cleanup-tui` in
`~/.local/bin` (`%USERPROFILE%\.local\bin` on Windows), adding it to your `PATH` if needed. To
uninstall, delete that file. Run the same command again to upgrade.

The `PATH` change applies to new terminals only, so the terminal you installed from will not find
the command yet. Open a new terminal, or run the `source` line the installer prints (Linux and
macOS):

```bash
source ~/.config/local-git-branch-cleanup-tui/env.sh     # sh, bash, zsh
source ~/.config/local-git-branch-cleanup-tui/env.fish   # fish
```

If the installer prints `~/.profile: No such file or directory`, `~/.profile` is most likely a
symlink to a missing file (common with dotfile managers). The binary is still installed; fix or
remove the link, or add `~/.local/bin` to your `PATH` yourself.

### Download manually

If you prefer not to pipe a script into your shell, download the archive for your platform from the
[latest release](https://github.com/EmilIvanichkovv/omni-scripts/releases/latest):

| Platform              | Archive                                                          |
| --------------------- | ---------------------------------------------------------------- |
| Linux x86_64          | `local-git-branch-cleanup-tui-x86_64-unknown-linux-musl.tar.xz`  |
| Linux ARM64           | `local-git-branch-cleanup-tui-aarch64-unknown-linux-musl.tar.xz` |
| macOS (Intel)         | `local-git-branch-cleanup-tui-x86_64-apple-darwin.tar.xz`        |
| macOS (Apple Silicon) | `local-git-branch-cleanup-tui-aarch64-apple-darwin.tar.xz`       |
| Windows x86_64        | `local-git-branch-cleanup-tui-x86_64-pc-windows-msvc.zip`        |

Each archive has a `.sha256` file next to it. Verify the download, extract it and run the binary:

```bash
sha256sum -c local-git-branch-cleanup-tui-x86_64-unknown-linux-musl.tar.xz.sha256   # Linux
shasum -a 256 -c local-git-branch-cleanup-tui-aarch64-apple-darwin.tar.xz.sha256    # macOS
```

```powershell
Get-FileHash local-git-branch-cleanup-tui-x86_64-pc-windows-msvc.zip -Algorithm SHA256   # Windows
```

The Linux binaries are statically linked and run on any distribution.

### Requirements

- **Git** on your `PATH`.
- Optional: [GitHub CLI](https://cli.github.com/) (`gh`) for `--github`, or a Bitbucket Data Center
  HTTP access token in `BITBUCKET_TOKEN` for `--bitbucket`.

### Known limitations

- Binaries are not code-signed yet. On macOS, a binary downloaded through a browser may be blocked
  by Gatekeeper; run `xattr -d com.apple.quarantine local-git-branch-cleanup-tui` once. (The
  one-command installer is not affected.) On Windows, SmartScreen may warn about a downloaded
  `.exe`.
- Windows on ARM runs the x86_64 build through emulation; there is no native ARM64 build.

### Build from source

With Nix:

```bash
# Run directly (no build needed)
nix run github:EmilIvanichkovv/omni-scripts#local-git-branch-cleanup-tui

# Or from local checkout
nix run .#local-git-branch-cleanup-tui
```

With Cargo:

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
      --trunk <TRUNK>                Override the default trunk branch
  -f, --force                        Force delete unmerged branches (use with caution!)
      --cli                          Use CLI mode instead of TUI
      --dry-run                      Dry run mode - preview actions without executing
  -g, --github                       Enable GitHub PR integration (requires gh CLI)
  -b, --bitbucket                    Enable Bitbucket Data Center PR integration
                                     (requires BITBUCKET_TOKEN)
      --bitbucket-base-url <URL>     Override the derived Bitbucket base URL
      --bitbucket-project <PROJECT>  Override the derived Bitbucket project key
      --bitbucket-repo <REPO>        Override the derived Bitbucket repository slug
  -h, --help                         Print help
```

`--github` and `--bitbucket` are mutually exclusive, and each `--bitbucket-*` override requires
`--bitbucket`.

### Environment Variables (Bitbucket)

| Variable             | Required                | Purpose                                                          |
| -------------------- | ----------------------- | ---------------------------------------------------------------- |
| `BITBUCKET_TOKEN`    | Yes, with `--bitbucket` | Bitbucket Data Center HTTP access token (sent as a Bearer token) |
| `BITBUCKET_BASE_URL` | No                      | Base URL fallback when `--bitbucket-base-url` is absent          |
| `BITBUCKET_PROJECT`  | No                      | Project-key fallback when `--bitbucket-project` is absent        |
| `BITBUCKET_REPO`     | No                      | Repository-slug fallback when `--bitbucket-repo` is absent       |

Precedence for base URL, project, and repository: CLI flag > environment variable > value derived
from the `origin` remote. The token is environment-only and is never accepted as a CLI argument.

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

# Enable Bitbucket Data Center PR integration
export BITBUCKET_TOKEN='<HTTP_ACCESS_TOKEN>'
./target/release/local-git-branch-cleanup-tui --bitbucket

# Bitbucket with explicit overrides (when origin cannot be parsed)
./target/release/local-git-branch-cleanup-tui --bitbucket \
  --bitbucket-base-url 'https://bitbucket.example.com' \
  --bitbucket-project 'PROJ' \
  --bitbucket-repo 'my-repo'

# Using cargo run (from rust/ directory)
cargo run --release -- --cli
cargo run --release -- --trunk develop
cargo run --release -- --github
```

> **Tip:** Prefer a hidden prompt over pasting the token into your shell history:
> `read -rs BITBUCKET_TOKEN && export BITBUCKET_TOKEN`

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

| Key                | Action                                           |
| ------------------ | ------------------------------------------------ |
| `↑`/`↓` or `j`/`k` | Navigate branches                                |
| `Space`            | Toggle selection                                 |
| `a`                | Select all safe branches                         |
| `c`                | Clear selections                                 |
| `1`-`4` or `Tab`   | Switch filter tabs                               |
| `/`                | Search branches                                  |
| `s`                | Cycle sort mode                                  |
| `f`                | Toggle force mode                                |
| `d`                | Toggle dry run mode                              |
| `Enter`            | Delete selected (with confirmation)              |
| `o`                | Open PR in browser (with --github / --bitbucket) |
| `?`                | Show help                                        |
| `q` / `Esc` `Esc`  | Quit (press `Esc` twice within 1s)               |

### Search Syntax

| Syntax         | Description             |
| -------------- | ----------------------- |
| `feature`      | Filter by branch name   |
| `@author:john` | Filter by branch author |
| `@author:me`   | Filter by your branches |

## Branch Status Legend

| Icon | Status      | Description                                    | Deletable             |
| ---- | ----------- | ---------------------------------------------- | --------------------- |
| ✓    | merged      | Fully merged into trunk                        | ✅ Safe (`-d`)        |
| ↑    | pr-merged   | PR merged (squash/rebase), remote still exists | ✅ Safe (`-d`)        |
| ↕   | pr-diverged | PR merged, local has commits remote doesn't    | ⚠️ Requires `--force` |
| ↗   | gone        | Remote was deleted                             | ✅ Safe (`-d`)        |
| !    | unmerged    | Has unmerged commits                           | ⚠️ Requires `--force` |
| ○    | local       | Never pushed to the remote                     | ⚠️ Requires `--force` |
| ⊘    | protected   | main/master/develop                            | ❌ Never              |
| ◉    | current     | Currently checked out                          | ❌ Never              |

## Safety Features

1. **Protected Branches** - `main`, `master`, `develop`, and the current branch are never deleted
2. **Safe Delete by Default** - Uses `git branch -d` which fails if branch has unmerged commits
3. **Force Delete Opt-in** - Unmerged branches require explicit `--force` flag
4. **Confirmation Required** - CLI mode asks for confirmation before deletion

## Development

### Prerequisites

- Rust 1.75+ (or use Nix)
- Git
- GitHub CLI (`gh`) — only for `--github` PR integration
- Bitbucket Data Center HTTP access token — only for `--bitbucket` PR integration

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
│   │   ├── GITHUB_PR_CACHE.md
│   │   ├── BITBUCKET_SUPPORT.md
│   │   └── SEARCH_FEATURE.md
│   └── testing/        # Testing docs
│       ├── TESTING.md
│       └── TEST_SUMMARY.md
├── src/
│   ├── main.rs         # Entry point, CLI parsing, async tokio runtime
│   ├── app.rs          # Application state management
│   ├── git.rs          # Local Git integration and branch classification
│   ├── remote.rs       # Parse the origin remote into a normalized identity
│   ├── cache.rs        # SQLite PR cache (XDG_CACHE_HOME/omni-scripts/pr-cache.db)
│   ├── ui.rs           # TUI rendering with Ratatui
│   └── pr/
│       ├── mod.rs      # Shared PR types, provider trait, fetch coordinator
│       ├── github.rs   # GitHub provider (gh CLI)
│       └── bitbucket.rs # Bitbucket Data Center provider (REST API)
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

## Releases (maintainers)

Releases are automatic. Every merge to `main` that touches this tool and contains a releasable
commit publishes a new version: `feat` bumps the minor version, `fix`, `perf`, `build` and `revert`
bump the patch version, and breaking changes (`feat!:` or a `BREAKING CHANGE:` footer) bump the
major version (the minor version before 1.0). Merges with only `refactor`, `docs`, `test`, `chore`,
`ci` or `style` commits do not release; `refactor` commits are listed in the next release's
changelog. Commit subjects must follow Conventional Commits; CI checks every PR commit, since PRs
are rebase-merged.

The `auto-release` workflow commits the version bump and changelog entry and pushes the
`local-git-branch-cleanup-tui-vX.Y.Z` tag; the tag starts the `release` workflow (dist), which
builds the binaries, publishes the GitHub Release, and smoke tests the installers on every platform.

Manual commands (run inside `nix develop`):

| Command                          | What it does                                                         |
| -------------------------------- | -------------------------------------------------------------------- |
| `just release-plan [bump]`       | Show the next version, its changelog entry and the dist plan (local) |
| `just release-dry-run`           | Same decision made by the workflow on GitHub; pushes nothing         |
| `just release-pre [label] [ref]` | Publish a prerelease (e.g. `0.3.0-rc.1`) from a pushed branch        |
| `just release-pre-delete <ver>`  | Delete a prerelease and its tag                                      |
| `just release [bump]`            | Cut a stable release from `main` now, optionally forcing the bump    |
| `just release-status`            | Show the latest release runs and tags                                |

- To force a specific version (e.g. `1.0.0`), set it in `Cargo.toml` in a PR; it is released as is.
- Prereleases never change a branch and are never marked "latest", so the install commands above
  keep pointing at the last stable release. Install one with its tag in the URL:
  `.../releases/download/local-git-branch-cleanup-tui-v0.3.0-rc.1/local-git-branch-cleanup-tui-installer.sh`.
- Stable releases are never replaced or deleted. If one is broken, fix forward with the next merge.
- The one-command install uses `/releases/latest/`, which is the newest release in the whole
  repository. Once another tool here publishes releases, they must not be marked "latest" (or the
  install URL must change).

Full design: [Release and distribution spec](docs/specs/RELEASE_DISTRIBUTION.md).

## License

MIT. See [LICENSE](../../LICENSE). Part of the omni-scripts repository.
