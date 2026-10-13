//! `update` and `update --check` against a mock GitHub API. No real network: axoupdater is pointed
//! at the mock server and at a temporary receipt directory through environment variables.
#![allow(deprecated)] // Command::cargo_bin is deprecated but still functional

use assert_cmd::Command;
use predicates::prelude::*;
use std::path::Path;
use tempfile::TempDir;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const APP: &str = "local-git-branch-cleanup-tui";
const CURRENT: &str = env!("CARGO_PKG_VERSION");
const RELEASES: &str = "/api/v3/repos/EmilIvanichkovv/omni-scripts/releases";
/// The releases feed, which follows the GHE base URL like the API.
const FEED: &str = "/EmilIvanichkovv/omni-scripts/releases.atom";

/// A command that talks to `server` and reads install receipts from `config_dir` only. It runs in
/// `config_dir`, which is not a git repository: `update` must work from anywhere.
fn update_cmd(server: &MockServer, config_dir: &Path) -> Command {
    let mut cmd = Command::cargo_bin(APP).unwrap();
    cmd.current_dir(config_dir)
        .env("AXOUPDATER_CONFIG_PATH", config_dir)
        // The GHE variable accepts a plain http://127.0.0.1 URL and appends /api/v3.
        .env(
            "LOCAL_GIT_BRANCH_CLEANUP_TUI_INSTALLER_GHE_BASE_URL",
            format!("{}/", server.uri()),
        )
        .env_remove("LOCAL_GIT_BRANCH_CLEANUP_TUI_INSTALLER_GITHUB_BASE_URL")
        .env_remove("LOCAL_GIT_BRANCH_CLEANUP_TUI_GITHUB_TOKEN")
        .env_remove("GH_TOKEN")
        .env_remove("GITHUB_TOKEN");
    cmd
}

/// A releases feed listing `versions` of this app, newest first, plus another app's release.
fn feed_xml(versions: &[&str]) -> String {
    let entry = |tag: String| {
        format!(
            "<entry><id>tag:github.com,2008:Repository/1/{tag}</id><title>{tag}</title></entry>"
        )
    };
    let mut entries = vec![entry("other-tool-v100.0.0".to_string())];
    entries.extend(versions.iter().map(|v| entry(format!("{APP}-v{v}"))));
    format!(
        "<?xml version=\"1.0\"?><feed><id>tag:github.com,2008:x/releases</id>{}</feed>",
        entries.concat()
    )
}

async fn mount_feed(server: &MockServer, versions: &[&str]) {
    Mock::given(method("GET"))
        .and(path(FEED))
        .respond_with(ResponseTemplate::new(200).set_body_string(feed_xml(versions)))
        .mount(server)
        .await;
}

/// Answers `status` on the API endpoints, `expect`ing that many requests to each: the server
/// checks the counts when the test ends.
async fn mount_api(server: &MockServer, status: u16, endpoints: &[String], expect: u64) {
    for p in endpoints {
        Mock::given(method("GET"))
            .and(path(p.as_str()))
            .respond_with(ResponseTemplate::new(status))
            .expect(expect)
            .mount(server)
            .await;
    }
}

/// GitHub's quota endpoint, reporting `remaining` anonymous requests, reset in 23 minutes.
async fn mount_rate_limit(server: &MockServer, remaining: u64) {
    let reset = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + 23 * 60
        - 30;
    let core = serde_json::json!({ "limit": 60, "remaining": remaining, "reset": reset });
    Mock::given(method("GET"))
        .and(path("/api/v3/rate_limit"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({ "resources": { "core": core }, "rate": core })),
        )
        .mount(server)
        .await;
}

fn lookup_endpoints() -> Vec<String> {
    vec![format!("{RELEASES}/latest"), RELEASES.to_string()]
}

