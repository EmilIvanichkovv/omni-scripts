// Integration tests for local-git-branch-cleanup-tui

use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;
use std::path::PathBuf;
use std::process::Command as StdCommand;
use tempfile::TempDir;

/// Helper struct to manage a temporary Git repository for testing
struct TestRepo {
    _temp_dir: TempDir,
    path: PathBuf,
}

impl TestRepo {
    /// Create a new test repository with initial setup
    fn new() -> Self {
        let temp_dir = TempDir::new().expect("Failed to create temp directory");
        let path = temp_dir.path().to_path_buf();

        // Initialize git repo
        Self::run_git(&path, &["init"]);
        Self::run_git(&path, &["config", "user.name", "Test User"]);
        Self::run_git(&path, &["config", "user.email", "test@example.com"]);

        // Create initial commit on main
        fs::write(path.join("README.md"), "# Test Repo").expect("Failed to write file");
        Self::run_git(&path, &["add", "README.md"]);
        Self::run_git(&path, &["commit", "-m", "Initial commit"]);

        TestRepo {
            _temp_dir: temp_dir,
            path,
        }
    }

    /// Run a git command in the test repository
    fn run_git(path: &PathBuf, args: &[&str]) -> std::process::Output {
        StdCommand::new("git")
            .current_dir(path)
            .args(args)
            .output()
            .expect("Failed to execute git command")
    }

    /// Get the path to the test repository
    fn path(&self) -> &PathBuf {
        &self.path
    }

    /// Create a branch with a commit
    fn create_branch(&self, name: &str, commit_message: &str) {
        Self::run_git(&self.path, &["checkout", "-b", name]);
        let file_name = name.replace('/', "_");
        fs::write(
            self.path.join(format!("{}.txt", file_name)),
            format!("Content for {}", name),
        )
        .expect("Failed to write file");
        Self::run_git(&self.path, &["add", "."]);
        Self::run_git(&self.path, &["commit", "-m", commit_message]);
        Self::run_git(&self.path, &["checkout", "main"]);
    }

    /// Merge a branch into main
    fn merge_branch(&self, name: &str) {
        Self::run_git(&self.path, &["merge", "--no-ff", name]);
    }

    /// Delete a remote tracking branch to create a "gone" scenario
    /// This simulates the scenario where a remote branch was deleted
    fn create_gone_branch(&self, name: &str) {
        // Create a branch with upstream tracking
        Self::run_git(&self.path, &["checkout", "-b", name]);
        fs::write(
            self.path.join(format!("{}.txt", name)),
            format!("Content for {}", name),
        )
        .expect("Failed to write file");
        Self::run_git(&self.path, &["add", "."]);
        Self::run_git(&self.path, &["commit", "-m", &format!("Add {}", name)]);

        // Set up a fake remote tracking
        Self::run_git(
            &self.path,
            &[
                "config",
                &format!("branch.{}.remote", name),
                "origin",
            ],
        );
        Self::run_git(
            &self.path,
            &[
                "config",
                &format!("branch.{}.merge", name),
                &format!("refs/heads/{}", name),
            ],
        );

        Self::run_git(&self.path, &["checkout", "main"]);
    }
}

#[test]
fn test_cli_help() {
    let mut cmd = Command::cargo_bin("local-git-branch-cleanup-tui").unwrap();
    cmd.arg("--help");
    cmd.assert()
        .success()
        .stdout(predicate::str::contains("local-git-branch-cleanup-tui"))
        .stdout(predicate::str::contains("--trunk"))
        .stdout(predicate::str::contains("--force"));
}

#[test]
fn test_cli_version() {
    let mut cmd = Command::cargo_bin("local-git-branch-cleanup-tui").unwrap();
    cmd.arg("--version");
    cmd.assert()
        .success()
        .stdout(predicate::str::contains("0.1.0"));
}

#[test]
fn test_non_git_directory() {
    let temp_dir = TempDir::new().expect("Failed to create temp directory");

    let mut cmd = Command::cargo_bin("local-git-branch-cleanup-tui").unwrap();
    cmd.current_dir(temp_dir.path()).arg("--cli");

    // Should fail because it's not a git repository
    cmd.assert()
        .failure()
        .stderr(predicate::str::contains("Not inside a Git repository"));
}

#[test]
fn test_empty_repository() {
    let repo = TestRepo::new();

    let mut cmd = Command::cargo_bin("local-git-branch-cleanup-tui").unwrap();
    cmd.current_dir(repo.path()).arg("--cli");

    // Should succeed - empty repo only has main/master which is protected
    cmd.assert()
        .success()
        .stdout(predicate::str::contains("protected").or(predicate::str::contains("current")));
}

#[test]
fn test_repository_with_merged_branches() {
    let repo = TestRepo::new();

    // Create and merge a branch
    repo.create_branch("feature/merged", "Add merged feature");
    repo.merge_branch("feature/merged");

    let mut cmd = Command::cargo_bin("local-git-branch-cleanup-tui").unwrap();
    cmd.current_dir(repo.path()).arg("--cli");

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("feature/merged"))
        .stdout(predicate::str::contains("merged"));
}

