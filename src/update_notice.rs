//! The "update available" notice (docs/specs/SELF_UPDATE.md, section 4.2).
//!
//! At most once every 24 hours a background task asks GitHub for the newest release, with a short
//! timeout, and caches the answer. Every other run reads the cache and makes no request. Failures
//! are silent: the notice is a hint, never an error.

use crate::update;
use axoupdater::Version;
use serde::{Deserialize, Serialize};
use std::ffi::OsString;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::oneshot;

const APP: &str = env!("CARGO_PKG_NAME");
const CHECK_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);
const CHECK_TIMEOUT: Duration = Duration::from_secs(2);
/// Set to any value but `0` to turn the check off.
const DISABLE_ENV: &str = "LOCAL_GIT_BRANCH_CLEANUP_TUI_NO_UPDATE_CHECK";

/// The cache file: when the last check ran and the newest version it knows of. Nothing about the
/// user's repositories.
#[derive(Debug, Serialize, Deserialize, PartialEq)]
struct CheckCache {
    checked_at: i64,
    latest: Option<String>,
}

/// Whether the check may run, from the `--no-update-check` flag, the disable variable, the `CI`
/// variable and whether stdout is a terminal.
fn should_check(
    no_update_check: bool,
    disable_env: Option<OsString>,
    ci: bool,
    stdout_is_terminal: bool,
) -> bool {
    let disabled_by_env = disable_env.is_some_and(|value| !value.is_empty() && value != "0");
    !no_update_check && !disabled_by_env && !ci && stdout_is_terminal
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

/// A check is fresh for `CHECK_INTERVAL`. One dated in the future (a clock change) is not.
fn is_fresh(cache: &CheckCache, now: i64) -> bool {
    (0..CHECK_INTERVAL.as_secs() as i64).contains(&(now - cache.checked_at))
}

/// Next to the PR cache.
fn cache_path() -> Option<PathBuf> {
    crate::cache::cache_root().map(|root| {
        root.join("omni-scripts")
            .join(format!("{APP}-update-check.json"))
    })
}

fn read_cache(path: &Path) -> Option<CheckCache> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

fn write_cache(path: &Path, cache: &CheckCache) {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).ok();
    }
    if let Ok(json) = serde_json::to_string(cache) {
        std::fs::write(path, json).ok();
    }
}

/// Run `lookup`, cache its answer and send it to the app. A failed check is cached too, keeping the
/// version known before, and waits for the next interval.
///
/// Unless the app has stopped waiting: when it exits, the runtime cancels the lookup's DNS
/// resolution and the lookup fails, which is no answer to cache. The next run checks again.
async fn check(
    lookup: impl std::future::Future<Output = Option<Version>>,
    path: PathBuf,
    now: i64,
    known: Option<String>,
    sender: oneshot::Sender<Option<Version>>,
) {
    let found = lookup.await;
    if sender.is_closed() {
        return;
    }
    let cache = CheckCache {
        checked_at: now,
        latest: found.as_ref().map(ToString::to_string).or(known),
    };
    write_cache(&path, &cache);
    sender.send(found).ok();
}

fn current_version() -> Version {
    env!("CARGO_PKG_VERSION")
        .parse()
        .expect("CARGO_PKG_VERSION is valid semver")
}

/// The newest known release, and the background check's answer while it is still running.
pub struct UpdateNotice {
    latest: Option<Version>,
    pending: Option<oneshot::Receiver<Option<Version>>>,
}

impl UpdateNotice {
    /// Read the cache and, when it is older than 24 hours, start a background check. Returns at
    /// once; nothing here waits on the network. Must be called inside the tokio runtime.
    pub fn start(no_update_check: bool) -> Self {
        let disabled = Self {
            latest: None,
            pending: None,
        };
        if !should_check(
            no_update_check,
            std::env::var_os(DISABLE_ENV),
            std::env::var_os("CI").is_some(),
            std::io::stdout().is_terminal(),
        ) {
            return disabled;
        }
        let Some(path) = cache_path() else {
            return disabled;
        };

        let cached = read_cache(&path);
        let now = unix_now();
        let known = cached.as_ref().and_then(|c| c.latest.clone());
        let latest = known.as_deref().and_then(|v| v.parse().ok());
        if cached.as_ref().is_some_and(|c| is_fresh(c, now)) {
            return Self {
                latest,
                pending: None,
            };
        }

        let (sender, receiver) = oneshot::channel();
        let lookup = async {
            tokio::time::timeout(CHECK_TIMEOUT, update::latest_version(CHECK_TIMEOUT))
                .await
                .ok()
                .flatten()
        };
        tokio::spawn(check(lookup, path, now, known, sender));
        Self {
            latest,
            pending: Some(receiver),
        }
    }

