// Integration tests for local-git-branch-cleanup-tui

#![allow(deprecated)] // Command::cargo_bin is deprecated but still functional

use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;
use std::path::PathBuf;
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
        Command::new("git")
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
    #[allow(dead_code)]
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
            &["config", &format!("branch.{}.remote", name), "origin"],
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
    cmd.current_dir(repo.path()).arg("--cli").arg("--force");

    let output = cmd.output().expect("Failed to execute command");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("FORCE MODE"));
}

#[test]
fn test_dry_run_flag() {
    let repo = TestRepo::new();
    repo.create_branch("feature/test", "Test feature");

    let mut cmd = Command::cargo_bin("local-git-branch-cleanup-tui").unwrap();
    cmd.current_dir(repo.path()).arg("--cli").arg("--dry-run");

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

#[test]
fn test_github_flag_without_gh_cli() {
    // --github is a valid flag; running it in a repo where gh is not
    // available should not cause a panic or a CLI parse error.
    // We just verify the binary exits (any status) without crashing.
    let repo = TestRepo::new();

    let mut cmd = Command::cargo_bin("local-git-branch-cleanup-tui").unwrap();
    cmd.current_dir(repo.path()).arg("--cli").arg("--github");

    // The binary may exit 0 (no PRs found) or non-zero (gh not found / API
    // error), but it must not hard-crash (signal / panic).
    let output = cmd.output().expect("binary failed to start");
    // Status code 0 or 1 are both acceptable; what we rule out is a signal.
    assert!(
        output.status.code().is_some(),
        "--github without real gh should exit with a code, not a signal"
    );
}

#[test]
fn test_sequential_flag_accepted() {
    // --sequential is a hidden flag; the binary must accept it without error.
    let repo = TestRepo::new();

    let mut cmd = Command::cargo_bin("local-git-branch-cleanup-tui").unwrap();
    cmd.current_dir(repo.path())
        .arg("--cli")
        .arg("--sequential");

    cmd.assert().success();
}

#[test]
fn test_multiple_protected_branch_names() {
    // All of main / master / develop / development should be shown as protected.
    for protected in &["main", "master", "develop", "development"] {
        let repo = TestRepo::new();
        // Rename the initial 'main' branch to the protected name.
        TestRepo::run_git(repo.path(), &["branch", "-m", "master", protected]);
        // Create a feature branch so there's something to list.
        repo.create_branch("feature/test", "test");
        // Checkout 'feature/test' branch
        TestRepo::run_git(repo.path(), &["checkout", "feature/test"]);

        let mut cmd = Command::cargo_bin("local-git-branch-cleanup-tui").unwrap();
        cmd.current_dir(repo.path())
            .arg("--cli")
            .arg("--trunk")
            .arg(*protected);

        let output = cmd.output().expect("Failed to execute command");
        let stdout = String::from_utf8_lossy(&output.stdout);
        let line = stdout
            .lines()
            .find(|l| l.contains(protected) && l.contains('['))
            .unwrap_or("");
        assert!(
            line.contains("protected"),
            "Branch '{}' should be marked protected",
            protected
        );
    }
}

#[test]
fn test_dry_run_does_not_delete_branches() {
    let repo = TestRepo::new();
    repo.create_branch("feature/merged", "Merged feature");
    repo.merge_branch("feature/merged");

    // Run with --dry-run — the branch should still exist afterwards.
    let mut cmd = Command::cargo_bin("local-git-branch-cleanup-tui").unwrap();
    cmd.current_dir(repo.path()).arg("--cli").arg("--dry-run");
    cmd.assert().success();

    // Verify feature/merged still exists.
    let branches_out = TestRepo::run_git(repo.path(), &["branch"]);
    let branches = String::from_utf8_lossy(&branches_out.stdout);
    assert!(
        branches.contains("feature/merged"),
        "--dry-run must not delete any branch"
    );
}

// --- Bitbucket CLI contract (spec 13.5) ---

#[test]
fn test_github_and_bitbucket_flags_conflict() {
    let repo = TestRepo::new();
    let mut cmd = Command::cargo_bin("local-git-branch-cleanup-tui").unwrap();
    cmd.current_dir(repo.path())
        .arg("--github")
        .arg("--bitbucket");
    cmd.assert()
        .failure()
        .stderr(predicate::str::contains("cannot be used with"));
}

#[test]
fn test_bitbucket_override_requires_bitbucket_flag() {
    let repo = TestRepo::new();
    for flag in [
        "--bitbucket-base-url=https://example.com",
        "--bitbucket-project=PROJ",
        "--bitbucket-repo=demo-repo",
    ] {
        let mut cmd = Command::cargo_bin("local-git-branch-cleanup-tui").unwrap();
        cmd.current_dir(repo.path()).arg("--cli").arg(flag);
        cmd.assert().failure().stderr(
            predicate::str::contains("required").or(predicate::str::contains("--bitbucket")),
        );
    }
}

/// Build a command with all Bitbucket env vars cleared and an isolated cache
/// dir, so tests never depend on (or write to) the developer's environment.
fn bitbucket_cmd(repo: &TestRepo) -> Command {
    let mut cmd = Command::cargo_bin("local-git-branch-cleanup-tui").unwrap();
    cmd.current_dir(repo.path())
        .env_remove("BITBUCKET_TOKEN")
        .env_remove("BITBUCKET_BASE_URL")
        .env_remove("BITBUCKET_PROJECT")
        .env_remove("BITBUCKET_REPO")
        .env("XDG_CACHE_HOME", repo.path().join(".cache"));
    cmd
}

const BB_OVERRIDES: [&str; 6] = [
    "--bitbucket-base-url",
    "https://bitbucket.example.com",
    "--bitbucket-project",
    "PROJ",
    "--bitbucket-repo",
    "demo-repo",
];

#[test]
fn test_bitbucket_missing_token_exits_nonzero() {
    let repo = TestRepo::new();
    let mut cmd = bitbucket_cmd(&repo);
    cmd.args(["--cli", "--bitbucket"]).args(BB_OVERRIDES);
    cmd.assert()
        .failure()
        .stderr(predicate::str::contains("BITBUCKET_TOKEN"));
}

#[test]
fn test_bitbucket_empty_token_exits_nonzero() {
    let repo = TestRepo::new();
    let mut cmd = bitbucket_cmd(&repo);
    cmd.env("BITBUCKET_TOKEN", "   ");
    cmd.args(["--cli", "--bitbucket"]).args(BB_OVERRIDES);
    cmd.assert()
        .failure()
        .stderr(predicate::str::contains("BITBUCKET_TOKEN"));
}

#[test]
fn test_bitbucket_invalid_base_url_exits_nonzero() {
    let repo = TestRepo::new();
    let mut cmd = bitbucket_cmd(&repo);
    // Config resolution runs before the token check (spec 4.4), so no token
    // is needed to observe the URL error.
    cmd.args([
        "--cli",
        "--bitbucket",
        "--bitbucket-base-url",
        "ftp://bitbucket.example.com",
        "--bitbucket-project",
        "PROJ",
        "--bitbucket-repo",
        "demo-repo",
    ]);
    cmd.assert()
        .failure()
        .stderr(predicate::str::contains("--bitbucket-base-url"));
}

#[test]
fn test_bitbucket_fatal_error_occurs_before_tui() {
    let repo = TestRepo::new();
    // No --cli: without the fatal-before-TUI guarantee this would try to
    // enter the alternate screen instead of exiting with an error.
    let mut cmd = bitbucket_cmd(&repo);
    cmd.arg("--bitbucket").args(BB_OVERRIDES);
    cmd.assert()
        .failure()
        .stderr(predicate::str::contains("BITBUCKET_TOKEN"));
}

// --- Bitbucket end-to-end against a local wiremock server (no real network) ---

async fn mount_valid_repo(server: &wiremock::MockServer) {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, ResponseTemplate};
    Mock::given(method("GET"))
        .and(path("/rest/api/latest/projects/PROJ/repos/demo-repo"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "slug": "demo-repo",
            "project": {"key": "PROJ"}
        })))
        .mount(server)
        .await;
}

