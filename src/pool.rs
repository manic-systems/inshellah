// SPDX-License-Identifier: EUPL-1.2
//! bfs-queue worker pool for parallel subprocess scraping. workers pull jobs
//! from a shared queue and call a handler that can push child jobs back via a
//! `Submitter`; when in-flight hits zero the pool shuts down and `wait` returns.
//!
//! queue-back (not spawn-in-place) keeps workers fed on lopsided help trees
//! (one binary has 30 subs, another 1). a parking_lot condvar parks idle
//! workers; queue + in-flight + close state share one mutex so a wakeup can't
//! be missed.

use std::collections::VecDeque;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::thread::{self, JoinHandle};

use parking_lot::{Condvar, Mutex};

struct State<J> {
    queue: VecDeque<J>,
    /// queued + in-progress jobs. workers exit once wait() closed the pool and
    /// this hits 0.
    in_flight: usize,
    /// set by wait(), once top-level submission is done. workers must not exit
    /// on transient empties before this.
    closed: bool,
}

/// shared state behind an `Arc`, held by every worker and submitter.
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
            // last in-flight after wait() closed submission; wake workers to exit.
            self.notify.notify_all();
        }
    }
}

/// cheap-to-clone handle for a handler to enqueue more jobs.
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

pub struct ScrapePool<J> {
    inner: Arc<Inner<J>>,
    workers: Vec<JoinHandle<()>>,
}

impl<J: Send + 'static> ScrapePool<J> {
    /// spawn `num_workers` running `handler` on each job; the handler gets the
    /// job by value and a `&Submitter` for children.
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
                        // panics must not strand in_flight
                        let _ = catch_unwind(AssertUnwindSafe(|| handler(job, &submitter)));
                        inner.complete();
                    }
                })
            })
            .collect();
        ScrapePool { inner, workers }
    }

    /// submit a top-level job (before `wait`); handlers use `Submitter::submit`.
    pub fn submit(&self, job: J) {
        self.inner.submit(job);
    }

    /// block until all jobs complete, then join workers.
    pub fn wait(self) {
        {
            let mut state = self.inner.state.lock();
            state.closed = true;
            // wake workers to drain or exit; the close flag shares this lock,
            // so it can't race a worker entering the wait.
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

    #[test]
    fn panicking_handler_does_not_deadlock_and_workers_survive() {
        let processed = Arc::new(AtomicUsize::new(0));
        let pool = ScrapePool::new(2, {
            let processed = processed.clone();
            move |n: u32, _: &Submitter<u32>| {
                if n == 0 {
                    panic!("boom");
                }
                processed.fetch_add(1, Ordering::SeqCst);
            }
        });

        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        pool.submit(0);
        for i in 1..50u32 {
            pool.submit(i);
        }
        pool.wait();
        std::panic::set_hook(prev);

        assert_eq!(processed.load(Ordering::SeqCst), 49);
    }
}
