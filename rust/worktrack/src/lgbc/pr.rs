// Copied from rust/local-git-branch-cleanup-tui/src/pr/mod.rs at 7257f76bb3abe5719a52284f2b8d835151f9cb15. Local changes: see src/lgbc/ORIGIN.md.
// Provider-neutral pull request domain types, provider interface, and
// fetch coordinator.
//
// This module owns the shared PR data model used by branch classification,
// the SQLite cache, and the CLI/TUI rendering layers. Hosting-provider
// implementations (GitHub, Bitbucket Data Center) live in submodules and
// only exchange these shared types with the rest of the application.

use std::time::Duration;

/// Which hosting provider supplies PR data for this run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrProviderKind {
    BitbucketDataCenter,
}

impl PrProviderKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::BitbucketDataCenter => "Bitbucket",
        }
    }
}

/// Pull request state, normalized across providers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrState {
    /// PR is open (pending review/merge)
    Open,
    /// PR was merged
    Merged,
    /// PR was closed without merging (declined on Bitbucket)
    Closed,
}

impl PrState {
    /// Get display label for the PR state
    pub fn label(&self) -> &'static str {
        match self {
            PrState::Open => "open",
            PrState::Merged => "merged",
            PrState::Closed => "closed",
        }
    }

    /// Get icon for the PR state
    pub fn icon(&self) -> &'static str {
        match self {
            PrState::Open => "🟡",
            PrState::Merged => "🟢",
            PrState::Closed => "🔴",
        }
    }
}

/// Pull request information, normalized across providers.
///
/// For GitHub, `number` is the PR number; for Bitbucket Data Center it is
/// the pull request `id`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrInfo {
    /// PR number (e.g., 123)
    pub number: u64,
    /// PR state (open, merged, closed)
    pub state: PrState,
    /// PR title
    pub title: String,
    /// PR URL on the hosting provider
    pub url: String,
}

/// Typed provider error. A provider error means the lookup FAILED — it must
/// never be recorded as a negative ("no PR") cache entry.
///
/// Display output must be actionable and must never contain the access token,
/// the `Authorization` header, or complete response headers.
#[derive(Debug, Clone)]
pub enum PrProviderError {
    Configuration(String),
    Authentication(String),
    Forbidden(String),
    NotFound(String),
    RateLimited { retry_after: Option<Duration> },
    Transport(String),
    Tls(String),
    InvalidResponse(String),
    ProviderUnavailable(String),
}

impl std::fmt::Display for PrProviderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Configuration(msg) => write!(f, "configuration error: {msg}"),
            Self::Authentication(msg) => write!(f, "authentication failed: {msg}"),
            Self::Forbidden(msg) => write!(f, "access forbidden: {msg}"),
            Self::NotFound(msg) => write!(f, "not found: {msg}"),
            Self::RateLimited { retry_after } => match retry_after {
                Some(d) => write!(f, "rate limited (retry after {}s)", d.as_secs()),
                None => write!(f, "rate limited"),
            },
            Self::Transport(msg) => write!(f, "network error: {msg}"),
            Self::Tls(msg) => write!(f, "TLS error: {msg}"),
            Self::InvalidResponse(msg) => write!(f, "invalid provider response: {msg}"),
            Self::ProviderUnavailable(msg) => write!(f, "provider unavailable: {msg}"),
        }
    }
}

/// Interface every PR hosting provider implements.
///
/// Contract:
/// - `Ok(Some(pr))`: lookup succeeded and found a PR.
/// - `Ok(None)`: lookup succeeded and found no PR (may be negatively cached).
/// - `Err(_)`: lookup failed (must NOT be negatively cached).
/// - `validate()` verifies provider prerequisites and repository access
///   before branch lookups begin.
#[async_trait::async_trait]
pub trait PullRequestProvider: Send + Sync {
    fn kind(&self) -> PrProviderKind;
    fn cache_key(&self) -> String;
    fn max_concurrency(&self) -> usize;

    async fn validate(&self) -> Result<(), PrProviderError>;

    async fn get_pr_for_branch(&self, branch_name: &str)
        -> Result<Option<PrInfo>, PrProviderError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pr_state_label() {
        assert_eq!(PrState::Open.label(), "open");
        assert_eq!(PrState::Merged.label(), "merged");
        assert_eq!(PrState::Closed.label(), "closed");
    }

    #[test]
    fn test_pr_state_icon() {
        assert_eq!(PrState::Open.icon(), "🟡");
        assert_eq!(PrState::Merged.icon(), "🟢");
        assert_eq!(PrState::Closed.icon(), "🔴");
    }
}
