use std::any::Any;
use std::sync::{Arc, Mutex};

use crate::scheduler::{Runtime, SchedulerShared};
use crate::sync::Notify;

#[derive(Debug)]
pub struct TaskPanic {
    pub message: String,
}

impl TaskPanic {
    pub(crate) fn from_payload(payload: Box<dyn Any + Send>) -> Self {
        let message = if let Some(s) = payload.downcast_ref::<&str>() {
            (*s).to_string()
        } else if let Some(s) = payload.downcast_ref::<String>() {
            s.clone()
        } else {
            "task panicked".to_string()
        };
        Self { message }
    }
}

pub(crate) struct Slot<T> {
    value: Option<Result<T, TaskPanic>>,
    waiters: Vec<Notify>,
}

pub(crate) fn new_slot<T>() -> Arc<Mutex<Slot<T>>> {
    Arc::new(Mutex::new(Slot { value: None, waiters: Vec::new() }))
}

pub(crate) fn complete<T>(slot: &Mutex<Slot<T>>, result: Result<T, TaskPanic>) {
    let waiters = {
        let mut guard = slot.lock().unwrap();
        guard.value = Some(result);
        std::mem::take(&mut guard.waiters)
    };
    for waiter in waiters {
        waiter.notify();
    }
}

pub struct JoinHandle<T> {
    slot: Arc<Mutex<Slot<T>>>,
    scheduler: Arc<SchedulerShared>,
}

impl<T> JoinHandle<T> {
    pub(crate) fn from_parts(slot: Arc<Mutex<Slot<T>>>, scheduler: Arc<SchedulerShared>) -> Self {
        Self { slot, scheduler }
    }
}

impl<T: Send + 'static> JoinHandle<T> {
    pub fn join(&self) -> Result<T, TaskPanic> {
        loop {
            let notify = Notify::current(&self.scheduler);
            {
                let mut slot = self.slot.lock().unwrap();
                if let Some(result) = slot.value.take() {
                    return result;
                }
                slot.waiters.push(notify.clone());
            }
            notify.park();
        }
    }
}

pub fn spawn<T, F>(runtime: &Runtime, f: F) -> JoinHandle<T>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    let slot = new_slot();
    let body_slot = slot.clone();
    let abandon_slot = slot.clone();
    runtime.spawn_raw(
        move || {
            let result =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).map_err(TaskPanic::from_payload);
            complete(&body_slot, result);
        },
        Box::new(move |message| complete(&abandon_slot, Err(TaskPanic { message }))),
    );
    JoinHandle::from_parts(slot, runtime.shared().clone())
}
