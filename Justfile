# Default recipe - show available commands
default:
    @just --list

# Format and lint everything (main workflow)
format-and-lint:
    pre-commit run --all-files

# Build Rust project
build:
    cd rust && cargo build

# Run Rust tests
test-rust:
    cd rust && cargo test

# Quick validation (format check only, no modifications)
check:
    cd rust && cargo fmt --all -- --check
    prettier --check --prose-wrap always --print-width 100 "**/*.md"

# Inspect the PR cache database with sqlite3
cache-db:
    sqlite3 ~/.cache/omni-scripts/pr-cache.db

# Show all cached PR entries for the current repo
cache-show:
    sqlite3 -column -header ~/.cache/omni-scripts/pr-cache.db \
      "SELECT r.slug, c.branch_name, c.pr_number, c.pr_state, \
              datetime(c.cached_at, 'unixepoch', 'localtime') AS cached_at \
       FROM cached_prs c JOIN repositories r ON r.id = c.repository_id \
       ORDER BY r.slug, c.cached_at DESC;"

# Clear all cached PR entries (keeps the database file)
cache-clear:
    sqlite3 ~/.cache/omni-scripts/pr-cache.db "DELETE FROM cached_prs;"
    @echo "Cache cleared."
