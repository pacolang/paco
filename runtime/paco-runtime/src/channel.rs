use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use crate::scheduler::{Runtime, SchedulerShared};
use crate::sync::Notify;

struct Inner<T> {
    queue: Mutex<VecDeque<T>>,
    capacity: usize,
    send_waiters: Mutex<Vec<Notify>>,
    recv_waiters: Mutex<Vec<Notify>>,
    closed: AtomicBool,
    sender_count: AtomicUsize,
    scheduler: Arc<SchedulerShared>,
}

pub struct Sender<T> {
    inner: Arc<Inner<T>>,
}

pub struct Receiver<T> {
    inner: Arc<Inner<T>>,
}

impl<T> Clone for Sender<T> {
    fn clone(&self) -> Self {
        self.inner.sender_count.fetch_add(1, Ordering::AcqRel);
        Self { inner: self.inner.clone() }
    }
}

#[derive(Debug)]
pub struct SendError<T>(pub T);

#[derive(Debug, PartialEq, Eq)]
pub struct RecvError;

pub fn channel<T>(runtime: &Runtime, capacity: usize) -> (Sender<T>, Receiver<T>) {
    let inner = Arc::new(Inner {
        queue: Mutex::new(VecDeque::new()),
        capacity: capacity.max(1),
        send_waiters: Mutex::new(Vec::new()),
        recv_waiters: Mutex::new(Vec::new()),
        closed: AtomicBool::new(false),
        sender_count: AtomicUsize::new(1),
        scheduler: runtime.shared().clone(),
    });
    (Sender { inner: inner.clone() }, Receiver { inner })
}

impl<T> Sender<T> {
    pub fn send(&self, value: T) -> Result<(), SendError<T>> {
        loop {
            if self.inner.closed.load(Ordering::Acquire) {
                return Err(SendError(value));
            }
            let notify = {
                let mut queue = self.inner.queue.lock().unwrap();
                if self.inner.closed.load(Ordering::Acquire) {
                    return Err(SendError(value));
                }
                if queue.len() < self.inner.capacity {
                    queue.push_back(value);
                    drop(queue);
                    wake_all(&self.inner.recv_waiters);
                    return Ok(());
                }
                let notify = Notify::current(&self.inner.scheduler);
                self.inner.send_waiters.lock().unwrap().push(notify.clone());
                notify
            };
            notify.park();
        }
    }

    pub fn is_ready(&self) -> bool {
        !self.inner.closed.load(Ordering::Acquire) && self.inner.queue.lock().unwrap().len() < self.inner.capacity
    }

    /// Closes the channel immediately, regardless of how many `Sender`
    /// clones remain — matching Go's sender-side `close(ch)`, and spec.md's
    /// `tx.close()` example. Distinct from `Receiver::close`, which is a
    /// consumer signalling it wants no more values.
    pub fn close(&self) {
        self.inner.close();
    }
}

impl<T> Drop for Sender<T> {
    fn drop(&mut self) {
        if self.inner.sender_count.fetch_sub(1, Ordering::AcqRel) == 1 {
            self.inner.close();
        }
    }
}

impl<T> Receiver<T> {
    pub fn recv(&self) -> Result<T, RecvError> {
        loop {
            let notify = {
                let mut queue = self.inner.queue.lock().unwrap();
                if let Some(value) = queue.pop_front() {
                    drop(queue);
                    wake_all(&self.inner.send_waiters);
                    return Ok(value);
                }
                if self.inner.closed.load(Ordering::Acquire) {
                    return Err(RecvError);
                }
                let notify = Notify::current(&self.inner.scheduler);
                self.inner.recv_waiters.lock().unwrap().push(notify.clone());
                notify
            };
            notify.park();
        }
    }

    pub fn is_ready(&self) -> bool {
        !self.inner.queue.lock().unwrap().is_empty() || self.inner.closed.load(Ordering::Acquire)
    }

    pub fn close(&self) {
        self.inner.close();
    }
}

impl<T> Inner<T> {
    /// Waiters register while holding `queue`, so flipping `closed` under
    /// the same lock guarantees every registered waiter sees the wakeup.
    fn close(&self) {
        {
            let _queue = self.queue.lock().unwrap();
            self.closed.store(true, Ordering::Release);
        }
        wake_all(&self.send_waiters);
        wake_all(&self.recv_waiters);
    }
}

fn wake_all(waiters: &Mutex<Vec<Notify>>) {
    for notify in waiters.lock().unwrap().drain(..) {
        notify.notify();
    }
}
