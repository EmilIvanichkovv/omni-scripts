// Git integration module

use color_eyre::Result;
use std::process::Command;

/// Protected branch names that should never be deleted
const PROTECTED_BRANCHES: &[&str] = &["main", "master", "develop", "development"];

/// Branch classification status
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BranchStatus {
    /// Merged into trunk - safe to delete with `git branch -d`
    SafeMerged,
    /// Remote tracking branch was deleted (shows as [gone] in git branch -vv)
    GoneUpstream,
    /// Has unmerged commits - requires force delete with `git branch -D`
    Unmerged,
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
            BranchStatus::GoneUpstream => "gone",
            BranchStatus::Unmerged => "unmerged",
            BranchStatus::Protected => "protected",
            BranchStatus::Current => "current",
        }
    }

    /// Get an icon for the status
    pub fn icon(&self) -> &'static str {
        match self {
            BranchStatus::SafeMerged => "●",
            BranchStatus::GoneUpstream => "◆",
            BranchStatus::Unmerged => "▲",
            BranchStatus::Protected => "⛔",
            BranchStatus::Current => "★",
        }
    }

    /// Check if this branch can be safely deleted (without force)
    pub fn is_safe_to_delete(&self) -> bool {
        matches!(self, BranchStatus::SafeMerged | BranchStatus::GoneUpstream)
    }

    /// Check if this branch can be deleted at all
    pub fn is_deletable(&self) -> bool {
        !matches!(self, BranchStatus::Protected | BranchStatus::Current)
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
        return Ok(branch.strip_prefix("origin/").unwrap_or(&branch).to_string());
    }

    // Fallback: check if main or master exists
    for candidate in &["main", "master"] {
        let check = Command::new("git")
            .args(["rev-parse", "--verify", &format!("refs/heads/{}", candidate)])
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
        .args(["rev-list", "--left-right", "--count", &format!("{}...{}", branch, upstream)])
        .output()?;

    if !output.status.success() {
        return Ok((None, None));
    }

    let counts = String::from_utf8(output.stdout)?.trim().to_string();
    let parts: Vec<&str> = counts.split_whitespace().collect();
    
    let ahead = parts.get(0).and_then(|s| s.parse().ok());
    let behind = parts.get(1).and_then(|s| s.parse().ok());

    Ok((ahead, behind))
}

/// Check if a branch name is protected
pub fn is_protected_branch(branch: &str) -> bool {
    PROTECTED_BRANCHES.contains(&branch)
}

/// Get all local branches without remote counterparts
/// (Replicates bash script logic)
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
    let merged_branches = get_merged_branches(&trunk)?;
    let gone_branches = get_gone_branches()?;

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
        let last_commit_sha = details_parts.get(0).unwrap_or(&"").to_string();
        let last_commit_author = details_parts.get(1).unwrap_or(&"").to_string();
        let last_commit_message = details_parts.get(2).unwrap_or(&"").to_string();

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
        });
    }

    // Sort branches: protected/current first (so user sees them), then by status
    branches.sort_by(|a, b| {
        let order = |s: &BranchStatus| match s {
            BranchStatus::Current => 0,
            BranchStatus::Protected => 1,
            BranchStatus::SafeMerged => 2,
            BranchStatus::GoneUpstream => 3,
            BranchStatus::Unmerged => 4,
        };
        order(&a.status).cmp(&order(&b.status))
    });

    Ok(branches)
}

/// Classify a branch based on its relationship to trunk and current state
fn classify_branch(
    branch: &str,
    current_branch: &str,
    trunk: &str,
    merged_branches: &[String],
    is_gone: bool,
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

    // Check if merged into trunk
    if merged_branches.contains(&branch.to_string()) {
        return BranchStatus::SafeMerged;
    }

    // Otherwise it's unmerged
    BranchStatus::Unmerged
}

/// Delete a branch (force delete)
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
        return Err(color_eyre::eyre::eyre!("Failed to delete branch: {}", stderr.trim()));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_branch_status_label() {
        assert_eq!(BranchStatus::SafeMerged.label(), "merged");
        assert_eq!(BranchStatus::GoneUpstream.label(), "gone");
        assert_eq!(BranchStatus::Unmerged.label(), "unmerged");
        assert_eq!(BranchStatus::Protected.label(), "protected");
        assert_eq!(BranchStatus::Current.label(), "current");
    }

    #[test]
    fn test_branch_status_icon() {
        assert_eq!(BranchStatus::SafeMerged.icon(), "●");
        assert_eq!(BranchStatus::GoneUpstream.icon(), "◆");
        assert_eq!(BranchStatus::Unmerged.icon(), "▲");
        assert_eq!(BranchStatus::Protected.icon(), "⛔");
        assert_eq!(BranchStatus::Current.icon(), "★");
    }

    #[test]
    fn test_branch_status_is_safe_to_delete() {
        assert!(BranchStatus::SafeMerged.is_safe_to_delete());
        assert!(BranchStatus::GoneUpstream.is_safe_to_delete());
        assert!(!BranchStatus::Unmerged.is_safe_to_delete());
        assert!(!BranchStatus::Protected.is_safe_to_delete());
        assert!(!BranchStatus::Current.is_safe_to_delete());
    }

    #[test]
    fn test_branch_status_is_deletable() {
        assert!(BranchStatus::SafeMerged.is_deletable());
        assert!(BranchStatus::GoneUpstream.is_deletable());
        assert!(BranchStatus::Unmerged.is_deletable());
        assert!(!BranchStatus::Protected.is_deletable());
        assert!(!BranchStatus::Current.is_deletable());
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
            "feature/test",  // current
            "main",
            &["other-branch".to_string()],
            false,
        );
        assert_eq!(status, BranchStatus::Current);
    }

    #[test]
    fn test_classify_branch_protected() {
        let status = classify_branch(
            "main",
            "feature/test",
            "main",
            &[],
            false,
        );
        assert_eq!(status, BranchStatus::Protected);

        let status2 = classify_branch(
            "master",
            "feature/test",
            "main",
            &[],
            false,
        );
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
        );
        assert_eq!(status, BranchStatus::GoneUpstream);
    }

    #[test]
    fn test_classify_branch_merged() {
        let merged = vec!["feature/done".to_string()];
        let status = classify_branch(
            "feature/done",
            "main",
            "main",
            &merged,
            false,
        );
        assert_eq!(status, BranchStatus::SafeMerged);
    }

    #[test]
    fn test_classify_branch_unmerged() {
        let status = classify_branch(
            "feature/wip",
            "main",
            "main",
            &[],
            false,
        );
        assert_eq!(status, BranchStatus::Unmerged);
    }

    #[test]
    fn test_classify_branch_priority() {
        // Current branch takes priority over protected
        let status = classify_branch(
            "main",
            "main",  // current
            "main",
            &["main".to_string()],  // also merged
            false,
        );
        assert_eq!(status, BranchStatus::Current);

        // Protected takes priority over merged
        let status2 = classify_branch(
            "main",
            "feature/test",  // not current
            "develop",  // trunk is something else
            &["main".to_string()],  // merged
            false,
        );
        assert_eq!(status2, BranchStatus::Protected);
    }
}
