# OmniScripts

A versatile collection of scripts written in various programming languages, including Bash, Rust, D,
Nim, and more.

## Featured Tools

### 🧹 Local Git Branch Cleanup TUI

An interactive terminal user interface for cleaning up local Git branches that no longer have remote
counterparts.

**Location:** [`rust/local-git-branch-cleanup-tui/`](rust/local-git-branch-cleanup-tui/) **Status:**
✅ Production Ready (v0.2.0)

**Key Features:**

- 🎯 Interactive TUI with keyboard navigation
- 🛡️ Smart branch classification (merged, gone, unmerged, protected)
- ⚡ Safe delete by default (protects unmerged work)
- 🎨 Color-coded status indicators
- 📊 Details pane with commit information
- 🔍 Dry run mode for previewing deletions

**Quick Start (Nix):**

```bash
# Run directly with Nix (recommended)
nix run .#local-git-branch-cleanup-tui

# Or run the bash script version
nix run .#local-git-branch-cleanup
```

**Quick Start (Cargo):**

```bash
nix develop  # Enter dev environment with Rust toolchain
cd rust
cargo run -p local-git-branch-cleanup-tui
```

**Documentation:**

- [User Guide](rust/local-git-branch-cleanup-tui/README.md) - Installation, usage, and TUI guide
- [Architecture](rust/local-git-branch-cleanup-tui/specs/ARCHITECTURE.md) - Technical documentation
  for developers
- [Migration Guide](rust/local-git-branch-cleanup-tui/specs/MIGRATION.md) - Migrating from the bash
  script
- [Testing Guide](rust/local-git-branch-cleanup-tui/specs/TESTING.md) - Comprehensive testing
  checklist

**Also available:** [`bash/local-git-branch-cleanup.sh`](bash/local-git-branch-cleanup.sh) via
`nix run .#local-git-branch-cleanup`

---

## Additional Scripts

See individual directories for more tools and utilities.
