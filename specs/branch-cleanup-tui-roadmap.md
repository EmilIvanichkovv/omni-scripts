# Branch Cleanup TUI - Project Roadmap & Status

**Project**: Rust-based TUI replacement for `local-git-branch-cleanup.sh`  
**Target Name**: `local-git-branch-cleanup-tui`  
**Location**: `./rust/` directory in repository  
**Status**: 🚀 M2 Complete → Starting M3  
**Last Updated**: 2026-02-11

---

## Executive Summary

Replace the existing bash script [`bash/local-git-branch-cleanup.sh`](../bash/local-git-branch-cleanup.sh) with a modern Rust-based TUI (Text User Interface) that provides interactive branch cleanup capabilities.

**Current Bash Script Behavior:**
- Scans for local branches without remote counterparts
- Lists branches with last update time
- Prompts for confirmation before deletion
- Uses box-drawing characters for UI
- Force deletes (`-D`) all selected branches

**Proposed Enhancement:**
Interactive TUI with better branch classification, selective deletion, and improved UX.

---

## Project Milestones

### ✅ Milestone 0: Analysis & Planning
**Status**: ✅ Complete (2026-02-11)  
**Goal**: Validate requirements and finalize roadmap

- [x] Review existing bash script implementation
- [x] Document current functionality gaps
- [x] Finalize MVP feature set
- [x] Review and approve roadmap

#### Analysis Summary

**Current Bash Script Capabilities:**
1. ✅ Scans for branches without remote tracking (`git rev-parse --abbrev-ref --symbolic-full-name "$branch@{u}"`)
2. ✅ Shows last commit time for each branch (`git log -1 --format="%cr"`)
3. ✅ Displays branches in formatted box UI with ANSI colors
4. ✅ Requires explicit confirmation before deletion
5. ✅ Force deletes all selected branches (`git branch -D`)
6. ✅ Shows summary of deleted branches

**Identified Functionality Gaps:**

| Gap | Description | Priority | MVP? |
|-----|-------------|----------|------|
| **No selective deletion** | All-or-nothing: deletes all found branches or none | High | ✅ Yes |
| **No branch classification** | Doesn't distinguish merged vs unmerged branches | Medium | ✅ Yes |
| **Force delete only** | Always uses `-D` (dangerous for unmerged work) | High | ✅ Yes |
| **No "gone" upstream detection** | Misses branches whose remote was deleted after fetch | Medium | ✅ Yes |
| **Limited information** | Only shows last commit time, no ahead/behind info | Low | ❌ No |
| **No branch preview** | Can't inspect branch details before deciding | Low | ❌ No |
| **Static list** | Can't re-scan after partial cleanup | Low | ❌ No |

**Safety Concerns with Current Script:**
- ⚠️ Uses `-D` (force delete) exclusively - can lose unmerged work
- ⚠️ No protection for `main`/`master`/current branch - relies on "no remote" check
- ⚠️ Deletes ALL found branches if confirmed - no granular control

#### Finalized MVP Feature Set

**MVP = Milestone 1 + 2 + 3 + 4 + 5**

**Must Have (P0):**
- ✅ M1: Development environment setup
- ✅ M2: Core git integration (bash script parity)
- ✅ M3: Enhanced branch classification (safe/gone/unmerged)
- ✅ M4: Basic TUI with scrolling and navigation
- ✅ M5: Interactive selection and deletion

**Key MVP Improvements Over Bash Script:**
1. **Safety**: Uses `-d` by default (safe delete), only `-D` when explicitly enabled
2. **Selectivity**: Individual branch selection with checkboxes
3. **Intelligence**: Classifies branches (merged, gone, unmerged, protected)
4. **Protection**: Prevents deletion of main/master/develop/current branch
5. **Visibility**: Clear status indication for each branch
6. **Control**: Interactive TUI vs static list

**Explicitly Out of MVP (P2-P3):**
- ❌ Filter tabs (can filter mentally with good status icons)
- ❌ Details pane (branch name + status is sufficient)
- ❌ Dry run mode (confirmation modal provides safety)
- ❌ Help modal (footer key hints sufficient for MVP)
- ❌ CLI flags beyond `--trunk` (can add incrementally)
- ❌ Sparklines/fancy graphics (focus on function)

#### Risks Identified

**Technical Risks:**
- Git command parsing may vary across Git versions (need min version check)
- ANSI color support varies across terminals (test on Linux, macOS, Windows/WSL)
- Large repos (>200 branches) may have performance issues (add pagination if needed)

