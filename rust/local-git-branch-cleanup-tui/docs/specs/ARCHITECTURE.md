# Architecture Documentation

## Overview

`local-git-branch-cleanup-tui` is a Rust application that provides both CLI and TUI (Terminal User
Interface) modes for managing local Git branches. The architecture follows a modular design with
clear separation of concerns.

## Architecture Diagram

```
┌─────────────────────────────────────────────────────────────┐
│                         main.rs                              │
│  ┌───────────────────────────────────────────────────────┐  │
│  │  Entry Point & Orchestration                          │  │
│  │  - CLI argument parsing (clap)                        │  │
│  │  - Mode selection (CLI vs TUI)                        │  │
│  │  - Terminal initialization/teardown                   │  │
│  │  - Event loop management                              │  │
│  └───────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────┘
      │             │             │             │             │
      ▼             ▼             ▼             ▼             ▼
 ┌─────────┐   ┌─────────┐   ┌─────────┐   ┌─────────┐   ┌─────────┐
 │ app.rs  │   │ git.rs  │   │remote.rs│   │   pr/   │   │  ui.rs  │
 └─────────┘   └─────────┘   └─────────┘   └─────────┘   └─────────┘
 Application   Local Git     Origin        PR Providers  UI Rendering
 State Logic   Commands &    Remote        (GitHub /     (Ratatui)
               Classification Parsing      Bitbucket)
                                                │
                                                ▼
                                          ┌─────────┐
                                          │cache.rs │
                                          └─────────┘
                                          SQLite PR Cache
                                          (rusqlite)
```

The `pr/` module is the provider layer:

```
pr/
├── mod.rs        # Shared types (PrInfo, PrState, PrProviderKind, PrProviderError),
│                 # the PullRequestProvider trait, and the shared fetch coordinator
├── github.rs     # GitHub provider — wraps the gh CLI
└── bitbucket.rs  # Bitbucket Data Center provider — REST API over reqwest
```

## Module Responsibilities

### 1. `main.rs` - Entry Point & Orchestration

**Responsibilities:**

- Parse command-line arguments using `clap`
- Determine execution mode (CLI or TUI)
- Initialize the application with appropriate configuration
- Manage the terminal lifecycle (raw mode, alternate screen)
- Run the event loop (keyboard input handling)
- Coordinate between App state and UI rendering

**Key Components:**

- `Args` struct: CLI argument definitions
- `main()`: Application entry point
- `run_cli_mode()`: Legacy CLI interface
- `run_tui()`: TUI event loop and rendering

**Event Handling Flow:**

```
User Input → Crossterm Event → KeyCode Match → App State Update → UI Re-render
```

### 2. `app.rs` - Application State Management

**Responsibilities:**

- Maintain application state (branches, selections, filters, logs)
- Provide methods for state transitions (navigation, selection, filtering)
- Implement business logic for user actions
- Track UI state (current filter, modal visibility, force mode, dry run)

**Key Types:**

```rust
/// Filter modes for branch list
pub enum FilterMode {
    All,            // Show all branches
    SafeMerged,     // Show only merged branches
    GoneUpstream,   // Show only gone branches
    Unmerged,       // Show only unmerged branches
}

/// Log entry for tracking deletion actions
pub struct ActionLogEntry {
    pub branch_name: String,
    pub success: bool,
    pub message: String,
}

/// Main application state
pub struct App {
    // Branch data
    branches: Vec<BranchInfo>,
    current_branch: String,
    trunk: String,

    // UI state
    selected_index: usize,
    filter_mode: FilterMode,
    show_confirmation: bool,
    show_help: bool,

    // Selection state
    selected_branches: HashSet<usize>,

    // Action tracking
    action_log: Vec<ActionLogEntry>,

    // Mode flags
    force_mode: bool,
    dry_run: bool,
}
```

**Key Methods:**

- `new()`: Initialize with branch data
- Navigation: `select_next()`, `select_prev()`
- Filtering: `get_filtered_branches()`, `set_filter()`, `cycle_filter()`
- Selection: `toggle_selection()`, `select_all_safe()`, `clear_selection()`
- Actions: `delete_selected_branches()`, `toggle_force_mode()`, `toggle_dry_run()`
- State queries: `get_selected_branches()`, `get_filter_counts()`, `has_unmerged_selected()`

