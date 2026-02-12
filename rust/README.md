# Rust Tools

This directory contains Rust-based tools for the omni-scripts repository, organized as a [Cargo workspace](https://doc.rust-lang.org/book/ch14-03-cargo-workspaces.html).

## Structure

```
rust/
├── Cargo.toml                      # Workspace manifest
├── Cargo.lock                      # Shared dependency lock
├── omni-lib/                       # Shared library crate
│   ├── Cargo.toml
│   └── src/lib.rs
├── local-git-branch-cleanup-tui/   # Git branch cleanup TUI
│   ├── Cargo.toml
│   ├── src/
│   ├── tests/
│   └── README.md                   # App-specific docs
└── target/                         # Shared build output
```

## Available Tools

| Tool | Description | Status |
|------|-------------|--------|
| [local-git-branch-cleanup-tui](local-git-branch-cleanup-tui/) | Interactive TUI for cleaning up local git branches | ✅ Production Ready |

## Development

### Prerequisites

```bash
# Enter development shell (from repo root)
nix develop .#rust-tui
```

### Build All

```bash
cd rust
cargo build
```

### Build Specific Package

```bash
cargo build -p local-git-branch-cleanup-tui
cargo build -p omni-lib
```

### Run Tests

```bash
# All tests
cargo test

# Specific package
cargo test -p local-git-branch-cleanup-tui
```

### Run an App

```bash
cargo run -p local-git-branch-cleanup-tui
```

## Adding a New Tool

1. Create a new directory: `mkdir -p new-tool/src`
2. Add `Cargo.toml` using workspace dependencies:
   ```toml
   [package]
   name = "new-tool"
   version.workspace = true
   edition.workspace = true

   [dependencies]
   omni-lib.workspace = true  # Use shared library
   clap.workspace = true      # Use workspace dependency
   ```
3. Add to workspace `Cargo.toml`:
   ```toml
   [workspace]
   members = [
       "omni-lib",
       "local-git-branch-cleanup-tui",
       "new-tool",  # Add here
   ]
   ```
4. Create `src/main.rs` and start coding!

## Shared Library (omni-lib)

The `omni-lib` crate provides common utilities that can be shared across tools:

- Git operations helpers
- TUI components and themes
- CLI patterns

When you find yourself duplicating code across tools, consider extracting it to `omni-lib`.
