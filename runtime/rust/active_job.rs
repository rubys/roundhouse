//! ActiveJob runtime for the emitted Rust project — hand-written
//! counterpart of `runtime/ruby/active_job.rb` (read its comments for
//! the why of every posture below).
//!
//! The emitted `perform_later` bodies call:
//!   `record_performed(name)`, `enqueue_only()`, `drain_registered()`,
//!   `enqueue(|| { ...; None })`, `hold(name, || { ...; None })`.
//!
//! Three postures, selected by process-global state (global rather
//! than thread-local: the drain thread spawned by `main` must see what
//! request threads enqueue, exactly like the Ruby module constants):
//!   * inline (default): the caller runs the job itself;
//!   * `:test` (`enqueue_only()`): work is held, `perform_held` runs it;
//!   * queued (`drain_registered()`): work goes to a FIFO, `drain` runs it.
//!
//! Because the state is process-wide, Rust tests that assert on it in
//! parallel must serialize themselves (the emitted harness pushes
//! `enqueue_without_running` once at boot, like the Ruby one).
//!
//! The job closures are `FnOnce() -> Option<()> + Send + 'static`: the
//! emitted body ends in a literal `None` (Ruby's `nil`), so the return
//! type must be pinned to something `None` can infer. `enqueue` and
//! `hold` answer `R: Default` (Ruby's `nil`) so they unify with the
//! sibling `Self::new().perform(..)` branch in the emitted if/else
//! chain whatever that branch's type is.

use std::collections::VecDeque;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{Mutex, MutexGuard};

type Work = Box<dyn FnOnce() -> Option<()> + Send + 'static>;

struct State {
    performed: Vec<String>,
    /// Suspension depth (`ENQUEUE_ONLY`).
    enqueue_only: usize,
    /// Drains registered (`DRAINED`).
    drained: usize,
    pending: VecDeque<Work>,
    held: Vec<(String, Work)>,
}

static STATE: Mutex<State> = Mutex::new(State {
    performed: Vec::new(),
    enqueue_only: 0,
    drained: 0,
    pending: VecDeque::new(),
    held: Vec::new(),
});

/// A panicking job must not poison the queue for everyone else.
fn state() -> MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

pub struct ActiveJob;

impl ActiveJob {
    /// Append the job's class name to the performed log.
    pub fn record_performed(job_name: &str) {
        state().performed.push(job_name.to_string());
    }

    /// Jobs recorded so far, in call order. Never reset (tests read
    /// length deltas, like `Broadcasts::LOG`).
    pub fn performed() -> Vec<String> {
        state().performed.clone()
    }

    /// Whether the `:test` adapter is active (perform_later holds).
    pub fn enqueue_only() -> bool {
        state().enqueue_only > 0
    }

    /// Switch to the `:test` adapter (nests).
    pub fn enqueue_without_running() {
        state().enqueue_only += 1;
    }

    /// Back out one level of `enqueue_without_running`.
    pub fn run_enqueued() {
        let mut s = state();
        s.enqueue_only = s.enqueue_only.saturating_sub(1);
    }

    /// Whether a drain exists to pick queued work up.
    pub fn drain_registered() -> bool {
        state().drained > 0
    }

    pub fn register_drain() {
        state().drained += 1;
    }

    /// Put work on the FIFO queue and return (Ruby: `nil`).
    pub fn enqueue<F, R>(work: F) -> R
    where
        F: FnOnce() -> Option<()> + Send + 'static,
        R: Default,
    {
        state().pending.push_back(Box::new(work));
        R::default()
    }

    pub fn pending_count() -> usize {
        state().pending.len()
    }

