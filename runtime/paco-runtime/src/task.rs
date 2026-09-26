use std::cell::Cell;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use corosensei::{Coroutine, CoroutineResult, Yielder};

use crate::scheduler::SchedulerShared;

// Thread-local accessors stay out of line: a task can resume on another
// worker, and an inlined access could reuse the previous thread's slot.
#[inline(never)]
pub(crate) fn suspend_current() {
    let ptr = CURRENT_YIELDER.with(Cell::get);
    assert!(!ptr.is_null(), "suspend called outside a running task");
    let yielder = unsafe { &*(ptr as *const Yielder<(), ()>) };
    yielder.suspend(());
}

#[inline(never)]
pub(crate) fn current_waker() -> Option<Waker> {
    CURRENT_WAKER.with(|cell| cell.borrow().clone())
}

pub fn yield_now() {
    if let Some(waker) = current_waker() {
        waker.wake();
    }
    suspend_current();
}

thread_local! {
    static CURRENT_YIELDER: Cell<*const ()> = const { Cell::new(std::ptr::null()) };
    static CURRENT_WAKER: std::cell::RefCell<Option<Waker>> = const { std::cell::RefCell::new(None) };
}

pub(crate) type TaskCoroutine = Coroutine<(), (), (), corosensei::stack::DefaultStack>;

pub(crate) type OnAbandon = Box<dyn FnOnce(String) + Send>;

pub(crate) struct TaskEntry {
    coroutine: std::sync::Mutex<Option<TaskCoroutine>>,
    yielder: Arc<AtomicUsize>,
    on_abandon: std::sync::Mutex<Option<OnAbandon>>,
}

/// `slot` points at the `AtomicUsize` holding a running coroutine's
/// `Yielder<(), ()>` address.
unsafe fn suspend_through_slot(slot: *const ()) {
    let yielder = unsafe { &*slot.cast::<AtomicUsize>() }.load(Ordering::Acquire) as *const Yielder<(), ()>;
    unsafe { &*yielder }.suspend(());
}

// Safety: paco-borrow proves spawn captures are move-safe; only one worker
// resumes a task at a time, serialized by `coroutine`'s mutex.
unsafe impl Send for TaskEntry {}
unsafe impl Sync for TaskEntry {}

pub(crate) type TaskRef = Arc<TaskEntry>;

#[derive(Clone)]
pub(crate) struct Waker {
    task: TaskRef,
    scheduler: Arc<SchedulerShared>,
    fired: Arc<AtomicBool>,
}

impl Waker {
    pub(crate) fn wake(&self) {
        if !self.fired.swap(true, Ordering::AcqRel) {
            self.scheduler.enqueue(self.task.clone());
        }
    }
}

#[inline(never)]
pub(crate) fn new_waker_for_current(scheduler: &Arc<SchedulerShared>) -> Option<Waker> {
    CURRENT_TASK.with(|cell| {
        cell.borrow().clone().map(|task| Waker {
            task,
            scheduler: scheduler.clone(),
            fired: Arc::new(AtomicBool::new(false)),
        })
    })
}

thread_local! {
    static CURRENT_TASK: std::cell::RefCell<Option<TaskRef>> = const { std::cell::RefCell::new(None) };
}

pub(crate) fn spawn_task<F>(body: F, on_abandon: OnAbandon) -> TaskRef
where
    F: FnOnce() + Send + 'static,
{
    let yielder = Arc::new(AtomicUsize::new(0));
    let slot = yielder.clone();
    let coroutine = TaskCoroutine::new(move |yielder: &Yielder<(), ()>, ()| {
        let ptr = yielder as *const Yielder<(), ()> as *const ();
        slot.store(ptr as usize, Ordering::Release);
        CURRENT_YIELDER.with(|cell| cell.set(ptr));
        body();
    });
    Arc::new(TaskEntry {
        coroutine: std::sync::Mutex::new(Some(coroutine)),
        yielder,
        on_abandon: std::sync::Mutex::new(Some(on_abandon)),
    })
}

/// Runs `body` on a coroutine of the calling thread so a panic in it can
/// abandon it (see [`crate::abandon`]); `on_abandon` receives the message.
pub(crate) fn run_abandonable(body: impl FnOnce() + 'static, on_abandon: impl FnOnce(String)) {
    let slot = Arc::new(AtomicUsize::new(0));
    let inner = slot.clone();
    let mut coroutine = TaskCoroutine::new(move |yielder: &Yielder<(), ()>, ()| {
        inner.store(yielder as *const Yielder<(), ()> as usize, Ordering::Release);
        body();
    });
    loop {
        let isolated = crate::abandon::isolate(Arc::as_ptr(&slot).cast(), suspend_through_slot);
        let outcome = coroutine.resume(());
        drop(isolated);
        match outcome {
            CoroutineResult::Return(()) => return,
            CoroutineResult::Yield(()) => {
                if let Some(message) = crate::abandon::take_abandoned() {
                    unsafe { coroutine.force_reset() };
                    on_abandon(message);
                    return;
                }
            }
        }
    }
}

pub(crate) fn resume(task: &TaskRef, scheduler: &Arc<SchedulerShared>) -> TaskPoll {
    CURRENT_TASK.with(|cell| *cell.borrow_mut() = Some(task.clone()));
    let waker = new_waker_for_current(scheduler);
    CURRENT_WAKER.with(|cell| *cell.borrow_mut() = waker);

    let mut guard = task.coroutine.lock().unwrap();
    let coroutine = guard.as_mut().expect("resumed a finished task");
    CURRENT_YIELDER.with(|cell| cell.set(task.yielder.load(Ordering::Acquire) as *const ()));
    let isolated = crate::abandon::isolate(Arc::as_ptr(&task.yielder).cast(), suspend_through_slot);
    let outcome = catch_unwind(AssertUnwindSafe(|| coroutine.resume(())));
    drop(isolated);

    CURRENT_TASK.with(|cell| *cell.borrow_mut() = None);
    CURRENT_WAKER.with(|cell| *cell.borrow_mut() = None);
    CURRENT_YIELDER.with(|cell| cell.set(std::ptr::null()));

    match outcome {
        Ok(CoroutineResult::Yield(())) => match crate::abandon::take_abandoned() {
            Some(message) => {
                unsafe { coroutine.force_reset() };
                *guard = None;
                drop(guard);
                if let Some(on_abandon) = task.on_abandon.lock().unwrap().take() {
                    on_abandon(message);
                }
                TaskPoll::Finished
            }
            None => TaskPoll::Suspended,
        },
        Ok(CoroutineResult::Return(())) => {
            *guard = None;
            TaskPoll::Finished
        }
        Err(_) => {
            *guard = None;
            TaskPoll::Finished
        }
    }
}

pub(crate) enum TaskPoll {
    Suspended,
    Finished,
}