async fn mount_empty_pr_page(server: &wiremock::MockServer) {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, ResponseTemplate};
    Mock::given(method("GET"))
        .and(path(
            "/rest/api/latest/projects/PROJ/repos/demo-repo/pull-requests",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "values": [],
            "size": 0,
            "limit": 1,
            "isLastPage": true
        })))
        .mount(server)
        .await;
}

fn bitbucket_mock_args(server_uri: &str) -> Vec<String> {
    vec![
        "--bitbucket".to_string(),
        "--bitbucket-base-url".to_string(),
        server_uri.to_string(),
        "--bitbucket-project".to_string(),
        "PROJ".to_string(),
        "--bitbucket-repo".to_string(),
        "demo-repo".to_string(),
    ]
}

#[tokio::test]
async fn test_bitbucket_cli_shows_integration_enabled() {
    let repo = TestRepo::new();
    let server = wiremock::MockServer::start().await;
    mount_valid_repo(&server).await;
    mount_empty_pr_page(&server).await;

    let mut cmd = bitbucket_cmd(&repo);
    cmd.env("BITBUCKET_TOKEN", "fake-test-token");
    cmd.arg("--cli").args(bitbucket_mock_args(&server.uri()));
    cmd.assert()
        .success()
        .stdout(predicate::str::contains("Bitbucket PR integration enabled"));
}

