# OmniScripts

A versatile collection of scripts written in various programming languages, including Bash, Rust, D, Nim, and more.

## Featured Tools

### 🧹 Local Git Branch Cleanup TUI

An interactive terminal user interface for cleaning up local Git branches that no longer have remote counterparts.

**Location:** [`rust/`](rust/)  
**Status:** ✅ Production Ready (v0.2.0)

**Key Features:**
- 🎯 Interactive TUI with keyboard navigation
- 🛡️ Smart branch classification (merged, gone, unmerged, protected)
- ⚡ Safe delete by default (protects unmerged work)
- 🎨 Color-coded status indicators
- 📊 Details pane with commit information
- 🔍 Dry run mode for previewing deletions

**Quick Start:**
```bash
# Enter development environment
nix develop .#rust-tui

# Build and run
cd rust
cargo build --release
./target/release/local-git-branch-cleanup-tui
```

**Documentation:**
- [User Guide](rust/README.md) - Installation, usage, and TUI guide
- [Architecture](rust/ARCHITECTURE.md) - Technical documentation for developers
- [Migration Guide](rust/MIGRATION.md) - Migrating from the bash script
- [Testing Guide](rust/TESTING.md) - Comprehensive testing checklist

**Replaces:** [`bash/local-git-branch-cleanup.sh`](bash/local-git-branch-cleanup.sh) (legacy version still available)

---

## Additional Scripts

See individual directories for more tools and utilities.