    /// Run every queued job FIFO and answer how many ran. A panicking
    /// job is one lost job, never a lost drain; it is reported on
    /// stderr (Ruby: `warn`). Jobs enqueued by a job run in this pass.
    pub fn drain() -> usize {
        let mut ran = 0;
        loop {
            // Lock released before the job runs: it may enqueue.
            let next = state().pending.pop_front();
            let Some(work) = next else { break };
            match catch_unwind(AssertUnwindSafe(work)) {
                Ok(_) => ran += 1,
                Err(e) => {
                    let msg = e
                        .downcast_ref::<String>()
                        .cloned()
                        .or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string()))
                        .unwrap_or_default();
                    eprintln!("[job] a queued job raised: {msg}");
                }
            }
        }
        ran
    }

    /// `:test` adapter: keep the work (with its name for `only:`).
    pub fn hold<F, R>(job_name: &str, work: F) -> R
    where
        F: FnOnce() -> Option<()> + Send + 'static,
        R: Default,
    {
        state().held.push((job_name.to_string(), Box::new(work)));
        R::default()
    }

    /// Run held jobs named in `only` (all when empty), in enqueue order,
    /// answering how many ran. A job a held job holds runs in the same
    /// pass; jobs `only` leaves out stay held. A panic propagates (as
    /// in Ruby: the test sees it).
    pub fn perform_held(only: &[&str]) -> usize {
        let mut ran = 0;
        let mut i = 0;
        loop {
            let work = {
                let mut s = state();
                if i >= s.held.len() {
                    break;
                }
                if only.is_empty() || only.contains(&s.held[i].0.as_str()) {
                    Some(s.held.remove(i).1)
                } else {
                    None
                }
            };
            match work {
                Some(w) => {
                    w();
                    ran += 1;
                }
                None => i += 1,
            }
        }
        ran
    }

    pub fn held_count() -> usize {
        state().held.len()
    }

    /// Every test starts with nothing enqueued.
    pub fn clear_held() {
        state().held.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex as StdMutex};

    // State is process-global: serialize the tests that touch it.
    static GUARD: StdMutex<()> = StdMutex::new(());

    fn reset() {
        let mut s = state();
        s.performed.clear();
        s.enqueue_only = 0;
        s.drained = 0;
        s.pending.clear();
        s.held.clear();
    }

    // Shape copied from app_classes/*_job_class.rs (types simplified).
    struct Job;
    impl Job {
        fn new() -> Self { Job }
        fn perform(&self, n: Arc<AtomicUsize>, v: serde_json::Value) -> serde_json::Value {
            n.fetch_add(1, Ordering::SeqCst);
            v
        }
        fn perform_later(n: Arc<AtomicUsize>, room: serde_json::Value) {
            ActiveJob::record_performed("Room::PushMessageJob");
            if !(ActiveJob::enqueue_only()) { if ActiveJob::drain_registered() { ActiveJob::enqueue(move || {
                Self::new().perform(n.clone(), room.clone());
                None
            }) } else { Self::new().perform(n.clone(), room.clone()) } } else { ActiveJob::hold("Room::PushMessageJob", move || {
                Self::new().perform(n.clone(), room.clone());
                None
            }) };
        }
    }

    #[test]
    fn inline_runs_at_call_site() {
        let _g = GUARD.lock().unwrap_or_else(|e| e.into_inner());
        reset();
        let n = Arc::new(AtomicUsize::new(0));
        Job::perform_later(n.clone(), serde_json::json!(1));
        assert_eq!(n.load(Ordering::SeqCst), 1);
        assert_eq!(ActiveJob::performed(), vec!["Room::PushMessageJob"]);
    }

    #[test]
    fn test_adapter_holds_then_performs() {
        let _g = GUARD.lock().unwrap_or_else(|e| e.into_inner());
        reset();
        let n = Arc::new(AtomicUsize::new(0));
        ActiveJob::enqueue_without_running();
        ActiveJob::enqueue_without_running();
        Job::perform_later(n.clone(), serde_json::json!(1));
        Job::perform_later(n.clone(), serde_json::json!(2));
        assert_eq!(n.load(Ordering::SeqCst), 0);
        assert_eq!(ActiveJob::held_count(), 2);
        assert_eq!(ActiveJob::perform_held(&["Other"]), 0);
        assert_eq!(ActiveJob::perform_held(&[]), 2);
        assert_eq!(n.load(Ordering::SeqCst), 2);
        ActiveJob::run_enqueued();
        assert!(ActiveJob::enqueue_only()); // nested: still suspended
        ActiveJob::run_enqueued();
        assert!(!ActiveJob::enqueue_only());
    }

    #[test]
    fn queue_drains_fifo_and_survives_a_panic() {
        let _g = GUARD.lock().unwrap_or_else(|e| e.into_inner());
        reset();
        ActiveJob::register_drain();
        let order = Arc::new(StdMutex::new(Vec::new()));
        for i in 0..3 {
            let o = order.clone();
            ActiveJob::enqueue::<_, ()>(move || {
                if i == 1 { panic!("boom"); }
                o.lock().unwrap().push(i);
                None
            });
        }
        assert_eq!(ActiveJob::pending_count(), 3);
        assert_eq!(ActiveJob::drain(), 2);
        assert_eq!(*order.lock().unwrap(), vec![0, 2]);
        assert_eq!(ActiveJob::pending_count(), 0);
        // Lock not poisoned afterwards.
        assert!(ActiveJob::drain_registered());
    }
}
