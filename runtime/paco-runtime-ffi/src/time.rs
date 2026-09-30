//! `std::time`'s runtime primitives: sleeping a task and reading the
//! monotonic and wall clocks, as plain nanosecond counts so the Paco side
//! can be ordinary integer arithmetic.

use std::sync::OnceLock;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Suspends the calling task (or thread, outside the scheduler) for at
/// least `nanos` nanoseconds, without blocking a worker.
#[unsafe(no_mangle)]
pub extern "C" fn paco_rt_sleep_ns(nanos: u64) {
    crate::runtime().sleep_until(Instant::now() + Duration::from_nanos(nanos));
}

static MONOTONIC_EPOCH: OnceLock<Instant> = OnceLock::new();

/// Nanoseconds on a monotonic clock, from an arbitrary but fixed origin
/// (this process's first call). Never goes backwards.
#[unsafe(no_mangle)]
pub extern "C" fn paco_rt_monotonic_ns() -> u64 {
    MONOTONIC_EPOCH.get_or_init(Instant::now).elapsed().as_nanos() as u64
}

/// Nanoseconds of wall-clock time since the Unix epoch.
#[unsafe(no_mangle)]
pub extern "C" fn paco_rt_wall_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is set before the Unix epoch")
        .as_nanos() as u64
}
