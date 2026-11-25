//! Background reloads of the branch list (docs/specs/LIVE_REFRESH.md).
//!
//! A reload runs on its own thread and reports back on a channel that the
//! main loop drains every tick. At most one reload runs at a time; a request
//! made meanwhile is queued (one at most). Every reload carries the generation
//! it was started with, and a result from an older generation is dropped, so
//! a reload that read git before a deletion can never bring deleted branches
//! back.
//!
//! A full sync (`R`) takes the PR cache with it and hands it back with its
//! result; `poll` puts it back in the caller's slot as soon as any result
//! arrives, even one that is dropped as stale.

use crate::cache::PrCache;
use crate::git::{self, BranchInfo};
use crate::pr::{self, PullRequestProvider};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

/// How often automatic refresh checks whether the refs changed, unless
/// `--auto-refresh-interval` says otherwise
pub const DEFAULT_AUTO_REFRESH_INTERVAL: Duration = Duration::from_secs(10);

/// What a reload re-reads (ordered from least to most work)
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RefreshKind {
    /// Automatic: reload only when the refs fingerprint changed
    Check,
    /// Local git state only (`r`)
    Local,
    /// Fetch the remote and reload, keeping the PR data the app has
    /// (startup; `--no-fetch-on-start` skips it)
    Fetch,
    /// Fetch the remote, reload, and re-fetch PR data (`R`)
    Full,
}

/// PR re-fetch part of a full sync
struct PrJob {
    provider: Arc<dyn PullRequestProvider>,
    cache: Option<PrCache>,
    runtime: tokio::runtime::Handle,
}

/// A reload to run in the background
pub struct Job {
    pub generation: u64,
    pub kind: RefreshKind,
    /// Trunk to classify against (the one the TUI started with)
    pub trunk: String,
    /// Skip the reload when the refs still match this fingerprint
    pub known_fingerprint: Option<String>,
    /// Started by a timer, not a key
    pub automatic: bool,
    pr: Option<PrJob>,
}

/// What a background reload reports
pub enum JobResult {
    /// A step of a full sync started (shown in the footer)
    Progress { generation: u64, stage: String },
    Reloaded {
        generation: u64,
        branches: Vec<BranchInfo>,
        /// Refs fingerprint read just before the branches
        fingerprint: Option<String>,
        /// A full sync or the startup fetch: `git fetch` ran (when there
        /// is a remote), so `fetch_error` says whether it worked
        full_sync: bool,
        /// `git fetch` failed; the reload still ran on local data
        fetch_error: Option<String>,
        /// PR data was re-fetched, so it replaces what the app had
        prs_refetched: bool,
        /// Branches whose PR lookup failed during the re-fetch; they keep
        /// the PR info the app already had
        pr_failed: Vec<String>,
        pr_cache: Option<PrCache>,
        /// Started by a timer, not a key
        automatic: bool,
    },
    /// The refs still match the known fingerprint; nothing was re-read
    Unchanged { generation: u64 },
    Failed {
        generation: u64,
        error: String,
        /// Refs fingerprint read before the failed reload: checks skip
        /// these refs instead of failing again every interval
        fingerprint: Option<String>,
        pr_cache: Option<PrCache>,
    },
}

impl JobResult {
    fn generation(&self) -> u64 {
        match self {
            JobResult::Progress { generation, .. }
            | JobResult::Reloaded { generation, .. }
            | JobResult::Unchanged { generation }
            | JobResult::Failed { generation, .. } => *generation,
        }
    }

    fn take_pr_cache(&mut self) -> Option<PrCache> {
        match self {
            JobResult::Reloaded { pr_cache, .. } | JobResult::Failed { pr_cache, .. } => {
                pr_cache.take()
            }
            _ => None,
        }
    }
}

/// Starts a job; the real one spawns a thread, tests record the jobs
type Launcher = Box<dyn Fn(Job, Sender<JobResult>)>;

