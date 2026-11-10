// Worktree view (`--worktrees` / `w`): the view model for each worktree,
// background `git status` loading, filters, search, selection rules, removal
// and clipboard support. Parsing `git worktree list` lives in `crate::worktree`.

use crate::app::{branch_delete_needs_force, ActionLogEntry};
use crate::git::{BranchInfo, BranchStatus};
use crate::pr::PrInfo;
use crate::worktree::{self, WorktreeEntry};
use color_eyre::Result;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread;

/// Upper bound on concurrent `git status` processes
const MAX_STATUS_WORKERS: usize = 8;

/// How many changed paths the details pane lists per group
pub const MAX_CHANGED_PATHS: usize = 5;

/// The branch-classifier status of a worktree's branch.
///
/// The only place the worktree view reads a branch's status. A branch held by
/// another worktree is `protected-wt` in the branch view; here its
/// merge-based status (`worktree_shadowed_status`) is what matters, both for
/// the State column and for deleting the branch once its worktree is gone.
pub fn worktree_branch_status(b: &BranchInfo) -> BranchStatus {
    b.worktree_shadowed_status
        .clone()
        .unwrap_or_else(|| b.status.clone())
}

/// Counts from `git status --porcelain=v1`
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChangeCounts {
    /// Files with unstaged changes in the working tree
    pub modified: usize,
    /// Files with staged changes in the index
    pub staged: usize,
    /// Untracked files (or directories), shown as "new" (`N`)
    pub untracked: usize,
    /// Number of status lines (changed paths)
    pub files: usize,
    /// The first few paths with unstaged changes
    pub modified_paths: Vec<String>,
    /// The first few paths with staged changes
    pub staged_paths: Vec<String>,
    /// The first few untracked paths
    pub new_paths: Vec<String>,
}

/// One group of changed paths in the details pane
pub struct ChangeGroup<'a> {
    pub label: &'static str,
    pub count: usize,
    pub paths: &'a [String],
}

impl ChangeCounts {
    pub fn is_clean(&self) -> bool {
        self.files == 0
    }

    /// The non-empty groups in display order: Modified, Staged, New (the
    /// same order as `compact()`)
    pub fn groups(&self) -> Vec<ChangeGroup<'_>> {
        [
            ("Modified", self.modified, &self.modified_paths),
            ("Staged", self.staged, &self.staged_paths),
            ("New", self.untracked, &self.new_paths),
        ]
        .into_iter()
        .filter(|(_, count, _)| *count > 0)
        .map(|(label, count, paths)| ChangeGroup {
            label,
            count,
            paths,
        })
        .collect()
    }

    /// Compact form for the table: `clean` or e.g. `3M 1S 2N`
    pub fn compact(&self) -> String {
        if self.is_clean() {
            return "clean".to_string();
        }
        let mut parts = Vec::new();
        if self.modified > 0 {
            parts.push(format!("{}M", self.modified));
        }
        if self.staged > 0 {
            parts.push(format!("{}S", self.staged));
        }
        if self.untracked > 0 {
            parts.push(format!("{}N", self.untracked));
        }
        parts.join(" ")
    }
}

/// Parse `git status --porcelain=v1` output into change counts.
///
/// Each line is `XY path`: `??` is untracked, otherwise a non-space `X` is a
/// staged change and a non-space `Y` an unstaged one (a file can be both, and
/// is then listed in both groups).
pub fn parse_porcelain_status(output: &str) -> ChangeCounts {
    fn push_capped(paths: &mut Vec<String>, path: &str) {
        if paths.len() < MAX_CHANGED_PATHS {
            paths.push(path.to_string());
        }
    }

    let mut counts = ChangeCounts::default();
    for line in output.lines() {
        if line.len() < 3 {
            continue;
        }
        let mut chars = line.chars();
        let x = chars.next().unwrap_or(' ');
        let y = chars.next().unwrap_or(' ');
        if x == '!' && y == '!' {
            continue; // ignored files (only with --ignored)
        }
        let path = line.get(3..).unwrap_or_default();
        if x == '?' && y == '?' {
            counts.untracked += 1;
            push_capped(&mut counts.new_paths, path);
        } else {
            if x != ' ' {
                counts.staged += 1;
                push_capped(&mut counts.staged_paths, path);
            }
            if y != ' ' {
                counts.modified += 1;
                push_capped(&mut counts.modified_paths, path);
            }
        }
        counts.files += 1;
    }
    counts
}

/// Uncommitted-change info for a worktree
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Changes {
    /// `git status` is still running in the background
    Loading,
    Loaded(ChangeCounts),
    /// `git status` failed (message from git)
    Failed(String),
    /// No working tree to inspect (prunable or bare)
    NotApplicable,
}

/// The state shown in the worktree table, highest priority first
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorktreeState {
    /// The worktree the tool runs in
    Current,
    /// The worktree directory is gone (`git worktree prune` cleans it up)
    Prunable,
    /// Locked with `git worktree lock`
    Locked,
    /// Uncommitted or untracked changes
    Dirty,
    /// Bare repository entry
    Bare,
    /// HEAD is detached
    Detached,
    /// The branch's own status from the branch classifier
    Branch(BranchStatus),
    /// Branch not found among local branches (e.g. unborn)
    Unknown,
}

