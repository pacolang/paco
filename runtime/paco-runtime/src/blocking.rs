use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::join::{self, JoinHandle};
use crate::scheduler::Runtime;

const IDLE_TIMEOUT: Duration = Duration::from_secs(5);

type Job = Box<dyn FnOnce() + Send>;

struct Pool {
    jobs: Mutex<std::collections::VecDeque<Job>>,
    signal: (Mutex<()>, std::sync::Condvar),
    idle: AtomicUsize,
    threads: AtomicUsize,
}

pub struct BlockingPool {
    pool: Arc<Pool>,
}

impl Default for BlockingPool {
    fn default() -> Self {
        Self::new()
    }
}

impl BlockingPool {
    pub fn new() -> Self {
        Self {
            pool: Arc::new(Pool {
                jobs: Mutex::new(std::collections::VecDeque::new()),
                signal: (Mutex::new(()), std::sync::Condvar::new()),
                idle: AtomicUsize::new(0),
                threads: AtomicUsize::new(0),
            }),
        }
    }

    pub fn spawn<T, F>(&self, runtime: &Runtime, f: F) -> JoinHandle<T>
    where
        T: Send + 'static,
        F: FnOnce() -> T + Send + 'static,
    {
        let slot = join::new_slot();
        let body_slot = slot.clone();
        let abandon_slot = slot.clone();
        self.submit(Box::new(move || {
            crate::task::run_abandonable(
                move || {
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f))
                        .map_err(join::TaskPanic::from_payload);
                    join::complete(&body_slot, result);
                },
                |message| join::complete(&abandon_slot, Err(join::TaskPanic { message })),
            );
        }));
        JoinHandle::from_parts(slot, runtime.shared().clone())
    }

    fn submit(&self, job: Job) {
        self.pool.jobs.lock().unwrap().push_back(job);
        if self.pool.idle.load(Ordering::Acquire) == 0 {
            self.spawn_worker();
        }
        let (lock, cv) = &self.pool.signal;
        let _guard = lock.lock().unwrap();
        cv.notify_one();
    }

    fn spawn_worker(&self) {
        self.pool.threads.fetch_add(1, Ordering::AcqRel);
        let pool = self.pool.clone();
        std::thread::spawn(move || worker_loop(pool));
    }
}

fn worker_loop(pool: Arc<Pool>) {
    loop {
        let job = pool.jobs.lock().unwrap().pop_front();
        match job {
            Some(job) => job(),
            None => {
                pool.idle.fetch_add(1, Ordering::AcqRel);
                let (lock, cv) = &pool.signal;
                let guard = lock.lock().unwrap();
                if !pool.jobs.lock().unwrap().is_empty() {
                    pool.idle.fetch_sub(1, Ordering::AcqRel);
                    continue;
                }
                let (_guard, timeout) = cv.wait_timeout(guard, IDLE_TIMEOUT).unwrap();
                pool.idle.fetch_sub(1, Ordering::AcqRel);
                if timeout.timed_out() && pool.jobs.lock().unwrap().is_empty() {
                    pool.threads.fetch_sub(1, Ordering::AcqRel);
                    return;
                }
            }
        }
    }
}