#[test]
fn test_repository_with_unmerged_branches() {
    let repo = TestRepo::new();

    // Create an unmerged branch
    repo.create_branch("feature/unmerged", "Add unmerged feature");

    let mut cmd = Command::cargo_bin("local-git-branch-cleanup-tui").unwrap();
    cmd.current_dir(repo.path()).arg("--cli");

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("feature/unmerged"))
        .stdout(predicate::str::contains("unmerged"));
}

#[test]
fn test_protected_branches_not_shown() {
    let repo = TestRepo::new();

    // Main should exist but not be shown as deletable
    let mut cmd = Command::cargo_bin("local-git-branch-cleanup-tui").unwrap();
    cmd.current_dir(repo.path()).arg("--cli");

    let output = cmd.output().expect("Failed to execute command");
    let stdout = String::from_utf8_lossy(&output.stdout);

    // Should not show main as a branch to delete
    // (or if it does, it should be marked as protected)
    assert!(
        !stdout.contains("main") || stdout.contains("protected"),
        "Main branch should not be deletable or should be marked as protected"
    );
}

#[test]
fn test_trunk_override() {
    let repo = TestRepo::new();

    // Create a develop branch
    TestRepo::run_git(repo.path(), &["checkout", "-b", "develop"]);
    TestRepo::run_git(repo.path(), &["checkout", "main"]);

    // Create a branch merged into develop but not main
    repo.create_branch("feature/test", "Test feature");
    TestRepo::run_git(repo.path(), &["checkout", "develop"]);
    TestRepo::run_git(repo.path(), &["merge", "--no-ff", "feature/test"]);
    TestRepo::run_git(repo.path(), &["checkout", "main"]);

    // With default trunk (main), feature/test should be unmerged
    let mut cmd = Command::cargo_bin("local-git-branch-cleanup-tui").unwrap();
    cmd.current_dir(repo.path()).arg("--cli");
    cmd.assert()
        .success()
        .stdout(predicate::str::contains("feature/test"))
        .stdout(predicate::str::contains("unmerged"));

    // With trunk=develop, feature/test should be merged
    let mut cmd = Command::cargo_bin("local-git-branch-cleanup-tui").unwrap();
    cmd.current_dir(repo.path())
        .arg("--cli")
        .arg("--trunk")
        .arg("develop");
    cmd.assert()
        .success()
        .stdout(predicate::str::contains("feature/test"))
        .stdout(predicate::str::contains("merged"));
}

#[test]
fn test_force_flag() {
    let repo = TestRepo::new();
    repo.create_branch("feature/unmerged", "Unmerged work");

    // Without --force, should show force mode disabled
    let mut cmd = Command::cargo_bin("local-git-branch-cleanup-tui").unwrap();
    cmd.current_dir(repo.path()).arg("--cli");
    
    let output = cmd.output().expect("Failed to execute command");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!stdout.contains("FORCE MODE"));

    // With --force, should indicate force mode
    let mut cmd = Command::cargo_bin("local-git-branch-cleanup-tui").unwrap();
    cmd.current_dir(repo.path())
        .arg("--cli")
        .arg("--force");
    
    let output = cmd.output().expect("Failed to execute command");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("FORCE MODE"));
}

#[test]
fn test_dry_run_flag() {
    let repo = TestRepo::new();
    repo.create_branch("feature/test", "Test feature");

    let mut cmd = Command::cargo_bin("local-git-branch-cleanup-tui").unwrap();
    cmd.current_dir(repo.path())
        .arg("--cli")
        .arg("--dry-run");
    
    let output = cmd.output().expect("Failed to execute command");
    let stdout = String::from_utf8_lossy(&output.stdout);
    
    // Should indicate dry run mode
    assert!(stdout.contains("DRY RUN") || stdout.contains("Preview"));
}

#[test]
fn test_mixed_branch_types() {
    let repo = TestRepo::new();

    // Create various types of branches
    repo.create_branch("feature/merged", "Merged feature");
    repo.merge_branch("feature/merged");

    repo.create_branch("feature/unmerged1", "Unmerged feature 1");
    repo.create_branch("feature/unmerged2", "Unmerged feature 2");

    // Note: Testing "gone" branches is complex as it requires actual remote setup
    // We've covered the basic scenarios

    let mut cmd = Command::cargo_bin("local-git-branch-cleanup-tui").unwrap();
    cmd.current_dir(repo.path()).arg("--cli");

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("feature/merged"))
        .stdout(predicate::str::contains("feature/unmerged1"))
        .stdout(predicate::str::contains("feature/unmerged2"));
}

#[test]
fn test_branch_count_summary() {
    let repo = TestRepo::new();

    repo.create_branch("feature/one", "Feature one");
    repo.create_branch("feature/two", "Feature two");

    let mut cmd = Command::cargo_bin("local-git-branch-cleanup-tui").unwrap();
    cmd.current_dir(repo.path()).arg("--cli");

    let output = cmd.output().expect("Failed to execute command");
    let stdout = String::from_utf8_lossy(&output.stdout);

    // Should show some count of branches found
    assert!(
        stdout.contains("Found") || stdout.contains("branches") || stdout.contains("2"),
        "Output should indicate number of branches found"
    );
}