impl WorktreeState {
    pub fn label(&self) -> &'static str {
        match self {
            WorktreeState::Current => "current",
            WorktreeState::Prunable => "prunable",
            WorktreeState::Locked => "locked",
            WorktreeState::Dirty => "dirty",
            WorktreeState::Bare => "bare",
            WorktreeState::Detached => "detached",
            WorktreeState::Branch(status) => status.label(),
            WorktreeState::Unknown => "unknown",
        }
    }

    pub fn icon(&self) -> &'static str {
        match self {
            WorktreeState::Current => "◉",
            WorktreeState::Prunable => "✗",
            WorktreeState::Locked => "🔒",
            WorktreeState::Dirty => "✎",
            WorktreeState::Bare => "▣",
            WorktreeState::Detached => "○",
            WorktreeState::Branch(status) => status.icon(),
            WorktreeState::Unknown => "?",
        }
    }
}

/// How a selected worktree gets removed
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemovalAction {
    /// `git worktree remove <path>`
    Remove,
    /// `git worktree remove --force <path>` (dirty worktree, force mode)
    ForceRemove,
    /// `git worktree prune` (directory already gone)
    Prune,
}

impl RemovalAction {
    /// Arguments passed to `git`
    pub fn git_args(&self, path: &Path) -> Vec<String> {
        let path = path.to_string_lossy().into_owned();
        match self {
            RemovalAction::Remove => vec!["worktree".into(), "remove".into(), path],
            RemovalAction::ForceRemove => {
                vec!["worktree".into(), "remove".into(), "--force".into(), path]
            }
            RemovalAction::Prune => vec!["worktree".into(), "prune".into()],
        }
    }

    /// The command as the user would type it
    pub fn command_line(&self, path: &Path) -> String {
        format!("git {}", self.git_args(path).join(" "))
    }
}

/// Last-commit summary of a worktree's HEAD
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CommitSummary {
    pub sha: String,
    pub author: String,
    pub subject: String,
    pub relative: String,
    pub timestamp: i64,
}

/// One row of the worktree view
#[derive(Debug, Clone)]
pub struct WorktreeInfo {
    pub entry: WorktreeEntry,
    /// The worktree the tool runs in
    pub is_current: bool,
    pub changes: Changes,
    /// Branch-classifier status of the checked-out branch
    pub branch_status: Option<BranchStatus>,
    pub upstream: Option<String>,
    pub ahead: Option<usize>,
    pub behind: Option<usize>,
    pub commit: CommitSummary,
    pub pr_info: Option<PrInfo>,
}

impl WorktreeInfo {
    pub fn name(&self) -> String {
        self.entry.name()
    }

    /// Branch name, `(detached @ <sha>)` or `(bare)`
    pub fn branch_label(&self) -> String {
        if self.entry.is_bare {
            return "(bare)".to_string();
        }
        match &self.entry.branch {
            Some(branch) => branch.clone(),
            None => format!("(detached @ {})", short_sha(&self.entry.head)),
        }
    }

    pub fn is_prunable(&self) -> bool {
        self.entry.prunable.is_some()
    }

    pub fn is_locked(&self) -> bool {
        self.entry.locked.is_some()
    }

    /// Status loaded and the worktree has uncommitted or untracked changes
    pub fn is_dirty(&self) -> bool {
        matches!(&self.changes, Changes::Loaded(c) if !c.is_clean())
    }

    /// Status loaded and the worktree has no changes
    pub fn is_clean(&self) -> bool {
        matches!(&self.changes, Changes::Loaded(c) if c.is_clean())
    }

    /// Directory gone, or the branch is merged / PR-merged / upstream gone
    pub fn is_stale(&self) -> bool {
        self.is_prunable()
            || matches!(
                self.branch_status,
                Some(
                    BranchStatus::SafeMerged | BranchStatus::PrMerged | BranchStatus::GoneUpstream
                )
            )
    }

    /// State shown in the table, in priority order: current, prunable,
    /// locked, dirty, then the branch's own status (or detached)
    pub fn state(&self) -> WorktreeState {
        if self.is_current {
            WorktreeState::Current
        } else if self.is_prunable() {
            WorktreeState::Prunable
        } else if self.is_locked() {
            WorktreeState::Locked
        } else if self.is_dirty() {
            WorktreeState::Dirty
        } else if self.entry.is_bare {
            WorktreeState::Bare
        } else if self.entry.branch.is_none() {
            WorktreeState::Detached
        } else {
            match &self.branch_status {
                Some(status) => WorktreeState::Branch(status.clone()),
                None => WorktreeState::Unknown,
            }
        }
    }

    /// How this worktree would be removed, or `None` when it cannot be
    /// selected: the main, current, locked and bare worktrees never are,
    /// dirty ones only in force mode, and ones whose status is still loading
    /// (or failed) wait until it is known.
    pub fn removal_action(&self, force_mode: bool) -> Option<RemovalAction> {
        if self.entry.is_main || self.is_current || self.is_locked() || self.entry.is_bare {
            return None;
        }
        if self.is_prunable() {
            return Some(RemovalAction::Prune);
        }
        match &self.changes {
            Changes::Loaded(c) if c.is_clean() => Some(RemovalAction::Remove),
            Changes::Loaded(_) if force_mode => Some(RemovalAction::ForceRemove),
            _ => None,
        }
    }

    pub fn is_selectable(&self, force_mode: bool) -> bool {
        self.removal_action(force_mode).is_some()
    }

