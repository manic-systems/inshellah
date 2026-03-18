//! BFS-queue worker pool for parallel subprocess scraping.
//!
//! workers pull jobs from a shared queue and call a user-supplied
//! handler; the handler gets a `Submitter` to push newly-discovered
//! child jobs back onto the same queue. when the in-flight count
//! reaches zero the pool shuts down and `wait` returns.
//!
//! the queue-back design is deliberate: command-help trees are uneven
//! (one binary has 30 subs, another has 1). queue-back keeps every
//! worker fed; spawn-in-place would leave cores idle on lopsided trees.
//!
//! synchronization: `parking_lot::Condvar` parks workers when the queue is
//! empty. the queue, in-flight count, and close state live under one mutex so
//! the condvar predicate cannot miss a wakeup.
//! parking_lot gives no-poison locks (no `Result` noise on every
//! `lock()`) and a single-syscall fast path in the uncontended case.

use std::collections::VecDeque;
use std::sync::Arc;
use std::thread::{self, JoinHandle};

use parking_lot::{Condvar, Mutex};

struct State<J> {
    queue: VecDeque<J>,
    /// jobs created but not yet completed. counts both queued and
    /// in-progress jobs. workers can exit once wait() has closed the pool
    /// and this reaches 0.
    in_flight: usize,
    /// set by wait(), which is also the point where top-level submission is
    /// done. workers must not exit on transient empty periods before this.
    closed: bool,
}

/// shared state held behind an `Arc` by every worker and by the
/// submitter handles handed to the per-job handler.
struct Inner<J> {
    state: Mutex<State<J>>,
    notify: Condvar,
}

impl<J> Inner<J> {
    fn submit(&self, job: J) {
        let mut state = self.state.lock();
        state.in_flight += 1;
        state.queue.push_back(job);
        self.notify.notify_one();
    }

    fn next(&self) -> Option<J> {
        let mut state = self.state.lock();
        loop {
            if let Some(job) = state.queue.pop_front() {
                return Some(job);
            }
            if state.closed && state.in_flight == 0 {
                return None;
            }
            self.notify.wait(&mut state);
        }
    }

    fn complete(&self) {
        let mut state = self.state.lock();
        state.in_flight -= 1;
        if state.closed && state.in_flight == 0 {
            // we were the last in-flight job after wait() closed top-level
            // submission, so parked workers can wake and exit.
            self.notify.notify_all();
        }
    }
}

/// cheap-to-clone handle that lets a job handler enqueue further jobs.
/// passed by reference to the handler closure.
pub struct Submitter<J> {
    inner: Arc<Inner<J>>,
}

impl<J> Clone for Submitter<J> {
    fn clone(&self) -> Self {
        Submitter {
            inner: self.inner.clone(),
        }
    }
}

impl<J> Submitter<J> {
    pub fn submit(&self, job: J) {
        self.inner.submit(job);
    }
}

/// BFS-queue worker pool. each worker pulls a job, calls the handler
/// (which may submit further jobs via the passed `Submitter`), then marks
/// the job complete. when in-flight reaches zero the pool shuts down and
/// `wait` returns.
pub struct ScrapePool<J> {
    inner: Arc<Inner<J>>,
    workers: Vec<JoinHandle<()>>,
}

impl<J: Send + 'static> ScrapePool<J> {
    /// spawn `num_workers` threads that run `handler` on each job pulled
    /// from the queue. the handler receives the job by value and a
    /// `&Submitter` for enqueuing children.
    pub fn new<F>(num_workers: usize, handler: F) -> Self
    where
        F: Fn(J, &Submitter<J>) + Send + Sync + 'static,
    {
        let inner = Arc::new(Inner {
            state: Mutex::new(State {
                queue: VecDeque::new(),
                in_flight: 0,
                closed: false,
            }),
            notify: Condvar::new(),
        });
        let handler = Arc::new(handler);
        let workers = (0..num_workers.max(1))
            .map(|_| {
                let inner = inner.clone();
                let handler = handler.clone();
                thread::spawn(move || {
                    let submitter = Submitter {
                        inner: inner.clone(),
                    };
                    while let Some(job) = inner.next() {
                        handler(job, &submitter);
                        inner.complete();
                    }
                })
            })
            .collect();
        ScrapePool { inner, workers }
    }

    /// submit a top-level job. typically called by the orchestrating
    /// thread before `wait`; handlers should use `Submitter::submit`.
    pub fn submit(&self, job: J) {
        self.inner.submit(job);
    }

    /// block until all jobs (initial + transitively discovered) have
    /// completed, then join every worker thread.
    pub fn wait(self) {
        {
            let mut state = self.inner.state.lock();
            state.closed = true;
            // Wake workers so they can either drain queued work or exit if
            // the pool was empty. The close flag is guarded by this same lock,
            // so this cannot race with a worker entering the condvar wait.
            self.inner.notify.notify_all();
        }
        for w in self.workers {
            let _ = w.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    #[test]
    fn flat_jobs_processed_once_each() {
        let collected: Arc<Mutex<Vec<u32>>> = Arc::new(Mutex::new(Vec::new()));
        let pool = ScrapePool::new(4, {
            let collected = collected.clone();
            move |n: u32, _: &Submitter<u32>| {
                collected.lock().push(n);
            }
        });
        for i in 0..100u32 {
            pool.submit(i);
        }
        pool.wait();
        let mut got = collected.lock().clone();
        got.sort();
        assert_eq!(got, (0..100).collect::<Vec<_>>());
    }

    #[test]
    fn discovered_children_processed_to_completion() {
        // BFS expansion: every odd number under 10 spawns its successor.
        let collected: Arc<Mutex<Vec<u32>>> = Arc::new(Mutex::new(Vec::new()));
        let pool = ScrapePool::new(2, {
            let collected = collected.clone();
            move |n: u32, sub: &Submitter<u32>| {
                collected.lock().push(n);
                if n < 10 && n % 2 == 1 {
                    sub.submit(n + 1);
                }
            }
        });
        for i in [1u32, 3, 5, 7, 9] {
            pool.submit(i);
        }
        pool.wait();
        let mut got = collected.lock().clone();
        got.sort();
        assert_eq!(got, vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
    }

    #[test]
    fn transient_empty_queue_before_wait_does_not_stop_workers() {
        let processed = Arc::new(AtomicUsize::new(0));
        let pool = ScrapePool::new(1, {
            let processed = processed.clone();
            move |_: u32, _: &Submitter<u32>| {
                processed.fetch_add(1, Ordering::SeqCst);
            }
        });

        pool.submit(1);
        while processed.load(Ordering::SeqCst) == 0 {
            thread::yield_now();
        }
        thread::sleep(Duration::from_millis(10));
        pool.submit(2);
        pool.wait();

        assert_eq!(processed.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn wait_with_no_jobs_returns_immediately() {
        let pool: ScrapePool<()> = ScrapePool::new(2, |_, _| {});
        pool.wait();
    }
}
