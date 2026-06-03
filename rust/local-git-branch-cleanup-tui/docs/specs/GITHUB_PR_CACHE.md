# GitHub PR Cache — Feature Specification

**Status:** ✅ Phase 2.0 Complete · 🔬 Phase 2.1 Proposed **Created:** 2026-04-26 **Updated:**
2026-06-03 **Affects:** `git.rs`, `app.rs`, `main.rs`, `Cargo.toml` **New Files:** `src/cache.rs`,
`scripts/bench-pr-fetch.sh` **New Docs:** `docs/BENCHMARK_PR_FETCH.md`,
`docs/specs/PR_FETCH_BEHAVIOUR.md`

---

## Overview

When `--github` / `-g` is passed, the application calls `gh pr list --head <branch>` once per branch
— sequentially. In large repositories this becomes the dominant startup cost:

```
100 branches × ~1s per gh CLI call = ~100 seconds
```

This specification describes a three-phase improvement:

| Phase | Goal                          | Primary Change                           |
| ----- | ----------------------------- | ---------------------------------------- |
| 1     | Eliminate redundant API calls | SQLite cache layer                       |
| 2.0   | Reduce first-run latency      | Parallel `gh` execution via `rayon`      |
| 2.1   | Better I/O concurrency        | Replace `rayon` with `tokio` async tasks |
| 3     | User control and transparency | CLI flags + TUI signals                  |

Each phase is self-contained and shippable. They must be implemented in order since Phase 2.x builds
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

- [x] First run with `--github` behaves identically to the current implementation
- [x] Second run with `--github` (within TTL) makes zero `gh` subprocess calls
- [x] `pr-cache.db` is created at the XDG cache path on first run
- [x] Expired entries (older than TTL) are re-fetched and overwritten
- [x] "No PR" results are cached and don't trigger a re-fetch within TTL
- [ ] Deleting a branch via the TUI calls `cache.invalidate()` for that branch _(Phase 3)_
- [x] A corrupted or missing cache file falls back gracefully (no panic)
- [x] `cargo test` passes — unit tests cover `get()`, `set()`, `evict_stale()`, `invalidate()`

---

## Phase 2.0 — Parallel `gh` Execution (rayon)

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