/// Reload for `Check` and `Local`, on the calling thread
fn run_local(job: &Job) -> JobResult {
    // Read before the branches: a change in between triggers another reload
    let fingerprint = git::refs_fingerprint().ok();
    if fingerprint.is_some() && fingerprint == job.known_fingerprint {
        return JobResult::Unchanged {
            generation: job.generation,
        };
    }
    match git::get_branches_with_classification(Some(&job.trunk)) {
        Ok(branches) => JobResult::Reloaded {
            generation: job.generation,
            branches,
            fingerprint,
            full_sync: false,
            fetch_error: None,
            prs_refetched: false,
            pr_failed: Vec::new(),
            pr_cache: None,
            automatic: job.automatic,
        },
        Err(e) => JobResult::Failed {
            generation: job.generation,
            error: e.to_string(),
            fingerprint,
            pr_cache: None,
        },
    }
}

/// Full sync on the calling thread: fetch, reload, and re-fetch PRs when
/// the job carries a PR job (`Full` with a provider; never `Fetch`)
fn run_full(job: Job, tx: &Sender<JobResult>) -> JobResult {
    let generation = job.generation;
    let progress = |stage: String| {
        let _ = tx.send(JobResult::Progress { generation, stage });
    };

    let mut fetch_error = None;
    if let Some(remote) = git::get_primary_remote() {
        progress(format!("fetching {remote}…"));
        if let Err(e) = git::fetch_remote(&remote) {
            fetch_error = Some(e.to_string());
        }
    }

    progress("reloading…".to_string());
    let fingerprint = git::refs_fingerprint().ok();
    let mut branches = match git::get_branches_with_classification(Some(&job.trunk)) {
        Ok(branches) => branches,
        Err(e) => {
            return JobResult::Failed {
                generation,
                error: e.to_string(),
                fingerprint,
                pr_cache: job.pr.and_then(|pr| pr.cache),
            }
        }
    };

    let mut prs_refetched = false;
    let mut pr_failed = Vec::new();
    let mut pr_cache = None;
    if let Some(PrJob {
        provider,
        mut cache,
        runtime,
    }) = job.pr
    {
        progress("fetching PRs…".to_string());
        let report = runtime.block_on(pr::refetch_pr_info_for_branches(
            provider,
            &mut branches,
            cache.as_mut(),
        ));
        prs_refetched = true;
        pr_failed = report.errors.into_iter().map(|e| e.branch).collect();
        pr_cache = cache;
    }

    JobResult::Reloaded {
        generation,
        branches,
        fingerprint,
        full_sync: true,
        fetch_error,
        prs_refetched,
        pr_failed,
        pr_cache,
        automatic: job.automatic,
    }
}

/// Launch a reload on a plain thread (not tokio's blocking pool, whose
/// shutdown would wait for a slow git call when the user quits)
fn spawn_job(job: Job, tx: Sender<JobResult>) {
    thread::spawn(move || {
        let result = match job.kind {
            RefreshKind::Fetch | RefreshKind::Full => run_full(job, &tx),
            RefreshKind::Check | RefreshKind::Local => run_local(&job),
        };
        let _ = tx.send(result);
    });
}

/// Schedules background reloads and hands finished ones to the app
pub struct Refresher {
    generation: u64,
    /// Generation and kind of the reload that is running
    in_flight: Option<(u64, RefreshKind)>,
    /// Latest progress message of the running full sync or fetch
    stage: Option<String>,
    /// Reload to start when the running one finishes
    queued: Option<RefreshKind>,
    /// Finished result held back while the app cannot apply it
    pending: Option<JobResult>,
    /// Outcome (`fetch_error`, `automatic`) of a fetch whose reload was
    /// dropped as out of date; the next reload returned carries it
    unreported_fetch: Option<(Option<String>, bool)>,
    tx: Sender<JobResult>,
    rx: Receiver<JobResult>,
    launcher: Launcher,
    /// Whether to check for ref changes every `interval`
    pub auto: bool,
    /// Time between automatic checks
    pub interval: Duration,
    /// Time between automatic full syncs (`--auto-sync-interval`); None
    /// (`--no-auto-sync`): only on `R`
    sync_interval: Option<Duration>,
    /// When the last full sync started (or the clock was set at startup)
    last_sync: Option<Instant>,
    /// PR provider for full syncs (None: `R` only fetches and reloads)
    pub provider: Option<Arc<dyn PullRequestProvider>>,
    /// Fingerprint of the refs the branch list on screen was read from
    fingerprint: Option<String>,
    /// When the last reload or check started
    last_started: Option<Instant>,
}

