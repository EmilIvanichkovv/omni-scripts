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

/// What a reload re-reads
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RefreshKind {
    /// Local git state only (`r`)
    Local,
}

/// A reload to run in the background
#[derive(Debug, Clone)]
pub struct Job {
    pub generation: u64,
    /// Trunk to classify against (the one the TUI started with)
    pub trunk: String,
}

/// The outcome of a background reload
#[derive(Debug)]
pub enum JobResult {
    Reloaded {
        generation: u64,
        branches: Vec<BranchInfo>,
    },
    Failed {
        generation: u64,
        error: String,
    },
}

impl JobResult {
    fn generation(&self) -> u64 {
        match self {
            JobResult::Reloaded { generation, .. } | JobResult::Failed { generation, .. } => {
                *generation
            }
        }
    }
}

/// Starts a job; the real one spawns a thread, tests record the jobs
type Launcher = Box<dyn Fn(Job, Sender<JobResult>)>;

/// Run a reload on the calling thread
fn run_job(job: &Job) -> JobResult {
    match git::get_branches_with_classification(Some(&job.trunk)) {
        Ok(branches) => JobResult::Reloaded {
            generation: job.generation,
            branches,
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
        }
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
            self.start(kind, trunk);
        }
    }

    /// Make every result started so far stale (call before changing the
    /// branches on the main thread, e.g. deleting)
    pub fn invalidate(&mut self) {
        self.generation += 1;
        self.pending = None;
    }

    fn start(&mut self, kind: RefreshKind, trunk: &str) {
        self.generation += 1;
        self.in_flight = Some((self.generation, kind));
        let job = Job {
            generation: self.generation,
            trunk: trunk.to_string(),
        };
        (self.launcher)(job, self.tx.clone());
    }

    /// Collect finished reloads, start a queued one, and return the newest
    /// current result unless `hold` is set (the app keeps it until `hold`
    /// clears, e.g. while the delete confirmation is open)
    pub fn poll(&mut self, hold: bool, trunk: &str) -> Option<JobResult> {
        while let Ok(result) = self.rx.try_recv() {
            let generation = result.generation();
            if self.in_flight.is_some_and(|(g, _)| g == generation) {
                self.in_flight = None;
            }
            if generation == self.generation {
                self.pending = Some(result);
            }
        }
        if self.in_flight.is_none() {
            if let Some(kind) = self.queued.take() {
                self.start(kind, trunk);
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
            None
        } else {
            self.pending.take()
        }
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
        })
        .unwrap();
    }

    #[test]
    fn a_finished_reload_is_returned_once() {
        let (mut refresher, jobs) = recording();
        refresher.request(RefreshKind::Local, "main");
        assert_eq!(refresher.running(), Some(RefreshKind::Local));
        assert!(refresher.poll(false, "main").is_none());

        finish(&jobs, 0);
        assert!(refresher.poll(false, "main").is_some());
        assert_eq!(refresher.running(), None);
        assert!(refresher.poll(false, "main").is_none());
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
        assert!(refresher.poll(false, "main").is_none());
        assert_eq!(jobs.borrow().len(), 2);

        finish(&jobs, 1);
        assert!(refresher.poll(false, "main").is_some());
        assert_eq!(jobs.borrow().len(), 2);
    }

    #[test]
    fn a_reload_started_before_invalidate_is_dropped() {
        let (mut refresher, jobs) = recording();
        refresher.request(RefreshKind::Local, "main");
        refresher.invalidate();
        finish(&jobs, 0);
        assert!(refresher.poll(false, "main").is_none());
        assert_eq!(refresher.running(), None);
    }

    #[test]
    fn a_held_result_is_returned_when_the_hold_ends() {
        let (mut refresher, jobs) = recording();
        refresher.request(RefreshKind::Local, "main");
        finish(&jobs, 0);
        assert!(refresher.poll(true, "main").is_none());
        assert!(refresher.poll(true, "main").is_none());
        assert!(refresher.poll(false, "main").is_some());
    }

    #[test]
    fn a_held_result_is_dropped_by_invalidate() {
        let (mut refresher, jobs) = recording();
        refresher.request(RefreshKind::Local, "main");
        finish(&jobs, 0);
        assert!(refresher.poll(true, "main").is_none());
        refresher.invalidate();
        assert!(refresher.poll(false, "main").is_none());
    }

    #[test]
    fn jobs_carry_the_trunk() {
        let (mut refresher, jobs) = recording();
        refresher.request(RefreshKind::Local, "develop");
        assert_eq!(jobs.borrow()[0].0.trunk, "develop");
    }
}
