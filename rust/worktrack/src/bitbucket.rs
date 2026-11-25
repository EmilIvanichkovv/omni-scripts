//! worktrack-specific Bitbucket Data Center layer (spec 5) on top of the copied
//! client in `lgbc::bitbucket`.
//!
//! What lives here:
//! - [`Token`]: `BITBUCKET_TOKEN` from the environment only; never logged, never
//!   in errors, `Debug` is redacted (spec 11.3).
//! - [`HostGate`]: at most [`MAX_IN_FLIGHT_PER_HOST`] requests in flight per
//!   Bitbucket host; an HTTP 429 pauses that host's whole queue for
//!   `Retry-After` (spec 5.2).
//! - `pr_cache` / `build_cache` SQL ([`load_pr_cache`], [`upsert_pr_cache`],
//!   [`load_build_cache`], [`upsert_build_cache`], [`write_sync_rows`],
//!   [`load_cached_pr_data`]) and the pure cache rule [`pr_cache_decision`]
//!   (spec 5.2).
//! - [`BitbucketService::sync`]: one full sync over every repo (spec 5.1),
//!   returning a [`SyncReport`]; it performs no DB IO.
//!
//! Integration (refresh.rs, full sync task):
//! 1. Once per run: `let bb = Arc::new(BitbucketService::from_env());` (keep it for
//!    the app lifetime: it owns the per-host gates and HTTP clients). With
//!    `--offline`, do not call `sync`; report `PrStatus::Offline`.
//! 2. Startup render: `load_cached_pr_data(&reader)` gives the last known PR per
//!    `(repo_id, branch)` and CI per sha, served regardless of age.
//! 3. Full sync: read `load_pr_cache(&reader)`, build [`SyncInput`] (one
//!    [`SyncRepo`] per repo with a `BitbucketIdentity`; for a branch whose cached
//!    row is MERGED, set `reused_after_merge` from git: `merged_head(row)..tip`
//!    non-empty), then `let report = bb.sync(input).await;`.
//! 4. Persist: `let (prs, builds) = (report.pr_rows.clone(), report.build_rows.clone());
//!    writer.write(move |c| write_sync_rows(c, &prs, &builds)).await`.
//! 5. Use `report.latest_prs` / `report.ci` for `derive_state` and flags,
//!    `report.username` for ownership rule 2, and send
//!    `AppEvent::PrUpdated { status: report.status, synced_at: Some(report.synced_at) }`.

use crate::lgbc::bitbucket::{
    resolve_repository_input, BitbucketConfigInput, BitbucketHostClient, BitbucketRepository,
    HostPage, PullRequestDetails,
};
use crate::lgbc::pr::{PrProviderError, PrState};
use crate::model::{
    BitbucketIdentity, CiState, PrData, PrStateKind, RepoId, Reviewer, ReviewerStatus,
};
use crate::refresh::PrStatus;
use crate::sanitize::clean_line;
use color_eyre::Result;
use rusqlite::{params, Connection};
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::Semaphore;
use tokio::time::Instant;

/// The only source of the access token (spec 11.3).
pub const TOKEN_ENV: &str = "BITBUCKET_TOKEN";
/// Spec 5.2: max in-flight requests per Bitbucket host.
pub const MAX_IN_FLIGHT_PER_HOST: usize = 8;
/// Attempts per request when the host answers 429.
const RATE_LIMIT_ATTEMPTS: u32 = 3;
/// Pause when a 429 carries no usable `Retry-After`.
const DEFAULT_RATE_LIMIT_PAUSE: Duration = Duration::from_secs(5);
/// Upper bound on one pause, so a huge `Retry-After` cannot stall a sync.
const MAX_RATE_LIMIT_PAUSE: Duration = Duration::from_secs(60);
/// Guard against a server that never reports its last page.
const MAX_PAGES: usize = 50;

// ---------------------------------------------------------------------------
// Token
// ---------------------------------------------------------------------------

/// A Bitbucket HTTP access token. Never printed: `Debug` is redacted and there
/// is no `Display`.
#[derive(Clone)]
pub struct Token(String);

impl Token {
    /// `None` for an empty or whitespace-only value.
    pub fn new(raw: &str) -> Option<Self> {
        let t = raw.trim();
        (!t.is_empty()).then(|| Self(t.to_string()))
    }

    /// Read [`TOKEN_ENV`]; `None` when unset, empty, or not UTF-8.
    pub fn from_env() -> Option<Self> {
        std::env::var(TOKEN_ENV).ok().and_then(|v| Self::new(&v))
    }

    fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for Token {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Token(<redacted>)")
    }
}

// ---------------------------------------------------------------------------
// Per-host gate: concurrency limit + 429 pause
// ---------------------------------------------------------------------------

/// Concurrency limit and rate-limit pause shared by every request to one host.
pub struct HostGate {
    permits: Semaphore,
    paused_until: Mutex<Option<Instant>>,
}

impl HostGate {
    pub fn new(max_in_flight: usize) -> Self {
        Self {
            permits: Semaphore::new(max_in_flight.max(1)),
            paused_until: Mutex::new(None),
        }
    }

    /// Hold every request to this host (queued or new) for `duration`.
    /// A shorter pause never cuts a longer one short.
    pub fn pause_for(&self, duration: Duration) {
        let until = Instant::now() + duration;
        let mut guard = self.paused_until.lock().unwrap_or_else(|e| e.into_inner());
        if guard.is_none_or(|current| current < until) {
            *guard = Some(until);
        }
    }

    async fn wait_unpaused(&self) {
        loop {
            let until = *self.paused_until.lock().unwrap_or_else(|e| e.into_inner());
            match until {
                Some(u) if u > Instant::now() => tokio::time::sleep_until(u).await,
                _ => return,
            }
        }
    }

    /// Run one request under the gate. On `RateLimited` the host is paused for
    /// `Retry-After` (capped) and the request is retried, up to
    /// [`RATE_LIMIT_ATTEMPTS`] attempts in total.
    pub async fn run<T, F, Fut>(&self, mut op: F) -> Result<T, PrProviderError>
    where
        F: FnMut() -> Fut,
        Fut: Future<Output = Result<T, PrProviderError>>,
    {
        let mut attempt = 1;
        loop {
            self.wait_unpaused().await;
            let permit = self
                .permits
                .acquire()
                .await
                .expect("semaphore never closed");
            // A pause may have started while this request waited for a permit.
            self.wait_unpaused().await;
            let result = op().await;
            drop(permit);
            match result {
                Err(PrProviderError::RateLimited { retry_after })
                    if attempt < RATE_LIMIT_ATTEMPTS =>
                {
                    let pause = retry_after
                        .unwrap_or(DEFAULT_RATE_LIMIT_PAUSE)
                        .min(MAX_RATE_LIMIT_PAUSE);
                    tracing::warn!(
                        pause_secs = pause.as_secs(),
                        "Bitbucket rate limit, pausing host"
                    );
                    self.pause_for(pause);
                    attempt += 1;
                }
                other => return other,
            }
        }
    }
}

/// Fetch every page of a host listing through `gate`.
async fn all_pages<T, F, Fut>(
    gate: &HostGate,
    mut fetch: F,
) -> Result<(Vec<T>, Option<String>), PrProviderError>
where
    F: FnMut(u64) -> Fut,
    Fut: Future<Output = Result<HostPage<T>, PrProviderError>>,
{
    let mut start = 0;
    let mut values = Vec::new();
    let mut username = None;
    for _ in 0..MAX_PAGES {
        let page = gate.run(|| fetch(start)).await?;
        username = username.or(page.username);
        values.extend(page.values);
        match page.next_start {
            Some(next) if next > start => start = next,
            _ => break,
        }
    }
    Ok((values, username))
}

// ---------------------------------------------------------------------------
// Pure mapping
// ---------------------------------------------------------------------------

