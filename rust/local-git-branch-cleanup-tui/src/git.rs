// Git integration module — local Git operations only.
// Hosting-provider (PR) logic lives in `crate::pr`.

use crate::pr::PrInfo;
use color_eyre::Result;
use std::process::Command;

/// Protected branch names that should never be deleted
const PROTECTED_BRANCHES: &[&str] = &["main", "master", "develop", "development"];

/// Branch classification status
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BranchStatus {
    /// Merged into trunk - safe to delete with `git branch -d`
    SafeMerged,
    /// PR was merged (squash/rebase) but the remote branch still exists, so
    /// git ancestry cannot see the merge. The branch is in sync with its live
    /// upstream (ahead == 0), so `git branch -d` deletes it safely.
    PrMerged,
    /// PR was merged and the remote branch still exists, but the local branch
    /// has commits its upstream doesn't (ahead > 0) — unpushed work, or stale
    /// copies left behind by a remote rebase/amend. The tool cannot prove the
    /// local commits' content landed, so deletion requires force.
    PrDiverged,
    /// Remote tracking branch was deleted (shows as [gone] in git branch -vv)
    GoneUpstream,
    /// Has unmerged commits - requires force delete with `git branch -D`
    Unmerged,
    /// Never pushed: the repo has a remote but this branch has no upstream.
    /// Its commits may exist nowhere else (even when git ancestry says
    /// merged, the branch never went through the remote), so it is shown
    /// as its own status and deletion requires force.
    Local,
    /// Protected branch (main/master/develop) - cannot be deleted
    Protected,
    /// Currently checked out branch - cannot be deleted
    Current,
}

impl BranchStatus {
    /// Get a human-readable label for the status
    pub fn label(&self) -> &'static str {
        match self {
            BranchStatus::SafeMerged => "merged",
            BranchStatus::PrMerged => "pr-merged",
            BranchStatus::PrDiverged => "pr-diverged",
            BranchStatus::GoneUpstream => "gone",
            BranchStatus::Unmerged => "unmerged",
            BranchStatus::Local => "local",
            BranchStatus::Protected => "protected",
            BranchStatus::Current => "current",
        }
    }

    /// Get an icon for the status
    pub fn icon(&self) -> &'static str {
        match self {
            BranchStatus::SafeMerged => "✓",
            BranchStatus::PrMerged => "↑",
            BranchStatus::PrDiverged => "↕",
            BranchStatus::GoneUpstream => "↗",
            BranchStatus::Unmerged => "!",
            BranchStatus::Local => "○",
            BranchStatus::Protected => "⊘",
            BranchStatus::Current => "◉",
        }
    }

    /// Check if this branch can be safely deleted (without force)
    #[allow(dead_code)]
    pub fn is_safe_to_delete(&self) -> bool {
        matches!(
            self,
            BranchStatus::SafeMerged | BranchStatus::PrMerged | BranchStatus::GoneUpstream
        )
    }

    /// Check if this branch can be deleted at all
    pub fn is_deletable(&self) -> bool {
        !matches!(self, BranchStatus::Protected | BranchStatus::Current)
    }

    /// Check if deleting this branch requires force mode (`git branch -D`)
    pub fn requires_force(&self) -> bool {
        matches!(
            self,
            BranchStatus::Unmerged | BranchStatus::PrDiverged | BranchStatus::Local
        )
    }
}

/// Information about a Git branch
#[derive(Debug, Clone)]
pub struct BranchInfo {
    pub name: String,
    pub upstream: Option<String>,
    pub last_commit_relative: String,
    pub status: BranchStatus,
    /// Last commit SHA (short)
    pub last_commit_sha: String,
    /// Last commit author
    pub last_commit_author: String,
    /// Last commit message (first line)
    pub last_commit_message: String,
    /// Number of commits ahead of upstream (if tracked)
    pub ahead: Option<usize>,
    /// Number of commits behind upstream (if tracked)
    pub behind: Option<usize>,
    /// Last activity (last commit) timestamp for sorting
    pub last_activity_timestamp: i64,
    /// Branch creation date as Unix timestamp (first unique commit on branch)
    pub branch_created_timestamp: i64,
    /// Author who created the branch (author of first unique commit)
    pub branch_author: String,
    /// GitHub PR information (if --github flag enabled and PR exists)
    pub pr_info: Option<PrInfo>,
}

