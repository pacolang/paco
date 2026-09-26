use std::time::{Duration, Instant};

use paco_runtime::{Runtime, spawn};

#[test]
fn a_blocking_closure_does_not_delay_unrelated_tasks() {
    let runtime = Runtime::new(1);

    let blocking = runtime.spawn_blocking(|| {
        std::thread::sleep(Duration::from_millis(200));
        "slow"
    });

    let start = Instant::now();
    let fast = spawn(&runtime, || 1 + 1);
    assert_eq!(fast.join().unwrap(), 2);
    assert!(start.elapsed() < Duration::from_millis(150));

    assert_eq!(blocking.join().unwrap(), "slow");
}
