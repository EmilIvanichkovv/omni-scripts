# GitHub PR Cache — Feature Specification

**Status:** 📋 Planned **Created:** 2026-04-26 **Affects:** `git.rs`, `app.rs`, `main.rs`,
`Cargo.toml` **New Files:** `src/cache.rs`

---

## Overview

When `--github` / `-g` is passed, the application calls `gh pr list --head <branch>` once per branch
— sequentially. In large repositories this becomes the dominant startup cost:

```
100 branches × ~1s per gh CLI call = ~100 seconds
```

This specification describes a three-phase improvement:

| Phase | Goal                          | Primary Change          |
| ----- | ----------------------------- | ----------------------- |
| 1     | Eliminate redundant API calls | SQLite cache layer      |
| 2     | Reduce first-run latency      | Parallel `gh` execution |
| 3     | User control and transparency | CLI flags + TUI signals |

Each phase is self-contained and shippable. They must be implemented in order since Phase 2 builds
on Phase 1's cache layer, and Phase 3 surfaces internal state introduced in both.

---

## Background: Current Code Path

```
main.rs::main()
  └─ git::fetch_pr_info_for_branches(&mut branches)      // git.rs:549
       └─ for each branch:
            git::get_pr_info_for_branch(&branch.name)    // git.rs:487
              └─ Command::new("gh").args([...]).output()  // one blocking HTTP call per branch
```

The bottleneck is `get_pr_info_for_branch`: it spawns a new `gh` process per branch, each of which
authenticates and makes a GitHub API request. There is no memoization between runs.

---

## Phase 1 — SQLite Cache Layer

### Goal

On subsequent runs, serve PR data from a local SQLite database instead of calling the GitHub API. A
cached entry is considered valid for a configurable TTL (default: 1 hour). Branches with no cache
entry, or with expired entries, are fetched from the API and their results are stored.

### Why SQLite

- Embedded — no server, no daemon, no network
- ACID-compliant — safe concurrent reads from multiple terminal sessions
- Inspectable — users can query the database with `sqlite3` to debug issues
- Teaches real schema design, indexing, and migration concepts
- Small footprint: `rusqlite` adds ~500 KB to the binary

### New File: `src/cache.rs`

This module owns all cache concerns. No other module should read from or write to the SQLite file
directly.

#### Public Interface

```rust
use crate::git::PrInfo;
use color_eyre::Result;
use std::time::Duration;

/// Statistics about the cache for the current session.
pub struct CacheStats {
    /// Number of branches served from cache this session
    pub hits: usize,
    /// Number of branches that required a fresh API call
    pub misses: usize,
    /// Number of new entries written to the database this session
    pub writes: usize,
    /// Number of entries in the database for this repository
    pub total_entries: usize,
    /// Unix timestamp of the oldest cached entry for this repository
    pub oldest_entry_ts: Option<i64>,
}

pub struct PrCache {
    conn: rusqlite::Connection,
    repo: String,     // "owner/repo" string, used as the partition key
    ttl: Duration,
    stats: CacheStats,
}

impl PrCache {
    /// Open (or create) the cache database. Runs schema migrations automatically.
    /// `repository` must be the canonical "owner/repo" string (see `git::get_repo_slug()`).
    /// `ttl` controls how long a cached entry is considered fresh.
    pub fn open(repository: &str, ttl: Duration) -> Result<Self>;

    /// Return cached PR info if a valid (non-expired) entry exists.
    /// Increments `stats.hits` on success, `stats.misses` on miss or expiry.
    pub fn get(&mut self, branch_name: &str) -> Option<PrInfo>;

    /// Store a PR result (may be `None` — meaning "no PR found") in the database.
    /// Overwrites any existing entry for this (repo, branch) pair.
    /// Increments `stats.writes`.
    pub fn set(&self, branch_name: &str, pr_info: Option<&PrInfo>) -> Result<()>;

    /// Remove all cached entries for this repository that exceed `max_age`.
    /// Call this once on startup to avoid unbounded database growth.
    pub fn evict_stale(&self, max_age: Duration) -> Result<usize>;

    /// Remove the cached entry for a single branch. Used after branch deletion.
    pub fn invalidate(&self, branch_name: &str) -> Result<()>;

    /// Read-only snapshot of session statistics.
    pub fn stats(&self) -> &CacheStats;
}
```