**Project Risks:**
- Scope creep: Original spec had many P2/P3 features - now tightly scoped
- Adoption resistance: Users may prefer familiar bash script - need migration docs
- Edge cases: Detached HEAD, submodules, worktrees need testing

#### Success Criteria for MVP

**Functional:**
- [ ] 100% feature parity with bash script (M2)
- [ ] Enhanced safety: never deletes protected branches
- [ ] Enhanced control: per-branch selection
- [ ] Better classification: merged/gone/unmerged status

**Non-Functional:**
- [ ] Starts in <1s on repo with <50 branches
- [ ] No crashes on invalid input
- [ ] Clean error messages (no git command output leaks)

**Roadmap Approval:** ✅ Approved for implementation

---

### ✅ Milestone 1: Development Environment Setup
**Status**: ✅ Complete (2026-02-11)  
**Priority**: P0 (Blocker)  
**Estimated Effort**: 2-4 hours (Actual: ~2 hours)

#### Tasks
1. **Rust Environment via Nix** ✅
   - ~~Option A: Use ephemeral shell: `nix develop nixpkgs#cargo`~~
   - ✅ Option B: Added `devShells.rust-tui` to existing `flake.nix`
   - Updated nixpkgs to `nixos-unstable` for modern Rust toolchain
   - Included: cargo, rustc, rustfmt, clippy, rust-analyzer, git

2. **Project Initialization** ✅
   - Created `./rust` directory
   - Ran `cargo init --bin --name local-git-branch-cleanup-tui`
   - Set up `.gitignore` for Rust artifacts

3. **Dependency Management** ✅
   - Added core dependencies:
     - `ratatui` v0.30.0 (TUI framework) with `all-widgets` feature
     - `crossterm` v0.29.0 (terminal backend)
     - `color-eyre` v0.6.5 (error handling)
     - `clap` v4.5.57 (CLI parsing) with `derive` feature
     - `chrono` v0.4.43 (date formatting) with `clock` feature

4. **Module Structure** ✅
   - Created `src/app.rs` - Application state with navigation methods
   - Created `src/git.rs` - Git integration functions (bash script parity)
   - Created `src/ui.rs` - TUI rendering placeholder (for M4)
   - Updated `src/main.rs` - Entry point with CLI arg parsing

#### Acceptance Criteria
- [x] `cargo build` completes successfully
- [x] `cargo run` executes placeholder app
- [x] All team members can reproduce environment via `nix develop .#rust-tui`

#### Notes
- Upgraded nixpkgs from `24.05` to `unstable` to resolve Rust edition2024 requirement
- Module stubs include basic implementation ready for M2
- All dependencies compile without errors

---

### ✅ Milestone 2: Core Git Integration (MVP)
**Status**: ✅ Complete (2026-02-11)  
**Priority**: P0 (Blocker)  
**Estimated Effort**: 8-16 hours (Actual: ~2 hours)  
**Depends On**: M1

#### Goals
Replicate existing bash script functionality using Rust + Git commands.

#### Tasks
1. **Repository Detection** (`git.rs`)
   - Verify inside Git repo: `git rev-parse --show-toplevel`
   - Get current branch: `git branch --show-current`
   - Error handling for non-repo directories

2. **Branch Discovery**
   - Implement exact logic from bash script:
     ```rust
     fn get_branches_without_remote() -> Result<Vec<BranchInfo>>
     ```
   - For each branch, check if upstream exists
   - Parse last commit time: `git log -1 --format="%cr" <branch>`

3. **Branch Data Model**
   ```rust
   struct BranchInfo {
       name: String,
       upstream: Option<String>,
       last_commit_relative: String, // "2 days ago"
   }
   ```

4. **Branch Deletion**
   - Implement force delete: `git branch -D <name>`
   - Capture stdout/stderr
   - Track deleted branches

