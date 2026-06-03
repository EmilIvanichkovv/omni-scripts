#!/usr/bin/env bash
# bench-pr-fetch.sh — Benchmark sequential vs parallel PR fetching
#
# Usage:
#   ./bench-pr-fetch.sh <repo-path>
#
# Example:
#   ./bench-pr-fetch.sh ~/code/repos/metacraft-labs/blocksense/monorepo
#
# What it does:
#   1. Builds a release binary of local-git-branch-cleanup-tui
#   2. Clears the PR cache
#   3. Times a sequential (pre-Phase-2) cold run
#   4. Clears the cache again
#   5. Times a parallel (Phase-2) cold run
#   6. Times a warm-cache run (no cache clear)
#   7. Prints a summary table

set -euo pipefail

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

red()    { printf '\033[31m%s\033[0m\n' "$*"; }
green()  { printf '\033[32m%s\033[0m\n' "$*"; }
bold()   { printf '\033[1m%s\033[0m\n'  "$*"; }
header() { echo; bold "==> $*"; }

die() { red "ERROR: $*" >&2; exit 1; }

# ---------------------------------------------------------------------------
# Args
# ---------------------------------------------------------------------------

REPO_PATH="${1:-}"
if [[ -z "$REPO_PATH" ]]; then
    die "Usage: $0 <git-repo-path>"
fi

REPO_PATH="$(realpath "$REPO_PATH")"
[[ -d "$REPO_PATH/.git" ]] || die "'$REPO_PATH' is not a git repository"

# ---------------------------------------------------------------------------
# Locate / build binary
# ---------------------------------------------------------------------------

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORKSPACE_DIR="$(realpath "$SCRIPT_DIR/../../..")"   # rust/ parent
RUST_DIR="$WORKSPACE_DIR/rust"
BIN="$RUST_DIR/target/release/local-git-branch-cleanup-tui"

header "Building release binary"
(cd "$RUST_DIR" && cargo build --release -p local-git-branch-cleanup-tui)
echo "Binary: $BIN"

# ---------------------------------------------------------------------------
# Locate cache DB
# ---------------------------------------------------------------------------

DB="${XDG_CACHE_HOME:-$HOME/.cache}/omni-scripts/pr-cache.db"

SQLITE=""
for candidate in sqlite3 \
    /nix/store/*/bin/sqlite3; do
    if command -v "$candidate" &>/dev/null 2>&1; then
        SQLITE="$candidate"
        break
    fi
done
# Fallback: find via nix store glob (already expanded above, but handle no match)
if [[ -z "$SQLITE" ]]; then
    # Try nix shell
    SQLITE="nix shell nixpkgs#sqlite --command sqlite3"
fi

clear_cache() {
    if [[ -f "$DB" ]]; then
        $SQLITE "$DB" "DELETE FROM cached_prs;"
        echo "Cache cleared ($DB)"
    else
        echo "No cache DB found yet — will be created on first run"
    fi
}

# ---------------------------------------------------------------------------
# Count branches
# ---------------------------------------------------------------------------

BRANCH_COUNT=$(cd "$REPO_PATH" && git branch | wc -l | tr -d ' ')

# ---------------------------------------------------------------------------
# Run helper: prints output to terminal, writes fetch time to a temp file
# ---------------------------------------------------------------------------

TMPFILE=$(mktemp)
trap 'rm -f "$TMPFILE"' EXIT

run_bench() {
    local label="$1"; shift          # e.g. "Sequential"
    local extra_flags=("$@")         # e.g. --sequential

    echo
    echo "--- $label ---"
    local output
    output=$(cd "$REPO_PATH" && "$BIN" --github --fetch-only "${extra_flags[@]}" 2>&1)
    echo "$output"

    # Extract "fetch completed in Xs" — write to temp file so it survives subshell
    grep -oP 'fetch completed in \K[0-9.]+' <<<"$output" > "$TMPFILE"
}

# ---------------------------------------------------------------------------
# Benchmark runs
# ---------------------------------------------------------------------------

header "Repository: $REPO_PATH"
echo "Branches:   $BRANCH_COUNT"
echo "Cache DB:   $DB"

# Run 1: sequential, cold cache
header "Run 1 — Sequential (before), cold cache"
clear_cache
run_bench "Sequential" --sequential
SEQ_TIME=$(cat "$TMPFILE")

# Run 2: parallel, cold cache
header "Run 2 — Parallel / rayon (after), cold cache"
clear_cache
run_bench "Parallel"
PAR_TIME=$(cat "$TMPFILE")

# Run 3: parallel, warm cache (no clear)
header "Run 3 — Parallel, warm cache"
run_bench "Warm cache"
WARM_TIME=$(cat "$TMPFILE")

# ---------------------------------------------------------------------------
# Summary
# ---------------------------------------------------------------------------

header "Summary"

# Compute speedup (integer arithmetic via awk)
if [[ -n "$SEQ_TIME" && -n "$PAR_TIME" && "$PAR_TIME" != "0.00" ]]; then
    SPEEDUP=$(awk "BEGIN { printf \"%.1f\", $SEQ_TIME / $PAR_TIME }")
else
    SPEEDUP="N/A"
fi

printf '\n'
printf '%-30s %12s %12s %12s\n' "Run"              "Fetch time" "Branches" "Speedup vs seq"
printf '%-30s %12s %12s %12s\n' "---"              "----------" "--------" "--------------"
printf '%-30s %11ss %12s %12s\n' "Sequential (cold)" "$SEQ_TIME"  "$BRANCH_COUNT"  "1.0×"
printf '%-30s %11ss %12s %12s\n' "Parallel   (cold)" "$PAR_TIME"  "$BRANCH_COUNT"  "${SPEEDUP}×"
printf '%-30s %11ss %12s %12s\n' "Parallel   (warm)" "$WARM_TIME" "$BRANCH_COUNT"  "—"
printf '\n'

green "Done. Parallel is ${SPEEDUP}× faster than sequential on a cold cache."
