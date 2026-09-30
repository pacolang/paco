//! Suspending a task on a bare, FFI-provided file descriptor's readiness —
//! the primitive `std::net`'s socket wrappers need to avoid blocking a
//! worker while waiting for a connection or for data to arrive.

/// Suspends the calling task (or thread, outside the scheduler) until `fd`
/// is ready for `interest` (`0` for readable, any other value for
/// writable), without blocking a worker. Returns `0` on success, `1` if the
/// OS poller rejected the descriptor.
///
/// # Safety
/// `fd` must be a valid, open file descriptor for as long as this call
/// takes to return.
#[cfg(unix)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn paco_rt_wait_fd(fd: i32, interest: i32) -> i32 {
    let interest = if interest == 0 { paco_runtime::Interest::Readable } else { paco_runtime::Interest::Writable };
    match crate::runtime().wait_fd(fd, interest) {
        Ok(()) => 0,
        Err(_) => 1,
    }
}