impl Default for Refresher {
    fn default() -> Self {
        Self::with_launcher(Box::new(spawn_job))
    }
}

impl Refresher {
    fn with_launcher(launcher: Launcher) -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            generation: 0,
            in_flight: None,
            stage: None,
            queued: None,
            pending: None,
            unreported_fetch: None,
            tx,
            rx,
            launcher,
            auto: false,
            interval: DEFAULT_AUTO_REFRESH_INTERVAL,
            sync_interval: None,
            last_sync: None,
            provider: None,
            fingerprint: None,
            last_started: None,
        }
    }

    /// Run a full sync every `interval` from `now` on
    /// (`--auto-sync-interval`; None with `--no-auto-sync`). The first one
    /// waits a whole interval: startup has just read everything, and the
    /// startup fetch (`RefreshKind::Fetch`) brings the remote refs up to
    /// date without restarting this clock.
    pub fn set_sync_interval(&mut self, interval: Option<Duration>, now: Instant) {
        self.sync_interval = interval;
        self.last_sync = Some(now);
    }

    /// Make an automatic check due now (e.g. the terminal regained focus).
    /// Does nothing while automatic refresh is off.
    pub fn check_soon(&mut self) {
        self.last_started = None;
    }

    /// Record the refs the branch list on screen was read from
    pub fn set_fingerprint(&mut self, fingerprint: Option<String>) {
        self.fingerprint = fingerprint;
    }

    /// The kind of the reload that is running, if any
    pub fn running(&self) -> Option<RefreshKind> {
        self.in_flight.map(|(_, kind)| kind)
    }

    /// Latest progress message of the running full sync or fetch
    pub fn stage(&self) -> Option<&str> {
        self.stage.as_deref()
    }

    /// Start a reload, or queue it behind the running one. Queued requests
    /// merge into one, keeping the widest kind. A full sync takes the PR
    /// cache out of `pr_cache` until it finishes.
    pub fn request(&mut self, kind: RefreshKind, trunk: &str, pr_cache: &mut Option<PrCache>) {
        if self.in_flight.is_some() {
            self.queued = self.queued.max(Some(kind));
        } else {
            self.start(kind, trunk, Instant::now(), pr_cache, false);
        }
    }

    /// Make every result started so far stale (call before changing the
    /// branches on the main thread, e.g. deleting)
    pub fn invalidate(&mut self) {
        self.generation += 1;
    }

    fn start(
        &mut self,
        kind: RefreshKind,
        trunk: &str,
        now: Instant,
        pr_cache: &mut Option<PrCache>,
        automatic: bool,
    ) {
        self.generation += 1;
        self.in_flight = Some((self.generation, kind));
        self.stage = None;
        self.last_started = Some(now);
        if kind == RefreshKind::Full {
            // A manual R counts too: the next automatic sync is an interval away
            self.last_sync = Some(now);
        }
        let pr = match (kind, &self.provider, tokio::runtime::Handle::try_current()) {
            (RefreshKind::Full, Some(provider), Ok(runtime)) => Some(PrJob {
                provider: Arc::clone(provider),
                cache: pr_cache.take(),
                runtime,
            }),
            _ => None,
        };
        let job = Job {
            generation: self.generation,
            kind,
            trunk: trunk.to_string(),
            known_fingerprint: match kind {
                RefreshKind::Check => self.fingerprint.clone(),
                RefreshKind::Local | RefreshKind::Fetch | RefreshKind::Full => None,
            },
            automatic,
            pr,
        };
        (self.launcher)(job, self.tx.clone());
    }

    /// Collect finished reloads, start a queued one or an automatic check
    /// that is due, and return the newest current result unless `hold` is
    /// set (the app keeps it until `hold` clears, e.g. while the delete
    /// confirmation is open). A PR cache coming back from a full sync goes
    /// into `pr_cache` straight away.
    pub fn poll(
        &mut self,
        hold: bool,
        now: Instant,
        trunk: &str,
        pr_cache: &mut Option<PrCache>,
    ) -> Option<JobResult> {
        while let Ok(mut result) = self.rx.try_recv() {
            if let Some(cache) = result.take_pr_cache() {
                *pr_cache = Some(cache);
            }
            let generation = result.generation();
            let is_in_flight = self.in_flight.is_some_and(|(g, _)| g == generation);
            if let JobResult::Progress { stage, .. } = result {
                if is_in_flight {
                    self.stage = Some(stage);
                }
                continue;
            }
            if is_in_flight {
                self.in_flight = None;
                self.stage = None;
            }
            if generation != self.generation {
                self.keep_fetch_outcome(&result);
                continue;
            }
            match result {
                // Nothing to apply; the list on screen is current
                JobResult::Unchanged { .. } => {}
                result => self.pending = Some(result),
            }
        }
        if self.in_flight.is_none() {
            if let Some(kind) = self.queued.take() {
                // Only key presses queue
                self.start(kind, trunk, now, pr_cache, false);
            } else if self.auto_sync_due(hold, now) {
                // A full sync includes the reload a check would do
                self.start(RefreshKind::Full, trunk, now, pr_cache, true);
            } else if self.auto_check_due(hold, now) {
                self.start(RefreshKind::Check, trunk, now, pr_cache, true);
            }
        }
        // A result is only current while no newer reload was started
        if self
            .pending
            .as_ref()
            .is_some_and(|p| p.generation() != self.generation)
        {
            if let Some(stale) = self.pending.take() {
                self.keep_fetch_outcome(&stale);
            }
        }
        if hold {
            return None;
        }
        let mut result = self.pending.take();
        if let Some(JobResult::Reloaded {
            full_sync,
            fetch_error,
            automatic,
            ..
        }) = &mut result
        {
            // A reload that fetched itself has the newer outcome
            if let Some((error, was_automatic)) = self.unreported_fetch.take() {
                if !*full_sync {
                    *full_sync = true;
                    *fetch_error = error;
                    *automatic = was_automatic;
                }
            }
        }
        if let Some(
            JobResult::Reloaded { fingerprint, .. } | JobResult::Failed { fingerprint, .. },
        ) = &result
        {
            self.fingerprint = fingerprint.clone();
        }
        result
    }

    /// Keep the fetch outcome of a dropped full sync or fetch (a reload
    /// started after it, e.g. `r` pressed during the startup fetch), so a
    /// failed fetch is still reported
    fn keep_fetch_outcome(&mut self, result: &JobResult) {
        if let JobResult::Reloaded {
            full_sync: true,
            fetch_error,
            automatic,
            ..
        } = result
        {
            self.unreported_fetch = Some((fetch_error.clone(), *automatic));
        }
    }

    /// Whether an automatic full sync should start now: on, nothing
    /// waiting to be applied, and `sync_interval` since the last full sync
    /// started
    fn auto_sync_due(&self, hold: bool, now: Instant) -> bool {
        self.sync_interval.is_some_and(|interval| {
            !hold
                && self.pending.is_none()
                && self
                    .last_sync
                    .is_none_or(|at| now.duration_since(at) >= interval)
        })
    }

    /// Whether an automatic check should start now: on, nothing waiting to
    /// be applied, and `interval` since the last reload started
    fn auto_check_due(&self, hold: bool, now: Instant) -> bool {
        self.auto
            && !hold
            && self.pending.is_none()
            && self
                .last_started
                .is_none_or(|at| now.duration_since(at) >= self.interval)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    /// Jobs a test refresher started, with the channel to finish each one
    type Recorded = Rc<RefCell<Vec<(Job, Sender<JobResult>)>>>;

    /// A refresher whose jobs are recorded instead of run
    fn recording() -> (Refresher, Recorded) {
        let jobs = Rc::new(RefCell::new(Vec::new()));
        let recorded = Rc::clone(&jobs);
        let refresher = Refresher::with_launcher(Box::new(move |job, tx| {
            recorded.borrow_mut().push((job, tx));
        }));
        (refresher, jobs)
    }

    fn poll_at(refresher: &mut Refresher, hold: bool, now: Instant) -> Option<JobResult> {
        refresher.poll(hold, now, "main", &mut None)
    }

    fn poll(refresher: &mut Refresher, hold: bool) -> Option<JobResult> {
        poll_at(refresher, hold, Instant::now())
    }

    fn request(refresher: &mut Refresher, kind: RefreshKind, trunk: &str) {
        refresher.request(kind, trunk, &mut None);
    }

    fn send(jobs: &Recorded, index: usize, result: impl FnOnce(u64) -> JobResult) {
        let jobs = jobs.borrow();
        let (job, tx) = &jobs[index];
        tx.send(result(job.generation)).unwrap();
    }

    fn finish(jobs: &Recorded, index: usize) {
        send(jobs, index, |generation| JobResult::Reloaded {
            generation,
            branches: Vec::new(),
            fingerprint: Some(format!("refs-{generation}")),
            full_sync: false,
            fetch_error: None,
            prs_refetched: false,
            pr_failed: Vec::new(),
            pr_cache: None,
            automatic: false,
        });
    }

    fn finish_unchanged(jobs: &Recorded, index: usize) {
        send(jobs, index, |generation| JobResult::Unchanged {
            generation,
        });
    }

    #[test]
    fn a_finished_reload_is_returned_once() {
        let (mut refresher, jobs) = recording();
        request(&mut refresher, RefreshKind::Local, "main");
        assert_eq!(refresher.running(), Some(RefreshKind::Local));
        assert!(poll(&mut refresher, false).is_none());

        finish(&jobs, 0);
        assert!(poll(&mut refresher, false).is_some());
        assert_eq!(refresher.running(), None);
        assert!(poll(&mut refresher, false).is_none());
    }

    #[test]
    fn requests_during_a_reload_queue_a_single_follow_up() {
        let (mut refresher, jobs) = recording();
        request(&mut refresher, RefreshKind::Local, "main");
        request(&mut refresher, RefreshKind::Local, "main");
        request(&mut refresher, RefreshKind::Local, "main");
        assert_eq!(jobs.borrow().len(), 1);

        finish(&jobs, 0);
        // The first result is superseded by the follow-up that starts now
        assert!(poll(&mut refresher, false).is_none());
        assert_eq!(jobs.borrow().len(), 2);

        finish(&jobs, 1);
        assert!(poll(&mut refresher, false).is_some());
        assert_eq!(jobs.borrow().len(), 2);
    }

    #[test]
    fn a_queued_full_sync_wins_over_a_local_reload() {
        let (mut refresher, jobs) = recording();
        request(&mut refresher, RefreshKind::Local, "main");
        request(&mut refresher, RefreshKind::Full, "main");
        request(&mut refresher, RefreshKind::Local, "main");
        finish(&jobs, 0);
        poll(&mut refresher, false);
        assert_eq!(jobs.borrow()[1].0.kind, RefreshKind::Full);
    }

    #[test]
    fn a_reload_started_before_invalidate_is_dropped() {
        let (mut refresher, jobs) = recording();
        request(&mut refresher, RefreshKind::Local, "main");
        refresher.invalidate();
        finish(&jobs, 0);
        assert!(poll(&mut refresher, false).is_none());
        assert_eq!(refresher.running(), None);
    }

    #[test]
    fn a_held_result_is_returned_when_the_hold_ends() {
        let (mut refresher, jobs) = recording();
        request(&mut refresher, RefreshKind::Local, "main");
        finish(&jobs, 0);
        assert!(poll(&mut refresher, true).is_none());
        assert!(poll(&mut refresher, true).is_none());
        assert!(poll(&mut refresher, false).is_some());
    }

    #[test]
    fn a_held_result_is_dropped_by_invalidate() {
        let (mut refresher, jobs) = recording();
        request(&mut refresher, RefreshKind::Local, "main");
        finish(&jobs, 0);
        assert!(poll(&mut refresher, true).is_none());
        refresher.invalidate();
        assert!(poll(&mut refresher, false).is_none());
    }

    #[test]
    fn jobs_carry_the_trunk() {
        let (mut refresher, jobs) = recording();
        request(&mut refresher, RefreshKind::Local, "develop");
        assert_eq!(jobs.borrow()[0].0.trunk, "develop");
    }

    #[test]
    fn progress_of_the_running_sync_is_shown_until_it_finishes() {
        let (mut refresher, jobs) = recording();
        request(&mut refresher, RefreshKind::Full, "main");
        send(&jobs, 0, |generation| JobResult::Progress {
            generation,
            stage: "fetching origin…".to_string(),
        });
        assert!(poll(&mut refresher, false).is_none());
        assert_eq!(refresher.stage(), Some("fetching origin…"));
        assert_eq!(refresher.running(), Some(RefreshKind::Full));

        finish(&jobs, 0);
        assert!(poll(&mut refresher, false).is_some());
        assert_eq!(refresher.stage(), None);
    }

    #[test]
    fn a_stale_result_still_returns_the_pr_cache() {
        use rusqlite::Connection;
        let (mut refresher, jobs) = recording();
        request(&mut refresher, RefreshKind::Full, "main");
        refresher.invalidate();
        send(&jobs, 0, |generation| JobResult::Failed {
            generation,
            error: "boom".to_string(),
            fingerprint: None,
            pr_cache: Some(
                PrCache::open_with_conn(
                    Connection::open_in_memory().unwrap(),
                    "test/repo",
                    Duration::from_secs(60),
                )
                .unwrap(),
            ),
        });
        let mut slot = None;
        assert!(refresher
            .poll(false, Instant::now(), "main", &mut slot)
            .is_none());
        assert!(slot.is_some());
    }

    #[test]
    fn automatic_checks_run_only_when_on_and_due() {
        let (mut refresher, jobs) = recording();
        let now = Instant::now();
        poll_at(&mut refresher, false, now);
        assert!(jobs.borrow().is_empty(), "off by default");

        refresher.auto = true;
        refresher.set_fingerprint(Some("refs-0".into()));
        poll_at(&mut refresher, false, now);
        assert_eq!(jobs.borrow().len(), 1);
        assert_eq!(refresher.running(), Some(RefreshKind::Check));
        assert_eq!(
            jobs.borrow()[0].0.known_fingerprint.as_deref(),
            Some("refs-0")
        );

        finish_unchanged(&jobs, 0);
        assert!(poll_at(&mut refresher, false, now).is_none());
        assert_eq!(jobs.borrow().len(), 1, "not due yet");

        poll_at(&mut refresher, false, now + DEFAULT_AUTO_REFRESH_INTERVAL);
        assert_eq!(jobs.borrow().len(), 2);
    }

    #[test]
    fn automatic_checks_follow_the_configured_interval() {
        let (mut refresher, jobs) = recording();
        refresher.auto = true;
        refresher.interval = Duration::from_secs(3);
        let now = Instant::now();
        poll_at(&mut refresher, false, now);
        finish_unchanged(&jobs, 0);

        poll_at(&mut refresher, false, now + Duration::from_secs(2));
        assert_eq!(jobs.borrow().len(), 1);
        poll_at(&mut refresher, false, now + Duration::from_secs(3));
        assert_eq!(jobs.borrow().len(), 2);
    }

    #[test]
    fn automatic_syncs_start_one_interval_after_startup() {
        let (mut refresher, jobs) = recording();
        let now = Instant::now();
        let minute = Duration::from_secs(60);
        refresher.set_sync_interval(Some(minute), now);

        poll_at(&mut refresher, false, now);
        assert!(jobs.borrow().is_empty(), "startup data is fresh");

        poll_at(&mut refresher, false, now + minute);
        assert_eq!(jobs.borrow().len(), 1);
        assert_eq!(jobs.borrow()[0].0.kind, RefreshKind::Full);
        assert!(jobs.borrow()[0].0.automatic);
    }

    #[test]
    fn the_startup_fetch_keeps_the_sync_clock_and_the_pr_cache() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let _guard = runtime.enter();
        let (mut refresher, jobs) = recording();
        refresher.provider = Some(Arc::new(crate::pr::github::GitHubProvider::new()));
        let start = Instant::now();
        let minute = Duration::from_secs(60);
        refresher.set_sync_interval(Some(minute), start);

        let mut cache = Some(
            PrCache::open_with_conn(
                rusqlite::Connection::open_in_memory().unwrap(),
                "test/repo",
                Duration::from_secs(60),
            )
            .unwrap(),
        );
        refresher.start(
            RefreshKind::Fetch,
            "main",
            start + Duration::from_secs(1),
            &mut cache,
            false,
        );
        assert!(jobs.borrow()[0].0.pr.is_none(), "no PR re-fetch");
        assert!(cache.is_some(), "the PR cache stays with the app");
        assert_eq!(jobs.borrow()[0].0.known_fingerprint, None);

        // The first automatic sync is still one interval after startup
        finish(&jobs, 0);
        assert!(poll_at(&mut refresher, false, start + Duration::from_secs(59)).is_some());
        assert_eq!(
            jobs.borrow().len(),
            1,
            "the fetch did not restart the clock"
        );
        poll_at(&mut refresher, false, start + minute);
        assert_eq!(jobs.borrow().len(), 2);
        assert_eq!(jobs.borrow()[1].0.kind, RefreshKind::Full);
        assert!(
            jobs.borrow()[1].0.pr.is_some(),
            "a full sync re-fetches PRs"
        );
    }

    #[test]
    fn r_during_the_startup_fetch_queues_a_full_sync() {
        let (mut refresher, jobs) = recording();
        request(&mut refresher, RefreshKind::Fetch, "main");
        assert_eq!(refresher.running(), Some(RefreshKind::Fetch));
        request(&mut refresher, RefreshKind::Full, "main");
        request(&mut refresher, RefreshKind::Local, "main");
        finish(&jobs, 0);
        poll(&mut refresher, false);
        assert_eq!(jobs.borrow()[1].0.kind, RefreshKind::Full);
    }

    #[test]
    fn a_reload_queued_behind_a_failed_fetch_reports_the_fetch_error() {
        let (mut refresher, jobs) = recording();
        request(&mut refresher, RefreshKind::Fetch, "main");
        request(&mut refresher, RefreshKind::Local, "main");
        send(&jobs, 0, |generation| JobResult::Reloaded {
            generation,
            branches: Vec::new(),
            fingerprint: None,
            full_sync: true,
            fetch_error: Some("unreachable".to_string()),
            prs_refetched: false,
            pr_failed: Vec::new(),
            pr_cache: None,
            automatic: false,
        });
        // The fetch's branches are superseded by the queued r
        assert!(poll(&mut refresher, false).is_none());
        assert_eq!(jobs.borrow()[1].0.kind, RefreshKind::Local);

        finish(&jobs, 1);
        match poll(&mut refresher, false) {
            Some(JobResult::Reloaded {
                full_sync,
                fetch_error,
                ..
            }) => {
                assert!(full_sync);
                assert_eq!(fetch_error.as_deref(), Some("unreachable"));
            }
            _ => panic!("expected the r reload"),
        }
        // Reported once
        request(&mut refresher, RefreshKind::Local, "main");
        finish(&jobs, 2);
        assert!(matches!(
            poll(&mut refresher, false),
            Some(JobResult::Reloaded {
                full_sync: false,
                ..
            })
        ));
    }

    #[test]
    fn key_presses_start_manual_reloads() {
        let (mut refresher, jobs) = recording();
        request(&mut refresher, RefreshKind::Full, "main");
        request(&mut refresher, RefreshKind::Local, "main");
        finish(&jobs, 0);
        poll(&mut refresher, false);
        assert!(!jobs.borrow()[0].0.automatic);
        assert!(!jobs.borrow()[1].0.automatic, "queued by a key");
    }

    #[test]
    fn a_due_sync_wins_over_a_due_check() {
        let (mut refresher, jobs) = recording();
        refresher.auto = true;
        let now = Instant::now();
        let minute = Duration::from_secs(60);
        refresher.set_sync_interval(Some(minute), now);
        poll_at(&mut refresher, false, now + minute);
        assert_eq!(jobs.borrow().len(), 1);
        assert_eq!(jobs.borrow()[0].0.kind, RefreshKind::Full);
    }

    #[test]
    fn a_manual_full_sync_restarts_the_sync_clock() {
        let (mut refresher, jobs) = recording();
        let start = Instant::now();
        let minute = Duration::from_secs(60);
        refresher.set_sync_interval(Some(minute), start);

        // R at 50 s; it finishes, so the next automatic sync is at 110 s
        refresher.start(
            RefreshKind::Full,
            "main",
            start + Duration::from_secs(50),
            &mut None,
            false,
        );
        finish(&jobs, 0);
        poll_at(&mut refresher, false, start + Duration::from_secs(60));
        assert_eq!(jobs.borrow().len(), 1);
        poll_at(&mut refresher, false, start + Duration::from_secs(110));
        assert_eq!(jobs.borrow().len(), 2);
    }

    #[test]
    fn no_automatic_sync_while_a_result_is_held() {
        let (mut refresher, jobs) = recording();
        let now = Instant::now();
        let minute = Duration::from_secs(60);
        refresher.set_sync_interval(Some(minute), now);
        assert!(poll_at(&mut refresher, true, now + minute).is_none());
        assert!(jobs.borrow().is_empty());
    }

    #[test]
    fn check_soon_makes_a_check_due_at_once() {
        let (mut refresher, jobs) = recording();
        refresher.auto = true;
        let now = Instant::now();
        poll_at(&mut refresher, false, now);
        finish_unchanged(&jobs, 0);
        poll_at(&mut refresher, false, now);
        assert_eq!(jobs.borrow().len(), 1);

        refresher.check_soon();
        poll_at(&mut refresher, false, now);
        assert_eq!(jobs.borrow().len(), 2);
    }

    #[test]
    fn check_soon_does_nothing_while_automatic_refresh_is_off() {
        let (mut refresher, jobs) = recording();
        refresher.check_soon();
        poll(&mut refresher, false);
        assert!(jobs.borrow().is_empty());
    }

    #[test]
    fn no_automatic_check_while_a_result_is_held() {
        let (mut refresher, jobs) = recording();
        refresher.auto = true;
        let now = Instant::now();
        request(&mut refresher, RefreshKind::Local, "main");
        finish(&jobs, 0);
        assert!(poll_at(&mut refresher, true, now + DEFAULT_AUTO_REFRESH_INTERVAL).is_none());
        assert_eq!(jobs.borrow().len(), 1);
    }

    #[test]
    fn manual_reloads_ignore_the_fingerprint_and_update_it() {
        let (mut refresher, jobs) = recording();
        refresher.set_fingerprint(Some("old".into()));
        request(&mut refresher, RefreshKind::Local, "main");
        assert_eq!(jobs.borrow()[0].0.known_fingerprint, None);
        finish(&jobs, 0);
        poll(&mut refresher, false);

        refresher.auto = true;
        poll_at(
            &mut refresher,
            false,
            Instant::now() + DEFAULT_AUTO_REFRESH_INTERVAL,
        );
        assert_eq!(
            jobs.borrow()[1].0.known_fingerprint.as_deref(),
            Some("refs-1")
        );
    }

    #[test]
    fn a_failed_reload_is_not_retried_until_the_refs_change() {
        let (mut refresher, jobs) = recording();
        refresher.auto = true;
        refresher.set_fingerprint(Some("old".into()));
        let start = Instant::now();
        poll_at(&mut refresher, false, start);
        send(&jobs, 0, |generation| JobResult::Failed {
            generation,
            error: "no trunk".to_string(),
            fingerprint: Some("new".into()),
            pr_cache: None,
        });
        assert!(matches!(
            poll_at(&mut refresher, false, start),
            Some(JobResult::Failed { .. })
        ));

        poll_at(&mut refresher, false, start + DEFAULT_AUTO_REFRESH_INTERVAL);
        assert_eq!(
            jobs.borrow()[1].0.known_fingerprint.as_deref(),
            Some("new"),
            "the next check only reloads when the refs change again"
        );
    }
}