#### Database Location

Resolve in this priority order:

1. `$XDG_CACHE_HOME/omni-scripts/pr-cache.db`
2. `$HOME/.cache/omni-scripts/pr-cache.db`

Use the `dirs` crate (`dirs::cache_dir()`) to avoid manual path construction. Create the parent
directory if it does not exist.

#### Schema

```sql
-- Schema version tracking. Used by the migration runner.
CREATE TABLE IF NOT EXISTS schema_migrations (
    version   INTEGER PRIMARY KEY,
    applied_at INTEGER NOT NULL   -- Unix timestamp
);

-- One row per GitHub repository this tool has been used against.
CREATE TABLE IF NOT EXISTS repositories (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    slug       TEXT    NOT NULL UNIQUE,   -- "owner/repo"
    created_at INTEGER NOT NULL
);

-- One row per (repository, branch) pair.
-- Stores the last known PR state. NULL pr_number means "no PR found".
CREATE TABLE IF NOT EXISTS cached_prs (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    repository_id INTEGER NOT NULL REFERENCES repositories(id) ON DELETE CASCADE,
    branch_name   TEXT    NOT NULL,
    pr_number     INTEGER,          -- NULL if no PR exists
    pr_state      TEXT,             -- 'OPEN' | 'MERGED' | 'CLOSED' | NULL
    pr_title      TEXT,
    pr_url        TEXT,
    cached_at     INTEGER NOT NULL, -- Unix timestamp of when this was written
    UNIQUE(repository_id, branch_name)
);

-- Index to make TTL expiry queries fast.
CREATE INDEX IF NOT EXISTS idx_cached_prs_cached_at ON cached_prs(cached_at);
```

**Design notes for the implementing agent:**

- The `UNIQUE(repository_id, branch_name)` constraint means `INSERT OR REPLACE` is safe to use when
  writing cache entries — no manual upsert logic needed.
- The `ON DELETE CASCADE` on `cached_prs` means deleting a repository row automatically removes all
  its branch entries. Useful for a future "clear cache for this repo" command.
- `pr_number IS NULL` is the canonical representation of "we asked GitHub and found no PR". This
  distinguishes "never queried" (row absent) from "queried, no PR" (row present, `pr_number NULL`).
  Both cases should result in `None` from `PrCache::get()`, but only the latter prevents a redundant
  API call on the next run within TTL.

#### Migration Runner

```rust
const CURRENT_SCHEMA_VERSION: i64 = 1;

fn run_migrations(conn: &Connection) -> Result<()> {
    // Create schema_migrations if it doesn't exist yet (bootstrap case)
    conn.execute_batch(BOOTSTRAP_SQL)?;

    let version: i64 = conn.query_row(
        "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
        [],
        |row| row.get(0),
    )?;

    if version < 1 {
        conn.execute_batch(MIGRATION_V1_SQL)?;
        conn.execute(
            "INSERT INTO schema_migrations (version, applied_at) VALUES (1, ?1)",
            [unix_now()],
        )?;
    }

    // Future: if version < 2 { apply V2 migration ... }
    Ok(())
}
```

This pattern makes adding future migrations safe: apply only the delta, never re-run applied
migrations.

#### `get()` Implementation Logic

```rust
pub fn get(&mut self, branch_name: &str) -> Option<PrInfo> {
    let cutoff = unix_now() - self.ttl.as_secs() as i64;

    // A row must exist AND be within TTL AND belong to this repository.
    let row = self.conn.query_row(
        "SELECT pr_number, pr_state, pr_title, pr_url, cached_at
         FROM cached_prs
         WHERE repository_id = (SELECT id FROM repositories WHERE slug = ?1)
           AND branch_name = ?2
           AND cached_at > ?3",
        [&self.repo, branch_name, &cutoff.to_string()],
        |row| Ok((
            row.get::<_, Option<u64>>(0)?,
            row.get::<_, Option<String>>(1)?,
            row.get::<_, Option<String>>(2)?,
            row.get::<_, Option<String>>(3)?,
        )),
    ).ok();

    match row {
        Some((Some(number), Some(state_str), Some(title), Some(url))) => {
            let state = match state_str.as_str() {
                "OPEN"   => PrState::Open,
                "MERGED" => PrState::Merged,
                "CLOSED" => PrState::Closed,
                _        => { self.stats.misses += 1; return None; }
            };
            self.stats.hits += 1;
            Some(PrInfo { number, state, title, url })
        }
        Some((None, _, _, _)) => {
            // Cached "no PR" result — still a hit, don't call API
            self.stats.hits += 1;
            None
        }
        None => {
            self.stats.misses += 1;
            None
        }
    }
}
```

