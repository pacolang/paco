use std::sync::Arc;
use std::thread::Thread;

use crate::scheduler::SchedulerShared;
use crate::task::{self, Waker};

#[derive(Clone)]
pub(crate) enum Notify {
    Task(Waker),
    Thread(Thread),
}

impl Notify {
    pub(crate) fn current(scheduler: &Arc<SchedulerShared>) -> Self {
        match task::new_waker_for_current(scheduler) {
            Some(waker) => Notify::Task(waker),
            None => Notify::Thread(std::thread::current()),
        }
    }

    pub(crate) fn notify(&self) {
        match self {
            Notify::Task(waker) => waker.wake(),
            Notify::Thread(thread) => thread.unpark(),
        }
    }

    pub(crate) fn park(&self) {
        match self {
            Notify::Task(_) => task::suspend_current(),
            Notify::Thread(_) => std::thread::park(),
        }
    }
}
