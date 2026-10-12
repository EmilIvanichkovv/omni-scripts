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

# Check Rust dependencies against the RustSec advisory database
audit:
    cd rust && cargo audit

# Quick validation (format check only, no modifications)
check:
    cd rust && cargo fmt --all -- --check
    prettier --check --prose-wrap always --print-width 100 "**/*.md"

# Inspect the PR cache database with sqlite3
cache-db:
    sqlite3 "${XDG_CACHE_HOME:-$HOME/.cache}/omni-scripts/pr-cache.db"

# Show all cached PR entries for the current repo
cache-show:
    sqlite3 -column -header "${XDG_CACHE_HOME:-$HOME/.cache}/omni-scripts/pr-cache.db" \
      "SELECT r.slug, c.branch_name, c.pr_number, c.pr_state, \
              datetime(c.cached_at, 'unixepoch', 'localtime') AS cached_at \
       FROM cached_prs c JOIN repositories r ON r.id = c.repository_id \
       ORDER BY r.slug, c.cached_at DESC;"

# Clear all cached PR entries (keeps the database file)
cache-clear:
    sqlite3 "${XDG_CACHE_HOME:-$HOME/.cache}/omni-scripts/pr-cache.db" "DELETE FROM cached_prs;"
    @echo "Cache cleared."

# --- Releases (rust/local-git-branch-cleanup-tui/docs/specs/RELEASE_DISTRIBUTION.md) ---

app := "local-git-branch-cleanup-tui"

# Show the next release version, its changelog entry and what dist would build (no side effects)
release-plan bump="auto":
    scripts/release/next-version.sh --dry-run --bump {{ bump }}
    dist plan

# Run the release workflow on GitHub as a dry run (decides the version, pushes nothing)
release-dry-run bump="auto" ref=`git branch --show-current`:
    gh workflow run auto-release.yml --ref {{ ref }} -f bump={{ bump }} -f dry-run=true

# Publish a prerelease X.Y.Z-<label>.N from a pushed branch; no branch is changed
release-pre label="rc" ref=`git branch --show-current`:
    #!/usr/bin/env bash
    set -euo pipefail
    git fetch -q origin "{{ ref }}"
    if [ "$(git rev-parse "{{ ref }}")" != "$(git rev-parse "origin/{{ ref }}")" ]; then
        echo "{{ ref }} differs from origin/{{ ref }}; push it first (the release builds origin)" >&2
        exit 1
    fi
    gh workflow run auto-release.yml --ref "{{ ref }}" -f prerelease="{{ label }}"
    echo "Started. Follow with: just release-status"

# Delete a prerelease and its tag, e.g. `just release-pre-delete 0.3.0-rc.1`
release-pre-delete version:
    #!/usr/bin/env bash
    set -euo pipefail
    if [[ "{{ version }}" != *-* ]]; then
        echo "refusing: {{ version }} is a stable release, and those are never deleted" >&2
        exit 1
    fi
    tag="{{ app }}-v{{ version }}"
    gh release delete "$tag" --cleanup-tag --yes || git push origin --delete "refs/tags/$tag"

# Cut a stable release from main now (bump: auto, patch, minor or major)
release bump="auto":
    gh workflow run auto-release.yml --ref main -f bump={{ bump }}

# Show the latest release workflow runs and release tags
release-status:
    gh run list --workflow auto-release.yml --limit 3
    gh run list --workflow release.yml --limit 3
    git ls-remote --tags origin '{{ app }}-v*' | sed -n 's|.*refs/tags/\([^^]*\)$|\1|p' | sort -V | tail -n 5