    /// Case-insensitive match of `query` against name, branch and path
    pub fn matches_search(&self, query: &str) -> bool {
        let query = query.trim().to_lowercase();
        if query.is_empty() {
            return true;
        }
        self.name().to_lowercase().contains(&query)
            || self.branch_label().to_lowercase().contains(&query)
            || self
                .entry
                .path
                .to_string_lossy()
                .to_lowercase()
                .contains(&query)
    }
}

/// First 7 characters of a commit SHA
pub fn short_sha(sha: &str) -> &str {
    sha.get(..7).unwrap_or(sha)
}

/// Filter tabs of the worktree view
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorktreeFilter {
    All,
    Clean,
    Dirty,
    Stale,
}

impl WorktreeFilter {
    pub fn label(&self) -> &'static str {
        match self {
            WorktreeFilter::All => "ALL",
            WorktreeFilter::Clean => "CLEAN",
            WorktreeFilter::Dirty => "DIRTY",
            WorktreeFilter::Stale => "STALE",
        }
    }

    /// Cycle in tab order (Clean, Dirty, Stale, All)
    pub fn next(&self) -> Self {
        match self {
            WorktreeFilter::All => WorktreeFilter::Clean,
            WorktreeFilter::Clean => WorktreeFilter::Dirty,
            WorktreeFilter::Dirty => WorktreeFilter::Stale,
            WorktreeFilter::Stale => WorktreeFilter::All,
        }
    }

    pub fn matches(&self, wt: &WorktreeInfo) -> bool {
        match self {
            WorktreeFilter::All => true,
            WorktreeFilter::Clean => wt.is_clean(),
            WorktreeFilter::Dirty => wt.is_dirty(),
            WorktreeFilter::Stale => wt.is_stale(),
        }
    }
}

/// Order the table: main worktree first, then by last activity (newest first)
pub fn sort_worktrees(worktrees: &mut [WorktreeInfo]) {
    worktrees.sort_by(|a, b| {
        b.entry
            .is_main
            .cmp(&a.entry.is_main)
            .then(b.commit.timestamp.cmp(&a.commit.timestamp))
    });
}

/// Build the view model from `git worktree list` entries and the branch
/// classifier's output.
pub fn build_worktree_infos(
    entries: Vec<WorktreeEntry>,
    branches: &[BranchInfo],
    toplevel: &Path,
) -> Vec<WorktreeInfo> {
    let mut infos: Vec<WorktreeInfo> = entries
        .into_iter()
        .map(|entry| {
            let branch = entry
                .branch
                .as_deref()
                .and_then(|name| branches.iter().find(|b| b.name == name));
            let commit = match branch {
                Some(b) => CommitSummary {
                    sha: b.last_commit_sha.clone(),
                    author: b.last_commit_author.clone(),
                    subject: b.last_commit_message.clone(),
                    relative: b.last_commit_relative.clone(),
                    timestamp: b.last_activity_timestamp,
                },
                None => head_commit_summary(&entry.head).unwrap_or_default(),
            };
            let changes = if entry.prunable.is_some() || entry.is_bare {
                Changes::NotApplicable
            } else {
                Changes::Loading
            };
            WorktreeInfo {
                is_current: entry.is_at(toplevel),
                changes,
                branch_status: branch.map(worktree_branch_status),
                upstream: branch.and_then(|b| b.upstream.clone()),
                ahead: branch.and_then(|b| b.ahead),
                behind: branch.and_then(|b| b.behind),
                commit,
                pr_info: branch.and_then(|b| b.pr_info.clone()),
                entry,
            }
        })
        .collect();
    sort_worktrees(&mut infos);
    infos
}

/// Last-commit summary for a HEAD that is not a local branch (detached)
fn head_commit_summary(head: &str) -> Option<CommitSummary> {
    if head.is_empty() {
        return None;
    }
    let output = Command::new("git")
        .args(["log", "-1", "--format=%h%x1f%an%x1f%s%x1f%cr%x1f%ct", head])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut parts = text.trim_end().split('\x1f');
    Some(CommitSummary {
        sha: parts.next().unwrap_or_default().to_string(),
        author: parts.next().unwrap_or_default().to_string(),
        subject: parts.next().unwrap_or_default().to_string(),
        relative: parts.next().unwrap_or_default().to_string(),
        timestamp: parts.next().and_then(|t| t.parse().ok()).unwrap_or(0),
    })
}

/// Run `git status` in one worktree
fn read_changes(path: &Path) -> Changes {
    let output = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(["status", "--porcelain=v1"])
        // Status is read-only here; don't take index.lock from under the user
        .env("GIT_OPTIONAL_LOCKS", "0")
        .output();
    match output {
        Ok(out) if out.status.success() => Changes::Loaded(parse_porcelain_status(
            &String::from_utf8_lossy(&out.stdout),
        )),
        Ok(out) => Changes::Failed(String::from_utf8_lossy(&out.stderr).trim().to_string()),
        Err(e) => Changes::Failed(e.to_string()),
    }
}

/// A finished `git status` for one worktree
pub type StatusUpdate = (PathBuf, Changes);

