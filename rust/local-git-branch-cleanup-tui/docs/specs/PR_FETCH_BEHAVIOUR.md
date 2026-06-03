# GitHub PR Fetch — Current Behaviour

**Status:** ✅ Phase 2 Complete **Updated:** 2026-05-23 **Relevant files:** `src/git.rs`,
`src/cache.rs`, `src/main.rs`

---

## Overview

When `--github` / `-g` is passed, the application enriches each branch with its associated GitHub PR
(number, state, title, URL). This is done by calling the `gh` CLI once per branch miss.

Two mechanisms keep this fast: a **SQLite cache** (Phase 1) that avoids redundant API calls across
runs, and **parallel execution** via `rayon` (Phase 2) that fetches all cache misses concurrently.

---

## Startup Sequence

```
main()
  ├─ rayon::ThreadPoolBuilder::new()
  │    .num_threads(8)           // cap concurrent gh calls
  │    .build_global()
  │
  ├─ git::is_gh_cli_available()  // exit early if gh not installed
  ├─ git::get_repo_slug()        // derive "owner/repo" from remote URL
  ├─ cache::PrCache::open(&slug, ttl=1h)
  │    ├─ open/create ~/.cache/omni-scripts/pr-cache.db
  │    ├─ run schema migrations
  │    └─ evict entries older than 30 days
  │
  └─ git::fetch_pr_info_for_branches(&mut branches, Some(&mut pr_cache))
       └─ (see algorithm below)
```

If `PrCache::open()` fails (disk full, permissions, etc.) the call falls through to
`fetch_pr_info_for_branches(&mut branches, None)`, which runs the same parallel algorithm but skips
all cache reads and writes.

---

## Core Algorithm — `fetch_pr_info_for_branches()`

The function takes an optional mutable reference to a `PrCache`. Its behaviour depends on whether a
cache is available.

### With cache (`Some(&mut pr_cache)`)

```
┌──────────────────────────────────────────────────────────────┐
│ Pass 1 — synchronous cache scan                              │
│                                                              │
│  for each branch (sequential):                               │
│    cache.get(branch.name)                                    │
│      Hit(Some(pr_info)) → branch.pr_info = Some(pr_info)    │
│      Hit(None)          → branch.pr_info = None  (no PR,    │
│                           but cached — skip API call)        │
│      Miss               → add index to miss_indices          │
└──────────────────────────────────────────────────────────────┘
                  │ if miss_indices is empty → return early
                  ▼
┌──────────────────────────────────────────────────────────────┐
│ Pass 2 — parallel fetch (rayon, ≤ 8 threads)                │
│                                                              │
│  names = [branches[i].name for i in miss_indices]           │
│                                                              │
│  results = names.par_iter()                                  │
│              .map(|name| gh pr list --head <name> ...)       │
│              .collect()          // Vec<Option<PrInfo>>      │
│                                                              │
│  All gh subprocesses run concurrently up to the thread cap.  │
└──────────────────────────────────────────────────────────────┘
                  ▼
┌──────────────────────────────────────────────────────────────┐
│ Pass 3 — write-back (single-threaded)                        │
│                                                              │
│  for (idx, result) in miss_indices.zip(results):            │
│    branches[idx].pr_info = result                            │
│    cache.set(branch_name, result)   // writes to SQLite      │
└──────────────────────────────────────────────────────────────┘
```

### Without cache (`None`)

Identical to Pass 2 + Pass 3 above, but applied to **all** branches (there is no pass 1). Results
are never written to the database.

```
names   = all branch names
results = names.par_iter().map(get_pr_info_for_branch).collect()
zip(branches, results) → branch.pr_info = result
```

---

## Cache Behaviour

| Scenario               | Cache entry present?   | Within TTL? | Outcome                                   |
| ---------------------- | ---------------------- | ----------- | ----------------------------------------- |
| Warm run, PR exists    | Yes                    | Yes         | `Hit(Some(PrInfo))` — no `gh` call        |
| Warm run, no PR        | Yes (`pr_number NULL`) | Yes         | `Hit(None)` — no `gh` call                |
| Expired entry          | Yes                    | **No**      | `Miss` — fetches fresh, overwrites entry  |
| Cold run / first time  | No                     | —           | `Miss` — fetches fresh, writes entry      |
| Cache file unavailable | —                      | —           | Skips cache entirely, fetches in parallel |

**TTL:** 1 hour (hard-coded, configurable via `--cache-ttl` in Phase 3). **Stale eviction:** entries
older than 30 days are deleted on startup. **Cache location:**
`$XDG_CACHE_HOME/omni-scripts/pr-cache.db` (falls back to `~/.cache/`).

---

## Progress Output

```
🔗 Fetching GitHub PR info...
   42 from cache, 8 fetched from GitHub
```

If it is a fully cold run (0 cache hits) with more than 20 branches, an additional hint is printed:

```
   (tip: subsequent runs will be instant — results are cached for 1h)
```

---

## Latency Profile

Assuming ~1 s per `gh` call and the 8-thread cap:

| Branches | Cold run (no cache) | Warm run (all cached) |
| -------- | ------------------- | --------------------- |
| 10       | ~2 s                | ~0.1 s                |
| 50       | ~7 s                | ~0.1 s                |
| 100      | ~13 s               | ~0.1 s                |

---

## Thread Safety Notes

- **Pass 1** and **Pass 3** run on the main thread — `PrCache` (`rusqlite::Connection`) is only ever
  touched single-threaded.
- **Pass 2** runs on rayon's thread pool. Each task gets only a branch name (a `String` clone) and
  spawns an independent `gh` subprocess. No shared mutable state is accessed in pass 2.
- The thread pool is initialised once in `main()` via `ThreadPoolBuilder::build_global()`.
  Subsequent calls to `fetch_pr_info_for_branches()` reuse the same pool.
