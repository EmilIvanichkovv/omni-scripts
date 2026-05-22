/// SQLite-backed cache for GitHub PR data.
///
/// This module is the single owner of all cache I/O. No other module reads from
/// or writes to the SQLite file directly.
use crate::git::{PrInfo, PrState};
use color_eyre::Result;
use rusqlite::{params, Connection};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

// ---------------------------------------------------------------------------
// SQL constants
// ---------------------------------------------------------------------------

const BOOTSTRAP_SQL: &str = "
CREATE TABLE IF NOT EXISTS schema_migrations (
    version    INTEGER PRIMARY KEY,
    applied_at INTEGER NOT NULL
);
";

const MIGRATION_V1_SQL: &str = "
CREATE TABLE IF NOT EXISTS repositories (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    slug       TEXT    NOT NULL UNIQUE,
    created_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS cached_prs (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    repository_id INTEGER NOT NULL REFERENCES repositories(id) ON DELETE CASCADE,
    branch_name   TEXT    NOT NULL,
    pr_number     INTEGER,
    pr_state      TEXT,
    pr_title      TEXT,
    pr_url        TEXT,
    cached_at     INTEGER NOT NULL,
    UNIQUE(repository_id, branch_name)
);

CREATE INDEX IF NOT EXISTS idx_cached_prs_cached_at ON cached_prs(cached_at);
";

const CURRENT_SCHEMA_VERSION: i64 = 1;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Statistics about the cache for the current session.
#[derive(Debug, Clone, Default)]
pub struct CacheStats {
    /// Number of branches served from cache this session.
    pub hits: usize,
    /// Number of branches that required a fresh API call.
    pub misses: usize,
    /// Number of new entries written to the database this session.
    pub writes: usize,
    /// Number of entries in the database for this repository.
    pub total_entries: usize,
    /// Unix timestamp of the oldest cached entry for this repository.
    pub oldest_entry_ts: Option<i64>,
}

pub struct PrCache {
    conn: Connection,
    repo: String,
    ttl: Duration,
    stats: CacheStats,
}

impl PrCache {
    // -----------------------------------------------------------------------
    // Construction
    // -----------------------------------------------------------------------

    /// Open (or create) the on-disk cache database at the XDG cache path.
    ///
    /// Priority:
    /// 1. `$XDG_CACHE_HOME/omni-scripts/pr-cache.db`
    /// 2. `$HOME/.cache/omni-scripts/pr-cache.db`
    pub fn open(repository: &str, ttl: Duration) -> Result<Self> {
        let cache_dir = dirs::cache_dir()
            .ok_or_else(|| color_eyre::eyre::eyre!("Cannot determine cache directory"))?
            .join("omni-scripts");

        std::fs::create_dir_all(&cache_dir)?;

        let db_path = cache_dir.join("pr-cache.db");
        let conn = Connection::open(&db_path)?;
        Self::open_with_conn(conn, repository, ttl)
    }

    /// Internal constructor that accepts an existing connection. Used by tests
    /// to inject an in-memory database.
    pub fn open_with_conn(conn: Connection, repository: &str, ttl: Duration) -> Result<Self> {
        // Enable WAL mode for better concurrent access.
        conn.execute_batch("PRAGMA journal_mode=WAL;")?;
        // Enable foreign keys.
        conn.execute_batch("PRAGMA foreign_keys=ON;")?;

        run_migrations(&conn)?;

        // Ensure the repository row exists so queries can reference its id.
        conn.execute(
            "INSERT OR IGNORE INTO repositories (slug, created_at) VALUES (?1, ?2)",
            params![repository, unix_now()],
        )?;

        let mut cache = Self {
            conn,
            repo: repository.to_string(),
            ttl,
            stats: CacheStats::default(),
        };

        cache.refresh_aggregate_stats()?;
        Ok(cache)
    }

    // -----------------------------------------------------------------------
    // Public API
    // -----------------------------------------------------------------------

    /// Return cached PR info if a valid (non-expired) entry exists.
    ///
    /// * Returns `Some(PrInfo)` on a cache hit with a known PR.
    /// * Returns `None` on a cache hit for "no PR" (avoids a redundant API call).
    /// * Returns `None` on a miss or expired entry (caller should fetch from API).
    ///
    /// To distinguish a "no PR" hit from a true miss, use `is_cached()`.
    pub fn get(&mut self, branch_name: &str) -> CacheResult {
        let cutoff = unix_now() - self.ttl.as_secs() as i64;

        type Row = (Option<u64>, Option<String>, Option<String>, Option<String>);
        let row: Option<Row> = self
            .conn
            .query_row(
                "SELECT pr_number, pr_state, pr_title, pr_url
                 FROM cached_prs
                 WHERE repository_id = (SELECT id FROM repositories WHERE slug = ?1)
                   AND branch_name = ?2
                   AND cached_at > ?3",
                params![self.repo, branch_name, cutoff],
                |row| {
                    Ok((
                        row.get::<_, Option<u64>>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, Option<String>>(3)?,
                    ))
                },
            )
            .ok();

        match row {
            Some((Some(number), Some(state_str), Some(title), Some(url))) => {
                let state = match state_str.as_str() {
                    "OPEN" => PrState::Open,
                    "MERGED" => PrState::Merged,
                    "CLOSED" => PrState::Closed,
                    _ => {
                        self.stats.misses += 1;
                        return CacheResult::Miss;
                    }
                };
                self.stats.hits += 1;
                CacheResult::Hit(Some(PrInfo {
                    number,
                    state,
                    title,
                    url,
                }))
            }
            // Cached "no PR" result — still a hit, no API call needed.
            Some((None, _, _, _)) => {
                self.stats.hits += 1;
                CacheResult::Hit(None)
            }
            // PR number present but metadata incomplete — treat as miss.
            Some(_) => {
                self.stats.misses += 1;
                CacheResult::Miss
            }
            None => {
                self.stats.misses += 1;
                CacheResult::Miss
            }
        }
    }

    /// Store a PR result in the database.
    ///
    /// Pass `None` to cache the fact that no PR exists for this branch, preventing
    /// redundant API calls on subsequent runs within the TTL.
    pub fn set(&mut self, branch_name: &str, pr_info: Option<&PrInfo>) -> Result<()> {
        let (number, state, title, url): (Option<u64>, Option<&str>, Option<&str>, Option<&str>) =
            match pr_info {
                Some(pr) => (
                    Some(pr.number),
                    Some(pr.state.as_str()),
                    Some(pr.title.as_str()),
                    Some(pr.url.as_str()),
                ),
                None => (None, None, None, None),
            };

        self.conn.execute(
            "INSERT OR REPLACE INTO cached_prs
                (repository_id, branch_name, pr_number, pr_state, pr_title, pr_url, cached_at)
             VALUES (
                (SELECT id FROM repositories WHERE slug = ?1),
                ?2, ?3, ?4, ?5, ?6, ?7
             )",
            params![
                self.repo,
                branch_name,
                number,
                state,
                title,
                url,
                unix_now()
            ],
        )?;

        self.stats.writes += 1;
        Ok(())
    }

    /// Remove all cached entries for this repository that exceed `max_age`.
    ///
    /// Call this once on startup to prevent unbounded database growth.
    /// Returns the number of rows deleted.
    pub fn evict_stale(&self, max_age: Duration) -> Result<usize> {
        let cutoff = unix_now() - max_age.as_secs() as i64;
        let count = self.conn.execute(
            "DELETE FROM cached_prs
             WHERE repository_id = (SELECT id FROM repositories WHERE slug = ?1)
               AND cached_at < ?2",
            params![self.repo, cutoff],
        )?;
        Ok(count)
    }

    /// Remove the cached entry for a single branch.
    ///
    /// Call this after a branch is deleted so stale data does not accumulate.
    #[allow(dead_code)]
    pub fn invalidate(&self, branch_name: &str) -> Result<()> {
        self.conn.execute(
            "DELETE FROM cached_prs
             WHERE repository_id = (SELECT id FROM repositories WHERE slug = ?1)
               AND branch_name = ?2",
            params![self.repo, branch_name],
        )?;
        Ok(())
    }

    /// Read-only snapshot of session statistics.
    pub fn stats(&self) -> &CacheStats {
        &self.stats
    }

    // -----------------------------------------------------------------------
    // Private helpers
    // -----------------------------------------------------------------------

    /// Refresh `total_entries` and `oldest_entry_ts` in `stats`.
    fn refresh_aggregate_stats(&mut self) -> Result<()> {
        let (total, oldest): (usize, Option<i64>) = self.conn.query_row(
            "SELECT COUNT(*), MIN(cached_at)
             FROM cached_prs
             WHERE repository_id = (SELECT id FROM repositories WHERE slug = ?1)",
            params![self.repo],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        self.stats.total_entries = total;
        self.stats.oldest_entry_ts = oldest;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// CacheResult — richer return type than Option<Option<PrInfo>>
// ---------------------------------------------------------------------------

/// The outcome of a `PrCache::get()` call.
#[derive(Debug)]
pub enum CacheResult {
    /// A valid (non-expired) cache entry was found.
    /// Inner `Option<PrInfo>` is `None` when the entry records "no PR for this branch".
    Hit(Option<PrInfo>),
    /// No valid cache entry — the caller must fetch from the API.
    Miss,
}

impl CacheResult {
    #[allow(dead_code)]
    pub fn is_hit(&self) -> bool {
        matches!(self, CacheResult::Hit(_))
    }

    #[allow(dead_code)]
    pub fn into_pr_info(self) -> Option<PrInfo> {
        match self {
            CacheResult::Hit(info) => info,
            CacheResult::Miss => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Schema migrations
// ---------------------------------------------------------------------------

fn run_migrations(conn: &Connection) -> Result<()> {
    conn.execute_batch(BOOTSTRAP_SQL)?;

    let version: i64 = conn.query_row(
        "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
        [],
        |row| row.get(0),
    )?;

    if version < CURRENT_SCHEMA_VERSION {
        conn.execute_batch(MIGRATION_V1_SQL)?;
        conn.execute(
            "INSERT INTO schema_migrations (version, applied_at) VALUES (1, ?1)",
            params![unix_now()],
        )?;
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// PrState display helper (used for serialisation)
// ---------------------------------------------------------------------------

impl PrState {
    fn as_str(&self) -> &'static str {
        match self {
            PrState::Open => "OPEN",
            PrState::Merged => "MERGED",
            PrState::Closed => "CLOSED",
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn open_in_memory(ttl: Duration) -> PrCache {
        let conn = Connection::open_in_memory().unwrap();
        PrCache::open_with_conn(conn, "test/repo", ttl).unwrap()
    }

    fn make_pr(number: u64) -> PrInfo {
        PrInfo {
            number,
            state: PrState::Merged,
            title: format!("PR #{number}"),
            url: format!("https://github.com/test/repo/pull/{number}"),
        }
    }

    #[test]
    fn cache_miss_returns_none() {
        let mut cache = open_in_memory(Duration::from_secs(3600));
        assert!(matches!(cache.get("feature/foo"), CacheResult::Miss));
    }

    #[test]
    fn cache_hit_returns_pr_info() {
        let mut cache = open_in_memory(Duration::from_secs(3600));
        let pr = make_pr(42);
        cache.set("feature/foo", Some(&pr)).unwrap();

        match cache.get("feature/foo") {
            CacheResult::Hit(Some(info)) => assert_eq!(info.number, 42),
            other => panic!("expected Hit(Some(..)), got {other:?}"),
        }
    }

    #[test]
    fn cached_no_pr_does_not_re_query() {
        let mut cache = open_in_memory(Duration::from_secs(3600));
        cache.set("feature/no-pr", None).unwrap();

        // Should be a Hit(None) — not a Miss — so callers know not to re-fetch.
        match cache.get("feature/no-pr") {
            CacheResult::Hit(None) => {}
            other => panic!("expected Hit(None), got {other:?}"),
        }
        assert_eq!(cache.stats().hits, 1);
        assert_eq!(cache.stats().misses, 0);
    }

    #[test]
    fn expired_entry_treated_as_miss() {
        let mut cache = open_in_memory(Duration::ZERO); // TTL = 0s → always expired
        let pr = make_pr(7);
        cache.set("feature/bar", Some(&pr)).unwrap();

        assert!(matches!(cache.get("feature/bar"), CacheResult::Miss));
    }

    #[test]
    fn evict_stale_removes_old_entries() {
        let mut cache = open_in_memory(Duration::from_secs(3600));
        let pr = make_pr(1);
        cache.set("old-branch", Some(&pr)).unwrap();

        // Manually back-date the entry to 2 hours ago so evict_stale(1h) removes it.
        cache
            .conn
            .execute("UPDATE cached_prs SET cached_at = cached_at - 7200", [])
            .unwrap();

        let removed = cache.evict_stale(Duration::from_secs(3600)).unwrap();
        assert_eq!(removed, 1);

        // Entry should now be a miss.
        assert!(matches!(cache.get("old-branch"), CacheResult::Miss));
    }

    #[test]
    fn invalidate_removes_single_entry() {
        let mut cache = open_in_memory(Duration::from_secs(3600));
        let pr = make_pr(99);
        cache.set("to-delete", Some(&pr)).unwrap();
        cache.set("keep", Some(&make_pr(100))).unwrap();

        cache.invalidate("to-delete").unwrap();

        assert!(matches!(cache.get("to-delete"), CacheResult::Miss));
        assert!(matches!(cache.get("keep"), CacheResult::Hit(Some(_))));
    }

    #[test]
    fn schema_migration_runs_on_fresh_db() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).unwrap();
        let version: i64 = conn
            .query_row("SELECT MAX(version) FROM schema_migrations", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(version, CURRENT_SCHEMA_VERSION);
    }

    #[test]
    fn schema_migration_is_idempotent() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).unwrap();
        run_migrations(&conn).unwrap(); // Should not fail or duplicate rows.
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM schema_migrations", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn stats_counts_are_correct() {
        let mut cache = open_in_memory(Duration::from_secs(3600));
        cache.set("a", Some(&make_pr(1))).unwrap();
        cache.set("b", None).unwrap();

        cache.get("a"); // hit
        cache.get("b"); // hit (no-pr)
        cache.get("c"); // miss

        let s = cache.stats();
        assert_eq!(s.hits, 2);
        assert_eq!(s.misses, 1);
        assert_eq!(s.writes, 2);
    }
}
