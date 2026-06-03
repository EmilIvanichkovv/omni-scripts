# Benchmark — Parallel vs Sequential PR Fetching

**Date:** 2026-06-03 **Binary:** `local-git-branch-cleanup-tui` (release build,
`cargo build --release`) **Repository under test:** `metacraft-labs/blocksense` (monorepo)
**Branches:** 81 local branches

---

## Environment

| Property          | Value                                                         |
| ----------------- | ------------------------------------------------------------- |
| CPU logical cores | 32                                                            |
| Kernel            | Linux 6.11.11                                                 |
| rayon thread cap  | 8 (hard-coded `MAX_PARALLEL_WORKERS`)                         |
| Cache state       | Empty (cleared with `DELETE FROM cached_prs` before each run) |
| Network           | Same machine, authenticated `gh` CLI                          |

---

## Method

A hidden `--fetch-only` flag was added to the tool specifically for this benchmark. It performs the
full PR fetch (branch scan → cache check → GitHub API calls → cache write) and then calls
`std::process::exit(0)` — no TUI is launched, no branch list is rendered.

A hidden `--sequential` flag forces `rayon`'s `par_iter()` to be replaced with a plain `iter()`,
reproducing the pre-Phase-2 behaviour exactly on the same binary.

Each run starts with a completely empty `cached_prs` table so every branch is a cache miss and must
make a real `gh pr list` API call.

```
# Clear cache
sqlite3 ~/.cache/omni-scripts/pr-cache.db "DELETE FROM cached_prs;"

# Before (sequential)
time local-git-branch-cleanup-tui --github --sequential --fetch-only

# After (parallel, rayon ≤ 8 threads)
time local-git-branch-cleanup-tui --github --fetch-only

# Warm cache (all 81 entries already cached)
time local-git-branch-cleanup-tui --github --fetch-only
```

The `fetch completed in Xs` time is measured inside the binary using `std::time::Instant` around
`fetch_pr_info_for_branches()` only — it excludes branch scanning and startup.

---

## Results

| Run    | Mode                  | Branches | Cache hits | Cache misses | Fetch time  | Wall time |
| ------ | --------------------- | -------- | ---------- | ------------ | ----------- | --------- |
| Before | Sequential (no rayon) | 81       | 0          | 81           | **57.53 s** | 58.75 s   |
| After  | Parallel (rayon, ≤ 8) | 81       | 0          | 81           | **6.84 s**  | 8.05 s    |
| Warm   | Parallel (all cached) | 81       | 81         | 0            | **0.00 s**  | 1.19 s    |

---

## Analysis

### Cold run speedup

$$\text{speedup} = \frac{57.53\text{ s}}{6.84\text{ s}} \approx 8.4\times$$

With 81 branches and an 8-thread cap, the theoretical minimum is $\lceil 81/8 \rceil = 11$ batches.
At ~0.71 s per `gh` call (57.53 s / 81), the theoretical floor is
$11 \times 0.71 \approx 7.8\text{ s}$. The measured 6.84 s is slightly better than this estimate
because some calls complete faster than average and the thread pool keeps all 8 slots busy
throughout.

### Per-call latency

Dividing the sequential total by the number of branches gives the average `gh` round-trip time for
this repository and network:

$$\frac{57.53\text{ s}}{81\text{ branches}} \approx 0.71\text{ s per call}$$

### Warm cache

With all 81 entries in SQLite the fetch phase completes in under 1 ms (shown as `0.00s`). The 1.19 s
wall time is entirely git branch scanning and process startup — nothing to do with GitHub.

---

## Conclusion

| Scenario                | Before | After        | Improvement     |
| ----------------------- | ------ | ------------ | --------------- |
| First run (81 branches) | ~58 s  | ~8 s         | **8.4× faster** |
| Subsequent runs         | ~58 s  | ~1 s (cache) | **~58× faster** |

Phase 2 (rayon parallelism) reduces a nearly one-minute cold start to under 9 seconds for an
81-branch repository. Phase 1 (SQLite cache) then eliminates the API cost entirely on every
subsequent run within the 1-hour TTL.
