use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};
use std::io;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use polling::{Event, Events, Poller};

use crate::scheduler::SchedulerShared;
use crate::sync::Notify;

/// A sleeping task's wakeup time, ordered by `deadline` alone.
struct Timer {
    deadline: Instant,
    notify: Notify,
}

impl PartialEq for Timer {
    fn eq(&self, other: &Self) -> bool {
        self.deadline == other.deadline
    }
}
impl Eq for Timer {}
impl PartialOrd for Timer {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Timer {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.deadline.cmp(&other.deadline)
    }
}

/// A socket or file the OS poller can watch.
#[cfg(unix)]
pub trait Source: std::os::fd::AsRawFd + std::os::fd::AsFd {}
#[cfg(unix)]
impl<T: std::os::fd::AsRawFd + std::os::fd::AsFd> Source for T {}

/// A socket the OS poller can watch.
#[cfg(windows)]
pub trait Source: std::os::windows::io::AsRawSocket + std::os::windows::io::AsSocket {}
#[cfg(windows)]
impl<T: std::os::windows::io::AsRawSocket + std::os::windows::io::AsSocket> Source for T {}

pub(crate) struct IoDriver {
    poller: Poller,
    waiters: Mutex<HashMap<usize, Notify>>,
    timers: Mutex<BinaryHeap<Reverse<Timer>>>,
    next_key: AtomicUsize,
    shutdown: AtomicBool,
}

impl IoDriver {
    pub(crate) fn start() -> Arc<Self> {
        let driver = Arc::new(Self {
            poller: Poller::new().expect("failed to create OS I/O poller"),
            waiters: Mutex::new(HashMap::new()),
            timers: Mutex::new(BinaryHeap::new()),
            next_key: AtomicUsize::new(0),
            shutdown: AtomicBool::new(false),
        });
        let background = driver.clone();
        std::thread::spawn(move || poll_loop(background));
        driver
    }

    pub(crate) fn shutdown(&self) {
        self.shutdown.store(true, Ordering::Release);
        let _ = self.poller.notify();
    }

    pub(crate) fn wait_readable(&self, source: &impl Source, scheduler: &Arc<SchedulerShared>) -> io::Result<()> {
        self.wait_for(source, Event::readable, scheduler)
    }

    pub(crate) fn wait_writable(&self, source: &impl Source, scheduler: &Arc<SchedulerShared>) -> io::Result<()> {
        self.wait_for(source, Event::writable, scheduler)
    }

    fn wait_for(
        &self,
        source: &impl Source,
        event_for: fn(usize) -> Event,
        scheduler: &Arc<SchedulerShared>,
    ) -> io::Result<()> {
        let key = self.next_key.fetch_add(1, Ordering::AcqRel);
        let notify = Notify::current(scheduler);
        self.waiters.lock().unwrap().insert(key, notify.clone());
        unsafe { self.poller.add(source, event_for(key))? };
        notify.park();
        let _ = self.poller.delete(source);
        Ok(())
    }

    /// Suspends the caller until `deadline`, without blocking its worker.
    pub(crate) fn sleep_until(&self, deadline: Instant, scheduler: &Arc<SchedulerShared>) {
        let notify = Notify::current(scheduler);
        self.timers.lock().unwrap().push(Reverse(Timer { deadline, notify: notify.clone() }));
        // The poll loop may already be waiting on a later deadline (or
        // none); wake it so it recomputes its timeout against this one.
        let _ = self.poller.notify();
        notify.park();
    }

    /// Wakes and removes every timer whose deadline has passed.
    fn fire_expired_timers(&self) {
        let now = Instant::now();
        let mut timers = self.timers.lock().unwrap();
        while timers.peek().is_some_and(|Reverse(timer)| timer.deadline <= now) {
            let Reverse(timer) = timers.pop().unwrap();
            timer.notify.notify();
        }
    }

    /// The next timer's deadline, if any, for the poll loop's timeout.
    fn next_deadline(&self) -> Option<Instant> {
        self.timers.lock().unwrap().peek().map(|Reverse(timer)| timer.deadline)
    }
}

fn poll_loop(driver: Arc<IoDriver>) {
    let mut events = Events::new();
    loop {
        if driver.shutdown.load(Ordering::Acquire) {
            return;
        }
        events.clear();
        let wait_result = match driver.next_deadline() {
            Some(deadline) => driver.poller.wait_deadline(&mut events, deadline),
            None => driver.poller.wait(&mut events, None),
        };
        if wait_result.is_err() {
            continue;
        }
        for event in events.iter() {
            if let Some(notify) = driver.waiters.lock().unwrap().remove(&event.key) {
                notify.notify();
            }
        }
        driver.fire_expired_timers();
    }
}
