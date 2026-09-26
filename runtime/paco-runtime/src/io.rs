use std::collections::HashMap;
use std::io;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use polling::{Event, Events, Poller};

use crate::scheduler::SchedulerShared;
use crate::sync::Notify;

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
    next_key: AtomicUsize,
    shutdown: AtomicBool,
}

impl IoDriver {
    pub(crate) fn start() -> Arc<Self> {
        let driver = Arc::new(Self {
            poller: Poller::new().expect("failed to create OS I/O poller"),
            waiters: Mutex::new(HashMap::new()),
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
}

fn poll_loop(driver: Arc<IoDriver>) {
    let mut events = Events::new();
    loop {
        if driver.shutdown.load(Ordering::Acquire) {
            return;
        }
        events.clear();
        if driver.poller.wait(&mut events, None).is_err() {
            continue;
        }
        for event in events.iter() {
            if let Some(notify) = driver.waiters.lock().unwrap().remove(&event.key) {
                notify.notify();
            }
        }
    }
}
