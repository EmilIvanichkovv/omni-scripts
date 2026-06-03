# Benchmark — Sequential vs tokio PR Fetching

**Date:** 2026-06-03 **Binary:** `local-git-branch-cleanup-tui` (release build,
`cargo build --release`) **Repository under test:** `metacraft-labs/blocksense` (monorepo)
**Branches:** 81 local branches

---

## Environment

| Property          | Value                                                           |
| ----------------- | --------------------------------------------------------------- |
| CPU logical cores | 32                                                              |
| Kernel            | Linux 6.11.11                                                   |
| Concurrency       | tokio async tasks, semaphore cap 20 (`MAX_CONCURRENT_GH_CALLS`) |
| Cache state       | Empty (cleared with `DELETE FROM cached_prs` before each run)   |
| Network           | Same machine, authenticated `gh` CLI                            |

---

## Method

A hidden `--fetch-only` flag was added to the tool specifically for this benchmark. It performs the
full PR fetch (branch scan → cache check → GitHub API calls → cache write) and then calls
`std::process::exit(0)` — no TUI is launched, no branch list is rendered.

A hidden `--sequential` flag forces a plain awaited loop (one `gh` call at a time), reproducing the
pre-Phase-2 behaviour exactly on the same binary.

Each run starts with a completely empty `cached_prs` table so every branch is a cache miss and must
make a real `gh pr list` API call.

```
# Clear cache
sqlite3 ~/.cache/omni-scripts/pr-cache.db "DELETE FROM cached_prs;"

# Before (sequential)
time local-git-branch-cleanup-tui --github --sequential --fetch-only

# After (tokio, semaphore ≤ 20)
time local-git-branch-cleanup-tui --github --fetch-only

# Warm cache (all 81 entries already cached)
time local-git-branch-cleanup-tui --github --fetch-only
```

The `fetch completed in Xs` time is measured inside the binary using `std::time::Instant` around
`fetch_pr_info_for_branches()` only — it excludes branch scanning and startup.

---

## Results

### Phase 2.1 — tokio (current)

| Run    | Mode                         | Branches | Cache hits | Cache misses | Fetch time  | Wall time |
| ------ | ---------------------------- | -------- | ---------- | ------------ | ----------- | --------- |
| Before | Sequential (no concurrency)  | 81       | 0          | 81           | **60.73 s** | ~62 s     |
| After  | Tokio async (semaphore ≤ 20) | 81       | 0          | 81           | **3.46 s**  | ~5 s      |
| Warm   | Tokio (all cached)           | 81       | 81         | 0            | **0.00 s**  | ~1 s      |

### Phase 2.0 — rayon (historical, for comparison)

| Run    | Mode                  | Branches | Fetch time  |
| ------ | --------------------- | -------- | ----------- |
| Before | Sequential            | 81       | **57.53 s** |
| After  | Parallel (rayon, ≤ 8) | 81       | **6.84 s**  |
| Warm   | Parallel (all cached) | 81       | **0.00 s**  |

---

## Analysis

### Cold run speedup (Phase 2.1 — tokio)

$$\text{speedup} = \frac{60.73\text{ s}}{3.46\text{ s}} \approx 17.6\times$$

With 81 branches and a semaphore cap of 20, the theoretical minimum is $\lceil 81/20 \rceil = 5$
batches. At ~0.75 s per `gh` call (60.73 s / 81), the theoretical floor is
$5 \times 0.75 \approx 3.75\text{ s}$. The measured 3.46 s beats this because faster calls keep all
20 slots saturated throughout — OS threads yield while waiting for I/O rather than blocking.

### Phase 2.0 vs Phase 2.1 comparison

$$\frac{6.84\text{ s}}{3.46\text{ s}} \approx 2.0\times \text{ improvement from rayon → tokio}$$

The key difference: `rayon` blocks one OS thread per in-flight call (capped at 8), meaning only 8
calls can overlap at any moment. `tokio` tasks yield at `.await` while waiting for the child process
stdout, allowing a small thread pool to service far more in-flight calls simultaneously.

### Per-call latency

$$\frac{60.73\text{ s}}{81\text{ branches}} \approx 0.75\text{ s per call}$$

### Warm cache

With all 81 entries in SQLite the fetch phase completes in under 1 ms (shown as `0.00s`).

---

## Conclusion

| Scenario                | Sequential | rayon (2.0)  | tokio (2.1)  | Best improvement |
| ----------------------- | ---------- | ------------ | ------------ | ---------------- |
| First run (81 branches) | ~61 s      | ~7 s (8.4×)  | ~3.5 s       | **17.6× faster** |
| Subsequent runs         | ~61 s      | ~1 s (cache) | ~1 s (cache) | **~61× faster**  |

Phase 2.1 (tokio async) improves on Phase 2.0 (rayon) by ~2× on a cold cache, while using fewer OS
threads. Phase 1 (SQLite cache) still eliminates the API cost entirely on every subsequent run
within the 1-hour TTL.
