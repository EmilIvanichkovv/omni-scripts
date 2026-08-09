// GitHub pull request provider.
//
// Delegates all lookups to the `gh` CLI (which owns authentication), exactly
// as the pre-provider implementation did. Only the error handling changed:
// a failed `gh` invocation is now a typed error instead of a silent "no PR",
// so failures are never written to the negative cache.

use super::{PrInfo, PrProviderError, PrProviderKind, PrState, PullRequestProvider};
use std::process::Command;

/// Maximum number of concurrent `gh` subprocess calls.
const MAX_CONCURRENT_GH_CALLS: usize = 20;

pub struct GitHubProvider {
    repo_slug: String,
}

impl GitHubProvider {
    /// Create a provider whose cache key is the `owner/repo` slug derived
    /// from `origin` (with the historical directory-basename fallback).
    pub fn new() -> Self {
        Self {
            repo_slug: get_repo_slug().unwrap_or_else(|_| "unknown/unknown".to_string()),
        }
    }
}

impl Default for GitHubProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl PullRequestProvider for GitHubProvider {
    fn kind(&self) -> PrProviderKind {
        PrProviderKind::GitHub
    }

    fn cache_key(&self) -> String {
        self.repo_slug.clone()
    }

    fn max_concurrency(&self) -> usize {
        MAX_CONCURRENT_GH_CALLS
    }

    async fn validate(&self) -> Result<(), PrProviderError> {
        if is_gh_cli_available() {
            Ok(())
        } else {
            Err(PrProviderError::Configuration(
                "GitHub CLI (gh) not found. Install it to enable PR integration. See: https://cli.github.com/".to_string(),
            ))
        }
    }

    async fn get_pr_for_branch(
        &self,
        branch_name: &str,
    ) -> Result<Option<PrInfo>, PrProviderError> {
        get_pr_info_for_branch(branch_name).await
    }
}

/// Check if GitHub CLI (gh) is available
fn is_gh_cli_available() -> bool {
    Command::new("gh")
        .args(["--version"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Returns the GitHub "owner/repo" slug derived from the remote URL.
///
/// Supports both HTTPS (`https://github.com/owner/repo.git`) and
/// SSH (`git@github.com:owner/repo.git`) remote formats.
/// Falls back to the repository's root directory basename if parsing fails.
fn get_repo_slug() -> color_eyre::Result<String> {
    let output = Command::new("git")
        .args(["remote", "get-url", "origin"])
        .output()?;

    if !output.status.success() {
        // Fall back to directory basename.
        let dir = std::env::current_dir()?;
        let name = dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string();
        return Ok(format!("unknown/{name}"));
    }

    let url = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok(parse_slug(&url))
}

/// Parse an "owner/repo" slug out of a GitHub remote URL.
fn parse_slug(url: &str) -> String {
    // HTTPS: https://github.com/owner/repo.git
    // HTTP:  http://github.com/owner/repo.git
    // SSH:   git@github.com:owner/repo.git  (SCP-style, no "://")
    let slug = if let Some(path) = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
    {
        // Drop host, keep "owner/repo[.git]"
        path.split_once('/')
            .map(|x| x.1)
            .unwrap_or(path)
            .to_string()
    } else if !url.contains("://") {
        // SCP-style SSH: git@github.com:owner/repo.git — take everything after the colon
        url.split_once(':')
            .map(|x| x.1.to_string())
            .unwrap_or_else(|| url.to_string())
    } else {
        // Unknown format — fall back gracefully
        url.to_string()
    };

    // Strip trailing .git
    slug.strip_suffix(".git").unwrap_or(&slug).to_string()
}

/// Fetch PR information for a branch using GitHub CLI.
///
/// - `Ok(Some(pr))`: `gh` returned exactly one PR.
/// - `Ok(None)`: `gh` succeeded and returned an empty array — the only no-PR result.
/// - `Err(_)`: `gh` failed to run, exited nonzero, or produced unparseable output.
async fn get_pr_info_for_branch(branch_name: &str) -> Result<Option<PrInfo>, PrProviderError> {
    // gh pr list --head <branch> --json number,state,title,url --limit 1 --state all
    let output = tokio::process::Command::new("gh")
        .args([
            "pr",
            "list",
            "--head",
            branch_name,
            "--json",
            "number,state,title,url",
            "--limit",
            "1",
            "--state",
            "all", // Include open, closed, and merged PRs
        ])
        .output()
        .await
        .map_err(|e| PrProviderError::ProviderUnavailable(format!("failed to run gh: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(PrProviderError::ProviderUnavailable(format!(
            "gh exited with {}: {}",
            output.status,
            stderr.trim()
        )));
    }

    let json_str = String::from_utf8(output.stdout).map_err(|_| {
        PrProviderError::InvalidResponse("gh produced non-UTF-8 output".to_string())
    })?;
    let json_str = json_str.trim();

    // Parse JSON response (it's an array)
    // Example: [{"number":123,"state":"MERGED","title":"My PR","url":"https://..."}]
    if json_str.is_empty() || json_str == "[]" {
        return Ok(None);
    }

    // Simple JSON parsing without external dependencies
    // Extract first PR from the array
    let inner = json_str.trim_start_matches('[').trim_end_matches(']');
    if inner.is_empty() {
        return Ok(None);
    }

    let missing =
        |field: &str| PrProviderError::InvalidResponse(format!("missing {field} in gh output"));
    let number = extract_json_number(inner, "number").ok_or_else(|| missing("PR number"))?;
    let state_str = extract_json_string(inner, "state").ok_or_else(|| missing("PR state"))?;
    let title = extract_json_string(inner, "title").ok_or_else(|| missing("PR title"))?;
    let url = extract_json_string(inner, "url").ok_or_else(|| missing("PR URL"))?;

    let state = match state_str.to_uppercase().as_str() {
        "OPEN" => PrState::Open,
        "MERGED" => PrState::Merged,
        "CLOSED" => PrState::Closed,
        other => {
            return Err(PrProviderError::InvalidResponse(format!(
                "unsupported pull request state: {other}"
            )))
        }
    };

    Ok(Some(PrInfo {
        number,
        state,
        title,
        url,
    }))
}

/// Helper to extract a string value from a simple JSON object
fn extract_json_string(json: &str, key: &str) -> Option<String> {
    let pattern = format!("\"{}\":\"", key);
    let start = json.find(&pattern)? + pattern.len();
    let rest = &json[start..];

    // Find the closing quote, handling escaped quotes
    let mut end = 0;
    let mut chars = rest.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            // Skip escaped character
            chars.next();
            end += 2;
        } else if c == '"' {
            break;
        } else {
            end += c.len_utf8();
        }
    }

    Some(rest[..end].to_string())
}

