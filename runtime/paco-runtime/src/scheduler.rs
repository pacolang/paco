use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};

use crossbeam_deque::{Injector, Steal, Stealer, Worker};

use crate::blocking::BlockingPool;
use crate::io::IoDriver;
use crate::join::JoinHandle;
use crate::task::{self, TaskPoll, TaskRef};

pub(crate) struct SchedulerShared {
    injector: Injector<TaskRef>,
    stealers: Vec<Stealer<TaskRef>>,
    parked: (Mutex<usize>, Condvar),
    shutdown: AtomicBool,
    active: AtomicUsize,
    done: (Mutex<bool>, Condvar),
}

impl SchedulerShared {
    pub(crate) fn enqueue(&self, task: TaskRef) {
        self.injector.push(task);
        let (lock, cv) = &self.parked;
        let parked = lock.lock().unwrap();
        if *parked > 0 {
            cv.notify_one();
        }
    }

    fn task_finished(&self) {
        if self.active.fetch_sub(1, Ordering::AcqRel) == 1 {
            let (lock, cv) = &self.done;
            *lock.lock().unwrap() = true;
            cv.notify_all();
        }
    }
}

pub struct Runtime {
    shared: Arc<SchedulerShared>,
    workers: Vec<std::thread::JoinHandle<()>>,
    blocking: BlockingPool,
    io: Arc<IoDriver>,
}

impl Runtime {
    pub fn new(worker_count: usize) -> Self {
        let worker_count = worker_count.max(1);
        let workers: Vec<Worker<TaskRef>> = (0..worker_count).map(|_| Worker::new_fifo()).collect();
        let stealers = workers.iter().map(Worker::stealer).collect();
        let shared = Arc::new(SchedulerShared {
            injector: Injector::new(),
            stealers,
            parked: (Mutex::new(0), Condvar::new()),
            shutdown: AtomicBool::new(false),
            active: AtomicUsize::new(0),
            done: (Mutex::new(false), Condvar::new()),
        });

        let handles = workers
            .into_iter()
            .map(|local| {
                let shared = shared.clone();
                std::thread::spawn(move || worker_loop(local, shared))
            })
            .collect();

        Self {
            shared,
            workers: handles,
            blocking: BlockingPool::new(),
            io: IoDriver::start(),
        }
    }

    pub fn spawn_blocking<T, F>(&self, f: F) -> JoinHandle<T>
    where
        T: Send + 'static,
        F: FnOnce() -> T + Send + 'static,
    {
        self.blocking.spawn(self, f)
    }

    pub fn wait_readable(&self, source: &impl crate::io::Source) -> std::io::Result<()> {
        self.io.wait_readable(source, &self.shared)
    }

    pub fn wait_writable(&self, source: &impl crate::io::Source) -> std::io::Result<()> {
        self.io.wait_writable(source, &self.shared)
    }

    pub(crate) fn shared(&self) -> &Arc<SchedulerShared> {
        &self.shared
    }

    pub(crate) fn spawn_raw<F>(&self, body: F, on_abandon: task::OnAbandon) -> TaskRef
    where
        F: FnOnce() + Send + 'static,
    {
        self.shared.active.fetch_add(1, Ordering::AcqRel);
        let task = task::spawn_task(body, on_abandon);
        self.shared.enqueue(task.clone());
        task
    }

    /// Blocks the calling (non-task) thread until every spawned task has
    /// finished.
    pub fn run_until_idle(&self) {
        let (lock, cv) = &self.shared.done;
        let mut done = lock.lock().unwrap();
        while self.shared.active.load(Ordering::Acquire) > 0 {
            done = cv.wait(done).unwrap();
        }
        drop(done);
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
        self.io.shutdown();
        self.shared.shutdown.store(true, Ordering::Release);
        let (lock, cv) = &self.shared.parked;
        let _guard = lock.lock().unwrap();
        cv.notify_all();
        drop(_guard);
        for handle in self.workers.drain(..) {
            let _ = handle.join();
        }
    }
}

fn worker_loop(local: Worker<TaskRef>, shared: Arc<SchedulerShared>) {
    loop {
        if shared.shutdown.load(Ordering::Acquire) {
            return;
        }
        let task = find_task(&local, &shared);
        let Some(task) = task else {
            park(&shared);
            continue;
        };
        match task::resume(&task, &shared) {
            TaskPoll::Suspended => {}
            TaskPoll::Finished => shared.task_finished(),
        }
    }
}

fn find_task(local: &Worker<TaskRef>, shared: &SchedulerShared) -> Option<TaskRef> {
    if let Some(task) = local.pop() {
        return Some(task);
    }
    loop {
        match shared.injector.steal_batch_and_pop(local) {
            Steal::Success(task) => return Some(task),
            Steal::Retry => continue,
            Steal::Empty => break,
        }
    }
    for stealer in &shared.stealers {
        loop {
            match stealer.steal() {
                Steal::Success(task) => return Some(task),
                Steal::Retry => continue,
                Steal::Empty => break,
            }
        }
    }
    None
}

fn park(shared: &Arc<SchedulerShared>) {
    let (lock, cv) = &shared.parked;
    let mut parked = lock.lock().unwrap();
    *parked += 1;
    let (guard, _timeout) = cv
        .wait_timeout(parked, std::time::Duration::from_millis(10))
        .unwrap();
    parked = guard;
    *parked -= 1;
}
