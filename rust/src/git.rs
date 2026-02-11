// Git integration module

use color_eyre::Result;
use std::process::Command;

/// Information about a Git branch
#[derive(Debug, Clone)]
pub struct BranchInfo {
    pub name: String,
    pub upstream: Option<String>,
    pub last_commit_relative: String,
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

/// Get all local branches without remote counterparts
/// (Replicates bash script logic)
pub fn get_branches_without_remote() -> Result<Vec<BranchInfo>> {
    let mut branches = Vec::new();

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

        // If no upstream (exit code != 0), add to list
        if !upstream_check.status.success() {
            // Get last commit time
            let last_commit = Command::new("git")
                .args(["log", "-1", "--format=%cr", branch])
                .output()?;

            let last_commit_relative = String::from_utf8(last_commit.stdout)?.trim().to_string();

            branches.push(BranchInfo {
                name: branch.to_string(),
                upstream: None,
                last_commit_relative,
            });
        }
    }

    Ok(branches)
}

/// Delete a branch (force delete)
pub fn delete_branch(branch_name: &str) -> Result<()> {
    let output = Command::new("git")
        .args(["branch", "-D", branch_name])
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(color_eyre::eyre::eyre!("Failed to delete branch: {}", stderr));
    }

    Ok(())
}