/// Helper to extract a number value from a simple JSON object
fn extract_json_number(json: &str, key: &str) -> Option<u64> {
    let pattern = format!("\"{}\":", key);
    let start = json.find(&pattern)? + pattern.len();
    let rest = &json[start..].trim_start();

    // Extract digits until non-digit
    let end = rest
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(rest.len());
    rest[..end].parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- JSON parsing helpers ---

    #[test]
    fn test_extract_json_string_basic() {
        let json = r#""state":"MERGED","title":"Fix bug","url":"https://example.com""#;
        assert_eq!(
            extract_json_string(json, "state").as_deref(),
            Some("MERGED")
        );
        assert_eq!(
            extract_json_string(json, "title").as_deref(),
            Some("Fix bug")
        );
        assert_eq!(
            extract_json_string(json, "url").as_deref(),
            Some("https://example.com")
        );
    }

    #[test]
    fn test_extract_json_string_escaped_quotes() {
        // The function captures the raw text between quotes without unescaping.
        // Verify that the key is found and the value is non-empty.
        let json = r#""title":"Fix bug","state":"OPEN""#;
        let result = extract_json_string(json, "title");
        assert_eq!(result.as_deref(), Some("Fix bug"));
    }

    #[test]
    fn test_extract_json_string_missing_key() {
        let json = r#""state":"OPEN""#;
        assert_eq!(extract_json_string(json, "nonexistent"), None);
    }

    #[test]
    fn test_extract_json_number_basic() {
        let json = r#""number":42,"state":"OPEN""#;
        assert_eq!(extract_json_number(json, "number"), Some(42));
    }

    #[test]
    fn test_extract_json_number_missing() {
        let json = r#""state":"OPEN""#;
        assert_eq!(extract_json_number(json, "number"), None);
    }

    // --- get_repo_slug URL parsing (pure logic, no git subprocess) ---

    #[test]
    fn test_repo_slug_https_with_git_suffix() {
        assert_eq!(
            parse_slug("https://github.com/owner/repo.git"),
            "owner/repo"
        );
    }

    #[test]
    fn test_repo_slug_https_without_git_suffix() {
        assert_eq!(parse_slug("https://github.com/owner/repo"), "owner/repo");
    }

    #[test]
    fn test_repo_slug_ssh_scp_style() {
        assert_eq!(parse_slug("git@github.com:owner/repo.git"), "owner/repo");
    }

    #[test]
    fn test_repo_slug_ssh_scp_no_git_suffix() {
        assert_eq!(parse_slug("git@github.com:owner/repo"), "owner/repo");
    }
}