    /// The notice line when a release newer than this binary is known. Picks up the background
    /// check's answer once it has arrived; never waits for it.
    pub fn message(&mut self) -> Option<String> {
        if let Some(receiver) = &mut self.pending {
            match receiver.try_recv() {
                Ok(found) => {
                    self.latest = found.or(self.latest.take());
                    self.pending = None;
                }
                Err(oneshot::error::TryRecvError::Empty) => {}
                Err(oneshot::error::TryRecvError::Closed) => self.pending = None,
            }
        }
        self.latest
            .as_ref()
            .filter(|latest| **latest > current_version())
            .map(|latest| format!("Update available: {latest} (run: {APP} update)"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: i64 = 24 * 60 * 60;

    fn notice(latest: Option<&str>) -> UpdateNotice {
        UpdateNotice {
            latest: latest.map(|v| v.parse().unwrap()),
            pending: None,
        }
    }

    #[test]
    fn checks_by_default_on_a_terminal() {
        assert!(should_check(false, None, false, true));
    }

    #[test]
    fn each_opt_out_turns_the_check_off() {
        assert!(!should_check(true, None, false, true));
        assert!(!should_check(false, Some("1".into()), false, true));
        assert!(!should_check(false, None, true, true));
        assert!(!should_check(false, None, false, false));
    }

    #[test]
    fn empty_or_zero_disable_variable_keeps_the_check_on() {
        assert!(should_check(false, Some("".into()), false, true));
        assert!(should_check(false, Some("0".into()), false, true));
    }

    #[test]
    fn cache_is_fresh_for_24_hours() {
        let cache = |checked_at| CheckCache {
            checked_at,
            latest: None,
        };
        let now = 10 * DAY;
        assert!(is_fresh(&cache(now), now));
        assert!(is_fresh(&cache(now - DAY + 1), now));
        assert!(!is_fresh(&cache(now - DAY), now));
        assert!(!is_fresh(&cache(now + 60), now));
    }

    #[test]
    fn cache_round_trips_and_ignores_garbage() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("nested").join("check.json");
        let cache = CheckCache {
            checked_at: 42,
            latest: Some("1.2.3".to_string()),
        };
        write_cache(&path, &cache);
        assert_eq!(read_cache(&path), Some(cache));

        std::fs::write(&path, "not json").unwrap();
        assert_eq!(read_cache(&path), None);
    }

    #[test]
    fn newer_release_gives_the_notice() {
        assert_eq!(
            notice(Some("99.0.0")).message(),
            Some(format!("Update available: 99.0.0 (run: {APP} update)"))
        );
    }

    #[test]
    fn same_or_older_release_gives_no_notice() {
        assert_eq!(notice(Some(env!("CARGO_PKG_VERSION"))).message(), None);
        assert_eq!(notice(Some("0.0.1")).message(), None);
        assert_eq!(notice(None).message(), None);
    }

    #[test]
    fn background_answer_is_picked_up_when_it_arrives() {
        let (sender, receiver) = oneshot::channel();
        let mut notice = UpdateNotice {
            latest: None,
            pending: Some(receiver),
        };
        assert_eq!(notice.message(), None);
        sender.send(Some("99.0.0".parse().unwrap())).unwrap();
        assert!(notice.message().is_some());
        assert!(notice.pending.is_none());
    }

    #[tokio::test]
    async fn check_caches_and_sends_its_answer() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("check.json");
        let (sender, receiver) = oneshot::channel();

        check(
            async { None },
            path.clone(),
            42,
            Some("1.0.0".into()),
            sender,
        )
        .await;

        assert_eq!(receiver.await.unwrap(), None);
        // A failed check keeps the version known before.
        assert_eq!(
            read_cache(&path),
            Some(CheckCache {
                checked_at: 42,
                latest: Some("1.0.0".into())
            })
        );
    }

    #[tokio::test]
    async fn check_after_the_app_stopped_waiting_caches_nothing() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("check.json");
        let (sender, receiver) = oneshot::channel();
        drop(receiver);

        check(async { None }, path.clone(), 42, None, sender).await;

        assert!(!path.exists());
    }

    #[test]
    fn failed_background_check_keeps_the_known_version() {
        let (sender, receiver) = oneshot::channel();
        let mut notice = UpdateNotice {
            latest: Some("99.0.0".parse().unwrap()),
            pending: Some(receiver),
        };
        sender.send(None).unwrap();
        assert!(notice.message().is_some());
    }
}