#[tokio::test]
async fn test_bitbucket_fetch_only_exits_zero_on_success() {
    let repo = TestRepo::new();
    let server = wiremock::MockServer::start().await;
    mount_valid_repo(&server).await;
    mount_empty_pr_page(&server).await;

    let mut cmd = bitbucket_cmd(&repo);
    cmd.env("BITBUCKET_TOKEN", "fake-test-token");
    cmd.arg("--fetch-only")
        .args(bitbucket_mock_args(&server.uri()));
    cmd.assert().success();
}

#[tokio::test]
async fn test_bitbucket_fetch_only_exits_nonzero_on_lookup_failure() {
    let repo = TestRepo::new();
    let server = wiremock::MockServer::start().await;
    // Repository validation succeeds, but the PR endpoint is unmatched and
    // returns 404 — every branch lookup fails (and 404 is not retried).
    mount_valid_repo(&server).await;

    let mut cmd = bitbucket_cmd(&repo);
    cmd.env("BITBUCKET_TOKEN", "fake-test-token");
    cmd.arg("--fetch-only")
        .args(bitbucket_mock_args(&server.uri()));
    cmd.assert()
        .failure()
        .stderr(predicate::str::contains("failed"));
}

#[tokio::test]
async fn test_pr_merged_status_for_squash_merged_branch() {
    // A branch whose PR is merged (squash) has a live upstream and ahead == 0,
    // but its tip is not an ancestor of trunk. Git ancestry says "unmerged";
    // the PR data must upgrade it to "pr-merged".
    let repo = TestRepo::new();

    // Give the repo a real (file-based) origin so the branch has an upstream.
    let remote_dir = TempDir::new().expect("Failed to create remote dir");
    let remote_path = remote_dir.path().to_str().unwrap().to_string();
    TestRepo::run_git(repo.path(), &["init", "--bare", &remote_path]);
    TestRepo::run_git(repo.path(), &["remote", "add", "origin", &remote_path]);
    repo.create_branch("feature/squashed", "Feature commit");
    TestRepo::run_git(repo.path(), &["push", "-u", "origin", "feature/squashed"]);

    let server = wiremock::MockServer::start().await;
    mount_valid_repo(&server).await;

    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, ResponseTemplate};
    let pr_path = "/rest/api/latest/projects/PROJ/repos/demo-repo/pull-requests";
    Mock::given(method("GET"))
        .and(path(pr_path))
        .and(query_param("at", "refs/heads/feature/squashed"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "values": [{
                "id": 7,
                "title": "Squashed feature",
                "state": "MERGED",
                "fromRef": {
                    "id": "refs/heads/feature/squashed",
                    "displayId": "feature/squashed",
                    "repository": {"slug": "demo-repo", "project": {"key": "PROJ"}}
                },
                "links": {"self": [{"href": "https://bitbucket.example.com/pr/7"}]}
            }],
            "size": 1, "limit": 1, "isLastPage": true
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(pr_path))
        .and(query_param("at", "refs/heads/main"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "values": [], "size": 0, "limit": 1, "isLastPage": true
        })))
        .mount(&server)
        .await;

    let mut cmd = bitbucket_cmd(&repo);
    cmd.env("BITBUCKET_TOKEN", "fake-test-token");
    cmd.arg("--cli").args(bitbucket_mock_args(&server.uri()));
    cmd.write_stdin("n\n"); // decline the deletion prompt
    cmd.assert()
        .success()
        .stdout(predicate::str::contains("pr-merged"))
        .stdout(predicate::str::contains("PR #7"));
}