fn release_json(version: &str) -> serde_json::Value {
    let tag = format!("{APP}-v{version}");
    let asset = |name: String| {
        serde_json::json!({
            "url": format!("https://example.invalid/assets/{name}"),
            "browser_download_url": format!("https://example.invalid/{tag}/{name}"),
            "name": name,
        })
    };
    serde_json::json!({
        "tag_name": tag,
        "name": version,
        "url": "https://example.invalid/release",
        "prerelease": false,
        "assets": [asset(format!("{APP}-installer.sh")), asset(format!("{APP}-installer.ps1"))],
    })
}

async fn mock_latest_release(version: &str) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!("{RELEASES}/latest")))
        .respond_with(ResponseTemplate::new(200).set_body_json(release_json(version)))
        .mount(&server)
        .await;
    server
}

async fn mock_failing_api(status: u16) -> MockServer {
    let server = MockServer::start().await;
    for p in [format!("{RELEASES}/latest"), RELEASES.to_string()] {
        Mock::given(method("GET"))
            .and(path(p))
            .respond_with(ResponseTemplate::new(status))
            .mount(&server)
            .await;
    }
    server
}

/// Write an installer receipt for `prefix` into `config_dir`.
fn write_receipt(config_dir: &Path, prefix: &Path) {
    let receipt = serde_json::json!({
        "binaries": [APP],
        "install_prefix": prefix,
        "install_layout": "flat",
        "modify_path": false,
        "provider": { "source": "cargo-dist", "version": "0.33.0" },
        "source": {
            "app_name": APP,
            "name": "omni-scripts",
            "owner": "EmilIvanichkovv",
            "release_type": "github"
        },
        "version": CURRENT,
    });
    std::fs::write(
        config_dir.join(format!("{APP}-receipt.json")),
        receipt.to_string(),
    )
    .unwrap();
}

