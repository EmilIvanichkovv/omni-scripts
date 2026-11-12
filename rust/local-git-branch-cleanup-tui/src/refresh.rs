//! Background reloads of the branch list (docs/specs/LIVE_REFRESH.md).
//!
//! A reload runs on its own thread and reports back on a channel that the
//! main loop drains every tick. At most one reload runs at a time; a request
//! made meanwhile is queued (one at most). Every reload carries the generation
//! it was started with, and a result from an older generation is dropped, so
//! a reload that read git before a deletion can never bring deleted branches
//! back.

use crate::git::{self, BranchInfo};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

/// How often automatic refresh checks whether the refs changed
pub const AUTO_CHECK_INTERVAL: Duration = Duration::from_secs(2);

/// What a reload re-reads
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RefreshKind {
    /// Automatic: reload only when the refs fingerprint changed
    Check,
    /// Local git state only (`r`)
    Local,
}

/// A reload to run in the background
#[derive(Debug, Clone)]
pub struct Job {
    pub generation: u64,
    /// Trunk to classify against (the one the TUI started with)
    pub trunk: String,
    /// Skip the reload when the refs still match this fingerprint
    pub known_fingerprint: Option<String>,
}

/// The outcome of a background reload
#[derive(Debug)]
pub enum JobResult {
    Reloaded {
        generation: u64,
        branches: Vec<BranchInfo>,
        /// Refs fingerprint read just before the branches
        fingerprint: Option<String>,
    },
    /// The refs still match the known fingerprint; nothing was re-read
    Unchanged {
        generation: u64,
    },
    Failed {
        generation: u64,
        error: String,
    },
}

impl JobResult {
    fn generation(&self) -> u64 {
        match self {
            JobResult::Reloaded { generation, .. }
            | JobResult::Unchanged { generation }
            | JobResult::Failed { generation, .. } => *generation,
        }
    }
}

/// Starts a job; the real one spawns a thread, tests record the jobs
type Launcher = Box<dyn Fn(Job, Sender<JobResult>)>;

/// Run a reload on the calling thread
fn run_job(job: &Job) -> JobResult {
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
        },
        Err(e) => JobResult::Failed {
            generation: job.generation,
            error: e.to_string(),
        },
    }
}

/// Launch a reload on a plain thread (not tokio's blocking pool, whose
/// shutdown would wait for a slow git call when the user quits)
fn spawn_job(job: Job, tx: Sender<JobResult>) {
    thread::spawn(move || {
        let _ = tx.send(run_job(&job));
    });
}