#### Acceptance Criteria
- [x] Matches bash script output for test repositories
- [x] Correctly identifies branches without remotes
- [x] Successfully deletes branches after confirmation
- [x] Handles errors gracefully (branch doesn't exist, etc.)

#### Implementation Notes
- Implemented as CLI (not TUI) to match bash script exactly
- Verified on real repository with feat/TUI branch
- Both bash and Rust versions find identical branches
- Error handling includes worktree protection (cannot delete checked-out branches)
- Force delete (`-D`) used for M2 parity, will switch to safe delete (`-d`) in M3

#### Out of Scope (for M2)
- Advanced branch classification (merged/unmerged) → M3
- Detecting "gone" upstreams → M3
- Interactive TUI → M4
- Safe delete with `-d` → M3 (M2 uses `-D` for parity)

---

### 🎯 Milestone 3: Enhanced Branch Classification
**Status**: 🔴 Not Started  
**Priority**: P1 (High Value)  
**Estimated Effort**: 12-20 hours  
**Depends On**: M2

#### Goals
Improve branch analysis beyond "has remote" / "no remote".

#### Tasks
1. **Detect Default Branch**
   - Try `git symbolic-ref --short refs/remotes/origin/HEAD`
   - Fallback to `origin/main`
   - CLI override: `--trunk <branch>`

2. **Branch Status Classification**
   ```rust
   enum BranchStatus {
       SafeMerged,      // Merged into trunk
       GoneUpstream,    // Remote deleted
       Unmerged,        // Active feature branch
       Protected,       // main/master/develop
       Current,         // Current working branch
   }
   ```

3. **Merged Branch Detection**
   - `git branch --format='%(refname:short)' --merged <trunk>`
   - Exclude protected branches (main, master, develop, current)

4. **Gone Upstream Detection**
   - Parse `git branch -vv` for `[gone]` marker
   - Or use `git for-each-ref` with upstream tracking

5. **Ahead/Behind Calculation**
   - `git rev-list --left-right --count <branch>...<upstream>`
   - Parse into ahead/behind counts

6. **Protection Rules**
   - Never allow deletion of:
     - `main`, `master`, `develop`
     - Current branch
     - Trunk branch

#### Acceptance Criteria
- [ ] Correctly classifies 5 test scenarios
- [ ] Protected branches cannot be selected
- [ ] Merged branches identified accurately
- [ ] Gone branches detected after `git fetch --prune`

#### Risks
- **Risk**: Different Git configurations (e.g., `origin` vs other remotes)
- **Mitigation**: Support `--remote <name>` CLI flag

---

### 🎯 Milestone 4: Basic TUI (Non-Interactive List)
**Status**: 🔴 Not Started  
**Priority**: P1  
**Estimated Effort**: 16-24 hours  
**Depends On**: M3

#### Goals
Display branch information in terminal UI, read-only initially.

#### Tasks
1. **Ratatui Initialization**
   - Follow [hello-ratatui tutorial](https://ratatui.rs/tutorials/hello-ratatui/)
   - Set up terminal init/restore pattern
   - Basic event loop with quit (`q`)

2. **Layout Structure**
   ```
   ┌─────────────────────────────────────────┐
   │ HEADER: App Name + Repo Info           │
   ├─────────────────────────────────────────┤
   │ BRANCH LIST (scrollable)               │
   │                                         │
   ├─────────────────────────────────────────┤
   │ FOOTER: Key hints                      │
   └─────────────────────────────────────────┘
   ```

3. **Branch List Table**
   - Columns:
     - Status Icon (●/◆/▲/⏺/⛔)
     - Branch Name
     - Last Commit Age
     - Status Label
   - Scrolling with arrow keys or `j/k`
   - Highlight selected row

4. **Color Palette**
   - Background: `#101216` (deep charcoal)
   - Accent: `#2EC4B6` (cyan) for selection
   - Warning: `#FFB86C` (amber) for unmerged
   - Danger: `#FF5555` (red) for protected
   - Muted: `#A9B1D6` for normal text

5. **Header & Footer**
   - Header: App name, repo path, trunk branch
   - Footer: Navigation keys, quit hint

#### Acceptance Criteria
- [ ] Displays all branches from M3
- [ ] Scrollable list with keyboard navigation
- [ ] Status colors match specification
- [ ] Graceful handling of small terminal sizes

#### Out of Scope (for M4)
- Selection/deletion (read-only view)
- Filters
- Details pane

---

### 🎯 Milestone 5: Interactive Selection & Deletion
**Status**: 🔴 Not Started  
**Priority**: P0 (Core Functionality)  
**Estimated Effort**: 12-16 hours  
**Depends On**: M4

#### Goals
Enable branch selection and deletion within TUI.

#### Tasks
1. **Selection Mechanism**
   - Add checkbox column: `[x]` / `[ ]`
   - `Space` to toggle selection (if not protected)
   - Visual indication of selected branches

2. **Deletion Flow**
   - `Enter` opens confirmation modal
   - Modal shows:
     - Number of branches to delete
     - Command preview: `git branch -d <name>`
     - Warning for unmerged branches
   - `y` confirms, `n` cancels
   - Execute deletions sequentially

3. **Action Log**
   - Add log section (bottom or right pane)
   - Display:
     - `✓ Deleted: branch-name`
     - `✗ Error: reason`
   - Scrollable history

4. **Post-Deletion Refresh**
   - Re-scan branches after deletion
   - Update list automatically
   - Show summary: "Deleted 5 branches"

5. **Bulk Selection**
   - `a` key: select/deselect all safe branches
   - Smart selection based on current filter

#### Acceptance Criteria
- [ ] Can select individual branches
- [ ] Confirmation modal prevents accidental deletion
- [ ] Successful deletions logged
- [ ] Protected branches cannot be selected
- [ ] List updates after deletion

#### Risks
- **Risk**: Partial failures during batch deletion
- **Mitigation**: Continue on error, log all outcomes

---

### 🎯 Milestone 6: Filters & Details Pane
**Status**: 🔴 Not Started  
**Priority**: P2 (Nice to Have)  
**Estimated Effort**: 8-12 hours  
**Depends On**: M5

> **Note**: This is beyond MVP. M1-M5 deliver complete functional replacement.

#### Goals
Add filtering and detailed branch information view.

#### Tasks
1. **Filter Tabs**
   ```
   [ SAFE MERGED (N) ] [ UPSTREAM GONE (M) ] [ UNMERGED (K) ] [ ALL (T) ]
   ```
   - Keys `1-4` or `F1-F4` to switch
   - `Tab` to cycle
   - Show branch count per filter

2. **Details Pane** (Right Side, 30% width)
   - Branch name (large)
   - Status explanation
   - Upstream info
   - Ahead/behind details
   - Last commit SHA, author, message

3. **Split Layout**
   ```
   ┌─────────────────────────────────────────┐
   │ HEADER                                  │
   ├─────────────────────────────────────────┤
   │ FILTERS                                 │
   ├──────────────────┬──────────────────────┤
   │ BRANCH LIST      │ DETAILS PANE        │
   │                  │                      │
   ├──────────────────┴──────────────────────┤
   │ ACTION LOG                              │
   ├─────────────────────────────────────────┤
   │ FOOTER                                  │
   └─────────────────────────────────────────┘
   ```

#### Acceptance Criteria
- [ ] Filters work correctly
- [ ] Details pane updates on selection
- [ ] Layout adapts to terminal size

---

### 🎯 Milestone 7: Advanced Features & Polish
**Status**: 🔴 Not Started  
**Priority**: P3 (Optional)  
**Estimated Effort**: 8-16 hours  
**Depends On**: M6

#### Tasks
1. **Force Delete Mode**
   - `!` or `Shift+D` to enable
   - Warning in footer: "FORCE MODE ENABLED"
   - Allow deleting unmerged branches with `git branch -D`

2. **Dry Run Mode**
   - `d` to toggle
   - Show preview of actions without executing
   - Footer indicator: "DRY RUN: ON"

3. **Help Modal**
   - `?` key shows comprehensive key map
   - Centered overlay with all shortcuts

4. **CLI Flags**
   ```bash
   --trunk <branch>    # Override default branch
   --remote <name>     # Override default remote (origin)
   --no-fetch          # Skip git fetch --prune
   --force             # Enable force delete by default
   --dry-run           # Preview mode
   ```

5. **Performance Optimization**
   - Parallel branch queries where possible
   - Caching of git commands
   - Progress indicator for slow operations

6. **Visual Enhancements**
   - Nerd Font icons (with ASCII fallback)
   - Sparkline of commit activity (optional)
   - Animations for deletion (optional)

#### Acceptance Criteria
- [ ] All CLI flags functional
- [ ] Help is comprehensive
- [ ] Handles large repos (>100 branches) smoothly

---

### 🎯 Milestone 8: Testing & Validation
**Status**: 🔴 Not Started  
**Priority**: P1  
**Estimated Effort**: 8-12 hours  
**Depends On**: M5

#### Tasks
1. **Unit Tests**
   - Git command parsing
   - Branch classification logic
   - Status determination

2. **Integration Tests**
   - Create test Git repositories
   - Scenarios:
     - Repo with merged branches
     - Branches with gone upstreams
     - Mixed protected/safe branches
     - Non-git directory
     - Empty repository

3. **Manual Testing Checklist**
   - [ ] Bash script parity verification
   - [ ] Small repo (<10 branches)
   - [ ] Large repo (>100 branches)
   - [ ] Various terminal sizes
   - [ ] Error scenarios (network issues, permissions)

4. **Edge Cases**
   - Current branch on deleted remote
   - Detached HEAD state
   - Shallow clones
   - Submodules
   - Worktrees

#### Acceptance Criteria
- [ ] 80%+ code coverage
- [ ] All critical paths tested
- [ ] No regressions from bash script

---

### 🎯 Milestone 9: Documentation & Deployment
**Status**: 🔴 Not Started  
**Priority**: P1  
**Estimated Effort**: 4-8 hours  
**Depends On**: M8

#### Tasks
1. **User Documentation**
   - README for `./rust` directory
   - Usage examples
   - Screenshot/GIF of TUI
   - Comparison with bash script

2. **Developer Documentation**
   - Architecture overview
   - Module responsibilities
   - Extension points

3. **Nix Packaging**
   - Add to `pkgs/local-git-branch-cleanup/default.nix`
   - Build Rust binary with Nix
   - Installation via `nix profile install`

4. **Migration Guide**
   - How to switch from bash to Rust version
   - Feature comparison table
   - Rollback instructions

5. **CI/CD** (if applicable)
   - GitHub Actions for testing
   - Binary releases for major platforms
   - Changelog automation

#### Acceptance Criteria
- [ ] Complete README with examples
- [ ] Nix package builds successfully
- [ ] Migration path documented

---

## Technical Decisions

### ✅ Decided
- **Language**: Rust
- **TUI Framework**: Ratatui + Crossterm
- **CLI Parsing**: clap
- **Git Integration**: `std::process::Command` (not git2 library)
- **Development Environment**: Nix

### ⚠️ Pending Decisions
- ~~**Nix Integration Approach**: Ephemeral shell vs flake devShell?~~ ✅ Decided: flake devShell
- **Testing Framework**: Built-in + which test harness?
- **Release Strategy**: Replace bash script or run side-by-side?
- **Performance Target**: Max acceptable branches count?

---

## Risk Register

| Risk | Probability | Impact | Mitigation |
|------|-------------|--------|------------|
| Git command parsing fragile | Medium | High | Comprehensive test suite, version checks |
| Terminal compatibility issues | Low | Medium | Fallback to ASCII, test on multiple terminals |
| Performance with large repos | Medium | Medium | Parallel queries, pagination, profiling |
| Nix environment issues | Low | Medium | Document rustup fallback |
| Scope creep | High | Medium | Strict MVP definition, milestone gates |

---

## Success Metrics

### MVP Success (M1-M5)
- [ ] Feature parity with bash script
- [ ] Zero data loss incidents (branch safety)
- [ ] Positive team feedback

### Complete Success (M9)
- [ ] 50%+ adoption over bash script
- [ ] <1s startup time for repos with <50 branches
- [ ] Zero critical bugs in first month

---

## Getting Started Guide

### For Implementers

**Recommended Starting Point**: Milestone 1 → Milestone 2

**Why this order?**
1. **M1 (Environment)**: Unblocks all development
2. **M2 (Core Git)**: Validates core assumptions, achieves bash script parity
3. **M3 (Classification)**: Adds value without UI complexity
4. **M4-M5 (TUI)**: Builds on stable foundation

**First Week Goal**: Complete M1 + M2, demonstrating CLI version with exact bash script behavior.

**Quick Start**:
```bash
# From repo root
nix develop nixpkgs#cargo
mkdir -p rust && cd rust
cargo init --bin local-git-branch-cleanup-tui
cargo add ratatui crossterm color-eyre clap chrono

# Create module stubs
touch src/app.rs src/git.rs src/ui.rs

# Start with git.rs - replicate bash script logic
```

### For Reviewers
- **M2 Checkpoint**: Verify CLI version matches bash output
- **M4 Checkpoint**: UX review of TUI layout
- **M8 Checkpoint**: Security & safety review before release

---

## References

- Original bash script: [`bash/local-git-branch-cleanup.sh`](../bash/local-git-branch-cleanup.sh)
- Ratatui docs: https://ratatui.rs/
- Nix Rust guide: https://wiki.nixos.org/wiki/Rust

---

## Changelog

- **2026-02-11**: 
  - Initial roadmap created
  - ✅ Milestone 0 completed: Analysis & planning finalized
  - MVP scope defined (M1-M5)
  - Identified 7 functionality gaps in bash script
  - Documented 3 major safety concerns
  - Roadmap approved for implementation
  - ✅ Milestone 1 completed: Development environment setup
    - Created rust-tui devShell in flake.nix
    - Upgraded nixpkgs to unstable for modern Rust
    - Initialized Cargo project with all dependencies
    - Created module structure (app.rs, git.rs, ui.rs)
    - Verified build and execution
  - ✅ Milestone 2 completed: Core Git Integration (MVP)
    - Implemented CLI interface with full bash script parity
    - Added repository detection and verification
    - Implemented branch scanning for branches without remote counterparts
    - Added interactive confirmation prompt
    - Implemented branch deletion with comprehensive error handling
    - Verified behavior matches bash script exactly
    - Successfully tested on live repository