### Changes to `git.rs`

#### New function: `get_repo_slug()`

```rust
/// Returns the GitHub "owner/repo" slug derived from the remote URL.
/// Supports both HTTPS (https://github.com/owner/repo.git)
/// and SSH (git@github.com:owner/repo.git) remote formats.
/// Falls back to the repository's root directory basename if parsing fails.
pub fn get_repo_slug() -> Result<String>
```

Call `git remote get-url origin`, then parse the URL. Strip `.git` suffix if present.

#### Modified function: `fetch_pr_info_for_branches()`

Old signature:

```rust
pub fn fetch_pr_info_for_branches(branches: &mut [BranchInfo])
```

New signature:

```rust
pub fn fetch_pr_info_for_branches(branches: &mut [BranchInfo], cache: &mut PrCache)
```

New logic:

```
for each branch:
    if let Some(pr_info) = cache.get(&branch.name):
        branch.pr_info = Some(pr_info)   // cache hit — no API call
    else:
        if let Some(pr_info) = get_pr_info_for_branch(&branch.name):
            cache.set(&branch.name, Some(&pr_info))
            branch.pr_info = Some(pr_info)
        else:
            cache.set(&branch.name, None)  // cache "no PR" to avoid future misses
```

### Changes to `main.rs`

```rust
// In the github-enabled block:
if args.github {
    if git::is_gh_cli_available() {
        let repo_slug = git::get_repo_slug().unwrap_or_else(|_| "unknown/unknown".to_string());
        let ttl = Duration::from_secs(3600); // 1 hour default

        match cache::PrCache::open(&repo_slug, ttl) {
            Ok(mut pr_cache) => {
                pr_cache.evict_stale(Duration::from_secs(30 * 24 * 60 * 60)).ok();
                eprintln!("🔗 Fetching GitHub PR info...");
                git::fetch_pr_info_for_branches(&mut branches, &mut pr_cache);
                let stats = pr_cache.stats();
                if stats.hits > 0 {
                    eprintln!(
                        "   {} from cache, {} fetched from GitHub",
                        stats.hits, stats.misses
                    );
                }
                github_enabled = true;
            }
            Err(e) => {
                eprintln!("⚠️  PR cache unavailable ({}), fetching live data.", e);
                git::fetch_pr_info_for_branches_no_cache(&mut branches);
                github_enabled = true;
            }
        }
    } else {
        eprintln!("⚠️  GitHub CLI (gh) not found. Install it to enable PR integration.");
        eprintln!("   See: https://cli.github.com/");
    }
}
```

The `_no_cache` fallback ensures the flag keeps working even if the cache file cannot be opened
(e.g., disk full, permission error).

### Changes to `Cargo.toml`

```toml
[dependencies]
rusqlite = { version = "0.31", features = ["bundled"] }
dirs     = "5.0"
```

Use the `bundled` feature so `libsqlite3` does not need to be present on the host system.

### Acceptance Criteria for Phase 1

- [ ] First run with `--github` behaves identically to the current implementation
- [ ] Second run with `--github` (within TTL) makes zero `gh` subprocess calls
- [ ] `pr-cache.db` is created at the XDG cache path on first run
- [ ] Expired entries (older than TTL) are re-fetched and overwritten
- [ ] "No PR" results are cached and don't trigger a re-fetch within TTL
- [ ] Deleting a branch via the TUI calls `cache.invalidate()` for that branch
- [ ] A corrupted or missing cache file falls back gracefully (no panic)
- [ ] `cargo test` passes — unit tests cover `get()`, `set()`, `evict_stale()`, `invalidate()`

