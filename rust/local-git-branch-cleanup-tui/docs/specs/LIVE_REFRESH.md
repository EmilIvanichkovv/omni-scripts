# Live Refresh Specification

Status: Proposed. Target project: `rust/local-git-branch-cleanup-tui`. Source snapshot reviewed:
`main` @ `0cb5a89` (v0.7.0), 2026-11-19. Related: [GitHub PR Cache](GITHUB_PR_CACHE.md) phase 3
(issue #31), whose `Ctrl+R` keybinding this spec replaces (section 9).

## 1. Goals

1. **The branch list follows the repository while the TUI is open.** Branches created, deleted,
   committed to, merged or checked out from another terminal show up without restarting the tool.
2. **A key reloads on demand:** `r` re-reads local git state, `R` also fetches the remote and
   re-fetches PR data.
3. **The UI never freezes during a refresh.** All refresh work runs off the main thread, like the
   worktree view's `git status` loaders.
4. **A refresh never changes what is about to be deleted.** Selections survive a refresh by branch
   name, and a refresh never applies while the delete confirmation is open.

Non-goals: refreshing on a timer over the network by default (rate limits, section 10), watching
files with `inotify` or FSEvents (section 3.1), and live updates in CLI mode.

## 2. Current state

- **Branch data** is read once at startup by `git::get_branches_with_classification`
  (`src/git.rs:330`). It runs about 6 `git` subprocesses per branch (upstream check, last commit,
  commit details, activity, creation date) plus a few per run, so 200 branches cost about 1,200
  processes.
- **Re-reads happen only after a deletion:** `App::refresh_branches` (`src/app.rs:937`) is called
  after deleting branches (`src/main.rs:429`) and after removing worktrees (`src/app.rs:1102`). It
  runs on the main thread and keeps the PR info fetched at startup.
- **PR data** is fetched once before the TUI starts (`pr::fetch_pr_info_for_branches`, async on
  tokio). Since #29 the `PrCache` stays open on `App::pr_cache`.
- **Remote state** is never fetched. A branch shows as `gone` only after the user ran
  `git fetch --prune` elsewhere.
- **The main loop** wakes every 100 ms (`event::poll`, `src/main.rs:370`) and already drains two
  background channels each tick: the update notice (`update_notice.message()`) and the worktree
  status loaders (`app.poll_worktree_status()`, `std::sync::mpsc` + `thread::spawn` in
  `src/worktree_view.rs:481`).
- **Free keys:** `r` and `R`. `w` toggles the worktree view; `Ctrl+U`, `Ctrl+D`, `Ctrl+Q` are taken.

### 2.1 Defects that block live refresh

1. **Branch selection is stored by position.** `App::selected_branches: HashSet<usize>` holds
   indices into `App::branches`. A refresh that adds or removes a branch makes the same indices
   point at different branches, so the confirmation modal could list, and `delete_selected_branches`
   delete, branches the user never selected. It is harmless today only because every refresh follows
   a deletion that clears the selection. The worktree view already keys its selection by path
   (`WorktreeView::selected: HashSet<PathBuf>`).
2. **The cursor is stored by position.** `selected_index` points into the filtered list; when a
   branch above the cursor appears or disappears, the cursor moves to a different branch.
3. **A refresh drops `--trunk`.** `refresh_branches` calls `get_branches_with_classification(None)`,
   so after a deletion the trunk falls back to the detected default branch even when the user passed
   `--trunk`.

## 3. Research

### 3.1 Detecting local changes

| Option                                                                                | Cost per check          | Works on                                                                             | Verdict    |
| ------------------------------------------------------------------------------------- | ----------------------- | ------------------------------------------------------------------------------------ | ---------- |
| Re-read everything every N seconds                                                    | ~6 git calls per branch | everywhere                                                                           | too costly |
| File watcher (`notify` crate) on `.git/refs`, `packed-refs`, `HEAD`, `worktrees/`     | none while idle         | unreliable on WSL `/mnt` paths and network drives; must also handle linked worktrees | rejected   |
| **Fingerprint poll:** one cheap snapshot every 2 s, full re-read only when it changes | 3 git calls             | everywhere                                                                           | **chosen** |
| Terminal focus events (`FocusGained`)                                                 | none                    | most terminals; not all report focus                                                 | **add-on** |

The fingerprint is the concatenated output of:

```
git for-each-ref --format='%(refname) %(objectname) %(upstream) %(upstream:track)' refs/heads refs/remotes
git symbolic-ref -q HEAD            # checkout of another branch changes the "current" status
git worktree list --porcelain -z    # new or removed worktrees change protection (TUI-19)
```

Hashing is not needed; the strings are compared directly. A full re-read runs only when the
fingerprint differs from the last applied one.

`FocusGained` (crossterm `EnableFocusChange`) triggers an immediate fingerprint check, so switching
back from another terminal updates the list at once instead of within 2 s.

### 3.2 Remote and PR data

- **Remote:** `git fetch --prune <primary remote>` (`git::get_primary_remote`). It needs the network
  and may need credentials, so it only runs on `R` (section 10 covers prompts).
- **PR data:** `fetch_pr_info_for_branches` already runs on tokio with bounded concurrency. A forced
  refresh skips `cache.get()` (the `force_refresh` flag from GITHUB_PR_CACHE phase 3) and writes the
  results back to the cache. It only runs on `R`.

## 4. User experience

### 4.1 Keys

| Key | Action                                                                                         |
| --- | ---------------------------------------------------------------------------------------------- |
| `r` | Reload local git state now (same work as an automatic refresh)                                 |
| `R` | Full sync: `git fetch --prune` on the primary remote, reload local state, re-fetch all PR data |

Both work in the branch and worktree views and appear in the help modal and footer. `R` without a PR
provider does the fetch and the reload only.

### 4.2 Automatic refresh

- On by default. `--no-auto-refresh` turns off the fingerprint poll and the focus trigger; `r` and
  `R` still work.
- Interval 2 s, fixed. A check is skipped while one is running.

### 4.3 Footer status

- While running: `⟳ reloading…` or `⟳ fetching origin…` / `⟳ fetching PRs 12/48`.
- After a refresh that changed something: `updated 14:32:05` for 5 s, then nothing.
- On failure: a one-line `flash` message (`Fetch failed: …`), and the list keeps its old data.

### 4.4 What the user keeps across a refresh

- Selected branches, by name. A selected branch that no longer exists is dropped from the selection.
- The cursor stays on the same branch by name; if that branch is gone, it stays at the same row
  index, clamped to the list.
- Search query, filter tab, sort mode, scroll position (clamped), and the active view.
- PR info for branches that still exist, unless `R` replaced it.

### 4.5 When a refresh waits

A finished refresh is held, not applied, while the delete confirmation modal is open, and applied
when the modal closes. Help and info modals do not block it.

## 5. Design

### 5.1 Module `src/refresh.rs`

```rust
pub enum RefreshKind { Local, Full }

pub struct RefreshResult {
    pub generation: u64,
    pub fingerprint: String,
    pub branches: color_eyre::Result<Vec<BranchInfo>>,
    pub fetch_error: Option<String>,   // Full only
    pub pr_cache: Option<PrCache>,     // handed back after a Full refresh
}

pub struct Refresher {
    generation: u64,                   // bumped on every request and on every local deletion
    in_flight: Option<RefreshKind>,
    queued: Option<RefreshKind>,       // at most one; Full wins over Local
    last_fingerprint: String,
    last_check: Instant,
    rx: Option<Receiver<RefreshResult>>,
    pending: Option<RefreshResult>,    // held while the confirmation modal is open
}
```

- `Refresher::tick(&mut App, now)` runs every loop iteration next to `poll_worktree_status`: drains
  the channel, applies or holds the result, and every 2 s starts a fingerprint check on a thread.
- `request(kind)` starts a refresh, or queues one if a refresh is in flight. A second `r` during a
  refresh queues one follow-up instead of starting a second thread.
- **Stale results:** every result carries the generation it was started with. A result older than
  the current generation is dropped. A deletion bumps the generation and requests a `Local` refresh,
  so a result read before the deletion can never bring deleted branches back.

### 5.2 Threads

- `Local`: one `std::thread` runs the fingerprint and, if needed,
  `get_branches_with_classification(Some(&trunk))`. Same model as `spawn_status_loaders`.
- `Full`: a tokio task (`Handle::current().spawn`; the TUI already runs inside `#[tokio::main]`)
  runs the fetch and the classification with `spawn_blocking`, then `fetch_pr_info_for_branches`
  with `force_refresh`. It takes `App::pr_cache` by value (`Option::take`) and returns it in the
  result, so the cache needs no lock. While it is out, deletions skip `invalidate()` and the task
  invalidates nothing; the next startup's `evict_stale` covers the gap.

### 5.3 Applying a result

`App::apply_refresh(branches)` replaces `refresh_branches`:

1. Carry over `pr_info` by name for branches the result did not re-fetch, then
   `git::apply_pr_merge_status`.
2. Swap `self.branches`.
3. Rebuild `selected_branches` from the saved names (section 2.1 fix, below).
4. Restore the cursor by name (section 4.4), then clamp `scroll_offset`.
5. Rebuild `unique_authors` for `@author:` completion.
6. If the worktree view is loaded, call `load_worktrees()` so protection and the table follow.

The post-deletion paths call `request(Local)` instead of `refresh_branches()`, so they get the same
background, generation-safe path.

### 5.4 Fixes from section 2.1

- `selected_branches: HashSet<String>` (branch names). `toggle_selection`, `select_all`,
  `is_selected`, `get_selected_branches` and `delete_selected_branches` look branches up by name.
- `App` keeps the `--trunk` override (`trunk_override: Option<String>`) and passes it on every
  reload.

### 5.5 Credentials during `git fetch`

The TUI owns the terminal in raw mode, so a credential prompt would corrupt the screen and hang.
`git fetch` runs with:

- `GIT_TERMINAL_PROMPT=0` (no HTTPS username/password prompt),
- stdin set to null,
- `-c core.sshCommand="ssh -o BatchMode=yes"` only when neither `GIT_SSH_COMMAND` nor
  `core.sshCommand` is already set, so a user's SSH config is never overridden.

A fetch that needs a prompt then fails fast and shows
`Fetch failed: authentication required (run git fetch in a terminal)`. Credential helpers and
ssh-agent keep working.

## 6. Testing

- **Unit (`refresh.rs`):** queueing (one follow-up, `Full` wins), stale generations dropped, results
  held while the confirmation modal is open and applied after it closes.
- **Unit (`app.rs`):** selection and cursor by name survive a refresh that inserts a branch before
  them and one that removes a selected branch; `--trunk` survives a reload.
- **Integration:** a temp repo where a branch is created, then the fingerprint function reports a
  change, and a second call with no change reports none. The fetch path runs against a file-based
  bare remote (as in `test_pr_merged_status_for_squash_merged_branch`) where a remote branch is
  deleted and the local branch becomes `gone` after `R`.
- **Credentials:** `git fetch` against a remote URL that requires auth fails without hanging
  (timeout in the test).
- **Manual:** WSL on `/mnt/c`, macOS and Windows Terminal: create and delete branches in another
  terminal and confirm the list follows within 2 s and on focus.

## 7. Implementation phases

1. **Safe reload:** selection and cursor by name, `--trunk` kept, `Refresher` with background
   `Local` refresh, the `r` key, footer status. Post-deletion refreshes go through it.
2. **Automatic refresh:** fingerprint poll, `FocusGained` trigger, `--no-auto-refresh`.
3. **Full sync:** `R` with `git fetch --prune`, credential guards, forced PR re-fetch with the cache
   handed to the task. Updates GITHUB_PR_CACHE phase 3 to drop its `Ctrl+R`.

Each phase is one PR and leaves the tool releasable.

## 8. Acceptance criteria

- [ ] Selecting branches, then creating a branch elsewhere and refreshing, keeps exactly the same
      branches selected
- [ ] A selected branch deleted elsewhere is dropped from the selection after a refresh
- [ ] `r` reloads without freezing the UI (keys keep responding during the reload)
- [ ] `--trunk` is kept after a deletion and after a reload
- [ ] With auto refresh on, a branch created in another terminal appears within 2 s, and on focus
      where the terminal reports it
- [ ] With the confirmation modal open, no refresh changes the list or the selection
- [ ] `R` marks a branch whose remote was deleted as `gone`
- [ ] `R` with an auth-required remote fails with a message and does not hang or draw over the TUI
- [ ] `R` re-fetches PR data and writes it to the cache
- [ ] `--no-auto-refresh` stops the poll; `r` and `R` still work
- [ ] `cargo test` and `cargo clippy --all-targets -- -D warnings` pass

## 9. Decisions log

| #   | Decision                                                   | Why                                                                     |
| --- | ---------------------------------------------------------- | ----------------------------------------------------------------------- |
| 1   | Fingerprint polling over a file watcher                    | No new dependency; works on WSL and network drives; 3 git calls per 2 s |
| 2   | No network on a timer                                      | GitHub and Bitbucket rate limits; fetch may need credentials            |
| 3   | `R` replaces GITHUB_PR_CACHE phase 3's `Ctrl+R`            | One "sync everything" key is easier to remember than two refresh keys   |
| 4   | Auto refresh on by default, `--no-auto-refresh` to opt out | The cost is small and stale data is the problem being solved            |
| 5   | Selection by name, not index                               | Required for correctness once the list can change under the user        |

## 10. Risks

- **Credential prompts** during `git fetch` (section 5.5). Mitigated by disabling prompts; the cost
  is that `R` cannot fetch from remotes that only work with an interactive prompt.
- **Rate limits** on `R` with many branches: same as startup today; the cache TTL is not used, so
  repeated `R` presses each cost a full fetch. The footer shows progress so users see the cost.
- **Large repos:** a reload with 500+ branches takes seconds. It runs in the background, and the
  fingerprint keeps it from running when nothing changed.
- **Branch processes spawned while the user deletes:** deletion is synchronous on the main thread;
  the generation bump in section 5.1 makes any concurrent result harmless.

## 11. Follow-ups (not in this spec)

- `--auto-fetch <minutes>`: periodic `git fetch` for users who want it.
- Re-fetch PR data only for branches whose tip changed.
- The CLI-only parts of GITHUB_PR_CACHE phase 3 (`--refresh-cache`, `--cache-stats`, `--cache-ttl`,
  cache status in the header).