/// Verify we're inside a Git repository
pub fn verify_repo() -> Result<String> {
    let output = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .output()?;

    if !output.status.success() {
        return Err(color_eyre::eyre::eyre!("Not inside a Git repository"));
    }

    Ok(String::from_utf8(output.stdout)?.trim().to_string())
}

/// Get current branch name
pub fn get_current_branch() -> Result<String> {
    let output = Command::new("git")
        .args(["branch", "--show-current"])
        .output()?;

    Ok(String::from_utf8(output.stdout)?.trim().to_string())
}

/// Get current git user name from config
pub fn get_current_git_user() -> Result<String> {
    let output = Command::new("git").args(["config", "user.name"]).output()?;

    Ok(String::from_utf8(output.stdout)?.trim().to_string())
}

/// Detect the default/trunk branch
/// Tries: git symbolic-ref, then fallback to main/master
pub fn get_default_branch(trunk_override: Option<&str>) -> Result<String> {
    // Use CLI override if provided
    if let Some(trunk) = trunk_override {
        return Ok(trunk.to_string());
    }

    // Try to get the default branch from origin/HEAD
    let output = Command::new("git")
        .args(["symbolic-ref", "--short", "refs/remotes/origin/HEAD"])
        .output()?;

    if output.status.success() {
        let branch = String::from_utf8(output.stdout)?.trim().to_string();
        // Strip "origin/" prefix if present
        return Ok(branch
            .strip_prefix("origin/")
            .unwrap_or(&branch)
            .to_string());
    }

    // Fallback: check if main or master exists
    for candidate in &["main", "master"] {
        let check = Command::new("git")
            .args([
                "rev-parse",
                "--verify",
                &format!("refs/heads/{}", candidate),
            ])
            .output()?;

        if check.status.success() {
            return Ok(candidate.to_string());
        }
    }

    // Default to "main" if nothing found
    Ok("main".to_string())
}

/// Get list of branches merged into the trunk
pub fn get_merged_branches(trunk: &str) -> Result<Vec<String>> {
    let output = Command::new("git")
        .args(["branch", "--format=%(refname:short)", "--merged", trunk])
        .output()?;

    if !output.status.success() {
        return Ok(Vec::new());
    }

    let branches = String::from_utf8(output.stdout)?
        .lines()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    Ok(branches)
}

/// Get branches with "gone" upstream (remote was deleted)
pub fn get_gone_branches() -> Result<Vec<String>> {
    // Use git for-each-ref to get upstream status
    let output = Command::new("git")
        .args([
            "for-each-ref",
            "--format=%(refname:short) %(upstream:track)",
            "refs/heads/",
        ])
        .output()?;

    if !output.status.success() {
        return Ok(Vec::new());
    }

    let mut gone_branches = Vec::new();
    for line in String::from_utf8(output.stdout)?.lines() {
        if line.contains("[gone]") {
            if let Some(branch_name) = line.split_whitespace().next() {
                gone_branches.push(branch_name.to_string());
            }
        }
    }

    Ok(gone_branches)
}

/// Get ahead/behind counts for a branch relative to its upstream
fn get_ahead_behind_counts(branch: &str, upstream: &str) -> Result<(Option<usize>, Option<usize>)> {
    let output = Command::new("git")
        .args([
            "rev-list",
            "--left-right",
            "--count",
            &format!("{}...{}", branch, upstream),
        ])
        .output()?;

    if !output.status.success() {
        return Ok((None, None));
    }

    let counts = String::from_utf8(output.stdout)?.trim().to_string();
    let parts: Vec<&str> = counts.split_whitespace().collect();

    let ahead = parts.first().and_then(|s| s.parse().ok());
    let behind = parts.get(1).and_then(|s| s.parse().ok());

    Ok((ahead, behind))
}

/// Check if a branch name is protected
pub fn is_protected_branch(branch: &str) -> bool {
    PROTECTED_BRANCHES.contains(&branch)
}

/// Get all local branches without remote counterparts
/// (Replicates bash script logic)
#[allow(dead_code)]
pub fn get_branches_without_remote() -> Result<Vec<BranchInfo>> {
    get_branches_with_classification(None)
}

