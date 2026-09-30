//! Rust-only ABI tests for the timer and clock entry points.

use std::sync::Once;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use paco_runtime_ffi::{paco_rt_init, paco_rt_join, paco_rt_join_handle_release, paco_rt_monotonic_ns, paco_rt_sleep_ns, paco_rt_spawn, paco_rt_wall_ns};

static INIT: Once = Once::new();

fn ensure_init() {
    INIT.call_once(|| paco_rt_init());
}

unsafe extern "C-unwind" fn sleep_ten_millis(_captures: *const u8, _result_out: *mut u8) {
    paco_rt_sleep_ns(10_000_000);
}

#[test]
fn sleep_ns_suspends_the_task_for_at_least_the_requested_duration() {
    ensure_init();
    let start = Instant::now();
    let handle = unsafe { paco_rt_spawn(sleep_ten_millis, std::ptr::null(), 0, 0) };
    let status = unsafe { paco_rt_join(handle, std::ptr::null_mut(), 0, std::ptr::null_mut()) };
    unsafe { paco_rt_join_handle_release(handle) };
    assert_eq!(status, 0);
    assert!(start.elapsed().as_millis() >= 10, "{:?}", start.elapsed());
}

#[test]
fn monotonic_ns_never_goes_backwards_and_tracks_real_time() {
    ensure_init();
    let first = paco_rt_monotonic_ns();
    std::thread::sleep(std::time::Duration::from_millis(5));
    let second = paco_rt_monotonic_ns();
    assert!(second > first, "{second} should be after {first}");
    assert!((second - first) >= 5_000_000, "elapsed only {} ns", second - first);
}

#[test]
fn wall_ns_matches_the_system_clock() {
    ensure_init();
    let before = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos() as u64;
    let reported = paco_rt_wall_ns();
    let after = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos() as u64;
    assert!((before..=after).contains(&reported), "{reported} not within [{before}, {after}]");
}
