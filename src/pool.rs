// SPDX-License-Identifier: EUPL-1.2
//! bfs-queue worker pool for parallel subprocess scraping.
//!
//! workers pull jobs from a shared queue and call a handler that can push child
//! jobs back via a `Submitter`; when in-flight hits zero the pool shuts down
//! and `wait` returns.
//!
//! queue-back (not spawn-in-place) keeps workers fed on lopsided help trees
//! (one binary has 30 subs, another 1). a condvar parks idle
//! workers; queue + in-flight + close state share one mutex so a wakeup can't
//! be missed.

use std::{
    collections::VecDeque,
    iter,
    panic::{
        AssertUnwindSafe,
        catch_unwind,
    },
    sync::{
        Arc,
        Condvar,
        Mutex,
        PoisonError,
        atomic::{
            AtomicUsize,
            Ordering,
        },
    },
    thread::{
        self,
        JoinHandle,
    },
};

struct State<J> {
    queue:     VecDeque<J>,
    /// queued + in-progress jobs. workers exit once `wait()` closed the pool
    /// and this hits 0.
    in_flight: usize,
    /// set by `wait()`, once top-level submission is done. workers must not
    /// exit on transient empties before this.
    closed:    bool,
}

/// shared state behind an `Arc`, held by every worker and submitter.
struct Inner<J> {
    state:  Mutex<State<J>>,
    notify: Condvar,
}

impl<J> Inner<J> {
    fn submit(&self, job: J) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.in_flight += 1;
        state.queue.push_back(job);
        drop(state);
        self.notify.notify_one();
    }

    fn next(&self) -> Option<J> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        loop {
            if let Some(job) = state.queue.pop_front() {
                return Some(job);
            }
            if state.closed && state.in_flight == 0 {
                return None;
            }
            state = self
                .notify
                .wait(state)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }

    fn complete(&self) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.in_flight -= 1;
        if state.closed && state.in_flight == 0 {
            // last in-flight after wait() closed submission; wake workers to
            // exit.
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
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl<J> Submitter<J> {
    pub fn submit(&self, job: J) {
        self.inner.submit(job);
    }
}

pub struct ScrapePool<J> {
    inner:   Arc<Inner<J>>,
    workers: Vec<JoinHandle<()>>,
}

impl<J: Send + 'static> ScrapePool<J> {
    /// spawn `num_workers` running `handler` on each job; the handler gets the
    /// job by value and a `&Submitter` for children.
    #[expect(
        clippy::let_underscore_must_use,
        reason = "a handler panic is contained per job, the worker moves on to the next"
    )]
    pub fn new<F>(num_workers: usize, handler: F) -> Self
    where
        F: Fn(J, &Submitter<J>) + Send + Sync + 'static,
    {
        let inner = Arc::new(Inner {
            state:  Mutex::new(State {
                queue:     VecDeque::new(),
                in_flight: 0,
                closed:    false,
            }),
            notify: Condvar::new(),
        });
        let shared_handler = Arc::new(handler);
        let workers = iter::repeat_with(|| {
            let worker_inner = Arc::clone(&inner);
            let worker_handler = Arc::clone(&shared_handler);
            thread::spawn(move || {
                let submitter = Submitter {
                    inner: Arc::clone(&worker_inner),
                };
                while let Some(job) = worker_inner.next() {
                    // panics must not strand in_flight
                    let _: thread::Result<()> =
                        catch_unwind(AssertUnwindSafe(|| worker_handler(job, &submitter)));
                    worker_inner.complete();
                }
            })
        })
        .take(num_workers.max(1))
        .collect();
        Self { inner, workers }
    }

    /// submit a top-level job (before `wait`); handlers use
    /// `Submitter::submit`.
    pub fn submit(&self, job: J) {
        self.inner.submit(job);
    }

    /// block until all jobs complete, then join workers.
    #[expect(
        clippy::let_underscore_must_use,
        reason = "workers catch handler panics, so a join error carries nothing to report"
    )]
    pub fn wait(self) {
        self.inner
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .closed = true;
        // wake workers to drain or exit; the close flag shares this lock,
        // so it can't race a worker entering the wait.
        self.inner.notify.notify_all();
        for w in self.workers {
            let _: thread::Result<()> = w.join();
        }
    }
}