/// Get all local branches with full classification
/// This is the enhanced version that classifies branches by status
pub fn get_branches_with_classification(trunk_override: Option<&str>) -> Result<Vec<BranchInfo>> {
    let mut branches = Vec::new();

    // Get context for classification
    let current_branch = get_current_branch()?;
    let trunk = get_default_branch(trunk_override)?;
    // Also consider origin/<trunk>: a branch merged remotely while the local
    // trunk is stale would otherwise be misclassified as unmerged.
    let mut merged_branches = get_merged_branches(&trunk)?;
    for branch in get_merged_branches(&format!("origin/{trunk}"))? {
        if !merged_branches.contains(&branch) {
            merged_branches.push(branch);
        }
    }
    let gone_branches = get_gone_branches()?;

    // "local" (never pushed) only makes sense when the repo has a remote at
    // all; in a remote-less repo every branch would be local and the
    // merged/unmerged classification is more useful.
    let has_remote = Command::new("git")
        .args(["remote"])
        .output()
        .map(|o| o.status.success() && !o.stdout.is_empty())
        .unwrap_or(false);

    // Get all local branches
    let output = Command::new("git")
        .args(["for-each-ref", "--format=%(refname:short)", "refs/heads/"])
        .output()?;

    let branch_names = String::from_utf8(output.stdout)?;

    for branch in branch_names.lines() {
        let branch = branch.trim();
        if branch.is_empty() {
            continue;
        }

        // Check if branch has upstream
        let upstream_check = Command::new("git")
            .args([
                "rev-parse",
                "--abbrev-ref",
                "--symbolic-full-name",
                &format!("{}@{{u}}", branch),
            ])
            .output()?;

        let has_upstream = upstream_check.status.success();
        let upstream = if has_upstream {
            Some(String::from_utf8(upstream_check.stdout)?.trim().to_string())
        } else {
            None
        };

        // Determine if upstream is "gone"
        let is_gone = gone_branches.contains(&branch.to_string());

        // Get last commit time
        let last_commit = Command::new("git")
            .args(["log", "-1", "--format=%cr", branch])
            .output()?;

        let last_commit_relative = String::from_utf8(last_commit.stdout)?.trim().to_string();

        // Get commit details
        let commit_details = Command::new("git")
            .args(["log", "-1", "--format=%h|%an|%s", branch])
            .output()?;

        let details_str = String::from_utf8(commit_details.stdout)?.trim().to_string();
        let details_parts: Vec<&str> = details_str.split('|').collect();
        let last_commit_sha = details_parts.first().unwrap_or(&"").to_string();
        let last_commit_author = details_parts.get(1).unwrap_or(&"").to_string();
        let last_commit_message = details_parts.get(2).unwrap_or(&"").to_string();

        // Get last activity timestamp (last commit on branch)
        let activity_output = Command::new("git")
            .args(["log", "-1", "--format=%ct", branch])
            .output()?;
        let last_activity_timestamp = String::from_utf8(activity_output.stdout)?
            .trim()
            .parse::<i64>()
            .unwrap_or(0);

        // Get branch creation timestamp (first unique commit on branch, not on trunk)
        // This gives us when the branch was actually created/diverged from trunk
        let created_output = Command::new("git")
            .args([
                "log",
                "--format=%ct|%an",
                "--reverse",
                &format!("{}..{}", trunk, branch),
            ])
            .output()?;
        let created_info = String::from_utf8(created_output.stdout)?;
        let first_line = created_info.lines().next().unwrap_or("");
        let created_parts: Vec<&str> = first_line.split('|').collect();
        let branch_created_timestamp = created_parts
            .first()
            .and_then(|s| s.trim().parse::<i64>().ok())
            .unwrap_or(last_activity_timestamp); // Fallback to last activity if no unique commits
        let branch_author = created_parts
            .get(1)
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|| last_commit_author.clone()); // Fallback to last commit author

        // Get ahead/behind counts if there's an upstream
        let (ahead, behind) = if has_upstream && !is_gone {
            get_ahead_behind_counts(branch, upstream.as_ref().unwrap())?
        } else {
            (None, None)
        };

        // Determine branch status
        let status = classify_branch(
            branch,
            &current_branch,
            &trunk,
            &merged_branches,
            is_gone,
            has_remote && !has_upstream,
        );

        branches.push(BranchInfo {
            name: branch.to_string(),
            upstream,
            last_commit_relative,
            status,
            last_commit_sha,
            last_commit_author,
            last_commit_message,
            ahead,
            behind,
            last_activity_timestamp,
            branch_created_timestamp,
            branch_author,
            pr_info: None, // Will be populated later if --github flag is used
        });
    }

    // Sort branches: protected/current first (so user sees them), then by status
    branches.sort_by(|a, b| {
        let order = |s: &BranchStatus| match s {
            BranchStatus::Current => 0,
            BranchStatus::Protected => 1,
            BranchStatus::SafeMerged => 2,
            BranchStatus::PrMerged => 3,
            BranchStatus::PrDiverged => 4,
            BranchStatus::GoneUpstream => 5,
            BranchStatus::Unmerged => 6,
            BranchStatus::Local => 7,
        };
        order(&a.status).cmp(&order(&b.status))
    });

    Ok(branches)
}

