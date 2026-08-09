// Provider-neutral pull request domain types, provider interface, and
// fetch coordinator.
//
// This module owns the shared PR data model used by branch classification,
// the SQLite cache, and the CLI/TUI rendering layers. Hosting-provider
// implementations (GitHub, Bitbucket Data Center) live in submodules and
// only exchange these shared types with the rest of the application.

pub mod bitbucket;
pub mod github;

use crate::cache::{CacheResult, PrCache};
use crate::git::BranchInfo;
use std::sync::Arc;
use std::time::Duration;

/// Which hosting provider supplies PR data for this run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrProviderKind {
    GitHub,
    BitbucketDataCenter,
}

impl PrProviderKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::GitHub => "GitHub",
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

/// One failed branch lookup: the branch name and the sanitized provider error.
#[derive(Debug)]
pub struct BranchFetchError {
    pub branch: String,
    pub error: PrProviderError,
}

/// Outcome of a `fetch_pr_info_for_branches` run.
#[derive(Debug, Default)]
pub struct FetchReport {
    pub cache_hits: usize,
    pub cache_misses: usize,
    pub fetched_with_pr: usize,
    pub fetched_without_pr: usize,
    pub failed: usize,
    pub errors: Vec<BranchFetchError>,
}

/// Fetch PR information for multiple branches, consulting the cache first.
///
/// # Algorithm (three passes)
///
/// **Pass 1** — Serve cache hits synchronously. Collect indices of misses.
/// **Pass 2** — Fetch all misses concurrently using `tokio::task::JoinSet` and a
///              `Semaphore` capped at the provider's `max_concurrency()`.
/// **Pass 3** — Write the fetched results back into `branches` (original order)
///              and the cache. Failed lookups are recorded in the report and
///              never written to the cache.
///
/// `cache = None` fetches all branches without consulting the cache.
/// `sequential = true` caps effective concurrency at 1 — for benchmarking.
pub async fn fetch_pr_info_for_branches(
    provider: Arc<dyn PullRequestProvider>,
    branches: &mut [BranchInfo],
    cache: Option<&mut PrCache>,
    sequential: bool,
) -> FetchReport {
    use std::collections::HashMap;
    use tokio::sync::Semaphore;
    use tokio::task::JoinSet;

    let mut report = FetchReport::default();

    // Pass 2 helper — fetch a list of branch names, returning results indexed
    // by position so pass 3 can pair them back to their branch indices.
    // A cancelled/panicked task is a failed lookup, not a "no PR" result.
    async fn fetch_all(
        provider: Arc<dyn PullRequestProvider>,
        names: Vec<String>,
        sequential: bool,
    ) -> Vec<Result<Option<PrInfo>, PrProviderError>> {
        let concurrency = if sequential {
            1
        } else {
            provider.max_concurrency().max(1)
        };
        let sem = Arc::new(Semaphore::new(concurrency));
        let expected = names.len();
        let mut handles: JoinSet<(usize, Result<Option<PrInfo>, PrProviderError>)> = JoinSet::new();

        for (i, name) in names.into_iter().enumerate() {
            let sem = Arc::clone(&sem);
            let provider = Arc::clone(&provider);
            handles.spawn(async move {
                let _permit = sem.acquire_owned().await.expect("semaphore closed");
                (i, provider.get_pr_for_branch(&name).await)
            });
        }

        let mut map: HashMap<usize, Result<Option<PrInfo>, PrProviderError>> =
            HashMap::with_capacity(expected);
        while let Some(res) = handles.join_next().await {
            if let Ok((i, result)) = res {
                map.insert(i, result);
            }
        }

        (0..expected)
            .map(|i| {
                map.remove(&i).unwrap_or_else(|| {
                    Err(PrProviderError::ProviderUnavailable(
                        "lookup task was cancelled".to_string(),
                    ))
                })
            })
            .collect()
    }

    match cache {
        Some(pr_cache) => {
            // --- Pass 1: serve cache hits synchronously ---
            let mut miss_indices: Vec<usize> = Vec::new();

            for (i, branch) in branches.iter_mut().enumerate() {
                match pr_cache.get(&branch.name) {
                    CacheResult::Hit(pr_info) => {
                        branch.pr_info = pr_info;
                        report.cache_hits += 1;
                    }
                    CacheResult::Miss => {
                        miss_indices.push(i);
                    }
                }
            }

            report.cache_misses = miss_indices.len();
            if miss_indices.is_empty() {
                return report;
            }

            // --- Pass 2: fetch misses concurrently ---
            let names: Vec<String> = miss_indices
                .iter()
                .map(|&i| branches[i].name.clone())
                .collect();

            let results = fetch_all(provider, names, sequential).await;

            // --- Pass 3: write results back to branches and cache ---
            for (&idx, result) in miss_indices.iter().zip(results) {
                match result {
                    Ok(pr_opt) => {
                        let _ = pr_cache.set(&branches[idx].name, pr_opt.as_ref());
                        if pr_opt.is_some() {
                            report.fetched_with_pr += 1;
                        } else {
                            report.fetched_without_pr += 1;
                        }
                        branches[idx].pr_info = pr_opt;
                    }
                    Err(error) => {
                        report.failed += 1;
                        report.errors.push(BranchFetchError {
                            branch: branches[idx].name.clone(),
                            error,
                        });
                        branches[idx].pr_info = None;
                    }
                }
            }
        }
        None => {
            // No cache — fetch all branches.
            report.cache_misses = branches.len();
            let names: Vec<String> = branches.iter().map(|b| b.name.clone()).collect();
            let results = fetch_all(provider, names, sequential).await;
            for (branch, result) in branches.iter_mut().zip(results) {
                match result {
                    Ok(pr_opt) => {
                        if pr_opt.is_some() {
                            report.fetched_with_pr += 1;
                        } else {
                            report.fetched_without_pr += 1;
                        }
                        branch.pr_info = pr_opt;
                    }
                    Err(error) => {
                        report.failed += 1;
                        report.errors.push(BranchFetchError {
                            branch: branch.name.clone(),
                            error,
                        });
                        branch.pr_info = None;
                    }
                }
            }
        }
    }

    report
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

    // --- Fetch coordinator tests (fake provider, no network) ---

    use crate::git::BranchStatus;
    use rusqlite::Connection;
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    fn make_pr(number: u64) -> PrInfo {
        PrInfo {
            number,
            state: PrState::Merged,
            title: format!("PR #{number}"),
            url: format!("https://example.com/pull/{number}"),
        }
    }

    fn make_branch(name: &str) -> BranchInfo {
        BranchInfo {
            name: name.to_string(),
            upstream: None,
            last_commit_relative: "1 day ago".to_string(),
            status: BranchStatus::Unmerged,
            last_commit_sha: "abc1234".to_string(),
            last_commit_author: "Test".to_string(),
            last_commit_message: "test commit".to_string(),
            ahead: None,
            behind: None,
            last_activity_timestamp: 0,
            branch_created_timestamp: 0,
            branch_author: "Test".to_string(),
            pr_info: None,
        }
    }

    fn open_cache(ttl: Duration) -> PrCache {
        let conn = Connection::open_in_memory().unwrap();
        PrCache::open_with_conn(conn, "test/repo", ttl).unwrap()
    }

    /// Test provider returning canned per-branch results and recording calls.
    struct FakeProvider {
        responses: HashMap<String, Result<Option<PrInfo>, PrProviderError>>,
        calls: Mutex<Vec<String>>,
        max_concurrency: usize,
        in_flight: AtomicUsize,
        max_in_flight: AtomicUsize,
        delay: Option<Duration>,
    }

    impl FakeProvider {
        fn new(responses: HashMap<String, Result<Option<PrInfo>, PrProviderError>>) -> Self {
            Self {
                responses,
                calls: Mutex::new(Vec::new()),
                max_concurrency: 8,
                in_flight: AtomicUsize::new(0),
                max_in_flight: AtomicUsize::new(0),
                delay: None,
            }
        }

        fn call_count(&self) -> usize {
            self.calls.lock().unwrap().len()
        }
    }

    #[async_trait::async_trait]
    impl PullRequestProvider for FakeProvider {
        fn kind(&self) -> PrProviderKind {
            PrProviderKind::GitHub
        }

        fn cache_key(&self) -> String {
            "test/repo".to_string()
        }

        fn max_concurrency(&self) -> usize {
            self.max_concurrency
        }

        async fn validate(&self) -> Result<(), PrProviderError> {
            Ok(())
        }

        async fn get_pr_for_branch(
            &self,
            branch_name: &str,
        ) -> Result<Option<PrInfo>, PrProviderError> {
            let current = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
            self.max_in_flight.fetch_max(current, Ordering::SeqCst);
            self.calls.lock().unwrap().push(branch_name.to_string());
            if let Some(delay) = self.delay {
                tokio::time::sleep(delay).await;
            }
            self.in_flight.fetch_sub(1, Ordering::SeqCst);
            self.responses.get(branch_name).cloned().unwrap_or(Ok(None))
        }
    }

    #[tokio::test]
    async fn positive_cache_hit_avoids_provider_call() {
        let mut cache = open_cache(Duration::from_secs(3600));
        cache.set("feature/a", Some(&make_pr(7))).unwrap();

        let provider = Arc::new(FakeProvider::new(HashMap::new()));
        let mut branches = vec![make_branch("feature/a")];
        let report = fetch_pr_info_for_branches(
            Arc::clone(&provider) as _,
            &mut branches,
            Some(&mut cache),
            false,
        )
        .await;

        assert_eq!(provider.call_count(), 0);
        assert_eq!(branches[0].pr_info.as_ref().map(|p| p.number), Some(7));
        assert_eq!(report.cache_hits, 1);
        assert_eq!(report.cache_misses, 0);
    }

    #[tokio::test]
    async fn negative_cache_hit_avoids_provider_call() {
        let mut cache = open_cache(Duration::from_secs(3600));
        cache.set("feature/none", None).unwrap();

        let provider = Arc::new(FakeProvider::new(HashMap::new()));
        let mut branches = vec![make_branch("feature/none")];
        let report = fetch_pr_info_for_branches(
            Arc::clone(&provider) as _,
            &mut branches,
            Some(&mut cache),
            false,
        )
        .await;

        assert_eq!(provider.call_count(), 0);
        assert!(branches[0].pr_info.is_none());
        assert_eq!(report.cache_hits, 1);
    }

    #[tokio::test]
    async fn expired_cache_row_causes_provider_call() {
        let mut cache = open_cache(Duration::ZERO); // TTL = 0 → always expired
        cache.set("feature/x", Some(&make_pr(1))).unwrap();

        let mut responses = HashMap::new();
        responses.insert("feature/x".to_string(), Ok(Some(make_pr(99))));
        let provider = Arc::new(FakeProvider::new(responses));

        let mut branches = vec![make_branch("feature/x")];
        fetch_pr_info_for_branches(
            Arc::clone(&provider) as _,
            &mut branches,
            Some(&mut cache),
            false,
        )
        .await;

        assert_eq!(provider.call_count(), 1);
        assert_eq!(branches[0].pr_info.as_ref().map(|p| p.number), Some(99));
    }

    #[tokio::test]
    async fn ok_some_is_written_to_cache() {
        let mut cache = open_cache(Duration::from_secs(3600));
        let mut responses = HashMap::new();
        responses.insert("feature/a".to_string(), Ok(Some(make_pr(5))));
        let provider = Arc::new(FakeProvider::new(responses));

        let mut branches = vec![make_branch("feature/a")];
        let report = fetch_pr_info_for_branches(
            Arc::clone(&provider) as _,
            &mut branches,
            Some(&mut cache),
            false,
        )
        .await;

        assert_eq!(report.fetched_with_pr, 1);
        match cache.get("feature/a") {
            CacheResult::Hit(Some(pr)) => assert_eq!(pr.number, 5),
            other => panic!("expected cached PR, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn ok_none_is_written_as_negative_cache() {
        let mut cache = open_cache(Duration::from_secs(3600));
        let mut responses = HashMap::new();
        responses.insert("feature/a".to_string(), Ok(None));
        let provider = Arc::new(FakeProvider::new(responses));

        let mut branches = vec![make_branch("feature/a")];
        let report = fetch_pr_info_for_branches(
            Arc::clone(&provider) as _,
            &mut branches,
            Some(&mut cache),
            false,
        )
        .await;

        assert_eq!(report.fetched_without_pr, 1);
        assert!(matches!(cache.get("feature/a"), CacheResult::Hit(None)));
    }

    #[tokio::test]
    async fn error_is_never_written_to_cache() {
        let mut cache = open_cache(Duration::from_secs(3600));
        let mut responses = HashMap::new();
        responses.insert(
            "feature/broken".to_string(),
            Err(PrProviderError::ProviderUnavailable("boom".to_string())),
        );
        let provider = Arc::new(FakeProvider::new(responses));

        let mut branches = vec![make_branch("feature/broken")];
        let report = fetch_pr_info_for_branches(
            Arc::clone(&provider) as _,
            &mut branches,
            Some(&mut cache),
            false,
        )
        .await;

        assert_eq!(report.failed, 1);
        assert_eq!(report.errors.len(), 1);
        assert_eq!(report.errors[0].branch, "feature/broken");
        assert!(branches[0].pr_info.is_none());
        // The failure must NOT be recorded as a negative cache entry.
        assert!(matches!(cache.get("feature/broken"), CacheResult::Miss));
    }

    #[tokio::test]
    async fn mixed_results_produce_correct_report_counts() {
        let mut cache = open_cache(Duration::from_secs(3600));
        cache.set("cached", Some(&make_pr(1))).unwrap();

        let mut responses = HashMap::new();
        responses.insert("with-pr".to_string(), Ok(Some(make_pr(2))));
        responses.insert("without-pr".to_string(), Ok(None));
        responses.insert(
            "failing".to_string(),
            Err(PrProviderError::Transport("reset".to_string())),
        );
        let provider = Arc::new(FakeProvider::new(responses));

        let mut branches = vec![
            make_branch("cached"),
            make_branch("with-pr"),
            make_branch("without-pr"),
            make_branch("failing"),
        ];
        let report = fetch_pr_info_for_branches(
            Arc::clone(&provider) as _,
            &mut branches,
            Some(&mut cache),
            false,
        )
        .await;

        assert_eq!(report.cache_hits, 1);
        assert_eq!(report.cache_misses, 3);
        assert_eq!(report.fetched_with_pr, 1);
        assert_eq!(report.fetched_without_pr, 1);
        assert_eq!(report.failed, 1);
    }

    #[tokio::test]
    async fn result_order_matches_branch_order_under_concurrency() {
        let mut responses = HashMap::new();
        for i in 0..12u64 {
            responses.insert(format!("branch-{i}"), Ok(Some(make_pr(i))));
        }
        let mut provider = FakeProvider::new(responses);
        provider.delay = Some(Duration::from_millis(5));
        let provider = Arc::new(provider);

        let mut branches: Vec<BranchInfo> = (0..12)
            .map(|i| make_branch(&format!("branch-{i}")))
            .collect();
        fetch_pr_info_for_branches(Arc::clone(&provider) as _, &mut branches, None, false).await;

        for (i, branch) in branches.iter().enumerate() {
            assert_eq!(
                branch.pr_info.as_ref().map(|p| p.number),
                Some(i as u64),
                "branch {i} got the wrong PR"
            );
        }
    }

    #[tokio::test]
    async fn sequential_mode_never_exceeds_one_in_flight() {
        let mut provider = FakeProvider::new(HashMap::new());
        provider.delay = Some(Duration::from_millis(5));
        let provider = Arc::new(provider);

        let mut branches: Vec<BranchInfo> =
            (0..6).map(|i| make_branch(&format!("b-{i}"))).collect();
        fetch_pr_info_for_branches(Arc::clone(&provider) as _, &mut branches, None, true).await;

        assert_eq!(provider.max_in_flight.load(Ordering::SeqCst), 1);
        assert_eq!(provider.call_count(), 6);
    }

    #[tokio::test]
    async fn provider_concurrency_limit_is_respected() {
        let mut provider = FakeProvider::new(HashMap::new());
        provider.max_concurrency = 3;
        provider.delay = Some(Duration::from_millis(10));
        let provider = Arc::new(provider);

        let mut branches: Vec<BranchInfo> =
            (0..10).map(|i| make_branch(&format!("b-{i}"))).collect();
        fetch_pr_info_for_branches(Arc::clone(&provider) as _, &mut branches, None, false).await;

        assert!(provider.max_in_flight.load(Ordering::SeqCst) <= 3);
        assert_eq!(provider.call_count(), 10);
    }

    #[tokio::test]
    async fn empty_branch_slice_is_noop() {
        let provider = Arc::new(FakeProvider::new(HashMap::new()));
        let mut branches: Vec<BranchInfo> = vec![];
        let report =
            fetch_pr_info_for_branches(Arc::clone(&provider) as _, &mut branches, None, false)
                .await;
        assert_eq!(provider.call_count(), 0);
        assert_eq!(report.cache_misses, 0);
    }
}