---

## Phase 2 — Parallel `gh` Execution

### Goal

Reduce first-run (cold cache) latency by fetching PR data for multiple branches concurrently.

### Prerequisite

Phase 1 must be complete. Parallelism applies only to the branches that are cache misses; hits are
served synchronously from the database in Phase 1's loop.

### Approach: `rayon` Thread Pool

`rayon` is the idiomatic Rust choice for CPU-bound and I/O-bound parallel iterators. It requires no
async runtime (no `tokio`, no `async/await` changes to the existing synchronous codebase).

```toml
[dependencies]
rayon = "1.10"
```

### Revised `fetch_pr_info_for_branches()` Logic

```rust
use rayon::prelude::*;

pub fn fetch_pr_info_for_branches(branches: &mut [BranchInfo], cache: &mut PrCache) {
    // --- Pass 1: Serve cache hits synchronously ---
    // Collect indices of branches that still need an API call.
    let mut miss_indices: Vec<usize> = Vec::new();

    for (i, branch) in branches.iter_mut().enumerate() {
        if let Some(pr_info) = cache.get(&branch.name) {
            branch.pr_info = Some(pr_info);
        } else {
            miss_indices.push(i);
        }
    }

    if miss_indices.is_empty() {
        return;
    }

    // --- Pass 2: Fetch misses in parallel ---
    // Collect branch names to avoid borrow conflicts.
    let names: Vec<String> = miss_indices
        .iter()
        .map(|&i| branches[i].name.clone())
        .collect();

    // Parallel fetch: returns Vec<Option<PrInfo>> aligned with `miss_indices`.
    let results: Vec<Option<PrInfo>> = names
        .par_iter()
        .map(|name| get_pr_info_for_branch(name))
        .collect();

    // --- Pass 3: Write results back to branches and cache ---
    for (&idx, result) in miss_indices.iter().zip(results.iter()) {
        branches[idx].pr_info = result.clone();
        cache.set(&branches[idx].name, result.as_ref()).ok();
    }
}
```

### Concurrency Limit

GitHub's authenticated API rate limit is 5 000 requests/hour (~83/minute). With `rayon`'s default
thread pool (number of logical CPUs), a machine with 16 cores would make 16 concurrent requests.
This is safe in practice, but to be explicit and configurable:

```rust
// Limit parallel workers to avoid overwhelming the GitHub API or the local system.
// Default of 8 is a reasonable balance for most machines.
const MAX_PARALLEL_WORKERS: usize = 8;

rayon::ThreadPoolBuilder::new()
    .num_threads(MAX_PARALLEL_WORKERS)
    .build_global()
    .ok(); // Ignore error if global pool already initialized
```

Call `ThreadPoolBuilder` once in `main()` before spawning anything, not inside
`fetch_pr_info_for_branches()`.

### Progress Reporting

With parallel execution the "🔗 Fetching GitHub PR info..." message is no longer informative because
the work completes in a burst rather than trickling in. Replace it with a before/after summary
already outlined in Phase 1:

```
🔗 Fetching GitHub PR info...
   42 from cache, 8 fetched from GitHub
```

If _all_ branches are misses (truly cold run) and the repo has > 20 branches, print an additional
hint:

```
   (tip: subsequent runs will be instant — results are cached for 1h)
```

### Expected Latency Improvement

Assuming ~1 s per `gh` call and 8 parallel workers:

| Branches | Phase 0 (current) | Phase 1 (cache warm) | Phase 2 (parallel, cold) |
| -------- | ----------------- | -------------------- | ------------------------ |
| 10       | ~10s              | ~0.1s                | ~2s                      |
| 50       | ~50s              | ~0.1s                | ~7s                      |
| 100      | ~100s             | ~0.1s                | ~13s                     |

### Acceptance Criteria for Phase 2

- [ ] `fetch_pr_info_for_branches()` with 0 cache entries takes roughly `ceil(N / 8)` seconds for N
      branches, not N seconds