/// map `items` across `num_workers` threads, keeping input order in the output.
///
/// # Panics
///
/// panics if `transform` panics on any item.
#[expect(
    clippy::expect_used,
    reason = "the atomic counter hands each index to exactly one worker"
)]
pub fn par_map<T, R, F>(items: Vec<T>, num_workers: usize, transform: F) -> Vec<R>
where
    T: Send,
    R: Send,
    F: Fn(T) -> R + Sync,
{
    let inputs: Vec<Mutex<Option<T>>> = items
        .into_iter()
        .map(|item| Mutex::new(Some(item)))
        .collect();
    let outputs: Vec<Mutex<Option<R>>> = inputs.iter().map(|_| Mutex::new(None)).collect();
    let next = AtomicUsize::new(0);
    thread::scope(|scope| {
        for _ in 0..num_workers.max(1).min(inputs.len()) {
            scope.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(input) = inputs.get(i) else {
                        break;
                    };
                    let item = input
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .take()
                        .expect("each index is claimed once");
                    *outputs[i].lock().unwrap_or_else(PoisonError::into_inner) =
                        Some(transform(item));
                }
            });
        }
    });
    outputs
        .into_iter()
        .map(|slot| {
            slot.into_inner()
                .unwrap_or_else(PoisonError::into_inner)
                .expect("every claimed index stores a result")
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::{
        panic,
        time::Duration,
    };

    use super::*;

    #[test]
    fn flat_jobs_processed_once_each() {
        let collected: Arc<Mutex<Vec<u32>>> = Arc::new(Mutex::new(Vec::new()));
        let pool = ScrapePool::new(4, {
            let sink = Arc::clone(&collected);
            move |n: u32, _: &Submitter<u32>| {
                sink.lock().unwrap().push(n);
            }
        });
        for i in 0..100_u32 {
            pool.submit(i);
        }
        pool.wait();
        let mut got = collected.lock().unwrap().clone();
        got.sort_unstable();
        assert_eq!(got, (0..100).collect::<Vec<_>>());
    }

    #[test]
    fn discovered_children_processed_to_completion() {
        let collected: Arc<Mutex<Vec<u32>>> = Arc::new(Mutex::new(Vec::new()));
        let pool = ScrapePool::new(2, {
            let sink = Arc::clone(&collected);
            move |n: u32, sub: &Submitter<u32>| {
                sink.lock().unwrap().push(n);
                if n < 10 && n % 2 == 1 {
                    sub.submit(n + 1);
                }
            }
        });
        for i in [1_u32, 3, 5, 7, 9] {
            pool.submit(i);
        }
        pool.wait();
        let mut got = collected.lock().unwrap().clone();
        got.sort_unstable();
        assert_eq!(got, vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
    }

    #[test]
    fn transient_empty_queue_before_wait_does_not_stop_workers() {
        let processed = Arc::new(AtomicUsize::new(0));
        let pool = ScrapePool::new(1, {
            let counter = Arc::clone(&processed);
            move |_: u32, _: &Submitter<u32>| {
                counter.fetch_add(1, Ordering::SeqCst);
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
        let pool: ScrapePool<()> = ScrapePool::new(2, |(), _| {});
        pool.wait();
    }

    #[test]
    fn panicking_handler_does_not_deadlock_and_workers_survive() {
        let processed = Arc::new(AtomicUsize::new(0));
        let pool = ScrapePool::new(2, {
            let counter = Arc::clone(&processed);
            move |n: u32, _: &Submitter<u32>| {
                assert!(n != 0, "boom");
                counter.fetch_add(1, Ordering::SeqCst);
            }
        });

        let prev = panic::take_hook();
        panic::set_hook(Box::new(|_| {}));
        pool.submit(0);
        for i in 1..50_u32 {
            pool.submit(i);
        }
        pool.wait();
        panic::set_hook(prev);

        assert_eq!(processed.load(Ordering::SeqCst), 49);
    }
}