/// Map a Bitbucket PR to the shared model, sanitizing all server text.
pub fn pr_data_from_details(pr: &PullRequestDetails) -> PrData {
    PrData {
        id: pr.id,
        state: match pr.state {
            PrState::Open => PrStateKind::Open,
            PrState::Merged => PrStateKind::Merged,
            PrState::Closed => PrStateKind::Declined,
        },
        title: clean_line(&pr.title),
        url: clean_line(&pr.url),
        author_slug: clean_line(&pr.author_slug),
        reviewers: pr
            .reviewers
            .iter()
            .map(|r| Reviewer {
                name: clean_line(&r.display_name),
                slug: clean_line(&r.slug),
                status: reviewer_status(&r.status),
            })
            .collect(),
        open_tasks: pr.open_task_count,
        comment_count: pr.comment_count,
        from_latest_commit: pr.from_latest_commit.as_deref().map(clean_line),
        updated_at: pr.updated_date_ms.map(|ms| ms.div_euclid(1000)),
        closed_at: pr.closed_date_ms.map(|ms| ms.div_euclid(1000)),
    }
}

/// `APPROVED` / `NEEDS_WORK`; anything else counts as not yet reviewed.
pub fn reviewer_status(raw: &str) -> ReviewerStatus {
    if raw.eq_ignore_ascii_case("APPROVED") {
        ReviewerStatus::Approved
    } else if raw.eq_ignore_ascii_case("NEEDS_WORK") {
        ReviewerStatus::NeedsWork
    } else {
        ReviewerStatus::Unapproved
    }
}

/// Spec 5.1: any FAILED → Failed; else any INPROGRESS → InProgress; else all
/// SUCCESSFUL → Successful; no statuses (or an unknown mix) → `None`.
pub fn aggregate_build_states<S: AsRef<str>>(states: &[S]) -> Option<CiState> {
    let any = |want: &str| states.iter().any(|s| s.as_ref().eq_ignore_ascii_case(want));
    if states.is_empty() {
        None
    } else if any("FAILED") {
        Some(CiState::Failed)
    } else if any("INPROGRESS") {
        Some(CiState::InProgress)
    } else if states
        .iter()
        .all(|s| s.as_ref().eq_ignore_ascii_case("SUCCESSFUL"))
    {
        Some(CiState::Successful)
    } else {
        None
    }
}

/// The `BitbucketRepository` for a discovered identity (values are explicit,
/// so `origin` is not read).
pub fn repository_for(
    identity: &BitbucketIdentity,
) -> Result<BitbucketRepository, PrProviderError> {
    resolve_repository_input(BitbucketConfigInput {
        cli_base_url: Some(identity.base_url.clone()),
        cli_project: Some(identity.project.clone()),
        cli_repo: Some(identity.repo.clone()),
        ..Default::default()
    })
}

/// Header classes of a failed request (spec 5.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SyncErrorKind {
    /// HTTP 401.
    Unauthorized,
    /// HTTP 403.
    Forbidden,
    /// Network, TLS, timeout, 5xx, or rate limit that outlasted retries.
    Unreachable,
    /// Anything else (bad response, unknown PR state, 404, configuration).
    Other,
}

pub fn classify_error(err: &PrProviderError) -> SyncErrorKind {
    match err {
        PrProviderError::Authentication(_) => SyncErrorKind::Unauthorized,
        PrProviderError::Forbidden(_) => SyncErrorKind::Forbidden,
        PrProviderError::Transport(_)
        | PrProviderError::Tls(_)
        | PrProviderError::ProviderUnavailable(_)
        | PrProviderError::RateLimited { .. } => SyncErrorKind::Unreachable,
        PrProviderError::Configuration(_)
        | PrProviderError::NotFound(_)
        | PrProviderError::InvalidResponse(_) => SyncErrorKind::Other,
    }
}

// ---------------------------------------------------------------------------
// Cache (spec 5.2, 10.1)
// ---------------------------------------------------------------------------

/// One `pr_cache` row. `pr: None` = lookup succeeded and found no PR.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrCacheRow {
    pub repo_id: RepoId,
    pub branch: String,
    pub pr: Option<PrData>,
    /// Unix seconds.
    pub fetched_at: i64,
}

/// One `build_cache` row. `state: None` = no build statuses for the commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildCacheRow {
    pub sha: String,
    pub state: Option<CiState>,
    pub fetched_at: i64,
}

/// Whether a full sync may use a cached PR row instead of asking Bitbucket.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheDecision {
    Use,
    Fetch,
}

/// Spec 5.2, for a full sync. `reused_after_merge` is the caller's git check
/// for a cached MERGED row: `merged_head(row)..tip` is non-empty. Startup
/// rendering does not use this; it serves every cached row.
pub fn pr_cache_decision(
    cached: Option<&PrCacheRow>,
    reused_after_merge: bool,
    now: i64,
    ttl_secs: i64,
) -> CacheDecision {
    let Some(row) = cached else {
        return CacheDecision::Fetch;
    };
    let age = now - row.fetched_at;
    let fresh = (0..ttl_secs).contains(&age);
    let use_if = |ok: bool| {
        if ok {
            CacheDecision::Use
        } else {
            CacheDecision::Fetch
        }
    };
    match row.pr.as_ref().map(|p| p.state) {
        None => use_if(fresh),
        Some(PrStateKind::Declined) => use_if(fresh),
        Some(PrStateKind::Merged) => use_if(!reused_after_merge),
        Some(PrStateKind::Open) => CacheDecision::Fetch,
    }
}

/// For a cached MERGED row, the PR's head commit the caller compares with the
/// branch tip to compute `reused_after_merge`.
pub fn merged_head(row: &PrCacheRow) -> Option<&str> {
    row.pr
        .as_ref()
        .filter(|p| p.state == PrStateKind::Merged)
        .and_then(|p| p.from_latest_commit.as_deref())
}

pub fn load_pr_cache(conn: &Connection) -> Result<Vec<PrCacheRow>> {
    let mut stmt = conn.prepare(
        "SELECT repo_id, branch, pr_id, state, title, url, author, reviewers, open_tasks,
                comment_count, from_latest_commit, pr_updated_at, fetched_at, closed_at
         FROM pr_cache",
    )?;
    let rows = stmt.query_map([], |r| {
        let pr_id: Option<i64> = r.get(2)?;
        let state: Option<String> = r.get(3)?;
        let reviewers: Option<String> = r.get(7)?;
        let pr = match (pr_id, state.as_deref().and_then(PrStateKind::parse)) {
            (None, _) => Ok(None),
            (Some(id), Some(state)) => Ok(Some(PrData {
                id: id as u64,
                state,
                title: r.get::<_, Option<String>>(4)?.unwrap_or_default(),
                url: r.get::<_, Option<String>>(5)?.unwrap_or_default(),
                author_slug: r.get::<_, Option<String>>(6)?.unwrap_or_default(),
                reviewers: reviewers
                    .and_then(|j| serde_json::from_str(&j).ok())
                    .unwrap_or_default(),
                open_tasks: r.get::<_, Option<u32>>(8)?.unwrap_or(0),
                comment_count: r.get::<_, Option<u32>>(9)?.unwrap_or(0),
                from_latest_commit: r.get(10)?,
                updated_at: r.get(11)?,
                closed_at: r.get(13)?,
            })),
            // A PR row with a state this build does not know: unusable.
            (Some(_), None) => Err(()),
        };
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            pr,
            r.get::<_, i64>(12)?,
        ))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (repo_id, branch, pr, fetched_at) = row?;
        if let Ok(pr) = pr {
            out.push(PrCacheRow {
                repo_id,
                branch,
                pr,
                fetched_at,
            });
        }
    }
    Ok(out)
}