/// Run `git status` for each path on a small pool of threads; results arrive
/// on the returned channel, which disconnects once all are done.
pub fn spawn_status_loaders(paths: Vec<PathBuf>) -> Receiver<StatusUpdate> {
    let (tx, rx) = mpsc::channel();
    let workers = paths.len().min(MAX_STATUS_WORKERS);
    // Popped from the back, so reverse to load in table order
    let queue = Arc::new(Mutex::new(paths.into_iter().rev().collect::<Vec<_>>()));
    for _ in 0..workers {
        let tx = tx.clone();
        let queue = Arc::clone(&queue);
        thread::spawn(move || loop {
            let next = queue.lock().ok().and_then(|mut q| q.pop());
            let Some(path) = next else { break };
            let changes = read_changes(&path);
            if tx.send((path, changes)).is_err() {
                break;
            }
        });
    }
    rx
}

/// List state of the worktree view
pub struct WorktreeView {
    pub worktrees: Vec<WorktreeInfo>,
    /// Selected worktrees, by path
    pub selected: HashSet<PathBuf>,
    pub filter: WorktreeFilter,
    /// Confirmation-modal toggle (`b`): also delete each removed worktree's branch
    pub delete_branches: bool,
    status_rx: Option<Receiver<StatusUpdate>>,
}

impl WorktreeView {
    /// List the repository's worktrees and start loading their status
    pub fn load(branches: &[BranchInfo], toplevel: &Path) -> Result<Self> {
        let infos = build_worktree_infos(worktree::list_worktrees()?, branches, toplevel);
        let pending: Vec<PathBuf> = infos
            .iter()
            .filter(|w| w.changes == Changes::Loading)
            .map(|w| w.entry.path.clone())
            .collect();
        let mut view = Self::from_infos(infos);
        view.status_rx = Some(spawn_status_loaders(pending));
        Ok(view)
    }

    /// A view over already-built rows (no background loading)
    pub fn from_infos(worktrees: Vec<WorktreeInfo>) -> Self {
        Self {
            worktrees,
            selected: HashSet::new(),
            filter: WorktreeFilter::All,
            delete_branches: false,
            status_rx: None,
        }
    }

    fn apply_status(&mut self, path: PathBuf, changes: Changes) {
        if let Some(wt) = self.worktrees.iter_mut().find(|w| w.entry.path == path) {
            wt.changes = changes;
        }
    }

    /// Apply status results that arrived since the last call
    pub fn poll_status(&mut self) {
        let mut updates = Vec::new();
        let mut done = false;
        if let Some(rx) = &self.status_rx {
            loop {
                match rx.try_recv() {
                    Ok(update) => updates.push(update),
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        done = true;
                        break;
                    }
                }
            }
        }
        for (path, changes) in updates {
            self.apply_status(path, changes);
        }
        if done {
            self.status_rx = None;
        }
    }

    /// Block until every status has arrived (CLI mode)
    pub fn wait_for_status(&mut self) {
        if let Some(rx) = self.status_rx.take() {
            for (path, changes) in rx {
                self.apply_status(path, changes);
            }
        }
    }

    /// Rows matching the active filter tab and the search query
    pub fn filtered(&self, query: &str) -> Vec<&WorktreeInfo> {
        self.worktrees
            .iter()
            .filter(|w| self.filter.matches(w) && w.matches_search(query))
            .collect()
    }

    pub fn filter_count(&self, filter: WorktreeFilter) -> usize {
        self.worktrees.iter().filter(|w| filter.matches(w)).count()
    }

    pub fn is_selected(&self, wt: &WorktreeInfo) -> bool {
        self.selected.contains(&wt.entry.path)
    }

    /// Toggle one worktree, if it is selectable
    pub fn toggle(&mut self, path: &Path, force_mode: bool) {
        let Some(wt) = self.worktrees.iter().find(|w| w.entry.path == path) else {
            return;
        };
        if !wt.is_selectable(force_mode) {
            return;
        }
        if !self.selected.remove(path) {
            self.selected.insert(path.to_path_buf());
        }
    }

    /// After a reload, keep what `old` knew: the filter tab, statuses already
    /// read (fresh ones replace them as they arrive), and the selection of
    /// worktrees that still exist and are still selectable
    pub fn carry_over_from(&mut self, old: &WorktreeView, force_mode: bool) {
        self.filter = old.filter;
        for wt in self.worktrees.iter_mut() {
            if wt.changes != Changes::Loading {
                continue;
            }
            if let Some(known) = old
                .worktrees
                .iter()
                .find(|o| o.entry.path == wt.entry.path && o.changes != Changes::Loading)
            {
                wt.changes = known.changes.clone();
            }
        }
        self.selected = old.selected.clone();
        self.retain_selectable(force_mode);
    }

    /// Drop selected worktrees that are not selectable with `force_mode`
    /// (dirty ones once force mode is off)
    pub fn retain_selectable(&mut self, force_mode: bool) {
        let worktrees = &self.worktrees;
        self.selected.retain(|path| {
            worktrees
                .iter()
                .any(|w| &w.entry.path == path && w.is_selectable(force_mode))
        });
    }

    /// `a`: select every clean (or prunable) stale worktree; when they are all
    /// selected already, clear the selection instead
    pub fn select_all_clean_stale(&mut self, force_mode: bool) {
        let candidates: Vec<PathBuf> = self
            .worktrees
            .iter()
            .filter(|w| {
                w.is_stale()
                    && matches!(
                        w.removal_action(force_mode),
                        Some(RemovalAction::Remove | RemovalAction::Prune)
                    )
            })
            .map(|w| w.entry.path.clone())
            .collect();
        if candidates.iter().all(|p| self.selected.contains(p)) {
            self.selected.clear();
        } else {
            self.selected.extend(candidates);
        }
    }

    /// Selected rows, in table order
    pub fn selected_infos(&self) -> Vec<&WorktreeInfo> {
        self.worktrees
            .iter()
            .filter(|w| self.is_selected(w))
            .collect()
    }
}