/// Upgrade `Unmerged` branches when the fetched PR data proves the merge that
/// git ancestry cannot see (squash/rebase merges).
///
/// A branch currently classified `Unmerged` (gone branches are already
/// deletable) whose upstream still exists and whose newest PR is merged
/// becomes:
/// - `PrMerged` when `ahead == 0` — nothing can be lost and
///   `git branch -d` succeeds against the upstream;
/// - `PrDiverged` when `ahead > 0` — the local branch has commits its
///   upstream doesn't (unpushed work, or stale copies after a remote
///   rebase/amend), so deletion still requires force.
///
/// Call after PR info has been fetched.
pub fn apply_pr_merge_status(branches: &mut [BranchInfo]) {
    use crate::pr::PrState;

    for branch in branches.iter_mut() {
        if branch.status == BranchStatus::Unmerged
            && branch.upstream.is_some()
            && matches!(&branch.pr_info, Some(pr) if pr.state == PrState::Merged)
        {
            match branch.ahead {
                Some(0) => branch.status = BranchStatus::PrMerged,
                Some(_) => branch.status = BranchStatus::PrDiverged,
                None => {}
            }
        }
    }
}

/// Classify a branch based on its relationship to trunk and current state
fn classify_branch(
    branch: &str,
    current_branch: &str,
    trunk: &str,
    merged_branches: &[String],
    is_gone: bool,
    is_local: bool,
) -> BranchStatus {
    // Check if it's the current branch
    if branch == current_branch {
        return BranchStatus::Current;
    }

    // Check if it's a protected branch
    if is_protected_branch(branch) || branch == trunk {
        return BranchStatus::Protected;
    }

    // Check if upstream is gone
    if is_gone {
        return BranchStatus::GoneUpstream;
    }

    // Never pushed: takes precedence over the merged check — a branch that
    // never reached the remote should not display as merged.
    if is_local {
        return BranchStatus::Local;
    }

    // Check if merged into trunk
    if merged_branches.contains(&branch.to_string()) {
        return BranchStatus::SafeMerged;
    }

    // Otherwise it's unmerged
    BranchStatus::Unmerged
}

/// Delete a branch (force delete)
#[allow(dead_code)]
pub fn delete_branch(branch_name: &str) -> Result<()> {
    delete_branch_with_mode(branch_name, true)
}

/// Delete a branch with optional force mode
/// - force=false: uses `git branch -d` (safe delete, fails if unmerged)
/// - force=true: uses `git branch -D` (force delete, always succeeds)
pub fn delete_branch_with_mode(branch_name: &str, force: bool) -> Result<()> {
    let flag = if force { "-D" } else { "-d" };

    let output = Command::new("git")
        .args(["branch", flag, branch_name])
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(color_eyre::eyre::eyre!(
            "Failed to delete branch: {}",
            stderr.trim()
        ));
    }

    Ok(())
}

