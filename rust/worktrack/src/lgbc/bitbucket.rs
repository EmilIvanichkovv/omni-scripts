// Copied from rust/local-git-branch-cleanup-tui/src/pr/bitbucket.rs at 7257f76bb3abe5719a52284f2b8d835151f9cb15. Local changes: see src/lgbc/ORIGIN.md.
// Bitbucket Data Center pull request provider.
//
// Talks to the Data Center REST API (`{base}/rest/api/latest`) over HTTPS
// with a Bearer HTTP access token. Owns configuration resolution (CLI >
// environment > derived-from-origin precedence), the reusable HTTP client,
// private response DTOs, state mapping, and the bounded retry policy.
//
// SECURITY: the access token is only ever stored inside the `reqwest`
// client's default headers (marked sensitive). No struct in this module
// derives `Debug` while holding the raw token, and the token never appears
// in errors, logs, or the cache key.

use super::pr::{PrInfo, PrProviderError, PrProviderKind, PrState, PullRequestProvider};
use reqwest::header::{
    HeaderMap, HeaderValue, ACCEPT, AUTHORIZATION, CONTENT_TYPE, LOCATION, RETRY_AFTER,
};
use std::time::Duration;
use url::Url;

/// Maximum number of concurrent Bitbucket API requests (spec 9.3).
const MAX_CONCURRENT_REQUESTS: usize = 8;
/// TCP/TLS connect timeout (spec 8.1).
const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// Total per-request timeout (spec 8.1).
const DEFAULT_TOTAL_TIMEOUT: Duration = Duration::from_secs(15);
/// Maximum total attempts per request, including the first (spec 8.5).
const MAX_ATTEMPTS: u32 = 3;
/// Cap on response bodies included in error messages (spec 8.4).
const ERROR_BODY_CAP: usize = 4096;

// ---------------------------------------------------------------------------
// Configuration resolution
// ---------------------------------------------------------------------------

/// Validated, normalized identity of one Bitbucket Data Center repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BitbucketRepository {
    /// Normalized base URL: http/https scheme, no credentials, no query or
    /// fragment, no trailing slash in the path, context path preserved.
    pub base_url: Url,
    /// Project key, uppercased when it was derived from `origin`.
    pub project_key: String,
    /// Repository slug with any final `.git` suffix removed.
    pub repository_slug: String,
}

/// Raw configuration candidates for `resolve_repository_input`.
///
/// Pure input model: the caller reads CLI flags, environment variables, and
/// the `origin`-derived values; this module only applies precedence and
/// normalization (spec 6.2). Never carries the access token.
#[derive(Debug, Clone, Default)]
pub struct BitbucketConfigInput {
    pub cli_base_url: Option<String>,
    pub cli_project: Option<String>,
    pub cli_repo: Option<String>,
    pub env_base_url: Option<String>,
    pub env_project: Option<String>,
    pub env_repo: Option<String>,
    pub derived_base_url: Option<String>,
    pub derived_project: Option<String>,
    pub derived_repo: Option<String>,
}

/// Where a configuration value came from, for source-dependent normalization.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ValueSource {
    Cli,
    Env,
    Derived,
}

impl ValueSource {
    fn label(self) -> &'static str {
        match self {
            Self::Cli => "the command line",
            Self::Env => "the environment",
            Self::Derived => "the origin remote",
        }
    }
}

/// Resolve one component with CLI > env > derived precedence (spec 4.2).
fn pick(
    cli: Option<String>,
    env: Option<String>,
    derived: Option<String>,
) -> Option<(String, ValueSource)> {
    cli.map(|v| (v, ValueSource::Cli))
        .or_else(|| env.map(|v| (v, ValueSource::Env)))
        .or_else(|| derived.map(|v| (v, ValueSource::Derived)))
}

/// Require a non-blank component, naming the missing piece and the exact
/// override flag in the error (spec 6.4).
fn required(
    value: Option<(String, ValueSource)>,
    what: &str,
    flag: &str,
) -> Result<(String, ValueSource), PrProviderError> {
    let missing = || {
        PrProviderError::Configuration(format!(
            "could not determine the Bitbucket {what}; supply {flag}"
        ))
    };
    match value {
        None => Err(missing()),
        Some((raw, source)) => {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                if source == ValueSource::Derived {
                    // A blank derived value is the same as no derived value.
                    Err(missing())
                } else {
                    Err(PrProviderError::Configuration(format!(
                        "the Bitbucket {what} provided via {} is empty; supply a non-empty value with {flag}",
                        source.label()
                    )))
                }
            } else {
                Ok((trimmed.to_string(), source))
            }
        }
    }
}

/// Parse and normalize the base URL (spec 6.2): http/https only, credentials
/// stripped, query/fragment dropped, trailing slash removed, context path
/// preserved.
fn normalize_base_url(raw: &str) -> Result<Url, PrProviderError> {
    // The url::ParseError display never echoes the input, so a base URL
    // containing credentials cannot leak through this error.
    let mut url = Url::parse(raw).map_err(|e| {
        PrProviderError::Configuration(format!(
            "invalid Bitbucket base URL ({e}); check --bitbucket-base-url"
        ))
    })?;

    match url.scheme() {
        "http" | "https" => {}
        other => {
            return Err(PrProviderError::Configuration(format!(
                "unsupported Bitbucket base URL scheme '{other}'; only http and https are supported — check --bitbucket-base-url"
            )))
        }
    }

    // Never keep credentials from a remote/base URL (spec 6.2, 14).
    let _ = url.set_username("");
    let _ = url.set_password(None);
    url.set_query(None);
    url.set_fragment(None);

    // Strip trailing slash but preserve a context path such as /bitbucket.
    let trimmed_path = url.path().trim_end_matches('/').to_string();
    url.set_path(&trimmed_path);

    Ok(url)
}

/// Resolve the Bitbucket identity of the repository at `repo`.
///
/// worktrack local change: the derived candidates come from the `origin`
/// remote of `repo` (via `remote::read_origin_in`) instead of the process's
/// current directory. `origin` is read only when some component has neither a
/// CLI nor an environment value and the caller left the derived candidates
/// empty; an unreadable or non-Bitbucket `origin` just leaves them empty, so
/// the error names the missing component. All precedence and normalization
/// rules live in the pure [`resolve_repository_input`].
pub fn resolve_repository(
    repo: &std::path::Path,
    mut input: BitbucketConfigInput,
) -> Result<BitbucketRepository, PrProviderError> {
    let needs_derived = (input.cli_base_url.is_none() && input.env_base_url.is_none())
        || (input.cli_project.is_none() && input.env_project.is_none())
        || (input.cli_repo.is_none() && input.env_repo.is_none());
    let has_derived = input.derived_base_url.is_some()
        || input.derived_project.is_some()
        || input.derived_repo.is_some();
    if needs_derived && !has_derived {
        if let Some(inferred) = super::remote::read_origin_in(repo)
            .ok()
            .and_then(|remote| super::remote::infer_bitbucket_repository(&remote).ok())
        {
            input.derived_base_url = Some(inferred.base_url);
            input.derived_project = Some(inferred.project_key);
            input.derived_repo = Some(inferred.repository_slug);
        }
    }
    resolve_repository_input(input)
}

/// Resolve and normalize the Bitbucket repository identity.
///
/// Pure function: reads no environment and performs no I/O. Precedence per
/// value is CLI > environment > derived-from-`origin` (spec 4.2). The project
/// key is uppercased only when it came from the derived source; explicit
/// CLI/env values are preserved apart from whitespace trimming (spec 6.2).
pub fn resolve_repository_input(
    input: BitbucketConfigInput,
) -> Result<BitbucketRepository, PrProviderError> {
    let (base_raw, _) = required(
        pick(
            input.cli_base_url,
            input.env_base_url,
            input.derived_base_url,
        ),
        "base URL",
        "--bitbucket-base-url (or set BITBUCKET_BASE_URL)",
    )?;
    let (project_raw, project_source) = required(
        pick(input.cli_project, input.env_project, input.derived_project),
        "project key",
        "--bitbucket-project (or set BITBUCKET_PROJECT)",
    )?;
    let (repo_raw, _) = required(
        pick(input.cli_repo, input.env_repo, input.derived_repo),
        "repository slug",
        "--bitbucket-repo (or set BITBUCKET_REPO)",
    )?;

    let base_url = normalize_base_url(&base_raw)?;

    let project_key = if project_source == ValueSource::Derived {
        project_raw.to_ascii_uppercase()
    } else {
        project_raw
    };

    let repository_slug = repo_raw
        .strip_suffix(".git")
        .unwrap_or(&repo_raw)
        .to_string();
    if repository_slug.trim().is_empty() {
        return Err(PrProviderError::Configuration(
            "the Bitbucket repository slug is empty after removing the .git suffix; supply --bitbucket-repo (or set BITBUCKET_REPO)".to_string(),
        ));
    }

    Ok(BitbucketRepository {
        base_url,
        project_key,
        repository_slug,
    })
}

/// Base URL as a string without a trailing slash, for cache keys and display.
fn base_url_str(repository: &BitbucketRepository) -> &str {
    repository.base_url.as_str().trim_end_matches('/')
}

// ---------------------------------------------------------------------------
// URL construction
// ---------------------------------------------------------------------------

/// `{base}/rest/api/latest/projects/{key}/repos/{slug}` with encoded segments.
fn repo_api_url(repository: &BitbucketRepository) -> Url {
    let mut url = repository.base_url.clone();
    {
        let mut segments = url
            .path_segments_mut()
            .expect("http(s) base URL always has path segments");
        segments.pop_if_empty();
        segments.extend(["rest", "api", "latest", "projects"]);
        segments.push(&repository.project_key);
        segments.push("repos");
        segments.push(&repository.repository_slug);
    }
    url
}

/// PR lookup URL with the spec 7.3 query. Built via `url` query
/// serialization so branch names with slashes, plus signs, spaces, or
/// Unicode are percent-encoded, never concatenated raw.
fn pr_lookup_url(repository: &BitbucketRepository, branch_name: &str) -> Url {
    let mut url = repo_api_url(repository);
    url.path_segments_mut()
        .expect("http(s) base URL always has path segments")
        .push("pull-requests");
    url.query_pairs_mut()
        .append_pair("direction", "OUTGOING")
        .append_pair("at", &format!("refs/heads/{branch_name}"))
        .append_pair("state", "ALL")
        .append_pair("order", "NEWEST")
        .append_pair("limit", "1");
    url
}