/// What happens to a removed worktree's branch
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BranchPlan {
    /// Leave branches alone (toggle off, or detached HEAD)
    None,
    /// `git branch -d` / `-D`
    Delete { name: String, force: bool },
    /// Toggle on, but the branch view's rules don't allow deleting it
    Keep { name: String, reason: String },
}

/// One selected worktree's removal
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemovalPlan {
    pub name: String,
    pub path: PathBuf,
    pub action: RemovalAction,
    pub branch: BranchPlan,
}

/// Plan removal of the selected worktrees. Branch deletion follows the branch
/// view's rules: protected/current branches are kept, force-only ones
/// (unmerged, local, pr-diverged) need force mode, and `-D` is used where the
/// branch view would use it.
pub fn plan_removals(
    selected: &[&WorktreeInfo],
    force_mode: bool,
    delete_branches: bool,
) -> Vec<RemovalPlan> {
    selected
        .iter()
        .filter_map(|wt| {
            let action = wt.removal_action(force_mode)?;
            let branch = match (&wt.entry.branch, &wt.branch_status) {
                _ if !delete_branches => BranchPlan::None,
                (None, _) => BranchPlan::None,
                (Some(name), None) => BranchPlan::Keep {
                    name: name.clone(),
                    reason: "not a local branch".to_string(),
                },
                (Some(name), Some(status)) if !status.is_deletable() => BranchPlan::Keep {
                    name: name.clone(),
                    reason: format!("{} branch", status.label()),
                },
                (Some(name), Some(status)) if status.requires_force() && !force_mode => {
                    BranchPlan::Keep {
                        name: name.clone(),
                        reason: format!("{} - needs force mode", status.label()),
                    }
                }
                (Some(name), Some(status)) => BranchPlan::Delete {
                    name: name.clone(),
                    force: branch_delete_needs_force(status, force_mode),
                },
            };
            Some(RemovalPlan {
                name: wt.name(),
                path: wt.entry.path.clone(),
                action,
                branch,
            })
        })
        .collect()
}