**State Transition Examples:**

```
Space Key → toggle_selection() → Update selected_branches HashSet → Re-render
Tab Key   → cycle_filter()     → Update filter_mode           → Re-render
Enter Key → Check selections   → show_confirmation = true     → Re-render modal
y Key     → delete_branches()  → Update action_log            → Refresh branches
```

### 3. `git.rs` - Local Git Integration & Branch Classification

**Responsibilities (local Git only — hosting-provider logic lives in `pr/`):**

- Execute Git commands via `std::process::Command`
- Parse Git output into structured data
- Classify branches by status
- Determine trunk branch
- Perform branch deletions
- Open URLs in the default browser (`open_url_in_browser`)

**Key Types:**

```rust
/// Branch status classification
pub enum BranchStatus {
    SafeMerged,      // Merged into trunk
    PrMerged,        // PR merged (squash/rebase), remote branch still exists
    PrDiverged,      // PR merged, but local has commits its upstream doesn't
    GoneUpstream,    // Remote was deleted
    Unmerged,        // Has unmerged commits
    Local,           // Never pushed (no tracking config and no origin/<branch>)
    Protected,       // main/master/develop
    Current,         // Currently checked out
}

/// Complete branch information (PrInfo is imported from crate::pr)
pub struct BranchInfo {
    pub name: String,
    pub status: BranchStatus,
    pub upstream: Option<String>,
    pub last_commit_relative: String,
    pub commit_sha: String,
    pub commit_author: String,
    pub commit_message: String,
    pub ahead: Option<usize>,
    pub behind: Option<usize>,
    pub pr_info: Option<PrInfo>,  // PR info (when a PR provider is enabled)
}
```

**Key Functions:**

```rust
// Repository validation
pub fn verify_repo() -> Result<String>
pub fn get_current_branch() -> Result<String>

// Trunk detection
pub fn get_trunk_branch(override_trunk: Option<String>) -> Result<String>

// Branch discovery and classification
pub fn get_branches() -> Result<Vec<BranchInfo>>
pub fn classify_branch(name: &str, current: &str, trunk: &str) -> Result<BranchStatus>

// Ahead/behind calculation
pub fn get_ahead_behind_counts(branch: &str, upstream: &str) -> Result<(usize, usize)>

// Branch operations
pub fn delete_branch(name: &str, force: bool) -> Result<String>

// Browser integration (used by the provider-neutral `o` shortcut)
pub fn open_url_in_browser(url: &str) -> Result<()>
```

**Git Command Usage:**

| Purpose         | Git Command                                                                                                                |
| --------------- | -------------------------------------------------------------------------------------------------------------------------- |
| Verify repo     | `git rev-parse --show-toplevel`                                                                                            |
| Current branch  | `git branch --show-current`                                                                                                |
| Trunk detection | `git symbolic-ref --short refs/remotes/origin/HEAD`                                                                        |
| Branch list     | `git for-each-ref refs/heads/`                                                                                             |
| Merged check    | `git branch --format='%(refname:short)' --merged <trunk>` (also run against `origin/<trunk>` to catch a stale local trunk) |
| Gone check      | Parse `[gone]` from `git for-each-ref`                                                                                     |
| Commit info     | `git log -1 --format="%cr\|%h\|%an\|%s"`                                                                                   |
| Ahead/behind    | `git rev-list --left-right --count <branch>...<upstream>`                                                                  |
| Delete          | `git branch -d/-D <branch>`                                                                                                |
| Open URL        | `xdg-open` (Linux) / `open` (macOS) / `start` (Windows)                                                                    |

**Classification Logic:**

```
1. Current branch check  → BranchStatus::Current
2. Protected name check  → BranchStatus::Protected (main/master/develop)
3. Gone upstream check   → BranchStatus::GoneUpstream
4. Never-pushed check    → BranchStatus::Local (repo has a remote, branch has
                           no tracking config AND no origin/<branch> ref; wins
                           over merged — a branch that never reached the remote
                           must not display as merged. A branch pushed without
                           -u uses origin/<branch> as its effective upstream)
5. Merged check          → BranchStatus::SafeMerged
6. Default               → BranchStatus::Unmerged
```