pub fn upsert_pr_cache(conn: &Connection, rows: &[PrCacheRow]) -> Result<()> {
    let mut stmt = conn.prepare(
        "INSERT INTO pr_cache (repo_id, branch, pr_id, state, title, url, author, reviewers,
                               open_tasks, comment_count, from_latest_commit, pr_updated_at,
                               fetched_at, closed_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
         ON CONFLICT(repo_id, branch) DO UPDATE SET
           pr_id = excluded.pr_id, state = excluded.state, title = excluded.title,
           url = excluded.url, author = excluded.author, reviewers = excluded.reviewers,
           open_tasks = excluded.open_tasks, comment_count = excluded.comment_count,
           from_latest_commit = excluded.from_latest_commit,
           pr_updated_at = excluded.pr_updated_at, fetched_at = excluded.fetched_at,
           closed_at = excluded.closed_at",
    )?;
    for row in rows {
        let pr = row.pr.as_ref();
        let reviewers = pr
            .map(|p| serde_json::to_string(&p.reviewers))
            .transpose()?;
        stmt.execute(params![
            row.repo_id,
            row.branch,
            pr.map(|p| p.id as i64),
            pr.map(|p| p.state.as_str()),
            pr.map(|p| p.title.as_str()),
            pr.map(|p| p.url.as_str()),
            pr.map(|p| p.author_slug.as_str()),
            reviewers,
            pr.map(|p| p.open_tasks),
            pr.map(|p| p.comment_count),
            pr.and_then(|p| p.from_latest_commit.as_deref()),
            pr.and_then(|p| p.updated_at),
            row.fetched_at,
            pr.and_then(|p| p.closed_at),
        ])?;
    }
    Ok(())
}

