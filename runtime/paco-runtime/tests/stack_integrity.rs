use paco_runtime::{Runtime, spawn};

fn sum_to(n: u64, frame_padding: [u64; 64]) -> u64 {
    if n == 0 {
        frame_padding.iter().sum::<u64>().min(1)
    } else {
        n + sum_to(n - 1, frame_padding)
    }
}

#[test]
fn deep_recursion_does_not_corrupt_the_task_stack() {
    let runtime = Runtime::new(1);
    let handle = spawn(&runtime, || sum_to(500, [7; 64]));
    assert_eq!(handle.join().unwrap(), (500 * 501) / 2 + 1);
}

#[test]
fn a_suspend_resume_cycle_preserves_local_stack_state() {
    let runtime = Runtime::new(2);
    let handle = spawn(&runtime, || {
        let canary = [0x1234_5678_9abc_def0_u64; 16];
        paco_runtime::yield_now();
        assert!(canary.iter().all(|&v| v == 0x1234_5678_9abc_def0));
        canary[0]
    });
    assert_eq!(handle.join().unwrap(), 0x1234_5678_9abc_def0);
}