In a repo without any remote, the never-pushed check is skipped entirely and branches keep the plain
merged/unmerged classification.

After PR info is fetched (`--github`/`--bitbucket`), `apply_pr_merge_status` upgrades `Unmerged`
branches whose newest PR is merged and whose upstream still exists: to `PrMerged` when `ahead == 0`
(no unpushed work, safe `-d`), or to `PrDiverged` when `ahead > 0` (local commits the upstream
doesn't have — unpushed work or stale copies after a remote rebase/amend; still requires force).
This catches squash/rebase merges that git ancestry cannot see. The same upgrade is re-applied by
`App::refresh_branches` after deletions, which carries the already-fetched `pr_info` over to the
re-classified list.

### 4. `remote.rs` - Origin Remote Parsing

**Responsibilities:**

- Read the `origin` remote URL (`git remote get-url origin`)
- Parse HTTP(S), `ssh://`, and SCP-like remote URLs into a normalized identity
- Infer the Bitbucket Data Center base URL, project key, and repository slug from `origin`
- Sanitize credentials from remote URLs in diagnostics

**Key Types & Functions:**

```rust
pub struct GitRemote {
    pub name: String,
    pub raw_url: String,
    pub host: Option<String>,
    pub path_segments: Vec<String>,
    pub transport: RemoteTransport, // Http | Https | Ssh | ScpLike | Local
}

pub fn read_origin() -> Result<GitRemote>
pub fn parse_remote_url(name: &str, url: &str) -> GitRemote
pub fn infer_bitbucket_repository(remote: &GitRemote) -> Result<InferredBitbucketRepo, String>
```

### 5. `pr/` - Pull Request Provider Layer

**Responsibilities:**

- `pr/mod.rs`: shared domain types (`PrInfo`, `PrState`, `PrProviderKind`, `PrProviderError`), the
  `PullRequestProvider` trait, and the shared fetch coordinator (`fetch_pr_info_for_branches`)
- `pr/github.rs`: GitHub provider — wraps the `gh` CLI (existing behavior moved behind the trait)
- `pr/bitbucket.rs`: Bitbucket Data Center provider — configuration resolution, `reqwest` HTTP
  client, typed `serde` DTOs, state mapping, and REST calls

**Provider Interface:**

```rust
#[async_trait::async_trait]
pub trait PullRequestProvider: Send + Sync {
    fn kind(&self) -> PrProviderKind;       // GitHub | BitbucketDataCenter
    fn cache_key(&self) -> String;          // provider-aware cache partition key
    fn max_concurrency(&self) -> usize;     // GitHub: 20, Bitbucket: 8

    async fn validate(&self) -> Result<(), PrProviderError>;

    async fn get_pr_for_branch(&self, branch_name: &str)
        -> Result<Option<PrInfo>, PrProviderError>;
}
```

**Contract:**

- `Ok(Some(pr))` — lookup succeeded and found a PR (cached).
- `Ok(None)` — lookup succeeded and found no PR (negatively cached).
- `Err(PrProviderError)` — lookup failed; **never** written to the cache.
- `validate()` runs before any branch lookups. Bitbucket validation failures (configuration,
  authentication, repository access) are fatal before the TUI opens; a missing `gh` CLI under
  `--github` remains a non-fatal warning.

**Fetch Coordinator (three passes):**

1. Pass 1 — serve cache hits synchronously, collect miss indices.
2. Pass 2 — fetch misses via `tokio::task::JoinSet` + `Semaphore` capped at the provider's
   `max_concurrency()` (1 with `--sequential`).
3. Pass 3 — write results back in original branch order; successful results go to the cache, failed
   lookups only to the `FetchReport`.

**State Mapping (Bitbucket):** `OPEN` → `Open`, `MERGED` → `Merged`, `DECLINED` → `Closed`. Unknown
states are `InvalidResponse` errors, never cached.

### 6. `ui.rs` - Terminal User Interface Rendering

**Responsibilities:**

- Render the TUI using Ratatui framework
- Layout management (header, filters, list, details, log, footer)
- Visual styling (colors, borders, highlights)
- Modal dialogs (confirmation, help)

**Key Components:**

```rust
/// Main rendering function
pub fn render_ui(frame: &mut Frame, app: &App)

/// Layout sections
fn render_header(area: Rect, frame: &mut Frame, app: &App)
fn render_filter_tabs(area: Rect, frame: &mut Frame, app: &App)
fn render_branch_list(area: Rect, frame: &mut Frame, app: &App)
fn render_details_pane(area: Rect, frame: &mut Frame, app: &App)
fn render_action_log(area: Rect, frame: &mut Frame, app: &App)
fn render_footer(area: Rect, frame: &mut Frame)
fn render_confirmation_modal(frame: &mut Frame, app: &App)
fn render_help_modal(frame: &mut Frame)
```

**Layout Structure:**

```
┌──────────────────────────────────────────────────────────┐
│ HEADER (3 lines)                                         │
│ - App name, repo path, trunk                             │
│ - Selected count, force mode, dry run indicators         │
├──────────────────────────────────────────────────────────┤
│ FILTER TABS (3 lines)                                    │
│ [SAFE MERGED (N)] [UPSTREAM GONE (M)] [UNMERGED] [ALL]  │
├──────────────────────────┬───────────────────────────────┤
│ BRANCH LIST (70%)        │ DETAILS PANE (30%)            │
│                          │                               │
│ [✓] ✓ branch-name        │ Branch: feature/xyz           │
│ [ ] ↗ old-feature        │ Status: Merged into main      │
│  -  ! wip-branch         │ Upstream: origin/feature/xyz  │
│                          │ Last Commit: abc123           │
│                          │ ...                           │
├──────────────────────────┴───────────────────────────────┤
│ ACTION LOG (4 lines, appears after deletion)             │
│ ✓ feature/done - Deleted (-d)                            │
│ ✗ feature/error - Error: ...                             │
├──────────────────────────────────────────────────────────┤
│ FOOTER (3 lines)                                         │
│ Status legend and keyboard shortcuts                     │
└──────────────────────────────────────────────────────────┘
```

**Color Palette:**

```rust
// Accent colors
const CYAN: Color = Color::Rgb(46, 196, 182);      // #2EC4B6 - Selection/Active
const AMBER: Color = Color::Rgb(255, 184, 108);    // #FFB86C - Warning/Unmerged
const YELLOW: Color = Color::Rgb(241, 250, 140);   // #F1FA8C - PR-diverged
const GREY_BLUE: Color = Color::Rgb(98, 114, 164); // #6272A4 - Local (never pushed)
const RED: Color = Color::Rgb(255, 85, 85);        // #FF5555 - Danger/Protected
const PURPLE: Color = Color::Rgb(189, 147, 249);   // #BD93F9 - Current branch
const PINK: Color = Color::Rgb(255, 121, 198);     // #FF79C6 - Selected highlight
const GREEN: Color = Color::Rgb(80, 250, 123);     // #50FA7B - Success
```

**Widgets Used:**

- `Paragraph`: Header, footer, details pane
- `Table`: Branch list with columns (checkbox, status, name, time, label)
- `Tabs`: Filter tabs
- `List`: Action log entries
- `Block`: Borders and titles
- `Clear`: Modal backgrounds

### 7. `cache.rs` — SQLite PR Cache

**Responsibilities:**

- Persist PR data between runs in a local SQLite database (provider-neutral)
- Serve cached entries within the configured TTL (default: 1 hour)
- Evict stale entries and invalidate individual branch entries on deletion
- Expose session statistics (hits, misses, writes)

**Key Types:**

```rust
/// Outcome of a cache lookup.
pub enum CacheResult {
    /// Valid non-expired entry found. None inner means "no PR for this branch".
    Hit(Option<PrInfo>),
    /// No valid entry — caller must fetch from the API.
    Miss,
}

/// Session-level statistics.
pub struct CacheStats {
    pub hits: usize,
    pub misses: usize,
    pub writes: usize,
    pub total_entries: usize,
    pub oldest_entry_ts: Option<i64>,
}

/// The cache handle. One instance per run, opened in main().
pub struct PrCache {
    conn: rusqlite::Connection,
    repo: String,   // provider-aware partition key (see below)
    ttl: Duration,
    stats: CacheStats,
}
```

**Cache Partition Key:**

The partition key comes from the provider's `cache_key()`, so entries from different providers and
hosts never collide:

- GitHub: `owner/repo` (unchanged, preserves existing cache data)
- Bitbucket: `bitbucket-dc|<normalized-base-url>|<UPPERCASE_PROJECT_KEY>|<repository-slug>`, e.g.
  `bitbucket-dc|https://bitbucket.example.com|PROJ|my-repo`

The key never contains the token, username, or query parameters.

**Public API:**

```rust
// Open or create the on-disk database (XDG_CACHE_HOME/omni-scripts/pr-cache.db)
pub fn PrCache::open(repository: &str, ttl: Duration) -> Result<Self>
// Also: open_with_conn() for in-memory testing

pub fn get(&mut self, branch_name: &str) -> CacheResult
pub fn set(&mut self, branch_name: &str, pr_info: Option<&PrInfo>) -> Result<()>
pub fn evict_stale(&self, max_age: Duration) -> Result<usize>
pub fn invalidate(&self, branch_name: &str) -> Result<()>
pub fn stats(&self) -> &CacheStats
```

**Database Location:** `$XDG_CACHE_HOME/omni-scripts/pr-cache.db` (falls back to
`$HOME/.cache/omni-scripts/pr-cache.db`)

**Schema (v1):**

```sql
schema_migrations  -- tracks applied migration versions
repositories       -- one row per provider-aware cache key
cached_prs         -- one row per (repository, branch); NULL pr_number = "no PR"
```

**Cache Logic:**

- A `NULL pr_number` row means _"we asked the provider and found no PR"_ — a cache hit that prevents
  a redundant API call on the next run within TTL.
- An absent row means _"never queried"_ — a miss, API call required.
- Failed lookups are **never** cached — only successful results are written.
- Schema migrations run automatically on `open()`; the delta pattern means only new versions are
  applied, never re-run.

---

## Data Flow

### 1. Application Initialization

```
main() → verify_repo() → get_current_branch() → get_trunk_branch() → get_branches()
                                                                           ↓
                                          classify_branch() ← for each branch
                                                   ↓
                                          App::new(branches, trunk, current)

-- When --github or --bitbucket is active: --
main() → build provider → provider.validate() → PrCache::open(provider.cache_key(), 1h TTL)
              ↓                                              ↓
   GitHubProvider (gh CLI)  or             evict_stale(30d) then:
   BitbucketProvider (REST)                Pass 1 — sync cache hits (no I/O)
                                                 ↓
                                           Pass 2 — tokio JoinSet + Semaphore
                                                   (GitHub: 20, Bitbucket: 8)
                                                   spawn one task per cache miss
                                                   each task: provider.get_pr_for_branch()
                                                 ↓
                                           Pass 3 — write results back via cache.set()
                                                   (failed lookups are never cached)
                                                 ↓
                                           results ordered by original branch index
```

### 2. User Interaction (TUI Mode)

```
Keyboard Input → Crossterm Event
                      ↓
              KeyCode::Space → app.toggle_selection()
              KeyCode::Enter → app.show_confirmation = true
              KeyCode::Char('y') → app.delete_selected_branches()
                      ↓
              render_ui(&mut frame, &app)
```

### 3. Branch Deletion

```
User presses 'y' in confirmation modal
          ↓
app.delete_selected_branches() → for each selected branch
          ↓                               ↓
    get branch info              git::delete_branch(name, force)
          ↓                               ↓
    log result                      git branch -d/-D
          ↓
app.action_log.push(ActionLogEntry)
          ↓
git::get_branches() (refresh)
          ↓
app.branches = new_branches
          ↓
UI re-renders with updated list
```

### 4. Filtering

```
User presses '1' (Safe Merged filter)
          ↓
app.set_filter(FilterMode::SafeMerged)
          ↓
render_ui() → app.get_filtered_branches()
          ↓
Filter branches where status == SafeMerged
          ↓
Render only filtered branches in table
```

## Extension Points

### Adding New Branch Statuses

1. Add variant to `BranchStatus` enum in `git.rs`
2. Implement `label()`, `icon()`, and safety methods
3. Update `classify_branch()` logic
4. Add filter mode in `app.rs` (optional)
5. Update color mapping in `ui.rs`

Example:

```rust
// git.rs
pub enum BranchStatus {
    // ... existing variants
    Stale, // Branches not updated in 6+ months
}

impl BranchStatus {
    pub fn icon(&self) -> &'static str {
        match self {
            // ... existing icons
            BranchStatus::Stale => "⏰",
        }
    }
}

// Classify logic
pub fn classify_branch(name: &str, current: &str, trunk: &str) -> Result<BranchStatus> {
    // ... existing checks

    // Check if last commit is older than 6 months
    let last_commit = get_last_commit_date(name)?;
    if last_commit.elapsed() > Duration::from_secs(180 * 24 * 60 * 60) {
        return Ok(BranchStatus::Stale);
    }

    // ... rest of logic
}
```

### Adding New UI Sections

1. Add state to `App` struct in `app.rs`
2. Create rendering function in `ui.rs`
3. Update layout in `render_ui()`
4. Add keyboard shortcuts in `main.rs` event loop

Example (adding a "Recently Deleted" section):

```rust
// app.rs
pub struct App {
    // ... existing fields
    pub recently_deleted: Vec<String>,
}

// ui.rs
fn render_recently_deleted(area: Rect, frame: &mut Frame, app: &App) {
    let items: Vec<ListItem> = app.recently_deleted
        .iter()
        .map(|name| ListItem::new(format!("🗑️  {}", name)))
        .collect();

    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title("Recently Deleted"));

    frame.render_widget(list, area);
}
```

### Adding CLI Flags

1. Add field to `Args` struct in `main.rs`
2. Pass value to `App::new()` or use in git operations
3. Update help text

Example (adding `--remote` flag):

```rust
// main.rs
#[derive(Parser, Debug)]
struct Args {
    // ... existing fields

    /// Override the default remote name (default: origin)
    #[arg(long, default_value = "origin")]
    remote: String,
}

// git.rs
pub fn get_trunk_branch(override_trunk: Option<String>, remote: &str) -> Result<String> {
    // Try symbolic-ref with custom remote
    let output = Command::new("git")
        .args(["symbolic-ref", "--short", &format!("refs/remotes/{}/HEAD", remote)])
        .output()?;
    // ... rest of logic
}
```

### Adding New Filters

1. Add variant to `FilterMode` enum in `app.rs`
2. Implement filtering logic in `get_filtered_branches()`
3. Update `label()`, `next()`, `from_number()` methods
4. Add tab rendering in `ui.rs`
5. Add keyboard shortcut in `main.rs`

## Testing Strategy

### Unit Tests

Located in each module's `#[cfg(test)]` section:

- **git.rs**: Branch status classification, Git command parsing
- **remote.rs**: Remote URL parsing (HTTP(S), `ssh://`, SCP-like), Bitbucket repository inference,
  credential sanitization
- **pr/mod.rs**: PrState display, fetch coordinator ordering, cache hits, negative caching, and
  error handling via a fake provider (via `#[tokio::test]`)
- **pr/github.rs**: `gh` output parsing and error/no-result separation
- **pr/bitbucket.rs**: DTO deserialization, state mapping, and `wiremock`-based HTTP mock tests
  (auth headers, query parameters, HTTP status handling, retries — no real network)
- **cache.rs**: Cache hit/miss/expiry, multi-repo isolation (including the namespaced Bitbucket
  key), overwrite, eviction, schema migration
- **app.rs**: State transitions, filtering, selection logic

Run: `cargo test`

### Integration Tests

Located in `tests/integration_test.rs`:

- CLI flag handling (`--sequential`, `--github`, `--bitbucket` and its overrides, `--dry-run`,
  `--trunk`, `--force`)
- Real Git repository scenarios (merged/unmerged/protected branches)
- Error handling (non-git directory, empty repo)

Run: `cargo test --test integration_test`

**Current coverage: 85 tests (69 unit + 16 integration)**

### Manual Testing

See [TESTING.md](TESTING.md) for comprehensive manual testing checklist.

## Dependencies

### Core Dependencies

| Crate         | Version | Purpose                                                       |
| ------------- | ------- | ------------------------------------------------------------- |
| `ratatui`     | 0.30.0  | TUI framework                                                 |
| `crossterm`   | 0.29.0  | Terminal backend (cross-platform)                             |
| `clap`        | 4.5.57  | CLI argument parsing                                          |
| `color-eyre`  | 0.6.5   | Error handling and reporting                                  |
| `chrono`      | 0.4.43  | Date/time formatting                                          |
| `tokio`       | 1       | Async runtime (`rt-multi-thread`, `process`, `sync`)          |
| `rusqlite`    | 0.31    | SQLite client (bundled libsqlite3)                            |
| `dirs`        | 5.0     | XDG-compliant cache directory                                 |
| `async-trait` | 0.1     | Object-safe async provider trait                              |
| `reqwest`     | 0.12    | HTTP client for the Bitbucket provider (rustls, native roots) |
| `serde`       | 1       | Typed deserialization of Bitbucket responses                  |
| `serde_json`  | 1       | JSON support                                                  |
| `url`         | 2       | Base URL normalization and query encoding                     |

### Development Dependencies

| Crate        | Purpose                                    |
| ------------ | ------------------------------------------ |
| `tempfile`   | Temporary Git repos for testing            |
| `assert_cmd` | CLI testing                                |
| `predicates` | Assertion helpers                          |
| `wiremock`   | Local HTTP mock server for Bitbucket tests |

## Performance Considerations

### Current Performance Profile

- **Startup time**: < 1s for repos with < 50 branches
- **Memory usage**: ~5MB for typical repos
- **Git command overhead**: Sequential execution
- **GitHub PR fetching (cold, sequential)**: ~0.75s per branch (~60s for 81 branches)
- **GitHub PR fetching (cold, tokio concurrent)**: ~3.5s for 81 branches (**17.6× faster** — Phase
  2.1 ✅)
- **GitHub PR fetching (warm cache)**: < 100ms total — served from SQLite (Phase 1 ✅)

### Optimization Opportunities

1. **Parallel Git queries**: Use `tokio` to classify branches concurrently
2. **Lazy loading**: Only fetch commit details for visible branches
3. **Pagination**: Limit displayed branches to viewport + buffer

Example parallel classification:

```rust
use tokio::task::JoinSet;

pub async fn classify_branches_concurrent(names: Vec<String>) -> Vec<BranchInfo> {
    let mut set = JoinSet::new();
    for name in names {
        set.spawn(async move { get_branch_info(name).await });
    }
    let mut results = vec![];
    while let Some(res) = set.join_next().await {
        if let Ok(Ok(info)) = res { results.push(info); }
    }
    results
}
```

## Security Considerations

1. **Command Injection**: Git commands are executed with explicit arguments (no shell expansion)
2. **Path Traversal**: Repository path is validated with `git rev-parse`
3. **Force Delete**: Requires explicit opt-in via `--force` flag
4. **Protected Branches**: Hardcoded list prevents accidental deletion

## Future Enhancements

See the [roadmap](../specs/branch-cleanup-tui-roadmap.md) for planned features:

- Remote upstream fetching before scan
- Undo/redo for deletions
- Branch age sparklines
- Export action log to file
- Configuration file support
- Multiple repository support

## References

- [Ratatui Documentation](https://ratatui.rs/)
- [Crossterm Documentation](https://docs.rs/crossterm/)
- [Git Internals](https://git-scm.com/book/en/v2/Git-Internals-Plumbing-and-Porcelain)
- [Project Roadmap](../specs/branch-cleanup-tui-roadmap.md)