/// `sequential = true` disables rayon and forces one-at-a-time execution.
/// Used by the hidden `--sequential` CLI flag for benchmarking only.
pub fn fetch_pr_info_for_branches(
    branches: &mut [BranchInfo],
    cache: Option<&mut PrCache>,
    sequential: bool,
) {
    match cache {
        Some(pr_cache) => {
            // --- Pass 1: Serve cache hits synchronously ---
            let mut miss_indices: Vec<usize> = Vec::new();

            for (i, branch) in branches.iter_mut().enumerate() {
                match pr_cache.get(&branch.name) {
                    CacheResult::Hit(pr_info) => { branch.pr_info = pr_info; }
                    CacheResult::Miss         => { miss_indices.push(i); }
                }
            }

            if miss_indices.is_empty() { return; }

            // --- Pass 2: Fetch misses (parallel or sequential) ---
            let names: Vec<String> = miss_indices
                .iter()
                .map(|&i| branches[i].name.clone())
                .collect();

            let results: Vec<Option<PrInfo>> = if sequential {
                names.iter().map(|n| get_pr_info_for_branch(n)).collect()
            } else {
                names.par_iter().map(|n| get_pr_info_for_branch(n)).collect()
            };

            // --- Pass 3: Write results back to branches and cache ---
            for (&idx, result) in miss_indices.iter().zip(results.iter()) {
                branches[idx].pr_info = result.clone();
                let _ = pr_cache.set(&branches[idx].name, result.as_ref());
            }
        }
        None => {
            // No cache — fetch all branches (parallel or sequential).
            let names: Vec<String> = branches.iter().map(|b| b.name.clone()).collect();
            let results: Vec<Option<PrInfo>> = if sequential {
                names.iter().map(|n| get_pr_info_for_branch(n)).collect()
            } else {
                names.par_iter().map(|n| get_pr_info_for_branch(n)).collect()
            };
            for (branch, result) in branches.iter_mut().zip(results) {
                branch.pr_info = result;
            }
        }
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

### Measured Latency Improvement

Benchmarked against `metacraft-labs/blocksense` monorepo (81 branches, cold cache). Full results in
`docs/BENCHMARK_PR_FETCH.md`. Average `gh` round-trip for this repo: ~0.71 s.

| Run                      | Mode                 | Branches | Fetch time | Speedup    |
| ------------------------ | -------------------- | -------- | ---------- | ---------- |
| Sequential (pre-Phase-2) | `--sequential`       | 81       | 57–61 s    | 1×         |
| rayon (Phase-2.0, cold)  | rayon ≤ 8 workers    | 81       | ~6.8 s     | **~8.4×**  |
| tokio (Phase-2.1, cold)  | default              | 81       | **3.46 s** | **~17.6×** |
| tokio (Phase-2.1, warm)  | default + full cache | 81       | < 0.01 s   | —          |

With 8 workers and ~0.75 s/call, the theoretical floor is ⌈81/8⌉ × 0.75 ≈ 8.4 s. The measured ~6.8 s
beats this because faster calls keep all 8 slots busy throughout. See Phase 2.1 for the tokio
replacement which achieves ~3.5 s with a semaphore of 20.

### Acceptance Criteria for Phase 2.0

- [x] `fetch_pr_info_for_branches()` with 0 cache entries takes roughly `ceil(N / 8)` seconds for N
      branches, not N seconds _(measured: ~6.7 s for 81 branches; floor ≈ 7.8 s)_
- [x] Cache hits are still served without any parallelism overhead
- [x] Results are identical to sequential execution (order does not matter for correctness)
- [x] No data races: all SQLite writes happen in pass 3 on the main thread; rayon tasks receive only
      `String` clones and spawn independent `gh` subprocesses
- [x] Rate-limit errors from `gh` (exit code non-zero) are handled gracefully —
      `get_pr_info_for_branch()` returns `None`, the branch gets no PR info, and nothing is written
      to the cache
- [x] `cargo test` passes
- [x] Nix `cargoHash` in `tui.nix` updated to account for `rayon` in `Cargo.lock`
- [x] Hidden `--sequential` flag added for benchmarking (restores pre-Phase-2 behaviour on demand)
- [x] Hidden `--fetch-only` flag added; exits after fetch with timing stats (used by benchmark
      script)
- [x] `scripts/bench-pr-fetch.sh` — automated benchmark script comparing sequential vs parallel
- [x] `docs/BENCHMARK_PR_FETCH.md` — recorded benchmark results

---

## Phase 2.1 — Async `gh` Execution (tokio)

### Goal

Replace the `rayon` thread pool with `tokio` async tasks. Each `gh` subprocess call is pure I/O — it
spawns a process, sends an HTTP request to GitHub, and reads stdout. Blocking an OS thread while
waiting for that I/O is wasteful. `tokio` multiplexes many concurrent tasks on a small number of OS
threads via the kernel's async I/O mechanisms (epoll / io_uring on Linux).

### Why tokio Is a Better Fit Than rayon Here

| Property               | `rayon` (Phase 2.0)                     | `tokio` (Phase 2.1)                           |
| ---------------------- | --------------------------------------- | --------------------------------------------- |
| Concurrency model      | Thread pool (8 OS threads, all blocked) | Async tasks on a small thread pool            |
| OS threads in use      | 8 (one per in-flight call)              | 2–4 (tokio default; tasks yield while idle)   |
| In-flight calls        | Capped at 8 (worker count)              | Capped by `Semaphore` (tunable, e.g. 20–50)   |
| Blocking while waiting | Yes — thread blocks on child stdout     | No — task yields; thread services other tasks |
| `async/await` changes  | None required                           | `get_pr_info_for_branch` becomes `async fn`   |
| Dependency             | `rayon = "1.10"`                        | `tokio` (likely already a transitive dep)     |

**Key insight:** with `rayon` capped at 8 workers, 81 branches require ⌈81/8⌉ = 11 rounds even if
every call returns instantly. With `tokio` + a semaphore of 20, ⌈81/20⌉ = 5 rounds, and threads are
freed between rounds to handle other work.

### Prerequisite

Phase 2.0 must be complete. Phase 2.1 is a drop-in replacement for the parallelism layer only; the
3-pass algorithm and `sequential: bool` flag are retained.

### New Dependency

```toml
[dependencies]
tokio = { version = "1", features = ["rt-multi-thread", "process", "macros"] }
```

Remove `rayon = "1.10"` from both workspace and crate `Cargo.toml`.

### Implementation Plan

#### Step 1 — Convert `get_pr_info_for_branch()` to async

```rust
// Before (git.rs)
fn get_pr_info_for_branch(branch_name: &str) -> Option<PrInfo> {
    let output = std::process::Command::new("gh")
        .args([...])
        .output()
        .ok()?;
    // ...
}

// After (git.rs)
async fn get_pr_info_for_branch(branch_name: &str) -> Option<PrInfo> {
    let output = tokio::process::Command::new("gh")
        .args([...])
        .output()
        .await
        .ok()?;
    // parsing logic unchanged
}
```

`tokio::process::Command` is a drop-in async replacement for `std::process::Command`. The parsing
logic inside the function is unchanged.

#### Step 2 — Convert `fetch_pr_info_for_branches()` to async (pass 2 only)

Pass 1 (cache hits) and pass 3 (write-back) remain synchronous. Only pass 2 changes:

```rust
use std::sync::Arc;
use tokio::sync::Semaphore;

/// Maximum number of concurrent `gh` subprocess calls.
const MAX_CONCURRENT_GH_CALLS: usize = 20;

pub async fn fetch_pr_info_for_branches(
    branches: &mut [BranchInfo],
    cache: Option<&mut PrCache>,
    sequential: bool,
) {
    // ... pass 1 unchanged ...

    // --- Pass 2: Fetch misses ---
    let results: Vec<Option<PrInfo>> = if sequential {
        // Sequential path unchanged — no async overhead
        let mut out = Vec::with_capacity(names.len());
        for name in &names {
            out.push(get_pr_info_for_branch(name).await);
        }
        out
    } else {
        let sem = Arc::new(Semaphore::new(MAX_CONCURRENT_GH_CALLS));
        let mut handles = tokio::task::JoinSet::new();

        for name in names {
            let sem = Arc::clone(&sem);
            handles.spawn(async move {
                let _permit = sem.acquire_owned().await.unwrap();
                get_pr_info_for_branch(&name).await
                // permit dropped here → slot freed for next task
            });
        }

        // Collect in spawn order — JoinSet returns in completion order,
        // so we pair results back to indices via a separate index map.
        let mut result_map = std::collections::HashMap::new();
        // (see full implementation note below)
        todo!()
    };

    // ... pass 3 unchanged ...
}
```

> **Implementation note on ordering:** `JoinSet::join_next()` returns tasks in completion order, not
> spawn order. To correctly pair results back to `miss_indices`, spawn each task with its index:
> `handles.spawn(async move { (i, get_pr_info_for_branch(&name).await) })` and collect into a
> `HashMap<usize, Option<PrInfo>>` keyed by index. Pass 3 then iterates `miss_indices` and looks up
> each result by index.

#### Step 3 — Add `#[tokio::main]` to `main()`

```rust
// Before
fn main() -> color_eyre::Result<()> {
    rayon::ThreadPoolBuilder::new()
        .num_threads(MAX_PARALLEL_WORKERS)
        .build_global()
        .ok();
    // ...
}

// After
#[tokio::main]
async fn main() -> color_eyre::Result<()> {
    // No ThreadPoolBuilder needed — tokio manages its own thread pool
    // ...
    fetch_pr_info_for_branches(&mut branches, Some(&mut pr_cache), sequential).await;
    // ...
}
```

The tokio runtime uses `num_cpus` threads by default but tasks yield at `.await` points, so the
actual OS thread count stays low regardless of concurrency.

#### Step 4 — Update concurrency constant and remove rayon bootstrap

```rust
// Remove:
const MAX_PARALLEL_WORKERS: usize = 8;
rayon::ThreadPoolBuilder::new() ...

// Add:
const MAX_CONCURRENT_GH_CALLS: usize = 20;
// (used inside fetch_pr_info_for_branches via Semaphore)
```

The semaphore of 20 stays well under GitHub's 5 000 req/hour burst limit while giving ⌈81/20⌉ = 5
round-trips instead of ⌈81/8⌉ = 11.

#### Step 5 — Update Nix `cargoHash`

After updating `Cargo.lock` (rayon removed, tokio version pinned), set `cargoHash` in
`nix/pkgs/local-git-branch-cleanup/tui.nix` to `lib.fakeHash`, run `nix build`, capture the correct
hash from the error output, and replace.

#### Step 6 — Extend benchmark

Update `scripts/bench-pr-fetch.sh` to add a third run mode (`--tokio`) so the three modes can be
compared side-by-side:

```
Mode         | Branches | Fetch time | Speedup
-------------|----------|------------|--------
Sequential   | 81       | ~55 s      | 1×
rayon (2.0)  | 81       | ~6.7 s     | ~8.4×
tokio (2.1)  | 81       | TBD        | TBD
```

Expected: with semaphore=20, theoretical floor = ⌈81/20⌉ × 0.71 ≈ 2.9 s. Actual will depend on
GitHub API variance.

### Acceptance Criteria for Phase 2.1

- [ ] `get_pr_info_for_branch()` uses `tokio::process::Command` and is declared `async fn`
- [ ] `fetch_pr_info_for_branches()` is declared `async fn`; internal pass 2 uses `JoinSet` +
      `Semaphore`
- [ ] `main()` is annotated `#[tokio::main]`
- [ ] `rayon` is removed from all `Cargo.toml` files
- [ ] `MAX_CONCURRENT_GH_CALLS` replaces `MAX_PARALLEL_WORKERS`; default value is 20
- [ ] Results are identical to sequential and Phase 2.0 output (order-independent correctness)
- [ ] `--sequential` flag still works (falls back to awaited sequential loop)
- [ ] Nix `cargoHash` updated for new `Cargo.lock`
- [ ] Benchmark script updated; new results recorded in `docs/BENCHMARK_PR_FETCH.md`
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

| File                                        | Phase | Change                                                                                               |
| ------------------------------------------- | ----- | ---------------------------------------------------------------------------------------------------- |
| `src/cache.rs`                              | 1     | New file — entire module                                                                             |
| `src/git.rs`                                | 1     | Add `get_repo_slug()`, modify `fetch_pr_info_for_branches()`                                         |
| `src/main.rs`                               | 1     | Initialize `PrCache`, wire into GitHub block                                                         |
| `Cargo.toml`                                | 1     | Add `rusqlite` (bundled), `dirs`                                                                     |
| `src/git.rs`                                | 2.0   | 3-pass parallel execution; `sequential: bool` param; `None`-cache path also parallelised             |
| `Cargo.toml`                                | 2.0   | Add `rayon = "1.10"`                                                                                 |
| `src/main.rs`                               | 2.0   | `ThreadPoolBuilder` (8 workers); hidden `--sequential` and `--fetch-only` flags; inline fetch timer  |
| `nix/pkgs/local-git-branch-cleanup/tui.nix` | 2.0   | Update `cargoHash` for new `Cargo.lock`                                                              |
| `scripts/bench-pr-fetch.sh`                 | 2.0   | New file — automated sequential vs parallel benchmark                                                |
| `docs/BENCHMARK_PR_FETCH.md`                | 2.0   | New file — recorded results (81 branches, 8.4× speedup)                                              |
| `docs/specs/PR_FETCH_BEHAVIOUR.md`          | 2.0   | New file — prose description of the fetch algorithm and cache behaviour                              |
| `src/git.rs`                                | 2.1   | `get_pr_info_for_branch` → `async fn`; pass 2 uses `JoinSet` + `Semaphore`                           |
| `src/main.rs`                               | 2.1   | `#[tokio::main]`; remove `ThreadPoolBuilder`; `MAX_CONCURRENT_GH_CALLS = 20`                         |
| `Cargo.toml`                                | 2.1   | Remove `rayon`; add `tokio = { version = "1", features = ["rt-multi-thread", "process", "macros"] }` |
| `nix/pkgs/local-git-branch-cleanup/tui.nix` | 2.1   | Update `cargoHash` for new `Cargo.lock`                                                              |
| `scripts/bench-pr-fetch.sh`                 | 2.1   | Add tokio run mode; record new results                                                               |
| `docs/BENCHMARK_PR_FETCH.md`                | 2.1   | Add tokio column to results table                                                                    |
| `src/main.rs`                               | 3     | Add `--refresh-cache`, `--cache-stats`, `--cache-ttl` flags                                          |
| `src/app.rs`                                | 3     | Add `cache_stats`, `pr_cache`, `refresh_pr_data()` to `App`                                          |
| `src/ui.rs`                                 | 3     | Header badge, details pane label, `Ctrl+R` keybinding                                                |
| `src/git.rs`                                | 3     | Add `force_refresh` param to `fetch_pr_info_for_branches()`                                          |
| `docs/specs/ARCHITECTURE.md`                | 3     | Update module diagram, types, dependencies table                                                     |
