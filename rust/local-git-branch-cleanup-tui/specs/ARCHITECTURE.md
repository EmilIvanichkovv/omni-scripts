# Architecture Documentation

## Overview

`local-git-branch-cleanup-tui` is a Rust application that provides both CLI and TUI (Terminal User Interface) modes for managing local Git branches. The architecture follows a modular design with clear separation of concerns.

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
           │                    │                    │
           ▼                    ▼                    ▼
    ┌──────────┐         ┌──────────┐         ┌──────────┐
    │  app.rs  │         │  git.rs  │         │  ui.rs   │
    └──────────┘         └──────────┘         └──────────┘
    Application          Git Integration      UI Rendering
    State Logic          & Classification     (Ratatui)
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

### 3. `git.rs` - Git Integration & Branch Classification

**Responsibilities:**
- Execute Git commands via `std::process::Command`
- Parse Git output into structured data
- Classify branches by status
- Determine trunk branch
- Perform branch deletions

**Key Types:**

```rust
/// Branch status classification
pub enum BranchStatus {
    SafeMerged,      // Merged into trunk
    GoneUpstream,    // Remote was deleted
    Unmerged,        // Has unmerged commits
    Protected,       // main/master/develop
    Current,         // Currently checked out
}

/// Complete branch information
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
```

**Git Command Usage:**

| Purpose | Git Command |
|---------|-------------|
| Verify repo | `git rev-parse --show-toplevel` |
| Current branch | `git branch --show-current` |
| Trunk detection | `git symbolic-ref --short refs/remotes/origin/HEAD` |
| Branch list | `git for-each-ref refs/heads/` |
| Merged check | `git branch --format='%(refname:short)' --merged <trunk>` |
| Gone check | Parse `[gone]` from `git for-each-ref` |
| Commit info | `git log -1 --format="%cr\|%h\|%an\|%s"` |
| Ahead/behind | `git rev-list --left-right --count <branch>...<upstream>` |
| Delete | `git branch -d/-D <branch>` |

**Classification Logic:**

```
1. Current branch check  → BranchStatus::Current
2. Protected name check  → BranchStatus::Protected (main/master/develop)
3. Merged check          → BranchStatus::SafeMerged
4. Gone upstream check   → BranchStatus::GoneUpstream
5. Default               → BranchStatus::Unmerged
```

### 4. `ui.rs` - Terminal User Interface Rendering

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

## Data Flow

### 1. Application Initialization

```
main() → verify_repo() → get_current_branch() → get_trunk_branch() → get_branches()
                                                                           ↓
                                          classify_branch() ← for each branch
                                                   ↓
                                          App::new(branches, trunk, current)
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
- **app.rs**: State transitions, filtering, selection logic

Run: `cargo test`

### Integration Tests

Located in `tests/integration_test.rs`:

- CLI flag handling
- Real Git repository scenarios
- Error handling

Run: `cargo test --test integration_test`

### Manual Testing

See [TESTING.md](TESTING.md) for comprehensive manual testing checklist.

## Dependencies

### Core Dependencies

| Crate | Version | Purpose |
|-------|---------|---------|
| `ratatui` | 0.30.0 | TUI framework |
| `crossterm` | 0.29.0 | Terminal backend (cross-platform) |
| `clap` | 4.5.57 | CLI argument parsing |
| `color-eyre` | 0.6.5 | Error handling and reporting |
| `chrono` | 0.4.43 | Date/time formatting |

### Development Dependencies

| Crate | Purpose |
|-------|---------|
| `tempfile` | Temporary Git repos for testing |
| `assert_cmd` | CLI testing |
| `predicates` | Assertion helpers |

## Performance Considerations

### Current Performance Profile

- **Startup time**: < 1s for repos with < 50 branches
- **Memory usage**: ~5MB for typical repos
- **Git command overhead**: Sequential execution (no parallelism yet)

### Optimization Opportunities

1. **Parallel Git queries**: Use `rayon` to classify branches concurrently
2. **Caching**: Cache Git command results between renders
3. **Lazy loading**: Only fetch commit details for visible branches
4. **Pagination**: Limit displayed branches to viewport + buffer

Example parallel classification:
```rust
use rayon::prelude::*;

pub fn get_branches() -> Result<Vec<BranchInfo>> {
    let branch_names = get_all_branch_names()?;
    
    let branches: Vec<BranchInfo> = branch_names
        .par_iter()  // Parallel iterator
        .map(|name| get_branch_info(name))
        .collect::<Result<Vec<_>>>()?;
    
    Ok(branches)
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
