use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use paco_runtime::{Runtime, spawn};

#[test]
fn a_hundred_sleeping_tasks_on_one_worker_finish_promptly() {
    let runtime = Arc::new(Runtime::new(1));
    let counter = Arc::new(AtomicUsize::new(0));
    let start = Instant::now();

    let handles: Vec<_> = (0..100)
        .map(|_| {
            let runtime_for_task = runtime.clone();
            let counter = counter.clone();
            spawn(&runtime, move || {
                runtime_for_task.sleep_until(Instant::now() + Duration::from_millis(10));
                counter.fetch_add(1, Ordering::AcqRel);
            })
        })
        .collect();

    for handle in handles {
        handle.join().unwrap();
    }

    assert_eq!(counter.load(Ordering::Acquire), 100);
    let elapsed = start.elapsed();
    assert!(elapsed < Duration::from_secs(1), "{elapsed:?}");
}

#[test]
fn sleeping_tasks_wake_in_deadline_order() {
    let runtime = Arc::new(Runtime::new(1));
    let order = Arc::new(std::sync::Mutex::new(Vec::new()));

    let handles: Vec<_> = [30u64, 10, 20]
        .into_iter()
        .map(|millis| {
            let runtime_for_task = runtime.clone();
            let order = order.clone();
            spawn(&runtime, move || {
                runtime_for_task.sleep_until(Instant::now() + Duration::from_millis(millis));
                order.lock().unwrap().push(millis);
            })
        })
        .collect();

    for handle in handles {
        handle.join().unwrap();
    }

    assert_eq!(*order.lock().unwrap(), vec![10, 20, 30]);
}