/// Fallback browser URL when a PR carries no self link (spec 7.5):
/// `{base}/projects/{key}/repos/{slug}/pull-requests/{id}/overview`.
fn fallback_pr_url(repository: &BitbucketRepository, id: u64) -> String {
    let mut url = repository.base_url.clone();
    {
        let mut segments = url
            .path_segments_mut()
            .expect("http(s) base URL always has path segments");
        segments.pop_if_empty();
        segments.push("projects");
        segments.push(&repository.project_key);
        segments.push("repos");
        segments.push(&repository.repository_slug);
        segments.push("pull-requests");
        segments.push(&id.to_string());
        segments.push("overview");
    }
    url.to_string()
}

// ---------------------------------------------------------------------------
// Response DTOs (private, spec 7.4)
// ---------------------------------------------------------------------------

/// One page of a Bitbucket paged response. Only the newest PR is needed
/// (`limit=1`), so `nextPageStart` is never followed; the pagination fields
/// are still parsed to pin down the server contract.
#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct Page<T> {
    values: Vec<T>,
    // Parsed to pin down the server contract (spec 7.5); not read outside tests.
    #[allow(dead_code)]
    size: usize,
    #[allow(dead_code)]
    limit: usize,
    #[allow(dead_code)]
    is_last_page: bool,
    #[serde(default)]
    #[allow(dead_code)]
    next_page_start: Option<usize>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct BitbucketPullRequest {
    id: u64,
    title: String,
    state: String,
    from_ref: BitbucketRef,
    links: BitbucketLinks,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct BitbucketRef {
    id: String,
    // Part of the spec 7.4 DTO shape; validation uses the fully qualified `id`.
    #[allow(dead_code)]
    display_id: String,
    repository: BitbucketRepositoryDto,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct BitbucketRepositoryDto {
    slug: String,
    project: BitbucketProjectDto,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct BitbucketProjectDto {
    key: String,
}

#[derive(Debug, serde::Deserialize)]
struct BitbucketLinks {
    #[serde(rename = "self", default)]
    self_links: Vec<BitbucketLink>,
}

#[derive(Debug, serde::Deserialize)]
struct BitbucketLink {
    href: String,
}

// ---------------------------------------------------------------------------
// Response mapping (spec 7.5, 7.6)
// ---------------------------------------------------------------------------

/// Map a Bitbucket PR state to the shared state, ASCII case-insensitively.
/// Unknown states are `InvalidResponse`, never silently "closed".
fn map_pr_state(raw: &str) -> Result<PrState, PrProviderError> {
    if raw.eq_ignore_ascii_case("OPEN") {
        Ok(PrState::Open)
    } else if raw.eq_ignore_ascii_case("MERGED") {
        Ok(PrState::Merged)
    } else if raw.eq_ignore_ascii_case("DECLINED") {
        Ok(PrState::Closed)
    } else {
        Err(PrProviderError::InvalidResponse(format!(
            "unsupported pull request state: {raw}"
        )))
    }
}

/// Validate and normalize a lookup page into `Option<PrInfo>` (spec 7.5).
///
/// An empty page is a successful "no PR" result. A non-empty page whose first
/// item does not match the requested branch ref or configured repository is
/// `InvalidResponse` — it must never be treated (or cached) as "no PR".
fn pr_from_page(
    page: Page<BitbucketPullRequest>,
    repository: &BitbucketRepository,
    branch_name: &str,
) -> Result<Option<PrInfo>, PrProviderError> {
    let Some(pr) = page.values.into_iter().next() else {
        return Ok(None);
    };

    let expected_ref = format!("refs/heads/{branch_name}");
    if pr.from_ref.id != expected_ref {
        return Err(PrProviderError::InvalidResponse(format!(
            "pull request source ref '{}' does not match the requested ref '{expected_ref}'",
            pr.from_ref.id
        )));
    }
    if !pr
        .from_ref
        .repository
        .slug
        .eq_ignore_ascii_case(&repository.repository_slug)
    {
        return Err(PrProviderError::InvalidResponse(format!(
            "pull request belongs to repository '{}', expected '{}'",
            pr.from_ref.repository.slug, repository.repository_slug
        )));
    }

    let state = map_pr_state(&pr.state)?;
    let url = pr
        .links
        .self_links
        .iter()
        .map(|link| link.href.trim())
        .find(|href| !href.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| fallback_pr_url(repository, pr.id));

    Ok(Some(PrInfo {
        number: pr.id,
        state,
        title: pr.title,
        url,
    }))
}

// ---------------------------------------------------------------------------
// HTTP error handling helpers (spec 8.4)
// ---------------------------------------------------------------------------

/// Join the reqwest error's source chain into one line, so timeouts, connect
/// failures, and TLS causes stay visible without exposing headers.
fn error_chain_string(err: &reqwest::Error) -> String {
    let mut parts = vec![err.to_string()];
    let mut source = std::error::Error::source(err);
    while let Some(cause) = source {
        parts.push(cause.to_string());
        source = cause.source();
    }
    parts.join(": ")
}

/// Heuristic TLS detection over the error source chain.
fn is_tls_error(err: &reqwest::Error) -> bool {
    let chain = error_chain_string(err).to_ascii_lowercase();
    chain.contains("certificate")
        || chain.contains("tls")
        || chain.contains("ssl")
        || chain.contains("handshake")
}

/// Map a reqwest transport error to a typed provider error (spec 8.3, 8.4).
fn classify_transport_error(err: &reqwest::Error) -> PrProviderError {
    if is_tls_error(err) {
        PrProviderError::Tls(format!(
            "TLS connection to the Bitbucket server failed: {}. If your organization uses a private certificate authority, the operating system / WSL trust store may need the organization's CA certificate installed. Certificate verification must stay enabled.",
            error_chain_string(err)
        ))
    } else if err.is_timeout() {
        PrProviderError::Transport(format!(
            "request to the Bitbucket server timed out: {}",
            error_chain_string(err)
        ))
    } else {
        PrProviderError::Transport(format!(
            "network error talking to the Bitbucket server: {}",
            error_chain_string(err)
        ))
    }
}

/// Reduce a redirect `Location` to host + path only: no credentials, query,
/// or fragment.
fn sanitize_location(raw: &str) -> String {
    match Url::parse(raw) {
        Ok(url) => format!("{}{}", url.host_str().unwrap_or(""), url.path()),
        // Relative redirect target: keep the path, drop query/fragment.
        Err(_) => raw.split(['?', '#']).next().unwrap_or("").to_string(),
    }
}

/// Turn (at most `ERROR_BODY_CAP` bytes of) an error body into one safe
/// diagnostic string (spec 8.4, 14).
fn sanitize_error_body(bytes: &[u8], truncated: bool) -> String {
    let text = String::from_utf8_lossy(bytes);
    let lower = text.to_ascii_lowercase();
    if lower.contains("bearer") || lower.contains("authorization") {
        return "[response body redacted: it contained authorization data]".to_string();
    }
    let cleaned: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let cleaned = cleaned.trim();
    if cleaned.is_empty() {
        "<empty body>".to_string()
    } else if truncated {
        format!("{cleaned}… (truncated)")
    } else {
        cleaned.to_string()
    }
}

/// Read at most `ERROR_BODY_CAP` bytes of the response body; never buffer an
/// unbounded body solely for an error message (spec 15).
async fn read_error_body(mut response: reqwest::Response) -> String {
    let mut buf: Vec<u8> = Vec::new();
    let mut truncated = false;
    while let Ok(Some(chunk)) = response.chunk().await {
        let remaining = ERROR_BODY_CAP - buf.len();
        if chunk.len() >= remaining {
            buf.extend_from_slice(&chunk[..remaining]);
            truncated = true;
            break;
        }
        buf.extend_from_slice(&chunk);
    }
    sanitize_error_body(&buf, truncated)
}

/// Parse a `Retry-After` header given in whole seconds. Invalid or missing
/// values are ignored (HTTP-date form is treated as invalid here).
fn parse_retry_after(headers: &HeaderMap) -> Option<Duration> {
    headers
        .get(RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()
        .map(Duration::from_secs)
}

/// Statuses eligible for retry (spec 8.5).
fn is_retryable_status(status: reqwest::StatusCode) -> bool {
    matches!(status.as_u16(), 429 | 502 | 503 | 504)
}

/// Backoff before the next attempt: ~250 ms then ~500 ms, or a valid server
/// `Retry-After` when it is longer (spec 8.5).
fn backoff_delay(completed_attempt: u32, retry_after: Option<Duration>) -> Duration {
    let base = if completed_attempt <= 1 {
        Duration::from_millis(250)
    } else {
        Duration::from_millis(500)
    };
    match retry_after {
        Some(server) if server > base => server,
        _ => base,
    }
}

/// Map a non-success HTTP response to a typed provider error (spec 8.4).
/// Consumes the response; bodies are capped and sanitized, and response
/// headers are never included.
async fn error_from_response(response: reqwest::Response) -> PrProviderError {
    let status = response.status();
    match status.as_u16() {
        301 | 302 | 303 | 307 | 308 => {
            let location = response
                .headers()
                .get(LOCATION)
                .and_then(|v| v.to_str().ok())
                .map(sanitize_location);
            let target = match location {
                Some(loc) if !loc.is_empty() => format!(" (redirected to {loc})"),
                _ => String::new(),
            };
            PrProviderError::Configuration(format!(
                "the Bitbucket server redirected the API request{target}; this usually means the base URL is wrong or the server sent the request to a sign-in page — check --bitbucket-base-url and that BITBUCKET_TOKEN is a valid HTTP access token"
            ))
        }
        400 => PrProviderError::InvalidResponse(format!(
            "Bitbucket rejected the request (HTTP 400): {}",
            read_error_body(response).await
        )),
        401 => PrProviderError::Authentication(
            "Bitbucket returned HTTP 401 — BITBUCKET_TOKEN is missing, invalid, or expired"
                .to_string(),
        ),
        403 => PrProviderError::Forbidden(
            "Bitbucket returned HTTP 403 — the token or account lacks read access to this repository".to_string(),
        ),
        404 => PrProviderError::NotFound(
            "Bitbucket returned HTTP 404 — the base URL, project key, or repository slug is incorrect (check --bitbucket-base-url, --bitbucket-project, --bitbucket-repo)".to_string(),
        ),
        429 => PrProviderError::RateLimited {
            retry_after: parse_retry_after(response.headers()),
        },
        500..=599 => PrProviderError::ProviderUnavailable(format!(
            "Bitbucket returned HTTP {status}: {}",
            read_error_body(response).await
        )),
        _ => PrProviderError::ProviderUnavailable(format!(
            "Bitbucket returned unexpected HTTP {status}: {}",
            read_error_body(response).await
        )),
    }
}

/// Deserialize a 2xx response as JSON. A 2xx page that is not JSON (for
/// example an HTML SSO login page) is `InvalidResponse` (spec 8.4).
async fn parse_json<T: serde::de::DeserializeOwned>(
    response: reqwest::Response,
) -> Result<T, PrProviderError> {
    let content_type = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    if !content_type.is_empty() && !content_type.contains("json") {
        return Err(PrProviderError::InvalidResponse(format!(
            "expected a JSON response but received '{content_type}' — the server may have returned a sign-in page instead of API data; check --bitbucket-base-url and BITBUCKET_TOKEN"
        )));
    }

    let body = response.text().await.map_err(|e| {
        PrProviderError::InvalidResponse(format!("failed to read response body: {e}"))
    })?;
    serde_json::from_str(&body)
        .map_err(|e| PrProviderError::InvalidResponse(format!("malformed JSON response: {e}")))
}

// ---------------------------------------------------------------------------
// Provider
// ---------------------------------------------------------------------------

/// Bitbucket Data Center PR provider.
///
/// Deliberately does NOT implement `Debug`: the reusable client carries the
/// Bearer token in its default headers (spec 8.2, 14).
pub struct BitbucketProvider {
    repository: BitbucketRepository,
    client: reqwest::Client,
}

impl BitbucketProvider {
    /// Create a provider with the spec 8.1 timeouts (5 s connect, 15 s total).
    ///
    /// The token is moved into the client's default `Authorization` header
    /// (marked sensitive) and is not stored anywhere else.
    pub fn new(repository: BitbucketRepository, token: String) -> Result<Self, PrProviderError> {
        Self::with_timeouts(
            repository,
            token,
            DEFAULT_CONNECT_TIMEOUT,
            DEFAULT_TOTAL_TIMEOUT,
        )
    }

    /// Private constructor with overridable timeouts, so tests can exercise
    /// the timeout path without waiting the full 15 seconds.
    fn with_timeouts(
        repository: BitbucketRepository,
        token: String,
        connect_timeout: Duration,
        total_timeout: Duration,
    ) -> Result<Self, PrProviderError> {
        let client = build_client(&token, connect_timeout, total_timeout)?;
        Ok(Self { repository, client })
    }

    /// Send a GET request with the spec 8.5 retry policy: at most three total
    /// attempts, only for 429/502/503/504 and transient connect errors, with
    /// ~250 ms / ~500 ms backoff or a longer valid server `Retry-After`.
    async fn execute_with_retry(&self, url: Url) -> Result<reqwest::Response, PrProviderError> {
        send_with_retry(&self.client, url, true).await
    }
}

/// The retry loop of `BitbucketProvider::execute_with_retry`, shared with
/// [`BitbucketHostClient`] (worktrack local change). With
/// `retry_rate_limited = false` an HTTP 429 is returned at once as
/// `RateLimited`, so the caller can pause every request to that host
/// instead of only this one.
async fn send_with_retry(
    client: &reqwest::Client,
    url: Url,
    retry_rate_limited: bool,
) -> Result<reqwest::Response, PrProviderError> {
    let mut attempt: u32 = 1;
    loop {
        match client.get(url.clone()).send().await {
            Ok(response) => {
                let status = response.status();
                let retry_this =
                    is_retryable_status(status) && (retry_rate_limited || status.as_u16() != 429);
                if retry_this && attempt < MAX_ATTEMPTS {
                    let retry_after = parse_retry_after(response.headers());
                    tokio::time::sleep(backoff_delay(attempt, retry_after)).await;
                    attempt += 1;
                    continue;
                }
                if status.is_success() {
                    return Ok(response);
                }
                return Err(error_from_response(response).await);
            }
            Err(err) => {
                // TLS failures also surface as connect errors; never
                // retry those (spec 8.5). Timeouts are not retried.
                if err.is_connect() && !is_tls_error(&err) && attempt < MAX_ATTEMPTS {
                    tokio::time::sleep(backoff_delay(attempt, None)).await;
                    attempt += 1;
                    continue;
                }
                return Err(classify_transport_error(&err));
            }
        }
    }
}

/// Build the reusable HTTP client: Bearer token in a sensitive default
/// header, JSON accept, worktrack User-Agent, timeouts, no redirects.
/// Extracted from `BitbucketProvider::with_timeouts` (worktrack local change)
/// so [`BitbucketHostClient`] shares it.
fn build_client(
    token: &str,
    connect_timeout: Duration,
    total_timeout: Duration,
) -> Result<reqwest::Client, PrProviderError> {
    let token = token.trim();
    if token.is_empty() {
        return Err(PrProviderError::Configuration(
            "BITBUCKET_TOKEN is empty; export a Bitbucket Data Center HTTP access token"
                .to_string(),
        ));
    }

    let mut auth = HeaderValue::from_str(&format!("Bearer {token}")).map_err(|_| {
        // Never echo the token itself.
        PrProviderError::Configuration(
            "BITBUCKET_TOKEN contains characters that cannot be sent in an HTTP header".to_string(),
        )
    })?;
    auth.set_sensitive(true);

    let mut headers = HeaderMap::new();
    headers.insert(AUTHORIZATION, auth);
    headers.insert(ACCEPT, HeaderValue::from_static("application/json"));

    // Redirects stay disabled on purpose: a REST endpoint redirecting to
    // an SSO/login page must surface as a configuration/authentication
    // error, not be followed as if it were API JSON (spec 8.1).
    crate::lgbc::tls::install_crypto_provider();
    reqwest::Client::builder()
        .default_headers(headers)
        .user_agent(concat!("worktrack/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(connect_timeout)
        .timeout(total_timeout)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| {
            PrProviderError::Configuration(format!("failed to construct the HTTP client: {e}"))
        })
}

#[async_trait::async_trait]
impl PullRequestProvider for BitbucketProvider {
    fn kind(&self) -> PrProviderKind {
        PrProviderKind::BitbucketDataCenter
    }

    /// Provider- and host-aware cache key (spec 10.1). Never contains the
    /// token, credentials, or query parameters.
    fn cache_key(&self) -> String {
        format!(
            "bitbucket-dc|{}|{}|{}",
            base_url_str(&self.repository),
            self.repository.project_key.to_ascii_uppercase(),
            self.repository.repository_slug
        )
    }

    fn max_concurrency(&self) -> usize {
        MAX_CONCURRENT_REQUESTS
    }

    /// One repository metadata request (spec 7.2): detects bad tokens,
    /// missing permission, and misderived project/repository once, before
    /// per-branch lookups begin.
    async fn validate(&self) -> Result<(), PrProviderError> {
        let response = self
            .execute_with_retry(repo_api_url(&self.repository))
            .await?;
        let dto: BitbucketRepositoryDto = parse_json(response).await?;

        if !dto
            .slug
            .eq_ignore_ascii_case(&self.repository.repository_slug)
        {
            return Err(PrProviderError::InvalidResponse(format!(
                "repository validation returned slug '{}', expected '{}'",
                dto.slug, self.repository.repository_slug
            )));
        }
        if !dto
            .project
            .key
            .eq_ignore_ascii_case(&self.repository.project_key)
        {
            return Err(PrProviderError::InvalidResponse(format!(
                "repository validation returned project key '{}', expected '{}'",
                dto.project.key, self.repository.project_key
            )));
        }
        Ok(())
    }

    /// Newest PR whose source branch is `refs/heads/<branch_name>` (spec 7.3,
    /// 7.5). An empty page is a successful no-PR result.
    async fn get_pr_for_branch(
        &self,
        branch_name: &str,
    ) -> Result<Option<PrInfo>, PrProviderError> {
        let url = pr_lookup_url(&self.repository, branch_name);
        let response = self.execute_with_retry(url).await?;
        let page: Page<BitbucketPullRequest> = parse_json(response).await?;
        pr_from_page(page, &self.repository, branch_name)
    }
}

// ---------------------------------------------------------------------------
// worktrack milestone 2: host-level requests (worktrack spec 5.1)
//
// Everything below this line up to the tests is new in worktrack, not copied.
// Requests are single pages; the caller drives paging so that every request
// passes its per-host concurrency gate. An HTTP 429 is never retried here:
// it comes back as `RateLimited` so the caller can pause the whole host.
// ---------------------------------------------------------------------------

/// Response header carrying the authenticated user's name.
const X_AUSERNAME: &str = "X-AUSERNAME";
/// Page size for the dashboard and build-status requests.
const HOST_PAGE_LIMIT: u32 = 100;

/// `{base}/{segments...}` with every segment percent-encoded.
fn host_url(base: &Url, segments: &[&str]) -> Url {
    let mut url = base.clone();
    {
        let mut path = url
            .path_segments_mut()
            .expect("http(s) base URL always has path segments");
        path.pop_if_empty();
        path.extend(segments);
    }
    url
}

/// `GET /rest/api/1.0/dashboard/pull-requests?role=AUTHOR&state=OPEN&limit=100&start=N`.
fn dashboard_url(base: &Url, start: u64) -> Url {
    let mut url = host_url(base, &["rest", "api", "1.0", "dashboard", "pull-requests"]);
    url.query_pairs_mut()
        .append_pair("role", "AUTHOR")
        .append_pair("state", "OPEN")
        .append_pair("limit", &HOST_PAGE_LIMIT.to_string())
        .append_pair("start", &start.to_string());
    url
}

/// Newest PR of one branch in any state:
/// `GET /rest/api/1.0/projects/{p}/repos/{r}/pull-requests?at=refs/heads/{branch}&direction=OUTGOING&state=ALL&order=NEWEST&limit=1`.
fn latest_pr_url(repository: &BitbucketRepository, branch_name: &str) -> Url {
    let mut url = host_url(
        &repository.base_url,
        &[
            "rest",
            "api",
            "1.0",
            "projects",
            &repository.project_key,
            "repos",
            &repository.repository_slug,
            "pull-requests",
        ],
    );
    url.query_pairs_mut()
        .append_pair("at", &format!("refs/heads/{branch_name}"))
        .append_pair("direction", "OUTGOING")
        .append_pair("state", "ALL")
        .append_pair("order", "NEWEST")
        .append_pair("limit", "1");
    url
}

/// `GET /rest/build-status/1.0/commits/{sha}?limit=100&start=N`.
fn build_status_url(base: &Url, sha: &str, start: u64) -> Url {
    let mut url = host_url(base, &["rest", "build-status", "1.0", "commits", sha]);
    url.query_pairs_mut()
        .append_pair("limit", &HOST_PAGE_LIMIT.to_string())
        .append_pair("start", &start.to_string());
    url
}

/// `GET /rest/api/1.0/application-properties`: a cheap authenticated request
/// whose only use is its `X-AUSERNAME` response header.
fn application_properties_url(base: &Url) -> Url {
    host_url(base, &["rest", "api", "1.0", "application-properties"])
}

/// A lenient page: `isLastPage` defaults to true and `size`/`limit` are not
/// required, since only `values` and the paging cursor are read.
#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct PageV1<T> {
    #[serde(default = "Vec::new")]
    values: Vec<T>,
    #[serde(default = "default_true")]
    is_last_page: bool,
    #[serde(default)]
    next_page_start: Option<u64>,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct DetailedPullRequestDto {
    id: u64,
    #[serde(default)]
    title: String,
    state: String,
    from_ref: DetailedRefDto,
    to_ref: DetailedRefDto,
    #[serde(default)]
    author: Option<ParticipantDto>,
    #[serde(default)]
    reviewers: Vec<ParticipantDto>,
    #[serde(default)]
    properties: Option<PrPropertiesDto>,
    #[serde(default)]
    updated_date: Option<i64>,
    #[serde(default)]
    closed_date: Option<i64>,
    #[serde(default)]
    links: Option<BitbucketLinks>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct DetailedRefDto {
    id: String,
    #[serde(default)]
    display_id: Option<String>,
    #[serde(default)]
    latest_commit: Option<String>,
    repository: BitbucketRepositoryDto,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ParticipantDto {
    user: UserDto,
    #[serde(default)]
    status: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct UserDto {
    #[serde(default)]
    name: String,
    #[serde(default)]
    slug: Option<String>,
    #[serde(default)]
    display_name: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct PrPropertiesDto {
    #[serde(default)]
    open_task_count: Option<u32>,
    #[serde(default)]
    comment_count: Option<u32>,
}

#[derive(Debug, serde::Deserialize)]
struct BuildStatusDto {
    state: String,
}

/// Project key + slug of a repository as reported in a PR ref.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoRef {
    pub project_key: String,
    pub slug: String,
}

impl RepoRef {
    /// Same repository, compared ASCII case-insensitively.
    pub fn same_repo(&self, project_key: &str, slug: &str) -> bool {
        self.project_key.eq_ignore_ascii_case(project_key) && self.slug.eq_ignore_ascii_case(slug)
    }
}

/// One reviewer of a PR, raw from the payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewerDetails {
    /// User `name` (login).
    pub name: String,
    /// User `slug`; falls back to `name`.
    pub slug: String,
    /// User `displayName`; falls back to `name`.
    pub display_name: String,
    /// `APPROVED`, `NEEDS_WORK`, `UNAPPROVED`, or whatever the server sent.
    pub status: String,
}

/// A pull request with the fields worktrack needs (spec 5.1). Text is raw
/// server text; callers sanitize before display.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullRequestDetails {
    pub id: u64,
    pub title: String,
    pub state: PrState,
    /// First non-empty self link, or the fallback overview URL.
    pub url: String,
    /// Author user slug (falls back to the user name); empty when absent.
    pub author_slug: String,
    pub reviewers: Vec<ReviewerDetails>,
    pub open_task_count: u32,
    pub comment_count: u32,
    /// `fromRef.id`, e.g. `refs/heads/feature/x`.
    pub from_ref_id: String,
    /// `fromRef.displayId`, e.g. `feature/x`.
    pub from_branch: String,
    pub from_latest_commit: Option<String>,
    pub from_repo: RepoRef,
    pub to_repo: RepoRef,
    /// Milliseconds since the epoch, as sent by the server.
    pub updated_date_ms: Option<i64>,
    pub closed_date_ms: Option<i64>,
}

impl PullRequestDetails {
    /// A PR whose source repository differs from its target (spec 5.1: ignored).
    pub fn is_from_fork(&self) -> bool {
        !self
            .from_repo
            .same_repo(&self.to_repo.project_key, &self.to_repo.slug)
    }
}

/// One page of a host-level listing plus the `X-AUSERNAME` header of the
/// response that carried it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostPage<T> {
    pub values: Vec<T>,
    /// `Some(start)` of the next page, `None` on the last page.
    pub next_start: Option<u64>,
    pub username: Option<String>,
}

fn username_header(headers: &HeaderMap) -> Option<String> {
    headers
        .get(X_AUSERNAME)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string)
}

fn repo_ref(dto: &BitbucketRepositoryDto) -> RepoRef {
    RepoRef {
        project_key: dto.project.key.clone(),
        slug: dto.slug.clone(),
    }
}

/// Map a detailed PR DTO; an unknown state is `InvalidResponse`.
fn details_from_dto(
    dto: DetailedPullRequestDto,
    base_url: &Url,
) -> Result<PullRequestDetails, PrProviderError> {
    let state = map_pr_state(&dto.state)?;
    let to_repo = repo_ref(&dto.to_ref.repository);
    let from_repo = repo_ref(&dto.from_ref.repository);
    let url = dto
        .links
        .as_ref()
        .and_then(|l| {
            l.self_links
                .iter()
                .map(|link| link.href.trim())
                .find(|href| !href.is_empty())
                .map(str::to_string)
        })
        .unwrap_or_else(|| {
            fallback_pr_url(
                &BitbucketRepository {
                    base_url: base_url.clone(),
                    project_key: to_repo.project_key.clone(),
                    repository_slug: to_repo.slug.clone(),
                },
                dto.id,
            )
        });
    let from_branch = dto.from_ref.display_id.clone().unwrap_or_else(|| {
        dto.from_ref
            .id
            .strip_prefix("refs/heads/")
            .unwrap_or(&dto.from_ref.id)
            .to_string()
    });
    let author_slug = dto
        .author
        .as_ref()
        .map(|a| a.user.slug.clone().unwrap_or_else(|| a.user.name.clone()))
        .unwrap_or_default();
    let reviewers = dto
        .reviewers
        .into_iter()
        .map(|r| ReviewerDetails {
            slug: r.user.slug.clone().unwrap_or_else(|| r.user.name.clone()),
            display_name: r
                .user
                .display_name
                .clone()
                .unwrap_or_else(|| r.user.name.clone()),
            name: r.user.name,
            status: r.status.unwrap_or_default(),
        })
        .collect();
    let (open_task_count, comment_count) = dto
        .properties
        .map(|p| (p.open_task_count.unwrap_or(0), p.comment_count.unwrap_or(0)))
        .unwrap_or((0, 0));
    Ok(PullRequestDetails {
        id: dto.id,
        title: dto.title,
        state,
        url,
        author_slug,
        reviewers,
        open_task_count,
        comment_count,
        from_ref_id: dto.from_ref.id,
        from_branch,
        from_latest_commit: dto.from_ref.latest_commit,
        from_repo,
        to_repo,
        updated_date_ms: dto.updated_date,
        closed_date_ms: dto.closed_date,
    })
}

fn next_start<T>(page: &PageV1<T>) -> Option<u64> {
    if page.is_last_page {
        None
    } else {
        page.next_page_start
    }
}

/// Bitbucket Data Center client for host-level requests (dashboard, build
/// status, current user) and detailed per-branch PR lookups.
///
/// Like `BitbucketProvider`, deliberately does NOT implement `Debug`: the
/// client carries the Bearer token in its default headers.
pub struct BitbucketHostClient {
    base_url: Url,
    client: reqwest::Client,
}

impl BitbucketHostClient {
    /// Client for the server at `base_url` (normalized like the provider's
    /// base URL), with the 5 s connect / 15 s total timeouts.
    pub fn new(base_url: &str, token: &str) -> Result<Self, PrProviderError> {
        Self::with_timeouts(
            base_url,
            token,
            DEFAULT_CONNECT_TIMEOUT,
            DEFAULT_TOTAL_TIMEOUT,
        )
    }

    pub fn with_timeouts(
        base_url: &str,
        token: &str,
        connect_timeout: Duration,
        total_timeout: Duration,
    ) -> Result<Self, PrProviderError> {
        let base_url = normalize_base_url(base_url)?;
        let client = build_client(token, connect_timeout, total_timeout)?;
        Ok(Self { base_url, client })
    }

    pub fn base_url(&self) -> &Url {
        &self.base_url
    }

    async fn get(&self, url: Url) -> Result<reqwest::Response, PrProviderError> {
        send_with_retry(&self.client, url, false).await
    }

    /// The authenticated user's name from the `X-AUSERNAME` header.
    /// `Ok(None)` when the server did not send it.
    pub async fn current_user(&self) -> Result<Option<String>, PrProviderError> {
        let response = self.get(application_properties_url(&self.base_url)).await?;
        Ok(username_header(response.headers()))
    }

    /// One page of my open PRs across all repositories (role AUTHOR).
    /// Any PR with an unknown state fails the page.
    pub async fn dashboard_page(
        &self,
        start: u64,
    ) -> Result<HostPage<PullRequestDetails>, PrProviderError> {
        let response = self.get(dashboard_url(&self.base_url, start)).await?;
        let username = username_header(response.headers());
        let page: PageV1<DetailedPullRequestDto> = parse_json(response).await?;
        let next_start = next_start(&page);
        let values = page
            .values
            .into_iter()
            .map(|dto| details_from_dto(dto, &self.base_url))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(HostPage {
            values,
            next_start,
            username,
        })
    }

    /// Newest PR in any state whose source is `refs/heads/<branch_name>` in
    /// `repository`. `Ok(None)` = no PR. A PR for another ref or source
    /// repository is `InvalidResponse`, never "no PR".
    pub async fn latest_pull_request_for_branch(
        &self,
        repository: &BitbucketRepository,
        branch_name: &str,
    ) -> Result<Option<PullRequestDetails>, PrProviderError> {
        let response = self.get(latest_pr_url(repository, branch_name)).await?;
        let page: PageV1<DetailedPullRequestDto> = parse_json(response).await?;
        let Some(dto) = page.values.into_iter().next() else {
            return Ok(None);
        };
        let expected_ref = format!("refs/heads/{branch_name}");
        if dto.from_ref.id != expected_ref {
            return Err(PrProviderError::InvalidResponse(format!(
                "pull request source ref '{}' does not match the requested ref '{expected_ref}'",
                dto.from_ref.id
            )));
        }
        let pr = details_from_dto(dto, &repository.base_url)?;
        if !pr
            .from_repo
            .same_repo(&repository.project_key, &repository.repository_slug)
        {
            return Err(PrProviderError::InvalidResponse(format!(
                "pull request belongs to repository '{}', expected '{}'",
                pr.from_repo.slug, repository.repository_slug
            )));
        }
        Ok(Some(pr))
    }

    /// One page of the raw build states (`SUCCESSFUL`, `FAILED`,
    /// `INPROGRESS`, ...) reported for commit `sha`.
    pub async fn build_status_page(
        &self,
        sha: &str,
        start: u64,
    ) -> Result<HostPage<String>, PrProviderError> {
        let response = self
            .get(build_status_url(&self.base_url, sha, start))
            .await?;
        let username = username_header(response.headers());
        let page: PageV1<BuildStatusDto> = parse_json(response).await?;
        let next_start = next_start(&page);
        Ok(HostPage {
            values: page.values.into_iter().map(|s| s.state).collect(),
            next_start,
            username,
        })
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Obviously fake token for every test (spec 14).
    const FAKE_TOKEN: &str = "fake-test-token-not-real";

    fn resolved(base: &str, project: &str, repo: &str) -> BitbucketRepository {
        resolve_repository_input(BitbucketConfigInput {
            cli_base_url: Some(base.to_string()),
            cli_project: Some(project.to_string()),
            cli_repo: Some(repo.to_string()),
            ..Default::default()
        })
        .unwrap()
    }

    fn example_repository() -> BitbucketRepository {
        resolved("https://bitbucket.example.com", "PROJ", "demo-repo")
    }

    // --- Configuration resolution: precedence ---

    #[test]
    fn cli_beats_env_and_derived_for_every_field() {
        let repo = resolve_repository_input(BitbucketConfigInput {
            cli_base_url: Some("https://cli.example.com".to_string()),
            cli_project: Some("CLIPROJ".to_string()),
            cli_repo: Some("cli-repo".to_string()),
            env_base_url: Some("https://env.example.com".to_string()),
            env_project: Some("ENVPROJ".to_string()),
            env_repo: Some("env-repo".to_string()),
            derived_base_url: Some("https://derived.example.com".to_string()),
            derived_project: Some("derivedproj".to_string()),
            derived_repo: Some("derived-repo".to_string()),
        })
        .unwrap();
        assert_eq!(repo.base_url.as_str(), "https://cli.example.com/");
        assert_eq!(repo.project_key, "CLIPROJ");
        assert_eq!(repo.repository_slug, "cli-repo");
    }

    #[test]
    fn env_beats_derived_for_every_field() {
        let repo = resolve_repository_input(BitbucketConfigInput {
            env_base_url: Some("https://env.example.com".to_string()),
            env_project: Some("ENVPROJ".to_string()),
            env_repo: Some("env-repo".to_string()),
            derived_base_url: Some("https://derived.example.com".to_string()),
            derived_project: Some("derivedproj".to_string()),
            derived_repo: Some("derived-repo".to_string()),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(repo.base_url.as_str(), "https://env.example.com/");
        assert_eq!(repo.project_key, "ENVPROJ");
        assert_eq!(repo.repository_slug, "env-repo");
    }

    #[test]
    fn derived_values_used_when_nothing_else_is_set() {
        let repo = resolve_repository_input(BitbucketConfigInput {
            derived_base_url: Some("https://bitbucket.example.com".to_string()),
            derived_project: Some("proj".to_string()),
            derived_repo: Some("demo-repo.git".to_string()),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(repo.base_url.as_str(), "https://bitbucket.example.com/");
        assert_eq!(repo.project_key, "PROJ");
        assert_eq!(repo.repository_slug, "demo-repo");
    }

    #[test]
    fn precedence_is_per_value_not_per_source() {
        // CLI supplies only the project; base URL falls to env, repo to derived.
        let repo = resolve_repository_input(BitbucketConfigInput {
            cli_project: Some("CLIPROJ".to_string()),
            env_base_url: Some("https://env.example.com".to_string()),
            derived_repo: Some("derived-repo".to_string()),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(repo.base_url.as_str(), "https://env.example.com/");
        assert_eq!(repo.project_key, "CLIPROJ");
        assert_eq!(repo.repository_slug, "derived-repo");
    }

    // --- Configuration resolution: normalization ---

    #[test]
    fn derived_project_key_is_uppercased() {
        let repo = resolve_repository_input(BitbucketConfigInput {
            derived_base_url: Some("https://host.example.com".to_string()),
            derived_project: Some("proj".to_string()),
            derived_repo: Some("repo".to_string()),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(repo.project_key, "PROJ");
    }

    #[test]
    fn explicit_project_key_case_is_preserved() {
        let repo = resolved("https://host.example.com", "oG", "repo");
        assert_eq!(repo.project_key, "oG");
    }

    #[test]
    fn trailing_slash_is_stripped_from_base_url() {
        let repo = resolved("https://host.example.com/", "PROJ", "repo");
        assert_eq!(base_url_str(&repo), "https://host.example.com");
    }

    #[test]
    fn context_path_is_preserved_without_trailing_slash() {
        let repo = resolved("https://host.example.com/bitbucket/", "PROJ", "repo");
        assert_eq!(base_url_str(&repo), "https://host.example.com/bitbucket");
    }

    #[test]
    fn nonstandard_port_is_preserved() {
        let repo = resolved("http://host.example.com:7990", "PROJ", "repo");
        assert_eq!(base_url_str(&repo), "http://host.example.com:7990");
    }

    #[test]
    fn git_suffix_is_stripped_from_repository_slug() {
        let repo = resolved("https://host.example.com", "PROJ", "demo-repo.git");
        assert_eq!(repo.repository_slug, "demo-repo");
        // Only the final suffix is stripped.
        let repo = resolved("https://host.example.com", "PROJ", "repo.git.git");
        assert_eq!(repo.repository_slug, "repo.git");
    }

    #[test]
    fn credentials_are_stripped_from_base_url() {
        let repo = resolved("https://user:secret@host.example.com/", "PROJ", "repo");
        assert_eq!(base_url_str(&repo), "https://host.example.com");
        assert!(!repo.base_url.as_str().contains("secret"));
    }

    #[test]
    fn query_and_fragment_are_dropped_from_base_url() {
        let repo = resolved(
            "https://host.example.com/bitbucket?x=1#frag",
            "PROJ",
            "repo",
        );
        assert_eq!(base_url_str(&repo), "https://host.example.com/bitbucket");
    }

    // --- Configuration resolution: rejection ---

    #[test]
    fn non_http_scheme_is_rejected() {
        for base in ["ssh://git@host.example.com:7999", "ftp://host.example.com"] {
            let err = resolve_repository_input(BitbucketConfigInput {
                cli_base_url: Some(base.to_string()),
                cli_project: Some("PROJ".to_string()),
                cli_repo: Some("repo".to_string()),
                ..Default::default()
            })
            .unwrap_err();
            match err {
                PrProviderError::Configuration(msg) => {
                    assert!(msg.contains("scheme"), "unexpected message: {msg}")
                }
                other => panic!("expected Configuration error, got {other}"),
            }
        }
    }

    #[test]
    fn malformed_base_url_is_rejected() {
        let err = resolve_repository_input(BitbucketConfigInput {
            cli_base_url: Some("not a url".to_string()),
            cli_project: Some("PROJ".to_string()),
            cli_repo: Some("repo".to_string()),
            ..Default::default()
        })
        .unwrap_err();
        assert!(matches!(err, PrProviderError::Configuration(_)));
    }

    #[test]
    fn missing_base_url_error_names_the_flag() {
        let err = resolve_repository_input(BitbucketConfigInput {
            cli_project: Some("PROJ".to_string()),
            cli_repo: Some("repo".to_string()),
            ..Default::default()
        })
        .unwrap_err();
        match err {
            PrProviderError::Configuration(msg) => {
                assert!(msg.contains("base URL"), "unexpected message: {msg}");
                assert!(
                    msg.contains("--bitbucket-base-url"),
                    "unexpected message: {msg}"
                );
            }
            other => panic!("expected Configuration error, got {other}"),
        }
    }

    #[test]
    fn missing_project_error_names_the_flag() {
        let err = resolve_repository_input(BitbucketConfigInput {
            cli_base_url: Some("https://host.example.com".to_string()),
            cli_repo: Some("repo".to_string()),
            ..Default::default()
        })
        .unwrap_err();
        match err {
            PrProviderError::Configuration(msg) => {
                assert!(msg.contains("project key"), "unexpected message: {msg}");
                assert!(
                    msg.contains("--bitbucket-project"),
                    "unexpected message: {msg}"
                );
            }
            other => panic!("expected Configuration error, got {other}"),
        }
    }

    #[test]
    fn missing_repo_error_names_the_flag() {
        let err = resolve_repository_input(BitbucketConfigInput {
            cli_base_url: Some("https://host.example.com".to_string()),
            cli_project: Some("PROJ".to_string()),
            ..Default::default()
        })
        .unwrap_err();
        match err {
            PrProviderError::Configuration(msg) => {
                assert!(msg.contains("repository slug"), "unexpected message: {msg}");
                assert!(
                    msg.contains("--bitbucket-repo"),
                    "unexpected message: {msg}"
                );
            }
            other => panic!("expected Configuration error, got {other}"),
        }
    }

    #[test]
    fn whitespace_only_explicit_component_is_rejected() {
        let err = resolve_repository_input(BitbucketConfigInput {
            cli_base_url: Some("https://host.example.com".to_string()),
            cli_project: Some("   ".to_string()),
            cli_repo: Some("repo".to_string()),
            ..Default::default()
        })
        .unwrap_err();
        assert!(matches!(err, PrProviderError::Configuration(_)));
    }

    #[test]
    fn blank_derived_component_is_treated_as_missing() {
        let err = resolve_repository_input(BitbucketConfigInput {
            cli_base_url: Some("https://host.example.com".to_string()),
            cli_repo: Some("repo".to_string()),
            derived_project: Some("  ".to_string()),
            ..Default::default()
        })
        .unwrap_err();
        match err {
            PrProviderError::Configuration(msg) => {
                assert!(
                    msg.contains("--bitbucket-project"),
                    "unexpected message: {msg}"
                );
            }
            other => panic!("expected Configuration error, got {other}"),
        }
    }

    #[test]
    fn repo_slug_that_is_only_git_suffix_is_rejected() {
        let err = resolve_repository_input(BitbucketConfigInput {
            cli_base_url: Some("https://host.example.com".to_string()),
            cli_project: Some("PROJ".to_string()),
            cli_repo: Some(".git".to_string()),
            ..Default::default()
        })
        .unwrap_err();
        assert!(matches!(err, PrProviderError::Configuration(_)));
    }

    // --- Provider identity ---

    #[test]
    fn cache_key_matches_the_documented_example_exactly() {
        // Derived values: lowercase project key gets canonicalized.
        let repo = resolve_repository_input(BitbucketConfigInput {
            derived_base_url: Some("https://bitbucket.example.com".to_string()),
            derived_project: Some("proj".to_string()),
            derived_repo: Some("demo-repo.git".to_string()),
            ..Default::default()
        })
        .unwrap();
        let provider = BitbucketProvider::new(repo, FAKE_TOKEN.to_string()).unwrap();
        assert_eq!(
            provider.cache_key(),
            "bitbucket-dc|https://bitbucket.example.com|PROJ|demo-repo"
        );
    }

    #[test]
    fn cache_key_uppercases_an_explicit_lowercase_project_key() {
        let provider = BitbucketProvider::new(
            resolved("https://host.example.com/bitbucket", "proj", "repo"),
            FAKE_TOKEN.to_string(),
        )
        .unwrap();
        assert_eq!(
            provider.cache_key(),
            "bitbucket-dc|https://host.example.com/bitbucket|PROJ|repo"
        );
    }

    #[test]
    fn provider_kind_and_concurrency() {
        let provider =
            BitbucketProvider::new(example_repository(), FAKE_TOKEN.to_string()).unwrap();
        assert_eq!(provider.kind(), PrProviderKind::BitbucketDataCenter);
        assert_eq!(provider.kind().label(), "Bitbucket");
        assert_eq!(provider.max_concurrency(), 8);
    }

    #[test]
    fn empty_token_is_rejected() {
        // `unwrap_err()` needs `T: Debug`, which the provider deliberately
        // does not implement (spec 14), so match instead.
        match BitbucketProvider::new(example_repository(), "   ".to_string()) {
            Err(PrProviderError::Configuration(_)) => {}
            Err(other) => panic!("expected Configuration error, got {other}"),
            Ok(_) => panic!("expected an error for an empty token"),
        }
    }

    // --- URL construction ---

    #[test]
    fn repo_api_url_includes_context_path() {
        let repo = resolved("https://host.example.com/bitbucket", "PROJ", "demo-repo");
        assert_eq!(
            repo_api_url(&repo).as_str(),
            "https://host.example.com/bitbucket/rest/api/latest/projects/PROJ/repos/demo-repo"
        );
    }

    #[test]
    fn pr_lookup_url_percent_encodes_branch_ref() {
        let repo = example_repository();

        let url = pr_lookup_url(&repo, "feature/x");
        assert_eq!(
            url.path(),
            "/rest/api/latest/projects/PROJ/repos/demo-repo/pull-requests"
        );
        let query = url.query().unwrap();
        assert!(query.contains("direction=OUTGOING"), "query: {query}");
        assert!(
            query.contains("at=refs%2Fheads%2Ffeature%2Fx"),
            "query: {query}"
        );
        assert!(query.contains("state=ALL"), "query: {query}");
        assert!(query.contains("order=NEWEST"), "query: {query}");
        assert!(query.contains("limit=1"), "query: {query}");

        // Spaces, plus signs, and Unicode must be encoded, never raw.
        let query = pr_lookup_url(&repo, "my branch")
            .query()
            .unwrap()
            .to_string();
        assert!(
            query.contains("at=refs%2Fheads%2Fmy+branch"),
            "query: {query}"
        );

        let query = pr_lookup_url(&repo, "a+b").query().unwrap().to_string();
        assert!(query.contains("at=refs%2Fheads%2Fa%2Bb"), "query: {query}");

        let query = pr_lookup_url(&repo, "функция").query().unwrap().to_string();
        assert!(!query.contains("функция"), "query: {query}");
        assert!(query.contains("at=refs%2Fheads%2F%D1%84"), "query: {query}");
    }

    #[test]
    fn fallback_pr_url_is_built_from_repository_identity() {
        assert_eq!(
            fallback_pr_url(&example_repository(), 42),
            "https://bitbucket.example.com/projects/PROJ/repos/demo-repo/pull-requests/42/overview"
        );
    }

    // --- State mapping (spec 7.6) ---

    #[test]
    fn known_states_map_to_shared_states() {
        assert_eq!(map_pr_state("OPEN").unwrap(), PrState::Open);
        assert_eq!(map_pr_state("MERGED").unwrap(), PrState::Merged);
        assert_eq!(map_pr_state("DECLINED").unwrap(), PrState::Closed);
    }

    #[test]
    fn state_mapping_is_ascii_case_insensitive() {
        assert_eq!(map_pr_state("open").unwrap(), PrState::Open);
        assert_eq!(map_pr_state("Merged").unwrap(), PrState::Merged);
        assert_eq!(map_pr_state("dEcLiNeD").unwrap(), PrState::Closed);
    }

    #[test]
    fn unknown_state_is_rejected_not_mapped_to_closed() {
        let err = map_pr_state("SUPERSEDED").unwrap_err();
        match err {
            PrProviderError::InvalidResponse(msg) => {
                assert_eq!(msg, "unsupported pull request state: SUPERSEDED");
            }
            other => panic!("expected InvalidResponse, got {other}"),
        }
    }

    // --- Page mapping (spec 7.5, DTO tests 13.2) ---

    fn page_json(
        id: u64,
        title: &str,
        state: &str,
        ref_id: &str,
        slug: &str,
        self_hrefs: &[&str],
    ) -> String {
        let links: Vec<serde_json::Value> =
            self_hrefs.iter().map(|h| json!({ "href": h })).collect();
        json!({
            "size": 1,
            "limit": 1,
            "isLastPage": true,
            "values": [{
                "id": id,
                "title": title,
                "state": state,
                "fromRef": {
                    "id": ref_id,
                    "displayId": ref_id.trim_start_matches("refs/heads/"),
                    "repository": { "slug": slug, "project": { "key": "PROJ" } }
                },
                "links": { "self": links },
                "unknownField": "ignored"
            }]
        })
        .to_string()
    }

    fn parse_page(json: &str) -> Page<BitbucketPullRequest> {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn pagination_fields_are_parsed() {
        let page: Page<BitbucketPullRequest> = serde_json::from_str(
            &json!({
                "size": 0,
                "limit": 25,
                "isLastPage": false,
                "nextPageStart": 25,
                "values": []
            })
            .to_string(),
        )
        .unwrap();
        assert_eq!(page.size, 0);
        assert_eq!(page.limit, 25);
        assert!(!page.is_last_page);
        assert_eq!(page.next_page_start, Some(25));
    }

    #[test]
    fn empty_page_is_a_successful_no_pr_result() {
        let page: Page<BitbucketPullRequest> = serde_json::from_str(
            &json!({ "size": 0, "limit": 1, "isLastPage": true, "values": [] }).to_string(),
        )
        .unwrap();
        let result = pr_from_page(page, &example_repository(), "feature/x").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn pr_id_maps_to_pr_info_number() {
        let page = parse_page(&page_json(
            4242,
            "A title",
            "OPEN",
            "refs/heads/feature/x",
            "demo-repo",
            &["https://bitbucket.example.com/pr/4242"],
        ));
        let pr = pr_from_page(page, &example_repository(), "feature/x")
            .unwrap()
            .unwrap();
        assert_eq!(pr.number, 4242);
        assert_eq!(pr.state, PrState::Open);
    }

    #[test]
    fn title_with_quotes_backslashes_unicode_and_newlines_is_copied_verbatim() {
        let title = "Fix \"quoted\" \\path\\ — тест 🚀\nsecond line";
        let page = parse_page(&page_json(
            1,
            title,
            "MERGED",
            "refs/heads/feature/x",
            "demo-repo",
            &["https://example.com/pr/1"],
        ));
        let pr = pr_from_page(page, &example_repository(), "feature/x")
            .unwrap()
            .unwrap();
        assert_eq!(pr.title, title);
    }

    #[test]
    fn first_non_empty_self_link_is_selected() {
        let page = parse_page(&page_json(
            7,
            "T",
            "OPEN",
            "refs/heads/feature/x",
            "demo-repo",
            &[
                "",
                "  ",
                "https://bitbucket.example.com/pr/7",
                "https://other.example.com",
            ],
        ));
        let pr = pr_from_page(page, &example_repository(), "feature/x")
            .unwrap()
            .unwrap();
        assert_eq!(pr.url, "https://bitbucket.example.com/pr/7");
    }

    #[test]
    fn missing_self_link_uses_fallback_url() {
        let page = parse_page(&page_json(
            9,
            "T",
            "DECLINED",
            "refs/heads/feature/x",
            "demo-repo",
            &[],
        ));
        let pr = pr_from_page(page, &example_repository(), "feature/x")
            .unwrap()
            .unwrap();
        assert_eq!(
            pr.url,
            "https://bitbucket.example.com/projects/PROJ/repos/demo-repo/pull-requests/9/overview"
        );
        assert_eq!(pr.state, PrState::Closed);
    }

    #[test]
    fn from_ref_must_match_requested_branch_exactly() {
        // Same prefix, different branch — must not be accepted.
        let page = parse_page(&page_json(
            1,
            "T",
            "OPEN",
            "refs/heads/feature/x-other",
            "demo-repo",
            &["https://example.com/pr/1"],
        ));
        let err = pr_from_page(page, &example_repository(), "feature/x").unwrap_err();
        assert!(matches!(err, PrProviderError::InvalidResponse(_)));
    }

    #[test]
    fn pr_from_wrong_repository_is_rejected() {
        let page = parse_page(&page_json(
            1,
            "T",
            "OPEN",
            "refs/heads/feature/x",
            "other-repo",
            &["https://example.com/pr/1"],
        ));
        let err = pr_from_page(page, &example_repository(), "feature/x").unwrap_err();
        assert!(matches!(err, PrProviderError::InvalidResponse(_)));
    }

    #[test]
    fn repository_slug_match_is_case_insensitive() {
        let page = parse_page(&page_json(
            1,
            "T",
            "OPEN",
            "refs/heads/feature/x",
            "Demo-Repo",
            &["https://example.com/pr/1"],
        ));
        assert!(pr_from_page(page, &example_repository(), "feature/x")
            .unwrap()
            .is_some());
    }

    #[test]
    fn unknown_state_in_page_is_rejected() {
        let page = parse_page(&page_json(
            1,
            "T",
            "SUPERSEDED",
            "refs/heads/feature/x",
            "demo-repo",
            &["https://example.com/pr/1"],
        ));
        let err = pr_from_page(page, &example_repository(), "feature/x").unwrap_err();
        match err {
            PrProviderError::InvalidResponse(msg) => {
                assert!(msg.contains("unsupported pull request state"));
            }
            other => panic!("expected InvalidResponse, got {other}"),
        }
    }

    // --- Error-body sanitization ---

    #[test]
    fn error_body_with_authorization_text_is_redacted() {
        let msg = sanitize_error_body(b"Authorization: Bearer abc123", false);
        assert!(!msg.contains("abc123"));
        assert!(msg.contains("redacted"));
    }

    // --- HTTP mock tests (wiremock, no real network; spec 13.3) ---

    mod http {
        use super::*;
        use wiremock::matchers::{header, method, path, query_param};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        const USER_AGENT: &str = concat!("worktrack/", env!("CARGO_PKG_VERSION"));

        fn provider_for(server: &MockServer) -> BitbucketProvider {
            BitbucketProvider::new(
                resolved(&server.uri(), "PROJ", "demo-repo"),
                FAKE_TOKEN.to_string(),
            )
            .unwrap()
        }

        fn repo_metadata_json() -> serde_json::Value {
            json!({
                "slug": "demo-repo",
                "id": 1,
                "name": "Crash Games",
                "state": "AVAILABLE",
                "project": { "key": "PROJ", "name": "PROJ Project" }
            })
        }

        fn pr_page_json(
            id: u64,
            state: &str,
            branch: &str,
            self_href: Option<&str>,
        ) -> serde_json::Value {
            let links = match self_href {
                Some(href) => json!({ "self": [{ "href": href }] }),
                None => json!({ "self": [] }),
            };
            json!({
                "size": 1,
                "limit": 1,
                "isLastPage": true,
                "values": [{
                    "id": id,
                    "title": "Test PR",
                    "state": state,
                    "fromRef": {
                        "id": format!("refs/heads/{branch}"),
                        "displayId": branch,
                        "repository": { "slug": "demo-repo", "project": { "key": "PROJ" } }
                    },
                    "links": links
                }]
            })
        }

        fn empty_page_json() -> serde_json::Value {
            json!({ "size": 0, "limit": 1, "isLastPage": true, "values": [] })
        }

        #[tokio::test]
        async fn validate_sends_expected_request_and_succeeds() {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path("/rest/api/latest/projects/PROJ/repos/demo-repo"))
                .and(header(
                    "Authorization",
                    format!("Bearer {FAKE_TOKEN}").as_str(),
                ))
                .and(header("Accept", "application/json"))
                .and(header("User-Agent", USER_AGENT))
                .respond_with(ResponseTemplate::new(200).set_body_json(repo_metadata_json()))
                .expect(1)
                .mount(&server)
                .await;

            provider_for(&server).validate().await.unwrap();
        }

        #[tokio::test]
        async fn validate_rejects_mismatched_repository_slug() {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                    "slug": "some-other-repo",
                    "project": { "key": "PROJ" }
                })))
                .mount(&server)
                .await;

            let err = provider_for(&server).validate().await.unwrap_err();
            assert!(matches!(err, PrProviderError::InvalidResponse(_)));
        }

        #[tokio::test]
        async fn validate_rejects_mismatched_project_key() {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                    "slug": "demo-repo",
                    "project": { "key": "OTHER" }
                })))
                .mount(&server)
                .await;

            let err = provider_for(&server).validate().await.unwrap_err();
            assert!(matches!(err, PrProviderError::InvalidResponse(_)));
        }

        #[tokio::test]
        async fn pr_lookup_sends_expected_request_with_encoded_slashed_branch() {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path(
                    "/rest/api/latest/projects/PROJ/repos/demo-repo/pull-requests",
                ))
                .and(query_param("direction", "OUTGOING"))
                .and(query_param("at", "refs/heads/feature/x"))
                .and(query_param("state", "ALL"))
                .and(query_param("order", "NEWEST"))
                .and(query_param("limit", "1"))
                .and(header(
                    "Authorization",
                    format!("Bearer {FAKE_TOKEN}").as_str(),
                ))
                .and(header("Accept", "application/json"))
                .and(header("User-Agent", USER_AGENT))
                .respond_with(ResponseTemplate::new(200).set_body_json(pr_page_json(
                    12,
                    "OPEN",
                    "feature/x",
                    Some("https://example.com/pr/12"),
                )))
                .expect(1)
                .mount(&server)
                .await;

            let pr = provider_for(&server)
                .get_pr_for_branch("feature/x")
                .await
                .unwrap()
                .unwrap();
            assert_eq!(pr.number, 12);
            assert_eq!(pr.state, PrState::Open);
            assert_eq!(pr.title, "Test PR");
            assert_eq!(pr.url, "https://example.com/pr/12");
        }

        #[tokio::test]
        async fn merged_pr_maps_to_merged() {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .respond_with(ResponseTemplate::new(200).set_body_json(pr_page_json(
                    2,
                    "MERGED",
                    "main",
                    Some("https://example.com/pr/2"),
                )))
                .mount(&server)
                .await;

            let pr = provider_for(&server)
                .get_pr_for_branch("main")
                .await
                .unwrap()
                .unwrap();
            assert_eq!(pr.state, PrState::Merged);
        }

        #[tokio::test]
        async fn declined_pr_maps_to_closed() {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .respond_with(ResponseTemplate::new(200).set_body_json(pr_page_json(
                    3,
                    "DECLINED",
                    "main",
                    Some("https://example.com/pr/3"),
                )))
                .mount(&server)
                .await;

            let pr = provider_for(&server)
                .get_pr_for_branch("main")
                .await
                .unwrap()
                .unwrap();
            assert_eq!(pr.state, PrState::Closed);
        }

        #[tokio::test]
        async fn empty_values_is_ok_none() {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .respond_with(ResponseTemplate::new(200).set_body_json(empty_page_json()))
                .mount(&server)
                .await;

            let result = provider_for(&server)
                .get_pr_for_branch("main")
                .await
                .unwrap();
            assert!(result.is_none());
        }

        #[tokio::test]
        async fn missing_self_url_uses_fallback_built_from_base_url() {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .respond_with(
                    ResponseTemplate::new(200)
                        .set_body_json(pr_page_json(77, "OPEN", "main", None)),
                )
                .mount(&server)
                .await;

            let pr = provider_for(&server)
                .get_pr_for_branch("main")
                .await
                .unwrap()
                .unwrap();
            assert_eq!(
                pr.url,
                format!(
                    "{}/projects/PROJ/repos/demo-repo/pull-requests/77/overview",
                    server.uri()
                )
            );
        }

        #[tokio::test]
        async fn login_redirect_is_a_configuration_error_and_not_followed() {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .respond_with(ResponseTemplate::new(302).insert_header(
                    "Location",
                    "https://sso.example.com/login?next=%2Fdashboard&session=fake-secret",
                ))
                .expect(1) // the redirect target is never requested
                .mount(&server)
                .await;

            let err = provider_for(&server).validate().await.unwrap_err();
            match err {
                PrProviderError::Configuration(msg) => {
                    assert!(msg.contains("sso.example.com/login"), "message: {msg}");
                    // Query string (and anything in it) must be sanitized away.
                    assert!(!msg.contains("fake-secret"), "message: {msg}");
                    assert!(!msg.contains("next="), "message: {msg}");
                }
                other => panic!("expected Configuration error, got {other}"),
            }
        }

        #[tokio::test]
        async fn http_401_is_authentication_error() {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .respond_with(ResponseTemplate::new(401))
                .expect(1) // never retried
                .mount(&server)
                .await;

            let err = provider_for(&server).validate().await.unwrap_err();
            assert!(matches!(err, PrProviderError::Authentication(_)));
        }

        #[tokio::test]
        async fn http_403_is_forbidden_error() {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .respond_with(ResponseTemplate::new(403))
                .expect(1) // never retried
                .mount(&server)
                .await;

            let err = provider_for(&server).validate().await.unwrap_err();
            assert!(matches!(err, PrProviderError::Forbidden(_)));
        }

        #[tokio::test]
        async fn http_404_is_not_found_error() {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .respond_with(ResponseTemplate::new(404))
                .expect(1) // never retried
                .mount(&server)
                .await;

            let err = provider_for(&server).validate().await.unwrap_err();
            assert!(matches!(err, PrProviderError::NotFound(_)));
        }

        #[tokio::test]
        async fn http_429_with_retry_after_is_rate_limited_after_retries() {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "1"))
                .expect(3) // all three attempts are used
                .mount(&server)
                .await;

            let err = provider_for(&server).validate().await.unwrap_err();
            match err {
                PrProviderError::RateLimited { retry_after } => {
                    assert_eq!(retry_after, Some(Duration::from_secs(1)));
                }
                other => panic!("expected RateLimited, got {other}"),
            }
        }

        #[tokio::test]
        async fn http_429_without_retry_after_is_rate_limited_after_retries() {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .respond_with(ResponseTemplate::new(429))
                .expect(3)
                .mount(&server)
                .await;

            let err = provider_for(&server).validate().await.unwrap_err();
            match err {
                PrProviderError::RateLimited { retry_after } => assert_eq!(retry_after, None),
                other => panic!("expected RateLimited, got {other}"),
            }
        }

        #[tokio::test]
        async fn http_503_then_success_is_retried_transparently() {
            let server = MockServer::start().await;
            // First attempt gets a 503, the retry gets the real page.
            Mock::given(method("GET"))
                .respond_with(ResponseTemplate::new(503))
                .up_to_n_times(1)
                .expect(1)
                .mount(&server)
                .await;
            Mock::given(method("GET"))
                .respond_with(ResponseTemplate::new(200).set_body_json(pr_page_json(
                    5,
                    "OPEN",
                    "main",
                    Some("https://example.com/pr/5"),
                )))
                .expect(1)
                .mount(&server)
                .await;

            let pr = provider_for(&server)
                .get_pr_for_branch("main")
                .await
                .unwrap()
                .unwrap();
            assert_eq!(pr.number, 5);
        }

        #[tokio::test]
        async fn persistent_503_exhausts_retries() {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .respond_with(ResponseTemplate::new(503))
                .expect(3) // exactly three total attempts
                .mount(&server)
                .await;

            let err = provider_for(&server).validate().await.unwrap_err();
            assert!(matches!(err, PrProviderError::ProviderUnavailable(_)));
        }

        #[tokio::test]
        async fn malformed_json_is_invalid_response_and_not_retried() {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .respond_with(
                    ResponseTemplate::new(200)
                        .set_body_raw("{ this is not json", "application/json"),
                )
                .expect(1)
                .mount(&server)
                .await;

            let err = provider_for(&server).validate().await.unwrap_err();
            match err {
                PrProviderError::InvalidResponse(msg) => {
                    assert!(msg.contains("malformed JSON"), "message: {msg}");
                }
                other => panic!("expected InvalidResponse, got {other}"),
            }
        }

        #[tokio::test]
        async fn html_login_page_with_200_is_invalid_response() {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .respond_with(ResponseTemplate::new(200).set_body_raw(
                    "<html><body><form action=\"/login\">Sign in</form></body></html>",
                    "text/html;charset=utf-8",
                ))
                .mount(&server)
                .await;

            let err = provider_for(&server).validate().await.unwrap_err();
            match err {
                PrProviderError::InvalidResponse(msg) => {
                    assert!(msg.contains("text/html"), "message: {msg}");
                    assert!(msg.contains("sign-in"), "message: {msg}");
                }
                other => panic!("expected InvalidResponse, got {other}"),
            }
        }

        #[tokio::test]
        async fn missing_required_dto_field_is_invalid_response() {
            let server = MockServer::start().await;
            // A PR without an `id` field.
            Mock::given(method("GET"))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                    "size": 1,
                    "limit": 1,
                    "isLastPage": true,
                    "values": [{
                        "title": "No id here",
                        "state": "OPEN",
                        "fromRef": {
                            "id": "refs/heads/main",
                            "displayId": "main",
                            "repository": { "slug": "demo-repo", "project": { "key": "PROJ" } }
                        },
                        "links": { "self": [] }
                    }]
                })))
                .mount(&server)
                .await;

            let err = provider_for(&server)
                .get_pr_for_branch("main")
                .await
                .unwrap_err();
            match err {
                PrProviderError::InvalidResponse(msg) => {
                    assert!(msg.contains("missing field"), "message: {msg}");
                }
                other => panic!("expected InvalidResponse, got {other}"),
            }
        }

        #[tokio::test]
        async fn slow_server_times_out_as_transport_error() {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .respond_with(
                    ResponseTemplate::new(200)
                        .set_body_json(repo_metadata_json())
                        .set_delay(Duration::from_secs(5)),
                )
                .mount(&server)
                .await;

            // Shortened test-only total timeout so this doesn't take 15 s.
            let provider = BitbucketProvider::with_timeouts(
                resolved(&server.uri(), "PROJ", "demo-repo"),
                FAKE_TOKEN.to_string(),
                Duration::from_secs(5),
                Duration::from_millis(300),
            )
            .unwrap();

            let start = std::time::Instant::now();
            let err = provider.validate().await.unwrap_err();
            // Timeouts are not retried, so this returns promptly.
            assert!(start.elapsed() < Duration::from_secs(3));
            assert!(matches!(err, PrProviderError::Transport(_)));
        }

        #[tokio::test]
        async fn oversized_error_body_is_truncated() {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .respond_with(ResponseTemplate::new(400).set_body_string("x".repeat(10 * 1024)))
                .expect(1) // 400 is never retried
                .mount(&server)
                .await;

            let err = provider_for(&server).validate().await.unwrap_err();
            match err {
                PrProviderError::InvalidResponse(msg) => {
                    assert!(msg.contains("truncated"), "message: {msg}");
                    assert!(
                        msg.len() < ERROR_BODY_CAP + 200,
                        "message length {} exceeds the cap",
                        msg.len()
                    );
                }
                other => panic!("expected InvalidResponse, got {other}"),
            }
        }
    }

    // --- worktrack local change: resolve_repository takes the repo path ---

    fn git_repo_with_origin(origin: Option<&str>) -> tempfile::TempDir {
        let dir = tempfile::TempDir::new().unwrap();
        let run = |args: &[&str]| {
            let ok = std::process::Command::new("git")
                .arg("-C")
                .arg(dir.path())
                .args(args)
                .output()
                .unwrap()
                .status
                .success();
            assert!(ok, "git {args:?} failed");
        };
        run(&["init", "-q"]);
        if let Some(url) = origin {
            run(&["remote", "add", "origin", url]);
        }
        dir
    }

    #[test]
    fn resolve_repository_derives_from_the_given_repo_origin() {
        let dir = git_repo_with_origin(Some("https://bb.example.com/scm/proj/demo-repo.git"));
        let repo = resolve_repository(dir.path(), BitbucketConfigInput::default()).unwrap();
        assert_eq!(base_url_str(&repo), "https://bb.example.com");
        assert_eq!(repo.project_key, "PROJ");
        assert_eq!(repo.repository_slug, "demo-repo");
    }

    #[test]
    fn resolve_repository_lets_explicit_values_override_origin() {
        let dir = git_repo_with_origin(Some("https://bb.example.com/scm/proj/demo-repo.git"));
        let repo = resolve_repository(
            dir.path(),
            BitbucketConfigInput {
                cli_project: Some("LEG".to_string()),
                cli_repo: Some("api".to_string()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(base_url_str(&repo), "https://bb.example.com");
        assert_eq!(repo.project_key, "LEG");
        assert_eq!(repo.repository_slug, "api");
    }

    #[test]
    fn resolve_repository_without_origin_names_the_missing_value() {
        let dir = git_repo_with_origin(None);
        let err = resolve_repository(dir.path(), BitbucketConfigInput::default()).unwrap_err();
        assert!(matches!(err, PrProviderError::Configuration(_)));
    }

    #[test]
    fn resolve_repository_skips_origin_when_everything_is_explicit() {
        // Not a git repository at all: origin must not be read.
        let dir = tempfile::TempDir::new().unwrap();
        let repo = resolve_repository(
            dir.path(),
            BitbucketConfigInput {
                cli_base_url: Some("https://host.example.com".to_string()),
                cli_project: Some("PROJ".to_string()),
                cli_repo: Some("repo".to_string()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(repo.repository_slug, "repo");
    }

    // --- worktrack milestone 2: host client ---

    mod host {
        use super::*;
        use wiremock::matchers::{header, method, path, query_param};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        const USER_AGENT: &str = concat!("worktrack/", env!("CARGO_PKG_VERSION"));

        fn client_for(server: &MockServer) -> BitbucketHostClient {
            BitbucketHostClient::new(&server.uri(), FAKE_TOKEN).unwrap()
        }

        fn pr_json(id: u64, state: &str, branch: &str, from_slug: &str) -> serde_json::Value {
            json!({
                "id": id,
                "title": "T",
                "state": state,
                "fromRef": {
                    "id": format!("refs/heads/{branch}"),
                    "displayId": branch,
                    "latestCommit": "abc123",
                    "repository": { "slug": from_slug, "project": { "key": "PROJ" } }
                },
                "toRef": {
                    "id": "refs/heads/main",
                    "displayId": "main",
                    "repository": { "slug": "demo-repo", "project": { "key": "PROJ" } }
                },
                "author": { "user": { "name": "jdoe", "slug": "jdoe", "displayName": "J Doe" } },
                "reviewers": [
                    { "user": { "name": "rev", "displayName": "Rev Iewer" }, "status": "NEEDS_WORK" }
                ],
                "properties": { "openTaskCount": 2, "commentCount": 5 },
                "updatedDate": 1_700_000_000_000_i64,
                "links": { "self": [] }
            })
        }

        #[test]
        fn new_urls_use_api_1_0_and_encode_values() {
            let repo = example_repository();
            let url = latest_pr_url(&repo, "feature/x y");
            assert_eq!(
                url.path(),
                "/rest/api/1.0/projects/PROJ/repos/demo-repo/pull-requests"
            );
            let q = url.query().unwrap();
            assert!(q.contains("at=refs%2Fheads%2Ffeature%2Fx+y"), "{q}");
            assert!(q.contains("direction=OUTGOING") && q.contains("state=ALL"));
            assert!(q.contains("order=NEWEST") && q.contains("limit=1"));

            let url = dashboard_url(&repo.base_url, 100);
            assert_eq!(url.path(), "/rest/api/1.0/dashboard/pull-requests");
            assert_eq!(
                url.query(),
                Some("role=AUTHOR&state=OPEN&limit=100&start=100")
            );

            let url = build_status_url(&repo.base_url, "deadbeef", 0);
            assert_eq!(url.path(), "/rest/build-status/1.0/commits/deadbeef");
        }

        #[test]
        fn empty_token_is_rejected_by_the_host_client() {
            assert!(matches!(
                BitbucketHostClient::new("https://h.example.com", " "),
                Err(PrProviderError::Configuration(_))
            ));
        }

        #[tokio::test]
        async fn dashboard_page_sends_headers_and_maps_details() {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path("/rest/api/1.0/dashboard/pull-requests"))
                .and(query_param("role", "AUTHOR"))
                .and(query_param("state", "OPEN"))
                .and(query_param("limit", "100"))
                .and(query_param("start", "0"))
                .and(header(
                    "Authorization",
                    format!("Bearer {FAKE_TOKEN}").as_str(),
                ))
                .and(header("User-Agent", USER_AGENT))
                .respond_with(
                    ResponseTemplate::new(200)
                        .insert_header("X-AUSERNAME", "jdoe")
                        .set_body_json(json!({
                            "isLastPage": false,
                            "nextPageStart": 100,
                            "values": [pr_json(7, "OPEN", "feature/x", "demo-repo")]
                        })),
                )
                .expect(1)
                .mount(&server)
                .await;

            let page = client_for(&server).dashboard_page(0).await.unwrap();
            assert_eq!(page.username.as_deref(), Some("jdoe"));
            assert_eq!(page.next_start, Some(100));
            let pr = &page.values[0];
            assert_eq!(pr.id, 7);
            assert_eq!(pr.state, PrState::Open);
            assert_eq!(pr.from_branch, "feature/x");
            assert_eq!(pr.from_latest_commit.as_deref(), Some("abc123"));
            assert_eq!(pr.author_slug, "jdoe");
            assert_eq!(pr.open_task_count, 2);
            assert_eq!(pr.comment_count, 5);
            assert_eq!(pr.reviewers[0].slug, "rev");
            assert_eq!(pr.reviewers[0].display_name, "Rev Iewer");
            assert_eq!(pr.reviewers[0].status, "NEEDS_WORK");
            assert_eq!(pr.updated_date_ms, Some(1_700_000_000_000));
            assert!(!pr.is_from_fork());
            assert_eq!(
                pr.url,
                format!(
                    "{}/projects/PROJ/repos/demo-repo/pull-requests/7/overview",
                    server.uri()
                )
            );
        }

        #[tokio::test]
        async fn rate_limit_is_returned_at_once_not_retried() {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "7"))
                .expect(1)
                .mount(&server)
                .await;
            let err = client_for(&server).dashboard_page(0).await.unwrap_err();
            match err {
                PrProviderError::RateLimited { retry_after } => {
                    assert_eq!(retry_after, Some(Duration::from_secs(7)))
                }
                other => panic!("expected RateLimited, got {other}"),
            }
        }

        #[tokio::test]
        async fn latest_pr_for_branch_rejects_another_source_repository() {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                    "values": [pr_json(1, "MERGED", "feature/x", "a-fork")]
                })))
                .mount(&server)
                .await;
            let repo = resolved(&server.uri(), "PROJ", "demo-repo");
            let err = client_for(&server)
                .latest_pull_request_for_branch(&repo, "feature/x")
                .await
                .unwrap_err();
            assert!(matches!(err, PrProviderError::InvalidResponse(_)));
        }

        #[tokio::test]
        async fn current_user_reads_the_header() {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path("/rest/api/1.0/application-properties"))
                .respond_with(
                    ResponseTemplate::new(200)
                        .insert_header("X-AUSERNAME", "jdoe")
                        .set_body_json(json!({ "version": "8.9.0" })),
                )
                .mount(&server)
                .await;
            assert_eq!(
                client_for(&server).current_user().await.unwrap().as_deref(),
                Some("jdoe")
            );
        }
    }
}