pub fn load_build_cache(conn: &Connection) -> Result<Vec<BuildCacheRow>> {
    let mut stmt = conn.prepare("SELECT sha, state, fetched_at FROM build_cache")?;
    let rows = stmt.query_map([], |r| {
        Ok(BuildCacheRow {
            sha: r.get(0)?,
            state: r
                .get::<_, Option<String>>(1)?
                .as_deref()
                .and_then(CiState::parse),
            fetched_at: r.get(2)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

pub fn upsert_build_cache(conn: &Connection, rows: &[BuildCacheRow]) -> Result<()> {
    let mut stmt = conn.prepare(
        "INSERT INTO build_cache (sha, state, fetched_at) VALUES (?1, ?2, ?3)
         ON CONFLICT(sha) DO UPDATE SET state = excluded.state, fetched_at = excluded.fetched_at",
    )?;
    for row in rows {
        stmt.execute(params![
            row.sha,
            row.state.map(CiState::as_str),
            row.fetched_at
        ])?;
    }
    Ok(())
}

/// Write a sync's rows in one transaction. Run through `store::DbWriter::write`.
pub fn write_sync_rows(
    conn: &Connection,
    prs: &[PrCacheRow],
    builds: &[BuildCacheRow],
) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    upsert_pr_cache(&tx, prs)?;
    upsert_build_cache(&tx, builds)?;
    tx.commit()?;
    Ok(())
}

/// Cached PR and CI data for the instant startup render (spec 5.2: served
/// whatever its age, OPEN included).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CachedPrData {
    pub prs: HashMap<(RepoId, String), PrCacheRow>,
    pub ci: HashMap<String, Option<CiState>>,
    /// Newest `fetched_at` over all PR rows: the "last synced" time.
    pub newest_fetched_at: Option<i64>,
}

pub fn load_cached_pr_data(conn: &Connection) -> Result<CachedPrData> {
    let mut out = CachedPrData::default();
    for row in load_pr_cache(conn)? {
        out.newest_fetched_at = out.newest_fetched_at.max(Some(row.fetched_at));
        out.prs
            .insert((row.repo_id.clone(), row.branch.clone()), row);
    }
    for row in load_build_cache(conn)? {
        out.ci.insert(row.sha, row.state);
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Sync
// ---------------------------------------------------------------------------

/// A branch of a repo to resolve PR (and CI) data for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncBranch {
    pub branch: String,
    pub tip: Option<String>,
    /// Build status is fetched for tips of unfinished branches only.
    pub unfinished: bool,
    /// Git check for a cached MERGED row: `merged_head..tip` is non-empty.
    pub reused_after_merge: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncRepo {
    pub repo_id: RepoId,
    pub identity: BitbucketIdentity,
    pub branches: Vec<SyncBranch>,
}

#[derive(Debug, Clone, Default)]
pub struct SyncInput {
    pub repos: Vec<SyncRepo>,
    /// `load_pr_cache` read just before the sync.
    pub cached: Vec<PrCacheRow>,
    pub pr_cache_ttl_mins: u64,
    /// `identity.bitbucket_user`; empty = resolve via `X-AUSERNAME`.
    pub bitbucket_user: String,
    /// Unix seconds.
    pub now: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrSource {
    /// Fetched in this sync.
    Fetched,
    /// Served from cache by the spec 5.2 rules.
    Cache,
    /// The lookup failed; this is the last cached value (show its age).
    StaleFallback,
}

/// The latest PR of one `(repo_id, branch)`; `pr: None` = no PR.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LatestPr {
    pub pr: Option<PrData>,
    pub fetched_at: i64,
    pub source: PrSource,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoSyncError {
    pub repo_id: RepoId,
    /// `PROJECT/repo`.
    pub repo_name: String,
    pub kind: SyncErrorKind,
    /// Sanitized, token-free message.
    pub message: String,
}

#[derive(Debug, Clone)]
pub struct SyncReport {
    /// Latest PR per `(repo_id, branch)`. Branches whose lookup failed without
    /// a cached fallback are absent (PR data unavailable). May also contain my
    /// open PRs (dashboard) for branches of known repos that were not in the
    /// input, for ownership rule 2.
    pub latest_prs: HashMap<(RepoId, String), LatestPr>,
    /// CI per tip sha; absent when the lookup failed.
    pub ci: HashMap<String, Option<CiState>>,
    /// Rows to write with [`write_sync_rows`]. Never contains errors.
    pub pr_rows: Vec<PrCacheRow>,
    pub build_rows: Vec<BuildCacheRow>,
    /// Configured `bitbucket_user`, else the `X-AUSERNAME` seen in this sync.
    pub username: Option<String>,
    /// At most one per repo.
    pub errors: Vec<RepoSyncError>,
    /// Failed build-status lookups (do not affect `status`).
    pub ci_errors: Vec<String>,
    /// Header status (spec 5.3).
    pub status: PrStatus,
    pub synced_at: i64,
}

impl SyncReport {
    fn empty(now: i64, status: PrStatus, username: Option<String>) -> Self {
        Self {
            latest_prs: HashMap::new(),
            ci: HashMap::new(),
            pr_rows: Vec::new(),
            build_rows: Vec::new(),
            username,
            errors: Vec::new(),
            ci_errors: Vec::new(),
            status,
            synced_at: now,
        }
    }

    fn record_error(&mut self, repo: &SyncRepo, err: &PrProviderError) {
        if self.errors.iter().any(|e| e.repo_id == repo.repo_id) {
            return;
        }
        self.errors.push(RepoSyncError {
            repo_id: repo.repo_id.clone(),
            repo_name: format!("{}/{}", repo.identity.project, repo.identity.repo),
            kind: classify_error(err),
            message: clean_line(&err.to_string()),
        });
    }

    fn set_fetched(&mut self, repo_id: &str, branch: &str, pr: Option<PrData>, now: i64) {
        self.pr_rows.push(PrCacheRow {
            repo_id: repo_id.to_string(),
            branch: branch.to_string(),
            pr: pr.clone(),
            fetched_at: now,
        });
        self.latest_prs.insert(
            (repo_id.to_string(), branch.to_string()),
            LatestPr {
                pr,
                fetched_at: now,
                source: PrSource::Fetched,
            },
        );
    }

    fn set_cached(&mut self, row: &PrCacheRow, source: PrSource) {
        self.latest_prs.insert(
            (row.repo_id.clone(), row.branch.clone()),
            LatestPr {
                pr: row.pr.clone(),
                fetched_at: row.fetched_at,
                source,
            },
        );
    }
}

/// Header status from the per-repo errors (spec 5.3). 401/403 win over
/// unreachable; unreachable shows the age of the newest cached data used.
pub fn summarize_status(
    errors: &[RepoSyncError],
    newest_cached: Option<i64>,
    now: i64,
) -> PrStatus {
    let pick = |kinds: &[SyncErrorKind]| -> Option<PrStatus> {
        let hit: Vec<&RepoSyncError> = errors.iter().filter(|e| kinds.contains(&e.kind)).collect();
        let first = hit.first()?;
        Some(PrStatus::Error {
            message: first.message.clone(),
            repos: hit.iter().map(|e| e.repo_name.clone()).collect(),
        })
    };
    if let Some(s) = pick(&[SyncErrorKind::Unauthorized, SyncErrorKind::Forbidden]) {
        return s;
    }
    if errors.iter().any(|e| e.kind == SyncErrorKind::Unreachable) {
        return PrStatus::Cached {
            age_secs: newest_cached.map(|t| (now - t).max(0)).unwrap_or(0),
        };
    }
    pick(&[SyncErrorKind::Other]).unwrap_or(PrStatus::Ok)
}

/// One Bitbucket host: its HTTP client and gate.
struct Host {
    client: BitbucketHostClient,
    gate: HostGate,
}

/// Long-lived Bitbucket access for the app: token, per-host clients and gates.
pub struct BitbucketService {
    token: Option<Token>,
    hosts: Mutex<HashMap<String, Arc<Host>>>,
    timeouts: Option<(Duration, Duration)>,
}

enum Job {
    Pr {
        repo: usize,
        branch: String,
        /// Already mapped; boxed to keep the variants small.
        result: Result<Option<Box<PrData>>, PrProviderError>,
    },
    Ci {
        repo: usize,
        sha: String,
        result: Result<(Vec<String>, Option<String>), PrProviderError>,
    },
}

fn host_key(base_url: &str) -> String {
    base_url.trim().trim_end_matches('/').to_string()
}

impl BitbucketService {
    /// Token from [`TOKEN_ENV`].
    pub fn from_env() -> Self {
        Self::new(Token::from_env())
    }

    pub fn new(token: Option<Token>) -> Self {
        Self {
            token,
            hosts: Mutex::new(HashMap::new()),
            timeouts: None,
        }
    }

    /// Test hook: short HTTP timeouts.
    #[cfg(test)]
    fn with_timeouts(token: Option<Token>, connect: Duration, total: Duration) -> Self {
        Self {
            timeouts: Some((connect, total)),
            ..Self::new(token)
        }
    }

    pub fn has_token(&self) -> bool {
        self.token.is_some()
    }

    fn host(&self, token: &Token, base_url: &str) -> Result<Arc<Host>, PrProviderError> {
        let key = host_key(base_url);
        let mut hosts = self.hosts.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(h) = hosts.get(&key) {
            return Ok(Arc::clone(h));
        }
        let client = match self.timeouts {
            Some((connect, total)) => {
                BitbucketHostClient::with_timeouts(&key, token.expose(), connect, total)?
            }
            None => BitbucketHostClient::new(&key, token.expose())?,
        };
        let host = Arc::new(Host {
            client,
            gate: HostGate::new(MAX_IN_FLIGHT_PER_HOST),
        });
        hosts.insert(key, Arc::clone(&host));
        Ok(host)
    }

    /// One full sync (spec 5.1, 5.2). Never fails as a whole: failures are in
    /// `report.errors` / `report.status`. Performs no DB IO.
    pub async fn sync(&self, input: SyncInput) -> SyncReport {
        let now = input.now;
        let configured_user =
            Some(input.bitbucket_user.trim().to_string()).filter(|u| !u.is_empty());
        let Some(token) = self.token.as_ref() else {
            return SyncReport::empty(now, PrStatus::NoToken, configured_user);
        };
        let ttl_secs =
            i64::try_from(input.pr_cache_ttl_mins.saturating_mul(60)).unwrap_or(i64::MAX);
        let cached: HashMap<(String, String), PrCacheRow> = input
            .cached
            .into_iter()
            .map(|r| ((r.repo_id.clone(), r.branch.clone()), r))
            .collect();
        let repos = input.repos;
        let mut report = SyncReport::empty(now, PrStatus::Ok, configured_user.clone());
        let mut seen_user: Option<String> = None;
        let mut fallback_used: Option<i64> = None;

        // Group repos by host, keeping input order.
        let mut groups: Vec<(String, Vec<usize>)> = Vec::new();
        for (i, repo) in repos.iter().enumerate() {
            let key = host_key(&repo.identity.base_url);
            match groups.iter_mut().find(|(k, _)| *k == key) {
                Some((_, idx)) => idx.push(i),
                None => groups.push((key, vec![i])),
            }
        }

        let mut jobs: tokio::task::JoinSet<Job> = tokio::task::JoinSet::new();
        let mut aborted: HashSet<usize> = HashSet::new();
        let mut live_hosts: Vec<Arc<Host>> = Vec::new();

        for (key, members) in &groups {
            let host = match self.host(token, key) {
                Ok(h) => h,
                Err(err) => {
                    for &i in members {
                        report.record_error(&repos[i], &err);
                        aborted.insert(i);
                    }
                    continue;
                }
            };

            // 1. Dashboard: all my open PRs on this host.
            let dashboard = all_pages(&host.gate, |start| host.client.dashboard_page(start)).await;
            let mut matched: HashSet<(usize, String)> = HashSet::new();
            match dashboard {
                Ok((prs, user)) => {
                    seen_user = seen_user.or(user);
                    let mut best: HashMap<(usize, String), PullRequestDetails> = HashMap::new();
                    for pr in prs {
                        if pr.is_from_fork() {
                            continue;
                        }
                        let Some(&i) = members.iter().find(|&&i| {
                            pr.to_repo
                                .same_repo(&repos[i].identity.project, &repos[i].identity.repo)
                        }) else {
                            continue;
                        };
                        let k = (i, pr.from_branch.clone());
                        let newer = best
                            .get(&k)
                            .is_none_or(|old| pr.updated_date_ms > old.updated_date_ms);
                        if newer {
                            best.insert(k, pr);
                        }
                    }
                    for ((i, branch), pr) in best {
                        report.set_fetched(
                            &repos[i].repo_id,
                            &branch,
                            Some(pr_data_from_details(&pr)),
                            now,
                        );
                        matched.insert((i, branch));
                    }
                }
                Err(err) => {
                    let kind = classify_error(&err);
                    for &i in members {
                        report.record_error(&repos[i], &err);
                    }
                    if kind != SyncErrorKind::Other {
                        // 401 / 403 / unreachable: every other request to this
                        // host would fail the same way.
                        aborted.extend(members.iter().copied());
                        continue;
                    }
                }
            }
            live_hosts.push(Arc::clone(&host));

            // 2. Per-branch lookups for branches the dashboard did not match.
            // 3. Build status for tips of unfinished branches.
            let mut shas: HashSet<String> = HashSet::new();
            for &i in members {
                let repo = &repos[i];
                let repository = match repository_for(&repo.identity) {
                    Ok(r) => Arc::new(r),
                    Err(err) => {
                        report.record_error(repo, &err);
                        aborted.insert(i);
                        continue;
                    }
                };
                for b in &repo.branches {
                    if b.unfinished {
                        if let Some(tip) = &b.tip {
                            if shas.insert(tip.clone()) {
                                let host = Arc::clone(&host);
                                let sha = tip.clone();
                                jobs.spawn(async move {
                                    let result = all_pages(&host.gate, |start| {
                                        host.client.build_status_page(&sha, start)
                                    })
                                    .await;
                                    Job::Ci {
                                        repo: i,
                                        sha,
                                        result,
                                    }
                                });
                            }
                        }
                    }
                    if matched.contains(&(i, b.branch.clone())) {
                        continue;
                    }
                    let row = cached.get(&(repo.repo_id.clone(), b.branch.clone()));
                    if pr_cache_decision(row, b.reused_after_merge, now, ttl_secs)
                        == CacheDecision::Use
                    {
                        if let Some(row) = row {
                            report.set_cached(row, PrSource::Cache);
                        }
                        continue;
                    }
                    let host = Arc::clone(&host);
                    let repository = Arc::clone(&repository);
                    let branch = b.branch.clone();
                    jobs.spawn(async move {
                        let result = host
                            .gate
                            .run(|| {
                                host.client
                                    .latest_pull_request_for_branch(&repository, &branch)
                            })
                            .await
                            .map(|pr| pr.as_ref().map(|p| Box::new(pr_data_from_details(p))));
                        Job::Pr {
                            repo: i,
                            branch,
                            result,
                        }
                    });
                }
            }
        }

        while let Some(joined) = jobs.join_next().await {
            let Ok(job) = joined else {
                tracing::error!("Bitbucket request task panicked");
                continue;
            };
            match job {
                Job::Pr {
                    repo,
                    branch,
                    result,
                } => match result {
                    Ok(pr) => {
                        report.set_fetched(&repos[repo].repo_id, &branch, pr.map(|p| *p), now);
                    }
                    Err(err) => {
                        report.record_error(&repos[repo], &err);
                        if let Some(row) = cached.get(&(repos[repo].repo_id.clone(), branch)) {
                            report.set_cached(row, PrSource::StaleFallback);
                            fallback_used = fallback_used.max(Some(row.fetched_at));
                        }
                    }
                },
                Job::Ci { repo, sha, result } => match result {
                    Ok((states, user)) => {
                        seen_user = seen_user.or(user);
                        let state = aggregate_build_states(&states);
                        report.ci.insert(sha.clone(), state);
                        report.build_rows.push(BuildCacheRow {
                            sha,
                            state,
                            fetched_at: now,
                        });
                    }
                    Err(err) => {
                        let r = &repos[repo];
                        report.ci_errors.push(format!(
                            "{}/{} {}: {}",
                            r.identity.project,
                            r.identity.repo,
                            sha.chars().take(12).collect::<String>(),
                            clean_line(&err.to_string())
                        ));
                    }
                },
            }
        }

        // The dashboard normally carries X-AUSERNAME; ask once if it did not.
        if configured_user.is_none() && seen_user.is_none() {
            if let Some(host) = live_hosts.first() {
                match host.gate.run(|| host.client.current_user()).await {
                    Ok(user) => seen_user = user,
                    Err(err) => {
                        tracing::warn!(error = %err, "could not resolve the Bitbucket user")
                    }
                }
            }
        }

        // Aborted repos: keep showing their last cached PR data.
        for &i in &aborted {
            let repo = &repos[i];
            for b in &repo.branches {
                if let Some(row) = cached.get(&(repo.repo_id.clone(), b.branch.clone())) {
                    report.set_cached(row, PrSource::StaleFallback);
                    fallback_used = fallback_used.max(Some(row.fetched_at));
                }
            }
        }

        report.username = configured_user.or(seen_user.map(|u| clean_line(&u)));
        report.status = summarize_status(&report.errors, fallback_used, now);
        report
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lgbc::bitbucket::{RepoRef, ReviewerDetails};
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use wiremock::matchers::{method, path, path_regex, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const FAKE_TOKEN: &str = "fake-test-token-not-real";
    const NOW: i64 = 1_800_000_000;
    const TTL: i64 = 3600;

    fn pr(state: PrStateKind) -> PrData {
        PrData {
            id: 1,
            state,
            title: "t".into(),
            url: "u".into(),
            author_slug: "me".into(),
            reviewers: vec![Reviewer {
                name: "Rev".into(),
                slug: "rev".into(),
                status: ReviewerStatus::Approved,
            }],
            open_tasks: 1,
            comment_count: 2,
            from_latest_commit: Some("head1".into()),
            updated_at: Some(100),
            closed_at: Some(200),
        }
    }

    fn row(branch: &str, pr: Option<PrData>, fetched_at: i64) -> PrCacheRow {
        PrCacheRow {
            repo_id: "r1".into(),
            branch: branch.into(),
            pr,
            fetched_at,
        }
    }

    // --- token ---

    #[test]
    fn token_rejects_blank_and_redacts_debug() {
        assert!(Token::new("  ").is_none());
        let t = Token::new(" secret-value ").unwrap();
        assert_eq!(t.expose(), "secret-value");
        assert!(!format!("{t:?}").contains("secret"));
        assert!(!BitbucketService::new(None).has_token());
    }

    // --- pure mapping ---

    #[test]
    fn build_state_aggregation() {
        let none: [&str; 0] = [];
        assert_eq!(aggregate_build_states(&none), None);
        assert_eq!(
            aggregate_build_states(&["SUCCESSFUL", "FAILED", "INPROGRESS"]),
            Some(CiState::Failed)
        );
        assert_eq!(
            aggregate_build_states(&["SUCCESSFUL", "INPROGRESS"]),
            Some(CiState::InProgress)
        );
        assert_eq!(
            aggregate_build_states(&["SUCCESSFUL", "successful"]),
            Some(CiState::Successful)
        );
        assert_eq!(aggregate_build_states(&["SUCCESSFUL", "CANCELLED"]), None);
    }

    fn details(state: PrState, title: &str) -> PullRequestDetails {
        PullRequestDetails {
            id: 9,
            title: title.into(),
            state,
            url: "https://bb/pr/9".into(),
            author_slug: "me".into(),
            reviewers: vec![
                ReviewerDetails {
                    name: "a".into(),
                    slug: "a".into(),
                    display_name: "A \u{1b}[31mRed".into(),
                    status: "NEEDS_WORK".into(),
                },
                ReviewerDetails {
                    name: "b".into(),
                    slug: "b".into(),
                    display_name: "B".into(),
                    status: "SOMETHING_NEW".into(),
                },
            ],
            open_task_count: 3,
            comment_count: 4,
            from_ref_id: "refs/heads/x".into(),
            from_branch: "x".into(),
            from_latest_commit: Some("abc".into()),
            from_repo: RepoRef {
                project_key: "P".into(),
                slug: "r".into(),
            },
            to_repo: RepoRef {
                project_key: "P".into(),
                slug: "r".into(),
            },
            updated_date_ms: Some(1_700_000_000_999),
            closed_date_ms: None,
        }
    }

    #[test]
    fn details_map_to_pr_data_sanitized() {
        let d = pr_data_from_details(&details(PrState::Closed, "Fix\u{1b}]0;evil\u{7} it\nnow"));
        assert_eq!(d.state, PrStateKind::Declined);
        assert_eq!(d.title, "Fix it now");
        assert_eq!(d.reviewers[0].name, "A Red");
        assert_eq!(d.reviewers[0].status, ReviewerStatus::NeedsWork);
        assert_eq!(d.reviewers[1].status, ReviewerStatus::Unapproved);
        assert_eq!(d.open_tasks, 3);
        assert_eq!(d.comment_count, 4);
        assert_eq!(d.updated_at, Some(1_700_000_000));
        assert_eq!(
            pr_data_from_details(&details(PrState::Merged, "t")).state,
            PrStateKind::Merged
        );
    }

    #[test]
    fn errors_are_classified_for_the_header() {
        use PrProviderError as E;
        assert_eq!(
            classify_error(&E::Authentication("x".into())),
            SyncErrorKind::Unauthorized
        );
        assert_eq!(
            classify_error(&E::Forbidden("x".into())),
            SyncErrorKind::Forbidden
        );
        assert_eq!(
            classify_error(&E::Transport("x".into())),
            SyncErrorKind::Unreachable
        );
        assert_eq!(
            classify_error(&E::RateLimited { retry_after: None }),
            SyncErrorKind::Unreachable
        );
        assert_eq!(
            classify_error(&E::InvalidResponse("x".into())),
            SyncErrorKind::Other
        );
    }

    fn err(repo: &str, kind: SyncErrorKind) -> RepoSyncError {
        RepoSyncError {
            repo_id: repo.into(),
            repo_name: format!("P/{repo}"),
            kind,
            message: format!("{kind:?}"),
        }
    }

    #[test]
    fn status_prefers_auth_errors_then_unreachable() {
        assert_eq!(summarize_status(&[], None, NOW), PrStatus::Ok);
        let errs = [
            err("a", SyncErrorKind::Unreachable),
            err("b", SyncErrorKind::Forbidden),
            err("c", SyncErrorKind::Unauthorized),
        ];
        match summarize_status(&errs, None, NOW) {
            PrStatus::Error { repos, .. } => assert_eq!(repos, vec!["P/b", "P/c"]),
            other => panic!("{other:?}"),
        }
        assert_eq!(
            summarize_status(
                &[err("a", SyncErrorKind::Unreachable)],
                Some(NOW - 7200),
                NOW
            ),
            PrStatus::Cached { age_secs: 7200 }
        );
        assert!(matches!(
            summarize_status(&[err("a", SyncErrorKind::Other)], None, NOW),
            PrStatus::Error { .. }
        ));
    }

    // --- cache decisions (spec 5.2) ---

    #[test]
    fn cache_decision_rules() {
        use CacheDecision::{Fetch, Use};
        assert_eq!(pr_cache_decision(None, false, NOW, TTL), Fetch);

        // none found: TTL
        let none_fresh = row("b", None, NOW - 10);
        let none_old = row("b", None, NOW - TTL);
        assert_eq!(pr_cache_decision(Some(&none_fresh), false, NOW, TTL), Use);
        assert_eq!(pr_cache_decision(Some(&none_old), false, NOW, TTL), Fetch);

        // DECLINED: TTL
        let declined = row("b", Some(pr(PrStateKind::Declined)), NOW - 10);
        let declined_old = row("b", Some(pr(PrStateKind::Declined)), NOW - TTL - 1);
        assert_eq!(pr_cache_decision(Some(&declined), false, NOW, TTL), Use);
        assert_eq!(
            pr_cache_decision(Some(&declined_old), false, NOW, TTL),
            Fetch
        );

        // MERGED: no TTL, until the branch is reused
        let merged = row("b", Some(pr(PrStateKind::Merged)), NOW - 100 * TTL);
        assert_eq!(pr_cache_decision(Some(&merged), false, NOW, TTL), Use);
        assert_eq!(pr_cache_decision(Some(&merged), true, NOW, TTL), Fetch);
        assert_eq!(merged_head(&merged), Some("head1"));
        assert_eq!(merged_head(&declined), None);

        // OPEN: never during a full sync
        let open = row("b", Some(pr(PrStateKind::Open)), NOW);
        assert_eq!(pr_cache_decision(Some(&open), false, NOW, TTL), Fetch);

        // A row from the future (clock skew) is not trusted.
        let future = row("b", None, NOW + 60);
        assert_eq!(pr_cache_decision(Some(&future), false, NOW, TTL), Fetch);
    }

    // --- cache SQL ---

    #[test]
    fn cache_rows_roundtrip_and_startup_load() {
        let conn = crate::store::open_in_memory().unwrap();
        let rows = vec![
            row("with-pr", Some(pr(PrStateKind::Open)), 10),
            row("no-pr", None, 20),
        ];
        let builds = vec![
            BuildCacheRow {
                sha: "s1".into(),
                state: Some(CiState::Failed),
                fetched_at: 10,
            },
            BuildCacheRow {
                sha: "s2".into(),
                state: None,
                fetched_at: 10,
            },
        ];
        write_sync_rows(&conn, &rows, &builds).unwrap();
        // Upsert replaces.
        let mut updated = rows[0].clone();
        updated.pr.as_mut().unwrap().state = PrStateKind::Merged;
        updated.fetched_at = 30;
        upsert_pr_cache(&conn, std::slice::from_ref(&updated)).unwrap();

        let mut loaded = load_pr_cache(&conn).unwrap();
        loaded.sort_by(|a, b| a.branch.cmp(&b.branch));
        assert_eq!(loaded, vec![rows[1].clone(), updated.clone()]);

        let data = load_cached_pr_data(&conn).unwrap();
        assert_eq!(data.newest_fetched_at, Some(30));
        assert_eq!(
            data.prs[&("r1".to_string(), "with-pr".to_string())],
            updated
        );
        assert_eq!(data.ci["s1"], Some(CiState::Failed));
        assert_eq!(data.ci["s2"], None);
    }

    #[test]
    fn cache_row_with_unknown_state_is_skipped() {
        let conn = crate::store::open_in_memory().unwrap();
        conn.execute(
            "INSERT INTO pr_cache (repo_id, branch, pr_id, state, fetched_at) VALUES ('r', 'b', 1, 'WEIRD', 1)",
            [],
        )
        .unwrap();
        assert!(load_pr_cache(&conn).unwrap().is_empty());
    }

    // --- gate ---

    #[tokio::test]
    async fn gate_limits_in_flight_requests() {
        let gate = Arc::new(HostGate::new(MAX_IN_FLIGHT_PER_HOST));
        let current = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let mut set = tokio::task::JoinSet::new();
        for _ in 0..20 {
            let (gate, current, peak) = (gate.clone(), current.clone(), peak.clone());
            set.spawn(async move {
                gate.run(|| async {
                    let now = current.fetch_add(1, Ordering::SeqCst) + 1;
                    peak.fetch_max(now, Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_millis(10)).await;
                    current.fetch_sub(1, Ordering::SeqCst);
                    Ok::<_, PrProviderError>(())
                })
                .await
            });
        }
        while let Some(r) = set.join_next().await {
            r.unwrap().unwrap();
        }
        assert_eq!(peak.load(Ordering::SeqCst), MAX_IN_FLIGHT_PER_HOST);
    }

    #[tokio::test]
    async fn gate_pause_holds_new_requests() {
        let gate = HostGate::new(2);
        gate.pause_for(Duration::from_millis(300));
        gate.pause_for(Duration::from_millis(10)); // never shortens
        let start = Instant::now();
        gate.run(|| async { Ok::<_, PrProviderError>(()) })
            .await
            .unwrap();
        assert!(start.elapsed() >= Duration::from_millis(300));
    }

    // --- sync against a mock server ---

    const DEMO_PRS: &str = "/rest/api/1.0/projects/PROJ/repos/demo/pull-requests";

    fn service() -> BitbucketService {
        BitbucketService::new(Token::new(FAKE_TOKEN))
    }

    fn branch(name: &str, tip: &str) -> SyncBranch {
        SyncBranch {
            branch: name.into(),
            tip: Some(tip.into()),
            unfinished: true,
            reused_after_merge: false,
        }
    }

    fn input(server: &MockServer, branches: Vec<SyncBranch>) -> SyncInput {
        SyncInput {
            repos: vec![SyncRepo {
                repo_id: "r1".into(),
                identity: BitbucketIdentity {
                    base_url: server.uri(),
                    project: "PROJ".into(),
                    repo: "demo".into(),
                },
                branches,
            }],
            cached: vec![],
            pr_cache_ttl_mins: 60,
            bitbucket_user: String::new(),
            now: NOW,
        }
    }

    fn pr_json(
        id: u64,
        state: &str,
        branch: &str,
        from: (&str, &str),
        to: (&str, &str),
    ) -> serde_json::Value {
        json!({
            "id": id,
            "title": format!("PR {id}"),
            "state": state,
            "fromRef": {
                "id": format!("refs/heads/{branch}"),
                "displayId": branch,
                "latestCommit": format!("tip-{branch}"),
                "repository": { "slug": from.1, "project": { "key": from.0 } }
            },
            "toRef": {
                "id": "refs/heads/main",
                "displayId": "main",
                "repository": { "slug": to.1, "project": { "key": to.0 } }
            },
            "author": { "user": { "name": "me", "slug": "me", "displayName": "Me" } },
            "reviewers": [
                { "user": { "name": "rev", "slug": "rev", "displayName": "Rev" }, "status": "APPROVED" }
            ],
            "properties": { "openTaskCount": 1, "commentCount": 3 },
            "updatedDate": 1_700_000_000_000_i64,
            "closedDate": 1_700_000_100_000_i64,
            "links": { "self": [{ "href": format!("https://bb/pr/{id}") }] }
        })
    }

    fn own(id: u64, state: &str, branch: &str) -> serde_json::Value {
        pr_json(id, state, branch, ("PROJ", "demo"), ("PROJ", "demo"))
    }

    fn page(values: Vec<serde_json::Value>, next: Option<u64>) -> serde_json::Value {
        match next {
            Some(n) => json!({ "values": values, "isLastPage": false, "nextPageStart": n }),
            None => json!({ "values": values, "isLastPage": true }),
        }
    }

    async fn mount_dashboard(server: &MockServer, values: Vec<serde_json::Value>) {
        Mock::given(method("GET"))
            .and(path("/rest/api/1.0/dashboard/pull-requests"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("X-AUSERNAME", "jdoe")
                    .set_body_json(page(values, None)),
            )
            .mount(server)
            .await;
    }

    async fn mount_no_builds(server: &MockServer) {
        Mock::given(method("GET"))
            .and(path_regex("^/rest/build-status/1.0/commits/.*"))
            .respond_with(ResponseTemplate::new(200).set_body_json(page(vec![], None)))
            .mount(server)
            .await;
    }

    fn latest<'a>(report: &'a SyncReport, branch: &str) -> &'a LatestPr {
        &report.latest_prs[&("r1".to_string(), branch.to_string())]
    }

    #[tokio::test]
    async fn no_token_makes_no_requests() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200))
            .expect(0)
            .mount(&server)
            .await;
        let mut inp = input(&server, vec![branch("a", "s")]);
        inp.bitbucket_user = "cfg".into();
        let report = BitbucketService::new(None).sync(inp).await;
        assert_eq!(report.status, PrStatus::NoToken);
        assert_eq!(report.username.as_deref(), Some("cfg"));
        assert!(report.latest_prs.is_empty() && report.pr_rows.is_empty());
    }

    #[tokio::test]
    async fn dashboard_is_paged_and_matches_branches() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/rest/api/1.0/dashboard/pull-requests"))
            .and(query_param("start", "0"))
            .and(query_param("role", "AUTHOR"))
            .and(query_param("state", "OPEN"))
            .and(query_param("limit", "100"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("X-AUSERNAME", "jdoe")
                    .set_body_json(page(vec![own(1, "OPEN", "a")], Some(100))),
            )
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/rest/api/1.0/dashboard/pull-requests"))
            .and(query_param("start", "100"))
            .respond_with(ResponseTemplate::new(200).set_body_json(page(
                vec![
                    own(2, "OPEN", "b"),
                    // Another repo on the same host: not ours to match.
                    pr_json(3, "OPEN", "c", ("OTHER", "x"), ("OTHER", "x")),
                    // A non-candidate branch of a known repo (ownership rule 2).
                    own(4, "OPEN", "remote-only"),
                ],
                None,
            )))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(DEMO_PRS))
            .respond_with(ResponseTemplate::new(500))
            .expect(0)
            .mount(&server)
            .await;
        mount_no_builds(&server).await;

        let report = service()
            .sync(input(&server, vec![branch("a", "sa"), branch("b", "sb")]))
            .await;
        assert_eq!(report.status, PrStatus::Ok, "{:?}", report.errors);
        assert_eq!(report.username.as_deref(), Some("jdoe"));
        let a = latest(&report, "a");
        assert_eq!(a.source, PrSource::Fetched);
        let a = a.pr.as_ref().unwrap();
        assert_eq!((a.id, a.state), (1, PrStateKind::Open));
        assert_eq!(a.reviewers[0].status, ReviewerStatus::Approved);
        assert_eq!((a.open_tasks, a.comment_count), (1, 3));
        assert_eq!(a.from_latest_commit.as_deref(), Some("tip-a"));
        assert_eq!(a.author_slug, "me");
        assert_eq!(a.updated_at, Some(1_700_000_000));
        assert_eq!(a.closed_at, Some(1_700_000_100));
        assert_eq!(latest(&report, "b").pr.as_ref().unwrap().id, 2);
        assert_eq!(latest(&report, "remote-only").pr.as_ref().unwrap().id, 4);
        assert_eq!(report.latest_prs.len(), 3);
        assert_eq!(report.pr_rows.len(), 3);
        assert!(report.pr_rows.iter().all(|r| r.fetched_at == NOW));
    }

    #[tokio::test]
    async fn fork_prs_are_ignored() {
        let server = MockServer::start().await;
        // From a personal fork into PROJ/demo, same branch name as ours.
        mount_dashboard(
            &server,
            vec![pr_json(5, "OPEN", "a", ("~JDOE", "demo"), ("PROJ", "demo"))],
        )
        .await;
        Mock::given(method("GET"))
            .and(path(DEMO_PRS))
            .and(query_param("at", "refs/heads/a"))
            .respond_with(ResponseTemplate::new(200).set_body_json(page(vec![], None)))
            .expect(1)
            .mount(&server)
            .await;
        mount_no_builds(&server).await;

        let report = service()
            .sync(input(&server, vec![branch("a", "sa")]))
            .await;
        let a = latest(&report, "a");
        assert_eq!(a.pr, None);
        assert_eq!(a.source, PrSource::Fetched);
    }

    #[tokio::test]
    async fn per_branch_lookup_respects_the_cache() {
        let server = MockServer::start().await;
        mount_dashboard(&server, vec![]).await;
        Mock::given(method("GET"))
            .and(path(DEMO_PRS))
            .and(query_param("at", "refs/heads/merged"))
            .and(query_param("direction", "OUTGOING"))
            .and(query_param("state", "ALL"))
            .and(query_param("order", "NEWEST"))
            .and(query_param("limit", "1"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(page(vec![own(7, "MERGED", "merged")], None)),
            )
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(DEMO_PRS))
            .and(query_param("at", "refs/heads/was-open"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(page(vec![own(8, "DECLINED", "was-open")], None)),
            )
            .expect(1)
            .mount(&server)
            .await;
        // Fresh DECLINED and old MERGED rows: no request.
        for b in ["declined", "old-merged"] {
            Mock::given(method("GET"))
                .and(path(DEMO_PRS))
                .and(query_param("at", format!("refs/heads/{b}")))
                .respond_with(ResponseTemplate::new(500))
                .expect(0)
                .mount(&server)
                .await;
        }
        mount_no_builds(&server).await;

        let mut inp = input(
            &server,
            vec![
                branch("merged", "s1"),
                branch("was-open", "s2"),
                branch("declined", "s3"),
                branch("old-merged", "s4"),
            ],
        );
        inp.cached = vec![
            row("was-open", Some(pr(PrStateKind::Open)), NOW - 5),
            row("declined", Some(pr(PrStateKind::Declined)), NOW - 5),
            row("old-merged", Some(pr(PrStateKind::Merged)), NOW - 100 * TTL),
        ];
        let report = service().sync(inp).await;
        assert_eq!(report.status, PrStatus::Ok, "{:?}", report.errors);

        let merged = latest(&report, "merged");
        assert_eq!(merged.pr.as_ref().unwrap().state, PrStateKind::Merged);
        assert_eq!(merged.source, PrSource::Fetched);
        assert_eq!(
            latest(&report, "was-open").pr.as_ref().unwrap().state,
            PrStateKind::Declined
        );
        assert_eq!(latest(&report, "declined").source, PrSource::Cache);
        assert_eq!(latest(&report, "old-merged").source, PrSource::Cache);
        let mut written: Vec<&str> = report.pr_rows.iter().map(|r| r.branch.as_str()).collect();
        written.sort();
        assert_eq!(written, vec!["merged", "was-open"]);
    }

    #[tokio::test]
    async fn build_status_is_aggregated_per_tip() {
        let server = MockServer::start().await;
        mount_dashboard(&server, vec![]).await;
        Mock::given(method("GET"))
            .and(path(DEMO_PRS))
            .respond_with(ResponseTemplate::new(200).set_body_json(page(vec![], None)))
            .mount(&server)
            .await;
        let states = |s: &[&str]| -> Vec<serde_json::Value> {
            s.iter()
                .map(|st| json!({ "state": st, "key": "k" }))
                .collect()
        };
        let cases: [(&str, serde_json::Value); 4] = [
            ("failed", page(states(&["SUCCESSFUL", "FAILED"]), None)),
            ("running", page(states(&["INPROGRESS", "SUCCESSFUL"]), None)),
            ("green", page(states(&["SUCCESSFUL"]), None)),
            ("nobuilds", page(vec![], None)),
        ];
        for (sha, body) in cases {
            Mock::given(method("GET"))
                .and(path(format!("/rest/build-status/1.0/commits/{sha}")))
                .respond_with(ResponseTemplate::new(200).set_body_json(body))
                .expect(1)
                .mount(&server)
                .await;
        }
        // A paged build-status listing: FAILED on the second page.
        Mock::given(method("GET"))
            .and(path("/rest/build-status/1.0/commits/paged"))
            .and(query_param("start", "0"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(page(states(&["SUCCESSFUL"]), Some(1))),
            )
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/rest/build-status/1.0/commits/paged"))
            .and(query_param("start", "1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(page(states(&["FAILED"]), None)))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/rest/build-status/1.0/commits/finished"))
            .respond_with(ResponseTemplate::new(500))
            .expect(0)
            .mount(&server)
            .await;

        let mut done = branch("done", "finished");
        done.unfinished = false;
        let report = service()
            .sync(input(
                &server,
                vec![
                    branch("a", "failed"),
                    branch("b", "running"),
                    branch("c", "green"),
                    branch("d", "nobuilds"),
                    branch("e", "paged"),
                    branch("same-tip", "green"),
                    done,
                ],
            ))
            .await;
        assert_eq!(report.ci["failed"], Some(CiState::Failed));
        assert_eq!(report.ci["running"], Some(CiState::InProgress));
        assert_eq!(report.ci["green"], Some(CiState::Successful));
        assert_eq!(report.ci["nobuilds"], None);
        assert_eq!(report.ci["paged"], Some(CiState::Failed));
        assert!(!report.ci.contains_key("finished"));
        assert_eq!(report.build_rows.len(), 5);
        assert!(report.ci_errors.is_empty());
    }

    #[tokio::test]
    async fn current_user_comes_from_the_header_unless_configured() {
        let server = MockServer::start().await;
        mount_dashboard(&server, vec![]).await;
        let report = service().sync(input(&server, vec![])).await;
        assert_eq!(report.username.as_deref(), Some("jdoe"));

        let mut inp = input(&server, vec![]);
        inp.bitbucket_user = "configured".into();
        let report = service().sync(inp).await;
        assert_eq!(report.username.as_deref(), Some("configured"));
    }

    #[tokio::test]
    async fn current_user_is_asked_when_the_dashboard_has_no_header() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/rest/api/1.0/dashboard/pull-requests"))
            .respond_with(ResponseTemplate::new(200).set_body_json(page(vec![], None)))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/rest/api/1.0/application-properties"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("X-AUSERNAME", "jdoe")
                    .set_body_json(json!({})),
            )
            .expect(1)
            .mount(&server)
            .await;
        let report = service().sync(input(&server, vec![])).await;
        assert_eq!(report.username.as_deref(), Some("jdoe"));
    }

    #[tokio::test]
    async fn http_401_aborts_the_host_and_keeps_cached_data() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/rest/api/1.0/dashboard/pull-requests"))
            .respond_with(ResponseTemplate::new(401))
            .expect(1)
            .mount(&server)
            .await;
        // Nothing else is requested from this host.
        Mock::given(method("GET"))
            .and(path_regex("^/rest/(api/1.0/projects|build-status)/.*"))
            .respond_with(ResponseTemplate::new(200))
            .expect(0)
            .mount(&server)
            .await;
        let mut inp = input(&server, vec![branch("a", "sa"), branch("b", "sb")]);
        inp.cached = vec![row("a", Some(pr(PrStateKind::Open)), NOW - 60)];
        let report = service().sync(inp).await;
        match &report.status {
            PrStatus::Error { message, repos } => {
                assert!(message.contains("401"), "{message}");
                assert!(!message.contains(FAKE_TOKEN));
                assert_eq!(repos, &vec!["PROJ/demo".to_string()]);
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(report.errors[0].kind, SyncErrorKind::Unauthorized);
        assert_eq!(latest(&report, "a").source, PrSource::StaleFallback);
        assert!(!report
            .latest_prs
            .contains_key(&("r1".to_string(), "b".to_string())));
        assert!(report.pr_rows.is_empty(), "errors are never cached");
    }

    #[tokio::test]
    async fn http_403_on_a_repo_is_reported_with_the_repo() {
        let server = MockServer::start().await;
        mount_dashboard(&server, vec![]).await;
        Mock::given(method("GET"))
            .and(path(DEMO_PRS))
            .respond_with(ResponseTemplate::new(403))
            .mount(&server)
            .await;
        mount_no_builds(&server).await;
        let report = service()
            .sync(input(&server, vec![branch("a", "sa")]))
            .await;
        match &report.status {
            PrStatus::Error { message, repos } => {
                assert!(message.contains("403"), "{message}");
                assert_eq!(repos, &vec!["PROJ/demo".to_string()]);
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(report.errors[0].kind, SyncErrorKind::Forbidden);
        assert!(report.pr_rows.is_empty());
    }

    #[tokio::test]
    async fn http_429_pauses_the_host_and_retries_after_retry_after() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/rest/api/1.0/dashboard/pull-requests"))
            .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "1"))
            .up_to_n_times(1)
            .expect(1)
            .mount(&server)
            .await;
        mount_dashboard(&server, vec![own(1, "OPEN", "a")]).await;
        mount_no_builds(&server).await;

        let start = std::time::Instant::now();
        let report = service()
            .sync(input(&server, vec![branch("a", "sa")]))
            .await;
        assert!(
            start.elapsed() >= Duration::from_secs(1),
            "Retry-After honoured"
        );
        assert_eq!(report.status, PrStatus::Ok, "{:?}", report.errors);
        assert_eq!(latest(&report, "a").pr.as_ref().unwrap().id, 1);
    }

    #[tokio::test]
    async fn unknown_pr_state_is_an_error_and_not_cached() {
        let server = MockServer::start().await;
        mount_dashboard(&server, vec![]).await;
        Mock::given(method("GET"))
            .and(path(DEMO_PRS))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(page(vec![own(3, "SUPERSEDED", "a")], None)),
            )
            .mount(&server)
            .await;
        mount_no_builds(&server).await;
        let report = service()
            .sync(input(&server, vec![branch("a", "sa")]))
            .await;
        assert_eq!(report.errors[0].kind, SyncErrorKind::Other);
        assert!(report.errors[0].message.contains("SUPERSEDED"));
        assert!(matches!(report.status, PrStatus::Error { .. }));
        assert!(report.latest_prs.is_empty());
        assert!(report.pr_rows.is_empty());
    }

    #[tokio::test]
    async fn unknown_state_on_the_dashboard_falls_back_to_per_branch_lookups() {
        let server = MockServer::start().await;
        mount_dashboard(&server, vec![own(1, "WEIRD", "a")]).await;
        Mock::given(method("GET"))
            .and(path(DEMO_PRS))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(page(vec![own(1, "OPEN", "a")], None)),
            )
            .expect(1)
            .mount(&server)
            .await;
        mount_no_builds(&server).await;
        let report = service()
            .sync(input(&server, vec![branch("a", "sa")]))
            .await;
        assert_eq!(report.errors[0].kind, SyncErrorKind::Other);
        assert_eq!(latest(&report, "a").source, PrSource::Fetched);
    }

    #[tokio::test]
    async fn unreachable_host_shows_cached_data_with_age() {
        // Nothing listens on port 9 of localhost.
        let svc = BitbucketService::with_timeouts(
            Token::new(FAKE_TOKEN),
            Duration::from_millis(200),
            Duration::from_millis(500),
        );
        let mut inp = SyncInput {
            repos: vec![SyncRepo {
                repo_id: "r1".into(),
                identity: BitbucketIdentity {
                    base_url: "http://127.0.0.1:9".into(),
                    project: "PROJ".into(),
                    repo: "demo".into(),
                },
                branches: vec![branch("a", "sa")],
            }],
            pr_cache_ttl_mins: 60,
            now: NOW,
            ..Default::default()
        };
        inp.cached = vec![row("a", Some(pr(PrStateKind::Open)), NOW - 7200)];
        let report = svc.sync(inp).await;
        assert_eq!(report.status, PrStatus::Cached { age_secs: 7200 });
        assert_eq!(report.errors[0].kind, SyncErrorKind::Unreachable);
        assert_eq!(latest(&report, "a").source, PrSource::StaleFallback);
    }
}