/// Schedules background reloads and hands finished ones to the app
pub struct Refresher {
    generation: u64,
    /// Generation and kind of the reload that is running
    in_flight: Option<(u64, RefreshKind)>,
    /// Reload to start when the running one finishes
    queued: Option<RefreshKind>,
    /// Finished result held back while the app cannot apply it
    pending: Option<JobResult>,
    tx: Sender<JobResult>,
    rx: Receiver<JobResult>,
    launcher: Launcher,
    /// Whether to check for ref changes every `AUTO_CHECK_INTERVAL`
    pub auto: bool,
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
            queued: None,
            pending: None,
            tx,
            rx,
            launcher,
            auto: false,
            fingerprint: None,
            last_started: None,
        }
    }

    /// Record the refs the branch list on screen was read from
    pub fn set_fingerprint(&mut self, fingerprint: Option<String>) {
        self.fingerprint = fingerprint;
    }

    /// The kind of the reload that is running, if any
    pub fn running(&self) -> Option<RefreshKind> {
        self.in_flight.map(|(_, kind)| kind)
    }

    /// Start a reload, or queue it behind the running one. Queued requests
    /// merge into one, keeping the widest kind.
    pub fn request(&mut self, kind: RefreshKind, trunk: &str) {
        if self.in_flight.is_some() {
            self.queued = self.queued.max(Some(kind));
        } else {
            self.start(kind, trunk, Instant::now());
        }
    }

    /// Make every result started so far stale (call before changing the
    /// branches on the main thread, e.g. deleting)
    pub fn invalidate(&mut self) {
        self.generation += 1;
        self.pending = None;
    }

    fn start(&mut self, kind: RefreshKind, trunk: &str, now: Instant) {
        self.generation += 1;
        self.in_flight = Some((self.generation, kind));
        self.last_started = Some(now);
        let job = Job {
            generation: self.generation,
            trunk: trunk.to_string(),
            known_fingerprint: match kind {
                RefreshKind::Check => self.fingerprint.clone(),
                RefreshKind::Local => None,
            },
        };
        (self.launcher)(job, self.tx.clone());
    }

    /// Collect finished reloads, start a queued one or an automatic check
    /// that is due, and return the newest current result unless `hold` is
    /// set (the app keeps it until `hold` clears, e.g. while the delete
    /// confirmation is open)
    pub fn poll(&mut self, hold: bool, trunk: &str, now: Instant) -> Option<JobResult> {
        while let Ok(result) = self.rx.try_recv() {
            let generation = result.generation();
            if self.in_flight.is_some_and(|(g, _)| g == generation) {
                self.in_flight = None;
            }
            if generation != self.generation {
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
                self.start(kind, trunk, now);
            } else if self.auto_check_due(hold, now) {
                self.start(RefreshKind::Check, trunk, now);
            }
        }
        // A result is only current while no newer reload was started
        if self
            .pending
            .as_ref()
            .is_some_and(|p| p.generation() != self.generation)
        {
            self.pending = None;
        }
        if hold {
            return None;
        }
        let result = self.pending.take();
        if let Some(JobResult::Reloaded { fingerprint, .. }) = &result {
            self.fingerprint = fingerprint.clone();
        }
        result
    }

    /// Whether an automatic check should start now: on, nothing waiting to
    /// be applied, and `AUTO_CHECK_INTERVAL` since the last reload started
    fn auto_check_due(&self, hold: bool, now: Instant) -> bool {
        self.auto
            && !hold
            && self.pending.is_none()
            && self
                .last_started
                .is_none_or(|at| now.duration_since(at) >= AUTO_CHECK_INTERVAL)
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

    fn finish(jobs: &Recorded, index: usize) {
        let jobs = jobs.borrow();
        let (job, tx) = &jobs[index];
        tx.send(JobResult::Reloaded {
            generation: job.generation,
            branches: Vec::new(),
            fingerprint: Some(format!("refs-{}", job.generation)),
        })
        .unwrap();
    }

    fn finish_unchanged(jobs: &Recorded, index: usize) {
        let jobs = jobs.borrow();
        let (job, tx) = &jobs[index];
        tx.send(JobResult::Unchanged {
            generation: job.generation,
        })
        .unwrap();
    }

    #[test]
    fn automatic_checks_run_only_when_on_and_due() {
        let (mut refresher, jobs) = recording();
        let now = Instant::now();
        refresher.poll(false, "main", now);
        assert!(jobs.borrow().is_empty(), "off by default");

        refresher.auto = true;
        refresher.set_fingerprint(Some("refs-0".into()));
        refresher.poll(false, "main", now);
        assert_eq!(jobs.borrow().len(), 1);
        assert_eq!(refresher.running(), Some(RefreshKind::Check));
        assert_eq!(
            jobs.borrow()[0].0.known_fingerprint.as_deref(),
            Some("refs-0")
        );

        finish_unchanged(&jobs, 0);
        assert!(refresher.poll(false, "main", now).is_none());
        assert_eq!(jobs.borrow().len(), 1, "not due yet");

        refresher.poll(false, "main", now + AUTO_CHECK_INTERVAL);
        assert_eq!(jobs.borrow().len(), 2);
    }

    #[test]
    fn no_automatic_check_while_a_result_is_held() {
        let (mut refresher, jobs) = recording();
        refresher.auto = true;
        let now = Instant::now();
        refresher.request(RefreshKind::Local, "main");
        finish(&jobs, 0);
        assert!(refresher
            .poll(true, "main", now + AUTO_CHECK_INTERVAL)
            .is_none());
        assert_eq!(jobs.borrow().len(), 1);
    }

    #[test]
    fn manual_reloads_ignore_the_fingerprint_and_update_it() {
        let (mut refresher, jobs) = recording();
        refresher.set_fingerprint(Some("old".into()));
        refresher.request(RefreshKind::Local, "main");
        assert_eq!(jobs.borrow()[0].0.known_fingerprint, None);
        finish(&jobs, 0);
        refresher.poll(false, "main", Instant::now());

        refresher.auto = true;
        refresher.poll(false, "main", Instant::now() + AUTO_CHECK_INTERVAL);
        assert_eq!(
            jobs.borrow()[1].0.known_fingerprint.as_deref(),
            Some("refs-1")
        );
    }

    #[test]
    fn a_finished_reload_is_returned_once() {
        let (mut refresher, jobs) = recording();
        refresher.request(RefreshKind::Local, "main");
        assert_eq!(refresher.running(), Some(RefreshKind::Local));
        assert!(refresher.poll(false, "main", Instant::now()).is_none());

        finish(&jobs, 0);
        assert!(refresher.poll(false, "main", Instant::now()).is_some());
        assert_eq!(refresher.running(), None);
        assert!(refresher.poll(false, "main", Instant::now()).is_none());
    }

    #[test]
    fn requests_during_a_reload_queue_a_single_follow_up() {
        let (mut refresher, jobs) = recording();
        refresher.request(RefreshKind::Local, "main");
        refresher.request(RefreshKind::Local, "main");
        refresher.request(RefreshKind::Local, "main");
        assert_eq!(jobs.borrow().len(), 1);

        finish(&jobs, 0);
        // The first result is superseded by the follow-up that starts now
        assert!(refresher.poll(false, "main", Instant::now()).is_none());
        assert_eq!(jobs.borrow().len(), 2);

        finish(&jobs, 1);
        assert!(refresher.poll(false, "main", Instant::now()).is_some());
        assert_eq!(jobs.borrow().len(), 2);
    }

    #[test]
    fn a_reload_started_before_invalidate_is_dropped() {
        let (mut refresher, jobs) = recording();
        refresher.request(RefreshKind::Local, "main");
        refresher.invalidate();
        finish(&jobs, 0);
        assert!(refresher.poll(false, "main", Instant::now()).is_none());
        assert_eq!(refresher.running(), None);
    }

    #[test]
    fn a_held_result_is_returned_when_the_hold_ends() {
        let (mut refresher, jobs) = recording();
        refresher.request(RefreshKind::Local, "main");
        finish(&jobs, 0);
        assert!(refresher.poll(true, "main", Instant::now()).is_none());
        assert!(refresher.poll(true, "main", Instant::now()).is_none());
        assert!(refresher.poll(false, "main", Instant::now()).is_some());
    }

    #[test]
    fn a_held_result_is_dropped_by_invalidate() {
        let (mut refresher, jobs) = recording();
        refresher.request(RefreshKind::Local, "main");
        finish(&jobs, 0);
        assert!(refresher.poll(true, "main", Instant::now()).is_none());
        refresher.invalidate();
        assert!(refresher.poll(false, "main", Instant::now()).is_none());
    }

    #[test]
    fn jobs_carry_the_trunk() {
        let (mut refresher, jobs) = recording();
        refresher.request(RefreshKind::Local, "develop");
        assert_eq!(jobs.borrow()[0].0.trunk, "develop");
    }
}