/// The directory of the binary under test, as the receipt check sees it.
fn test_binary_dir() -> std::path::PathBuf {
    assert_cmd::cargo::cargo_bin(APP)
        .canonicalize()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

#[tokio::test]
async fn check_reports_a_newer_release_with_exit_code_10() {
    let server = mock_latest_release("99.0.0").await;
    let config = TempDir::new().unwrap();

    update_cmd(&server, config.path())
        .args(["update", "--check"])
        .assert()
        .code(10)
        .stdout(predicate::str::contains(format!(
            "Update available: {CURRENT} -> 99.0.0"
        )))
        // No receipt: this copy cannot update itself, so it says how to upgrade.
        .stdout(predicate::str::contains("one-line installer"));
}

#[tokio::test]
async fn check_reports_up_to_date_with_exit_code_0() {
    let server = mock_latest_release(CURRENT).await;
    let config = TempDir::new().unwrap();

    update_cmd(&server, config.path())
        .args(["update", "--check"])
        .assert()
        .code(0)
        .stdout(predicate::str::contains(format!(
            "{APP} {CURRENT} is the latest version."
        )));
}

#[tokio::test]
async fn update_without_receipt_explains_and_changes_nothing() {
    // The installer assets are not served: if `update` tried to download one, it would fail
    // with a different message.
    let server = mock_latest_release("99.0.0").await;
    let config = TempDir::new().unwrap();

    update_cmd(&server, config.path())
        .arg("update")
        .assert()
        .code(1)
        .stderr(predicate::str::contains("cannot update itself"));
}

#[tokio::test]
async fn check_with_receipt_for_this_binary_offers_update_command() {
    let server = mock_latest_release("99.0.0").await;
    let config = TempDir::new().unwrap();
    write_receipt(config.path(), &test_binary_dir());

    update_cmd(&server, config.path())
        .args(["update", "--check"])
        .assert()
        .code(10)
        .stdout(predicate::str::contains(format!(
            "Run `{APP} update` to install it."
        )));
}

#[tokio::test]
async fn update_with_receipt_for_another_directory_names_that_directory() {
    let server = mock_latest_release("99.0.0").await;
    let config = TempDir::new().unwrap();
    let other = TempDir::new().unwrap();
    let other_dir = other.path().canonicalize().unwrap();
    write_receipt(config.path(), &other_dir);

    update_cmd(&server, config.path())
        .arg("update")
        .assert()
        .code(1)
        .stderr(predicate::str::contains(other_dir.display().to_string()));
}

#[tokio::test]
async fn server_error_exits_1() {
    let server = mock_failing_api(500).await;
    let config = TempDir::new().unwrap();

    update_cmd(&server, config.path())
        .args(["update", "--check"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("could not check for updates"));
}

#[tokio::test]
async fn rate_limit_error_suggests_a_token() {
    let server = mock_failing_api(403).await;
    let config = TempDir::new().unwrap();

    update_cmd(&server, config.path())
        .args(["update", "--check"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains(
            "LOCAL_GIT_BRANCH_CLEANUP_TUI_GITHUB_TOKEN",
        ));
}

#[test]
fn update_conflicts_with_tui_flags() {
    Command::cargo_bin(APP)
        .unwrap()
        .args(["--cli", "update"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("cannot be used with"));
}

#[tokio::test]
async fn check_reads_the_feed_when_the_api_is_rate_limited() {
    let server = MockServer::start().await;
    mount_feed(&server, &["99.0.0", CURRENT]).await;
    mount_api(&server, 403, &lookup_endpoints(), 0).await;
    let config = TempDir::new().unwrap();

    update_cmd(&server, config.path())
        .args(["update", "--check"])
        .assert()
        .code(10)
        .stdout(predicate::str::contains(format!(
            "Update available: {CURRENT} -> 99.0.0"
        )));
}

#[tokio::test]
async fn check_up_to_date_from_the_feed_makes_no_api_request() {
    let server = MockServer::start().await;
    mount_feed(&server, &[CURRENT]).await;
    mount_api(&server, 200, &lookup_endpoints(), 0).await;
    let config = TempDir::new().unwrap();

    update_cmd(&server, config.path())
        .args(["update", "--check"])
        .assert()
        .code(0)
        .stdout(predicate::str::contains(format!(
            "{APP} {CURRENT} is the latest version."
        )));
}

#[tokio::test]
async fn update_from_the_feed_looks_up_only_that_tag() {
    let server = MockServer::start().await;
    mount_feed(&server, &["99.0.0"]).await;
    mount_api(&server, 200, &lookup_endpoints(), 0).await;
    // Rate limited, so nothing gets installed; the one request is all this test needs.
    mount_api(&server, 403, &[format!("{RELEASES}/tags/{APP}-v99.0.0")], 1).await;
    mount_rate_limit(&server, 0).await;
    let config = TempDir::new().unwrap();
    write_receipt(config.path(), &test_binary_dir());

    update_cmd(&server, config.path())
        .arg("update")
        .assert()
        .code(1)
        .stdout(predicate::str::contains(format!(
            "Update available: {CURRENT} -> 99.0.0"
        )))
        .stderr(predicate::str::contains("error: the update failed.\n"))
        // axoupdater's misleading "not found" is replaced by the real cause.
        .stderr(predicate::str::contains("was not found").not())
        .stderr(predicate::str::contains("is used up. It resets at"))
        .stderr(predicate::str::contains("(in 23 minutes): try again then"))
        .stderr(predicate::str::contains(
            "LOCAL_GIT_BRANCH_CLEANUP_TUI_GITHUB_TOKEN",
        ));
}

#[tokio::test]
async fn failed_update_with_quota_left_gives_no_rate_limit_hint() {
    let server = MockServer::start().await;
    mount_feed(&server, &["99.0.0"]).await;
    // A missing release, not a rate limit: axoupdater reports both as "not found".
    mount_api(&server, 404, &[format!("{RELEASES}/tags/{APP}-v99.0.0")], 1).await;
    mount_rate_limit(&server, 42).await;
    let config = TempDir::new().unwrap();
    write_receipt(config.path(), &test_binary_dir());

    update_cmd(&server, config.path())
        .arg("update")
        .assert()
        .code(1)
        .stderr(predicate::str::contains("the update failed"))
        .stderr(predicate::str::contains("GITHUB_TOKEN").not());
}