fn run_git(args: &[String]) -> std::result::Result<(), String> {
    let output = Command::new("git")
        .args(args)
        .output()
        .map_err(|e| e.to_string())?;
    if output.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

/// Carry out (or, in dry-run mode, describe) the planned removals.
/// `git worktree prune` runs at most once, however many prunable worktrees
/// are selected. A deleted branch's cached PR entry is dropped from
/// `pr_cache`, as in the branch view.
pub fn execute_removals(
    plans: &[RemovalPlan],
    dry_run: bool,
    pr_cache: Option<&crate::cache::PrCache>,
) -> Vec<ActionLogEntry> {
    let mut log = Vec::new();
    let mut prune_result: Option<std::result::Result<(), String>> = None;

    for plan in plans {
        let command = plan.action.command_line(&plan.path);
        let removed = if dry_run {
            log.push(ActionLogEntry {
                branch_name: plan.name.clone(),
                success: true,
                message: format!("[DRY RUN] Would run: {}", command),
            });
            true
        } else {
            let result = if plan.action == RemovalAction::Prune {
                prune_result
                    .get_or_insert_with(|| run_git(&plan.action.git_args(&plan.path)))
                    .clone()
            } else {
                run_git(&plan.action.git_args(&plan.path))
            };
            let message = match plan.action {
                RemovalAction::Remove => "Removed worktree",
                RemovalAction::ForceRemove => "Removed worktree (--force)",
                RemovalAction::Prune => "Pruned worktree",
            };
            match result {
                Ok(()) => {
                    log.push(ActionLogEntry {
                        branch_name: plan.name.clone(),
                        success: true,
                        message: message.to_string(),
                    });
                    true
                }
                Err(e) => {
                    log.push(ActionLogEntry {
                        branch_name: plan.name.clone(),
                        success: false,
                        message: e,
                    });
                    false
                }
            }
        };

        if !removed {
            continue;
        }
        match &plan.branch {
            BranchPlan::None => {}
            BranchPlan::Keep { name, reason } => log.push(ActionLogEntry {
                branch_name: name.clone(),
                success: true,
                message: format!("Branch kept ({})", reason),
            }),
            BranchPlan::Delete { name, force } => {
                let flag = if *force { "-D" } else { "-d" };
                if dry_run {
                    log.push(ActionLogEntry {
                        branch_name: name.clone(),
                        success: true,
                        message: format!("[DRY RUN] Would delete branch ({})", flag),
                    });
                } else {
                    match crate::git::delete_branch_with_mode(name, *force) {
                        Ok(()) => {
                            if let Some(cache) = pr_cache {
                                cache.invalidate(name).ok();
                            }
                            log.push(ActionLogEntry {
                                branch_name: name.clone(),
                                success: true,
                                message: format!("Deleted branch ({})", flag),
                            })
                        }
                        Err(e) => log.push(ActionLogEntry {
                            branch_name: name.clone(),
                            success: false,
                            message: e.to_string(),
                        }),
                    }
                }
            }
        }
    }
    log
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(path: &str, branch: Option<&str>) -> WorktreeEntry {
        WorktreeEntry {
            path: PathBuf::from(path),
            head: "1a2b3c4d5e6f".to_string(),
            branch: branch.map(str::to_string),
            is_main: false,
            is_bare: false,
            is_detached: branch.is_none(),
            locked: None,
            prunable: None,
        }
    }

    fn wt(
        path: &str,
        branch: Option<&str>,
        status: Option<BranchStatus>,
        changes: Changes,
    ) -> WorktreeInfo {
        WorktreeInfo {
            entry: entry(path, branch),
            is_current: false,
            changes,
            branch_status: status,
            upstream: None,
            ahead: None,
            behind: None,
            commit: CommitSummary::default(),
            pr_info: None,
        }
    }

    fn clean() -> Changes {
        Changes::Loaded(ChangeCounts::default())
    }

    fn dirty() -> Changes {
        Changes::Loaded(parse_porcelain_status(" M src/a.rs\n?? new.txt\n"))
    }

    fn branch_info(name: &str, status: BranchStatus) -> BranchInfo {
        BranchInfo {
            name: name.to_string(),
            upstream: Some(format!("origin/{name}")),
            last_commit_relative: "2 days ago".to_string(),
            status,
            last_commit_sha: "abc1234".to_string(),
            last_commit_author: "Author".to_string(),
            last_commit_message: "Subject".to_string(),
            ahead: Some(1),
            behind: Some(0),
            last_activity_timestamp: 100,
            branch_created_timestamp: 0,
            branch_author: "Author".to_string(),
            pr_info: None,
            worktree: None,
            worktree_shadowed_status: None,
        }
    }

    /// A branch held by another worktree, as TUI-19's classifier reports it
    fn held_branch(name: &str, shadowed: BranchStatus) -> BranchInfo {
        let mut b = branch_info(name, BranchStatus::ProtectedWorktree);
        b.worktree = Some(PathBuf::from(format!("/wt/{name}")));
        b.worktree_shadowed_status = Some(shadowed);
        b
    }

    #[test]
    fn worktree_rows_use_the_shadowed_status_not_protected_wt() {
        let branches = vec![
            held_branch("merged", BranchStatus::SafeMerged),
            held_branch("unmerged", BranchStatus::Unmerged),
            branch_info("plain", BranchStatus::GoneUpstream),
        ];
        assert_eq!(
            worktree_branch_status(&branches[0]),
            BranchStatus::SafeMerged
        );
        assert_eq!(
            worktree_branch_status(&branches[2]),
            BranchStatus::GoneUpstream
        );

        let entries = vec![
            entry("/wt/merged", Some("merged")),
            entry("/wt/unmerged", Some("unmerged")),
        ];
        let mut infos = build_worktree_infos(entries, &branches, Path::new("/nonexistent"));
        for wt in infos.iter_mut() {
            wt.changes = clean();
            assert_ne!(wt.state().label(), BranchStatus::ProtectedWorktree.label());
        }
        let merged = infos.iter().find(|w| w.name() == "merged").unwrap();
        let unmerged = infos.iter().find(|w| w.name() == "unmerged").unwrap();
        assert_eq!(
            merged.state(),
            WorktreeState::Branch(BranchStatus::SafeMerged)
        );
        assert!(merged.is_stale());

        // Branch deletion decides -d/-D and protection from the shadowed status
        let plans = plan_removals(&[merged, unmerged], false, true);
        assert_eq!(
            plans[0].branch,
            BranchPlan::Delete {
                name: "merged".into(),
                force: false
            }
        );
        assert!(matches!(plans[1].branch, BranchPlan::Keep { .. }));
        let plans = plan_removals(&[unmerged], true, true);
        assert_eq!(
            plans[0].branch,
            BranchPlan::Delete {
                name: "unmerged".into(),
                force: true
            }
        );
    }

    #[test]
    fn parses_porcelain_counts_and_paths() {
        let out = " M src/a.rs\nM  src/b.rs\nMM src/c.rs\nA  added.rs\n?? new.txt\n?? dir/\nR  old.rs -> new.rs\n";
        let c = parse_porcelain_status(out);
        assert_eq!(c.modified, 2); // a.rs, c.rs
        assert_eq!(c.staged, 4); // b.rs, c.rs, added.rs, rename
        assert_eq!(c.untracked, 2);
        assert_eq!(c.files, 7);
        assert_eq!(c.compact(), "2M 4S 2N");

        // A file both staged and modified (MM) is listed in both groups
        assert_eq!(c.modified_paths, ["src/a.rs", "src/c.rs"]);
        assert_eq!(
            c.staged_paths,
            ["src/b.rs", "src/c.rs", "added.rs", "old.rs -> new.rs"]
        );
        assert_eq!(c.new_paths, ["new.txt", "dir/"]);

        let groups: Vec<_> = c
            .groups()
            .iter()
            .map(|g| (g.label, g.count, g.paths.len()))
            .collect();
        assert_eq!(
            groups,
            [("Modified", 2, 2), ("Staged", 4, 4), ("New", 2, 2)]
        );
    }

    #[test]
    fn change_groups_cap_paths_and_skip_empty_groups() {
        let out: String = (0..7).map(|i| format!("?? new{i}.txt\n")).collect();
        let c = parse_porcelain_status(&out);
        let groups = c.groups();
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].label, "New");
        assert_eq!(groups[0].count, 7);
        assert_eq!(groups[0].paths.len(), MAX_CHANGED_PATHS);
        assert_eq!(c.compact(), "7N");
    }

    #[test]
    fn empty_status_is_clean() {
        let c = parse_porcelain_status("");
        assert!(c.is_clean());
        assert_eq!(c.compact(), "clean");
        assert_eq!(parse_porcelain_status("!! target/\n").compact(), "clean");
    }

    #[test]
    fn state_priority_order() {
        let mut w = wt("/wt/a", Some("a"), Some(BranchStatus::SafeMerged), dirty());
        w.is_current = true;
        w.entry.prunable = Some(String::new());
        w.entry.locked = Some(String::new());
        assert_eq!(w.state(), WorktreeState::Current);
        w.is_current = false;
        assert_eq!(w.state(), WorktreeState::Prunable);
        w.entry.prunable = None;
        assert_eq!(w.state(), WorktreeState::Locked);
        w.entry.locked = None;
        assert_eq!(w.state(), WorktreeState::Dirty);
        w.changes = clean();
        assert_eq!(w.state(), WorktreeState::Branch(BranchStatus::SafeMerged));
        w.changes = Changes::Loading;
        assert_eq!(w.state(), WorktreeState::Branch(BranchStatus::SafeMerged));

        let d = wt("/wt/d", None, None, clean());
        assert_eq!(d.state(), WorktreeState::Detached);
        assert_eq!(d.branch_label(), "(detached @ 1a2b3c4)");
        assert_eq!(
            wt("/wt/u", Some("u"), None, clean()).state(),
            WorktreeState::Unknown
        );
    }

    #[test]
    fn selectability_rules() {
        let mut main = wt(
            "/repo",
            Some("main"),
            Some(BranchStatus::Protected),
            clean(),
        );
        main.entry.is_main = true;
        assert!(!main.is_selectable(true));

        let mut current = wt("/wt/cur", Some("c"), Some(BranchStatus::Current), clean());
        current.is_current = true;
        assert!(!current.is_selectable(true));

        let mut locked = wt("/wt/l", Some("l"), Some(BranchStatus::SafeMerged), clean());
        locked.entry.locked = Some("usb".into());
        assert!(!locked.is_selectable(true));

        let loading = wt(
            "/wt/x",
            Some("x"),
            Some(BranchStatus::SafeMerged),
            Changes::Loading,
        );
        assert!(!loading.is_selectable(true));
        let failed = wt("/wt/f", Some("f"), None, Changes::Failed("boom".into()));
        assert!(!failed.is_selectable(true));

        let d = wt("/wt/d", Some("d"), Some(BranchStatus::Unmerged), dirty());
        assert!(!d.is_selectable(false));
        assert!(d.is_selectable(true));

        let c = wt("/wt/c", Some("c"), Some(BranchStatus::Unmerged), clean());
        assert!(c.is_selectable(false));
    }

    #[test]
    fn removal_command_selection() {
        let c = wt("/wt/c", Some("c"), Some(BranchStatus::SafeMerged), clean());
        assert_eq!(c.removal_action(false), Some(RemovalAction::Remove));
        assert_eq!(
            RemovalAction::Remove.command_line(Path::new("/wt/c")),
            "git worktree remove /wt/c"
        );

        let d = wt("/wt/d", Some("d"), Some(BranchStatus::SafeMerged), dirty());
        assert_eq!(d.removal_action(false), None);
        assert_eq!(d.removal_action(true), Some(RemovalAction::ForceRemove));
        assert_eq!(
            RemovalAction::ForceRemove.command_line(Path::new("/wt/d")),
            "git worktree remove --force /wt/d"
        );

        let mut p = wt(
            "/wt/p",
            Some("p"),
            Some(BranchStatus::GoneUpstream),
            Changes::NotApplicable,
        );
        p.entry.prunable = Some("gitdir file points to non-existent location".into());
        assert_eq!(p.removal_action(false), Some(RemovalAction::Prune));
        assert_eq!(
            RemovalAction::Prune.command_line(Path::new("/wt/p")),
            "git worktree prune"
        );
    }

    #[test]
    fn filters_and_search() {
        let mut p = wt(
            "/wt/old",
            Some("feat/old"),
            Some(BranchStatus::Unmerged),
            Changes::NotApplicable,
        );
        p.entry.prunable = Some(String::new());
        let mut view = WorktreeView::from_infos(vec![
            wt(
                "/wt/fix-login",
                Some("fix/login"),
                Some(BranchStatus::SafeMerged),
                clean(),
            ),
            wt(
                "/wt/qa",
                Some("qa/session"),
                Some(BranchStatus::Unmerged),
                dirty(),
            ),
            wt(
                "/wt/spike",
                Some("feat/cache"),
                Some(BranchStatus::GoneUpstream),
                dirty(),
            ),
            wt("/wt/review", None, None, clean()),
            p,
        ]);
        assert_eq!(view.filter_count(WorktreeFilter::All), 5);
        assert_eq!(view.filter_count(WorktreeFilter::Clean), 2);
        assert_eq!(view.filter_count(WorktreeFilter::Dirty), 2);
        assert_eq!(view.filter_count(WorktreeFilter::Stale), 3);

        // Search covers name, branch and path, case-insensitively
        assert_eq!(view.filtered("LOGIN").len(), 1);
        assert_eq!(view.filtered("feat/").len(), 2);
        assert_eq!(view.filtered("/wt/").len(), 5);
        assert_eq!(view.filtered("detached").len(), 1);

        view.filter = WorktreeFilter::Stale;
        let names: Vec<String> = view.filtered("").iter().map(|w| w.name()).collect();
        assert_eq!(names, ["fix-login", "spike", "old"]);
        assert_eq!(view.filtered("spike").len(), 1);
    }

    #[test]
    fn select_all_picks_clean_stale_only_and_toggles() {
        let mut view = WorktreeView::from_infos(vec![
            wt(
                "/wt/merged",
                Some("m"),
                Some(BranchStatus::SafeMerged),
                clean(),
            ),
            wt(
                "/wt/gone-dirty",
                Some("g"),
                Some(BranchStatus::GoneUpstream),
                dirty(),
            ),
            wt(
                "/wt/unmerged",
                Some("u"),
                Some(BranchStatus::Unmerged),
                clean(),
            ),
        ]);
        view.select_all_clean_stale(true);
        assert_eq!(view.selected.len(), 1);
        assert!(view.selected.contains(Path::new("/wt/merged")));
        view.select_all_clean_stale(true);
        assert!(view.selected.is_empty());

        view.toggle(Path::new("/wt/gone-dirty"), false);
        assert!(view.selected.is_empty());
        view.toggle(Path::new("/wt/gone-dirty"), true);
        assert_eq!(view.selected.len(), 1);
    }

    #[test]
    fn plan_uses_branch_view_delete_rules() {
        let merged = wt("/wt/m", Some("m"), Some(BranchStatus::SafeMerged), clean());
        let gone = wt(
            "/wt/g",
            Some("g"),
            Some(BranchStatus::GoneUpstream),
            clean(),
        );
        let unmerged = wt("/wt/u", Some("u"), Some(BranchStatus::Unmerged), clean());
        let detached = wt("/wt/d", None, None, clean());
        let selected = [&merged, &gone, &unmerged, &detached];

        let plans = plan_removals(&selected, false, false);
        assert!(plans.iter().all(|p| p.branch == BranchPlan::None));

        let plans = plan_removals(&selected, false, true);
        assert_eq!(
            plans[0].branch,
            BranchPlan::Delete {
                name: "m".into(),
                force: false
            }
        );
        assert_eq!(
            plans[1].branch,
            BranchPlan::Delete {
                name: "g".into(),
                force: true
            }
        );
        assert!(matches!(plans[2].branch, BranchPlan::Keep { .. }));
        assert_eq!(plans[3].branch, BranchPlan::None);

        let plans = plan_removals(&selected, true, true);
        assert_eq!(
            plans[2].branch,
            BranchPlan::Delete {
                name: "u".into(),
                force: true
            }
        );
    }

    #[test]
    fn dry_run_describes_commands_without_running_them() {
        let plans = vec![RemovalPlan {
            name: "m".into(),
            path: PathBuf::from("/nonexistent/wt/m"),
            action: RemovalAction::Remove,
            branch: BranchPlan::Delete {
                name: "m".into(),
                force: false,
            },
        }];
        let log = execute_removals(&plans, true, None);
        assert_eq!(log.len(), 2);
        assert!(log.iter().all(|e| e.success));
        assert_eq!(
            log[0].message,
            "[DRY RUN] Would run: git worktree remove /nonexistent/wt/m"
        );
        assert_eq!(log[1].message, "[DRY RUN] Would delete branch (-d)");
    }

    #[test]
    fn dry_run_keeps_cached_pr_entry() {
        use crate::cache::{CacheResult, PrCache};
        use crate::pr::PrState;

        let conn = rusqlite::Connection::open_in_memory().unwrap();
        let mut cache =
            PrCache::open_with_conn(conn, "test/repo", std::time::Duration::from_secs(3600))
                .unwrap();
        let pr = PrInfo {
            number: 7,
            state: PrState::Merged,
            title: "PR #7".into(),
            url: "https://github.com/test/repo/pull/7".into(),
        };
        cache.set("m", Some(&pr)).unwrap();

        let plans = vec![RemovalPlan {
            name: "m".into(),
            path: PathBuf::from("/nonexistent/wt/m"),
            action: RemovalAction::Remove,
            branch: BranchPlan::Delete {
                name: "m".into(),
                force: false,
            },
        }];
        execute_removals(&plans, true, Some(&cache));

        assert!(matches!(cache.get("m"), CacheResult::Hit(Some(_))));
    }

    #[test]
    fn builds_rows_from_branch_classifier_and_sorts_main_first() {
        let mut main = entry("/repo", Some("main"));
        main.is_main = true;
        let entries = vec![
            entry("/wt/old", Some("old")),
            main,
            entry("/wt/new", Some("new")),
        ];
        let mut new = branch_info("new", BranchStatus::Unmerged);
        new.last_activity_timestamp = 500;
        let branches = vec![
            branch_info("main", BranchStatus::Protected),
            branch_info("old", BranchStatus::SafeMerged),
            new,
        ];
        let infos = build_worktree_infos(entries, &branches, Path::new("/nonexistent-toplevel"));
        let names: Vec<String> = infos.iter().map(|w| w.name()).collect();
        assert_eq!(names, ["repo", "new", "old"]);
        assert_eq!(infos[2].branch_status, Some(BranchStatus::SafeMerged));
        assert_eq!(infos[2].upstream.as_deref(), Some("origin/old"));
        assert_eq!(infos[2].changes, Changes::Loading);
        assert_eq!(infos[2].commit.subject, "Subject");
    }
}