- [ ] Cache hits are still served without any parallelism overhead
- [ ] Results are identical to sequential execution (order does not matter for correctness)
- [ ] No data races: `PrCache::set()` is safe to call from multiple threads (use `Mutex<PrCache>` or
      move writes to pass 3 as shown above)
- [ ] Rate-limit errors from `gh` (exit code non-zero) are handled gracefully — the branch simply
      gets no PR info, and nothing is cached for that branch
- [ ] `cargo test` passes

---

## Phase 3 — User Control and Transparency

### Goal

Expose cache behaviour to the user via CLI flags and TUI indicators. Users should be able to
understand cache state at a glance and override it when needed.

### New CLI Flags

Add to the `Args` struct in `main.rs`:

```rust
/// Bypass the PR cache and re-fetch all PR data from GitHub.
/// The refreshed data is written back to the cache.
#[arg(long)]
refresh_cache: bool,

/// Print PR cache statistics for this repository and exit.
#[arg(long)]
cache_stats: bool,

/// Override the cache TTL in seconds (default: 3600).
/// Use 0 to disable caching entirely for this run.
#[arg(long, default_value = "3600")]
cache_ttl: u64,
```

**`--refresh-cache` behaviour:**

Pass a `force_refresh: bool` parameter through to `fetch_pr_info_for_branches()`. When `true`, skip
`cache.get()` in pass 1 entirely — every branch is treated as a miss and fetched fresh. The results
are still written to the cache normally so the next run benefits from them.

**`--cache-stats` behaviour:**

Open the cache, print statistics, then `std::process::exit(0)`. Do not initialize the TUI.

Example output:

```
PR Cache — owner/repo
─────────────────────────────────────
Location : ~/.cache/omni-scripts/pr-cache.db
Entries  : 87 branches cached
Oldest   : 2026-04-26 10:14 (13h ago)
TTL      : 3600s (entries older than this are re-fetched)
```

**`--cache-ttl 0` behaviour:**

When TTL is zero, `PrCache::get()` always returns `None` (treat everything as expired). Results are
still written so a non-zero TTL run later can benefit from them.

### TUI Changes

#### Header: Cache Status Indicator

When GitHub integration is active, add a status badge to the header row that already shows the repo
path and trunk. Use a compact format:

```
 PR Cache: 42 cached · 8 live · last sync 4m ago
```

Three states:

- **All cached** (0 misses): `🗄  PR data from cache (4m ago)`
- **Mixed**: `🔄  42 cached · 8 fetched`
- **All live** (0 hits, cold run): `🌐  PR data fetched live`

Store the `CacheStats` in `App` (new field `pub cache_stats: Option<CacheStats>`) and read it in
`ui.rs`.

#### Details Pane: PR Source Label

In the details pane, next to the PR info block, show whether the data came from cache or live:

```
 PR #42  🟢 merged  [cached]
 "Fix the bug that affected login"
```

To support this, add a field to `BranchInfo`:

```rust
pub struct BranchInfo {
    // ... existing fields ...
    /// True if pr_info was served from cache rather than a live API call.
    pub pr_info_from_cache: bool,
}
```

Set this field to `true` in pass 1 of `fetch_pr_info_for_branches()` (cache hit), `false` in pass 3
(live fetch).

#### Keybinding: Force Refresh

Add a new keybinding visible in the footer when GitHub integration is active:

| Key      | Action                                             |
| -------- | -------------------------------------------------- |
| `Ctrl+R` | Re-fetch PR data for all branches and update cache |

This triggers `fetch_pr_info_for_branches()` with `force_refresh: true` and re-renders. The fetching
happens on the main thread (same as current behaviour) — a loading spinner or progress message is
not in scope for this phase.

### Changes to `app.rs`

```rust
pub struct App {
    // ... existing fields ...
    pub github_enabled: bool,
    pub cache_stats: Option<cache::CacheStats>,
    // Keep a reference or owned PrCache for on-demand refresh:
    pub pr_cache: Option<cache::PrCache>,
}
```

Add method:

```rust
impl App {
    /// Re-fetch all PR info, bypassing cache.
    pub fn refresh_pr_data(&mut self) {
        if let Some(ref mut pr_cache) = self.pr_cache {
            git::fetch_pr_info_for_branches(&mut self.branches, pr_cache, true);
            self.cache_stats = Some(pr_cache.stats().clone());
        }
    }
}
```

### Acceptance Criteria for Phase 3

- [ ] `--refresh-cache` causes all entries to be re-fetched even if valid cache entries exist
- [ ] `--cache-stats` prints statistics and exits without launching the TUI
- [ ] `--cache-ttl 0` disables cache reads for that run (still writes)
- [ ] Header shows cache status indicator when `--github` is active
- [ ] Details pane shows `[cached]` or `[live]` label next to PR info
- [ ] `Ctrl+R` in TUI triggers a full PR data refresh
- [ ] `cargo test` passes

---

## Cross-Cutting Concerns

### Error Handling

- All `rusqlite` calls should propagate errors via `color_eyre::Result`
- Cache failures must never panic or crash the main application flow
- The `PrCache::open()` failure path in `main.rs` (shown in Phase 1) is the safety net
- Log cache errors to `stderr` with `eprintln!` using `⚠️` prefix, matching existing style

### Testing

Each phase should add tests to the relevant module. Use an in-memory SQLite database for cache unit
tests to avoid filesystem side effects:

```rust
#[cfg(test)]
fn open_in_memory_cache(ttl: Duration) -> PrCache {
    PrCache::open_with_conn(
        rusqlite::Connection::open_in_memory().unwrap(),
        "test/repo",
        ttl,
    )
    .unwrap()
}
```

This requires splitting `PrCache::open()` into a private `open_with_conn(conn, repo, ttl)` that
accepts an existing connection, and a public `open(repo, ttl)` that creates the on-disk connection.

Key test cases:

```rust
// Phase 1
#[test] fn cache_miss_returns_none()
#[test] fn cache_hit_returns_pr_info()
#[test] fn cached_no_pr_does_not_re_query()
#[test] fn expired_entry_treated_as_miss()
#[test] fn evict_stale_removes_old_entries()
#[test] fn invalidate_removes_single_entry()
#[test] fn schema_migration_runs_on_fresh_db()
#[test] fn schema_migration_is_idempotent()

// Phase 2
#[test] fn parallel_fetch_results_match_sequential()
#[test] fn mixed_hit_miss_only_fetches_misses()

// Phase 3
#[test] fn force_refresh_bypasses_valid_cache()
#[test] fn cache_ttl_zero_disables_reads()
#[test] fn cache_stats_counts_are_correct()
```

### Documentation

Update `ARCHITECTURE.md` after Phase 1 is complete to:

- Add `cache.rs` to the module diagram
- Document `PrCache` in the "Key Types" section
- Update the Data Flow diagram to show the cache check before the `gh` subprocess call
- Add `rusqlite` and `dirs` to the Dependencies table

---

## File Change Summary

| File                         | Phase | Change                                                       |
| ---------------------------- | ----- | ------------------------------------------------------------ |
| `src/cache.rs`               | 1     | New file — entire module                                     |
| `src/git.rs`                 | 1     | Add `get_repo_slug()`, modify `fetch_pr_info_for_branches()` |
| `src/main.rs`                | 1     | Initialize `PrCache`, wire into GitHub block                 |
| `Cargo.toml`                 | 1     | Add `rusqlite` (bundled), `dirs`                             |
| `src/git.rs`                 | 2     | Parallel execution in `fetch_pr_info_for_branches()`         |
| `Cargo.toml`                 | 2     | Add `rayon`                                                  |
| `src/main.rs`                | 3     | Add `--refresh-cache`, `--cache-stats`, `--cache-ttl` flags  |
| `src/app.rs`                 | 3     | Add `cache_stats`, `pr_cache`, `refresh_pr_data()` to `App`  |
| `src/ui.rs`                  | 3     | Header badge, details pane label, `Ctrl+R` keybinding        |
| `src/git.rs`                 | 3     | Add `force_refresh` param to `fetch_pr_info_for_branches()`  |
| `docs/specs/ARCHITECTURE.md` | 3     | Update module diagram, types, dependencies table             |