/// Open a URL in the default browser
pub fn open_url_in_browser(url: &str) -> Result<()> {
    #[cfg(target_os = "linux")]
    {
        Command::new("xdg-open").arg(url).spawn()?;
    }

    #[cfg(target_os = "macos")]
    {
        Command::new("open").arg(url).spawn()?;
    }

    #[cfg(target_os = "windows")]
    {
        Command::new("cmd").args(["/C", "start", "", url]).spawn()?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_branch_status_label() {
        assert_eq!(BranchStatus::SafeMerged.label(), "merged");
        assert_eq!(BranchStatus::PrMerged.label(), "pr-merged");
        assert_eq!(BranchStatus::PrDiverged.label(), "pr-diverged");
        assert_eq!(BranchStatus::GoneUpstream.label(), "gone");
        assert_eq!(BranchStatus::Unmerged.label(), "unmerged");
        assert_eq!(BranchStatus::Local.label(), "local");
        assert_eq!(BranchStatus::Protected.label(), "protected");
        assert_eq!(BranchStatus::Current.label(), "current");
    }

    #[test]
    fn test_branch_status_icon() {
        assert_eq!(BranchStatus::SafeMerged.icon(), "✓");
        assert_eq!(BranchStatus::PrMerged.icon(), "↑");
        assert_eq!(BranchStatus::PrDiverged.icon(), "↕");
        assert_eq!(BranchStatus::GoneUpstream.icon(), "↗");
        assert_eq!(BranchStatus::Unmerged.icon(), "!");
        assert_eq!(BranchStatus::Local.icon(), "○");
        assert_eq!(BranchStatus::Protected.icon(), "⊘");
        assert_eq!(BranchStatus::Current.icon(), "◉");
    }

    #[test]
    fn test_branch_status_is_safe_to_delete() {
        assert!(BranchStatus::SafeMerged.is_safe_to_delete());
        assert!(BranchStatus::PrMerged.is_safe_to_delete());
        assert!(!BranchStatus::PrDiverged.is_safe_to_delete());
        assert!(BranchStatus::GoneUpstream.is_safe_to_delete());
        assert!(!BranchStatus::Unmerged.is_safe_to_delete());
        assert!(!BranchStatus::Local.is_safe_to_delete());
        assert!(!BranchStatus::Protected.is_safe_to_delete());
        assert!(!BranchStatus::Current.is_safe_to_delete());
    }

    #[test]
    fn test_branch_status_is_deletable() {
        assert!(BranchStatus::SafeMerged.is_deletable());
        assert!(BranchStatus::PrMerged.is_deletable());
        assert!(BranchStatus::PrDiverged.is_deletable());
        assert!(BranchStatus::GoneUpstream.is_deletable());
        assert!(BranchStatus::Unmerged.is_deletable());
        assert!(BranchStatus::Local.is_deletable());
        assert!(!BranchStatus::Protected.is_deletable());
        assert!(!BranchStatus::Current.is_deletable());
    }

    #[test]
    fn test_branch_status_requires_force() {
        assert!(!BranchStatus::SafeMerged.requires_force());
        assert!(!BranchStatus::PrMerged.requires_force());
        assert!(BranchStatus::PrDiverged.requires_force());
        assert!(!BranchStatus::GoneUpstream.requires_force());
        assert!(BranchStatus::Unmerged.requires_force());
        assert!(BranchStatus::Local.requires_force());
        assert!(!BranchStatus::Protected.requires_force());
        assert!(!BranchStatus::Current.requires_force());
    }

    #[test]
    fn test_is_protected_branch() {
        assert!(is_protected_branch("main"));
        assert!(is_protected_branch("master"));
        assert!(is_protected_branch("develop"));
        assert!(is_protected_branch("development"));
        assert!(!is_protected_branch("feature/test"));
        assert!(!is_protected_branch("bugfix/something"));
    }

    #[test]
    fn test_classify_branch_current() {
        let status = classify_branch(
            "feature/test",
            "feature/test", // current
            "main",
            &["other-branch".to_string()],
            false,
            false,
        );
        assert_eq!(status, BranchStatus::Current);
    }

    #[test]
    fn test_classify_branch_protected() {
        let status = classify_branch("main", "feature/test", "main", &[], false, false);
        assert_eq!(status, BranchStatus::Protected);

        let status2 = classify_branch("master", "feature/test", "main", &[], false, false);
        assert_eq!(status2, BranchStatus::Protected);
    }

    #[test]
    fn test_classify_branch_gone() {
        let status = classify_branch(
            "feature/old",
            "main",
            "main",
            &[],
            true,  // is_gone
            false, // is_local
        );
        assert_eq!(status, BranchStatus::GoneUpstream);
    }

    #[test]
    fn test_classify_branch_local() {
        // Never pushed: local wins even over a merged ancestry — the branch
        // never reached the remote, so it must not display as merged.
        let merged = vec!["feature/local-done".to_string()];
        let status = classify_branch("feature/local-done", "main", "main", &merged, false, true);
        assert_eq!(status, BranchStatus::Local);

        let status2 = classify_branch("feature/local-wip", "main", "main", &[], false, true);
        assert_eq!(status2, BranchStatus::Local);
    }

    #[test]
    fn test_classify_branch_merged() {
        let merged = vec!["feature/done".to_string()];
        let status = classify_branch("feature/done", "main", "main", &merged, false, false);
        assert_eq!(status, BranchStatus::SafeMerged);
    }

    #[test]
    fn test_classify_branch_unmerged() {
        let status = classify_branch("feature/wip", "main", "main", &[], false, false);
        assert_eq!(status, BranchStatus::Unmerged);
    }

    #[test]
    fn test_classify_branch_priority() {
        // Current branch takes priority over protected
        let status = classify_branch(
            "main",
            "main", // current
            "main",
            &["main".to_string()], // also merged
            false,
            false,
        );
        assert_eq!(status, BranchStatus::Current);

        // Protected takes priority over merged
        let status2 = classify_branch(
            "main",
            "feature/test",        // not current
            "develop",             // trunk is something else
            &["main".to_string()], // merged
            false,
            false,
        );
        assert_eq!(status2, BranchStatus::Protected);
    }

    // --- apply_pr_merge_status ---

    use crate::pr::{PrInfo, PrState};

    fn branch_with(
        status: BranchStatus,
        upstream: Option<&str>,
        ahead: Option<usize>,
        pr_state: Option<PrState>,
    ) -> BranchInfo {
        BranchInfo {
            name: "feature/x".to_string(),
            upstream: upstream.map(|s| s.to_string()),
            last_commit_relative: "1 day ago".to_string(),
            status,
            last_commit_sha: "abc1234".to_string(),
            last_commit_author: "Test".to_string(),
            last_commit_message: "test".to_string(),
            ahead,
            behind: None,
            last_activity_timestamp: 0,
            branch_created_timestamp: 0,
            branch_author: "Test".to_string(),
            pr_info: pr_state.map(|state| PrInfo {
                number: 1,
                state,
                title: "PR".to_string(),
                url: "https://example.com/pr/1".to_string(),
            }),
        }
    }

    #[test]
    fn pr_merge_status_upgrades_unmerged_with_merged_pr() {
        let mut branches = vec![branch_with(
            BranchStatus::Unmerged,
            Some("origin/feature/x"),
            Some(0),
            Some(PrState::Merged),
        )];
        apply_pr_merge_status(&mut branches);
        assert_eq!(branches[0].status, BranchStatus::PrMerged);
    }

    #[test]
    fn pr_merge_status_marks_diverged_with_unpushed_commits() {
        // ahead > 0: local commits would be lost — flagged pr-diverged,
        // still requires force to delete.
        let mut branches = vec![branch_with(
            BranchStatus::Unmerged,
            Some("origin/feature/x"),
            Some(2),
            Some(PrState::Merged),
        )];
        apply_pr_merge_status(&mut branches);
        assert_eq!(branches[0].status, BranchStatus::PrDiverged);
    }

    #[test]
    fn pr_merge_status_keeps_unmerged_with_unknown_ahead() {
        // ahead unknown (rev-list failed): nothing can be proven.
        let mut branches = vec![branch_with(
            BranchStatus::Unmerged,
            Some("origin/feature/x"),
            None,
            Some(PrState::Merged),
        )];
        apply_pr_merge_status(&mut branches);
        assert_eq!(branches[0].status, BranchStatus::Unmerged);
    }

    #[test]
    fn pr_merge_status_keeps_unmerged_without_upstream() {
        let mut branches = vec![branch_with(
            BranchStatus::Unmerged,
            None,
            None,
            Some(PrState::Merged),
        )];
        apply_pr_merge_status(&mut branches);
        assert_eq!(branches[0].status, BranchStatus::Unmerged);
    }

    #[test]
    fn pr_merge_status_ignores_open_and_closed_prs() {
        for state in [PrState::Open, PrState::Closed] {
            let mut branches = vec![branch_with(
                BranchStatus::Unmerged,
                Some("origin/feature/x"),
                Some(0),
                Some(state),
            )];
            apply_pr_merge_status(&mut branches);
            assert_eq!(branches[0].status, BranchStatus::Unmerged);
        }
    }

    #[test]
    fn pr_merge_status_ignores_branches_without_pr() {
        let mut branches = vec![branch_with(
            BranchStatus::Unmerged,
            Some("origin/feature/x"),
            Some(0),
            None,
        )];
        apply_pr_merge_status(&mut branches);
        assert_eq!(branches[0].status, BranchStatus::Unmerged);
    }

    #[test]
    fn pr_merge_status_leaves_gone_branches_alone() {
        // A gone branch is already deletable; its status must not change.
        let mut branches = vec![branch_with(
            BranchStatus::GoneUpstream,
            None,
            None,
            Some(PrState::Merged),
        )];
        apply_pr_merge_status(&mut branches);
        assert_eq!(branches[0].status, BranchStatus::GoneUpstream);
    }
}
