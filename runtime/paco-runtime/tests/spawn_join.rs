use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use paco_runtime::{Runtime, spawn};

#[test]
fn spawning_more_tasks_than_workers_runs_them_all() {
    let runtime = Runtime::new(2);
    let counter = Arc::new(AtomicUsize::new(0));

    let handles: Vec<_> = (0..50)
        .map(|i| {
            let counter = counter.clone();
            spawn(&runtime, move || {
                counter.fetch_add(1, Ordering::AcqRel);
                i
            })
        })
        .collect();

    for (i, handle) in handles.into_iter().enumerate() {
        assert_eq!(handle.join().unwrap(), i);
    }
    assert_eq!(counter.load(Ordering::Acquire), 50);
}

#[test]
fn a_panicking_task_does_not_abort_and_returns_task_panic() {
    let runtime = Runtime::new(2);
    let handle = spawn(&runtime, || -> i32 { panic!("boom") });
    let error = handle.join().unwrap_err();
    assert!(error.message.contains("boom"));
}

#[test]
fn run_until_idle_waits_for_all_spawned_tasks() {
    let runtime = Runtime::new(2);
    let counter = Arc::new(AtomicUsize::new(0));
    for _ in 0..10 {
        let counter = counter.clone();
        spawn(&runtime, move || {
            counter.fetch_add(1, Ordering::AcqRel);
        });
    }
    runtime.run_until_idle();
    assert_eq!(counter.load(Ordering::Acquire), 10);
}
