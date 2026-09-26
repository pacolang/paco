//! Rust-only ABI tests: hand-constructed byte buffers, no Paco/Cranelift
//! involved. Proves the FFI design before any compiler integration.

use std::sync::Once;

use paco_runtime_ffi::{
    PacoStr, paco_print_int, paco_rt_panic, paco_rt_panic_bounds, paco_rt_spawn_blocking, paco_rt_channel, paco_rt_generator_new, paco_rt_generator_next, paco_rt_generator_release,
    paco_rt_generator_yield, paco_rt_init, paco_rt_join, paco_rt_join_handle_release, paco_rt_receiver_release,
    paco_rt_recv, paco_rt_send, paco_rt_sender_release, paco_rt_sender_retain, paco_rt_spawn,
};

unsafe fn make_channel(capacity: usize) -> (*mut paco_runtime_ffi::SenderOpaque, *mut paco_runtime_ffi::ReceiverOpaque) {
    let mut sender = std::ptr::null_mut();
    let mut receiver = std::ptr::null_mut();
    unsafe { paco_rt_channel(capacity, &mut sender, &mut receiver) };
    (sender, receiver)
}

static INIT: Once = Once::new();

fn ensure_init() {
    INIT.call_once(|| paco_rt_init());
}

unsafe extern "C-unwind" fn write_known_pattern(_captures: *const u8, result_out: *mut u8) {
    unsafe { *result_out.cast::<i64>() = 42 };
}

#[test]
fn spawn_and_join_round_trips_a_known_result() {
    ensure_init();
    let handle = unsafe { paco_rt_spawn(write_known_pattern, std::ptr::null(), 0, 8) };
    let mut result = [0u8; 8];
    let status = unsafe { paco_rt_join(handle, result.as_mut_ptr(), 8, std::ptr::null_mut()) };
    assert_eq!(status, 0);
    assert_eq!(i64::from_ne_bytes(result), 42);
}

unsafe extern "C-unwind" fn double_capture(captures: *const u8, result_out: *mut u8) {
    unsafe {
        let input = *captures.cast::<i64>();
        *result_out.cast::<i64>() = input * 2;
    }
}

#[test]
fn spawn_reads_its_captures_buffer() {
    ensure_init();
    let captures = 21i64.to_ne_bytes();
    let handle = unsafe { paco_rt_spawn(double_capture, captures.as_ptr(), 8, 8) };
    let mut result = [0u8; 8];
    let status = unsafe { paco_rt_join(handle, result.as_mut_ptr(), 8, std::ptr::null_mut()) };
    assert_eq!(status, 0);
    assert_eq!(i64::from_ne_bytes(result), 42);
}

unsafe extern "C-unwind" fn panics(_captures: *const u8, _result_out: *mut u8) {
    panic!("intentional test panic");
}

#[test]
fn join_reports_a_panicking_task() {
    ensure_init();
    let handle = unsafe { paco_rt_spawn(panics, std::ptr::null(), 0, 0) };
    let status = unsafe { paco_rt_join(handle, std::ptr::null_mut(), 0, std::ptr::null_mut()) };
    assert_eq!(status, 1);
}

#[test]
fn channel_sends_and_receives_a_fixed_size_blob() {
    ensure_init();
    let (sender, receiver) = unsafe { make_channel(4) };

    let value = 7i64.to_ne_bytes();
    let send_status = unsafe { paco_rt_send(sender, value.as_ptr(), 8) };
    assert_eq!(send_status, 0);

    let mut received = [0u8; 8];
    let recv_status = unsafe { paco_rt_recv(receiver, received.as_mut_ptr(), 8) };
    assert_eq!(recv_status, 0);
    assert_eq!(i64::from_ne_bytes(received), 7);
}

#[test]
fn channel_send_and_recv_hand_off_between_a_spawned_task_and_the_caller() {
    ensure_init();
    let (sender, receiver) = unsafe { make_channel(1) };

    struct Captures {
        sender: *mut paco_runtime_ffi::SenderOpaque,
    }
    unsafe extern "C-unwind" fn producer(captures: *const u8, _result_out: *mut u8) {
        let captures = unsafe { &*captures.cast::<Captures>() };
        let value = 99i64.to_ne_bytes();
        unsafe { paco_rt_send(captures.sender, value.as_ptr(), 8) };
    }

    let captures = Captures { sender };
    let handle = unsafe {
        paco_rt_spawn(
            producer,
            std::ptr::from_ref(&captures).cast(),
            std::mem::size_of::<Captures>(),
            0,
        )
    };

    let mut received = [0u8; 8];
    let recv_status = unsafe { paco_rt_recv(receiver, received.as_mut_ptr(), 8) };
    assert_eq!(recv_status, 0);
    assert_eq!(i64::from_ne_bytes(received), 99);

    unsafe { paco_rt_join(handle, std::ptr::null_mut(), 0, std::ptr::null_mut()) };
}

static CANCELLED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

unsafe extern "C-unwind" fn yields_three_values(_captures: *const u8, cancelled: i64) {
    if cancelled != 0 {
        CANCELLED.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        return;
    }
    for value in [1i64, 2, 3] {
        let bytes = value.to_ne_bytes();
        if unsafe { paco_rt_generator_yield(bytes.as_ptr(), 8) } != 0 {
            CANCELLED.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            return;
        }
    }
}

#[test]
fn generator_yields_the_expected_sequence_across_repeated_next_calls() {
    let handle = unsafe { paco_rt_generator_new(yields_three_values, std::ptr::null(), 0) };

    let mut values = Vec::new();
    loop {
        let mut out = [0u8; 8];
        let status = unsafe { paco_rt_generator_next(handle, out.as_mut_ptr(), 8) };
        if status != 0 {
            break;
        }
        values.push(i64::from_ne_bytes(out));
    }

    assert_eq!(values, vec![1, 2, 3]);

    // Exhausted: every later call keeps returning 1, not panicking or
    // resuming a finished coroutine.
    let mut out = [0u8; 8];
    let status = unsafe { paco_rt_generator_next(handle, out.as_mut_ptr(), 8) };
    assert_eq!(status, 1);
}

#[test]
fn releasing_the_last_sender_closes_the_channel() {
    ensure_init();
    let (sender, receiver) = unsafe { make_channel(2) };
    unsafe { paco_rt_sender_retain(sender) };
    unsafe { paco_rt_sender_release(sender) };
    let value = 5i64.to_ne_bytes();
    assert_eq!(unsafe { paco_rt_send(sender, value.as_ptr(), 8) }, 0);
    unsafe { paco_rt_sender_release(sender) };

    let mut out = [0u8; 8];
    assert_eq!(unsafe { paco_rt_recv(receiver, out.as_mut_ptr(), 8) }, 0);
    assert_eq!(i64::from_ne_bytes(out), 5);
    assert_eq!(unsafe { paco_rt_recv(receiver, out.as_mut_ptr(), 8) }, 1);
    unsafe { paco_rt_receiver_release(receiver) };
}

#[test]
fn releasing_a_join_handle_detaches_the_task_without_cancelling_it() {
    ensure_init();
    let (sender, receiver) = unsafe { make_channel(1) };

    unsafe extern "C-unwind" fn producer(captures: *const u8, _result_out: *mut u8) {
        let sender = unsafe { *captures.cast::<*mut paco_runtime_ffi::SenderOpaque>() };
        let value = 11i64.to_ne_bytes();
        unsafe { paco_rt_send(sender, value.as_ptr(), 8) };
        unsafe { paco_rt_sender_release(sender) };
    }

    let handle = unsafe { paco_rt_spawn(producer, std::ptr::from_ref(&sender).cast(), 8, 0) };
    unsafe { paco_rt_join_handle_release(handle) };

    let mut out = [0u8; 8];
    assert_eq!(unsafe { paco_rt_recv(receiver, out.as_mut_ptr(), 8) }, 0);
    assert_eq!(i64::from_ne_bytes(out), 11);
    assert_eq!(unsafe { paco_rt_recv(receiver, out.as_mut_ptr(), 8) }, 1);
    unsafe { paco_rt_receiver_release(receiver) };
}

#[test]
fn releasing_an_unfinished_generator_resumes_it_cancelled() {
    let before = CANCELLED.load(std::sync::atomic::Ordering::SeqCst);
    let suspended = unsafe { paco_rt_generator_new(yields_three_values, std::ptr::null(), 0) };
    let mut out = [0u8; 8];
    assert_eq!(unsafe { paco_rt_generator_next(suspended, out.as_mut_ptr(), 8) }, 0);
    unsafe { paco_rt_generator_release(suspended) };
    let unstarted = unsafe { paco_rt_generator_new(yields_three_values, std::ptr::null(), 0) };
    unsafe { paco_rt_generator_release(unstarted) };
    assert_eq!(CANCELLED.load(std::sync::atomic::Ordering::SeqCst) - before, 2);
}

fn paco_str(text: &'static str) -> PacoStr {
    PacoStr { ptr: text.as_ptr(), len: text.len() as i64 }
}

fn run_child(test: &str) -> std::process::Output {
    std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", test, "--nocapture", "--test-threads=1"])
        .env("PACO_PANIC_CHILD", "1")
        .output()
        .unwrap()
}

#[test]
fn panic_flushes_stdout_reports_the_location_and_exits_101() {
    if std::env::var_os("PACO_PANIC_CHILD").is_some() {
        paco_print_int(7);
        let message = paco_str("boom");
        unsafe { paco_rt_panic(&message, c"input.paco".as_ptr(), 3, 5, 0, 0) };
    }
    let output = run_child("panic_flushes_stdout_reports_the_location_and_exits_101");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(101), "{stderr}");
    assert!(stderr.contains("panic at input.paco:3:5: boom\n"), "{stderr}");
    assert!(String::from_utf8_lossy(&output.stdout).contains("7\n"));
}

#[test]
fn bounds_panics_name_the_index_and_length() {
    if std::env::var_os("PACO_PANIC_CHILD").is_some() {
        unsafe { paco_rt_panic_bounds(5, 3, c"input.paco".as_ptr(), 4, 7, 0, 0) };
    }
    let output = run_child("bounds_panics_name_the_index_and_length");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(101), "{stderr}");
    assert!(stderr.contains("panic at input.paco:4:7: index 5 out of bounds for length 3\n"), "{stderr}");
}

unsafe extern "C-unwind" fn paco_panics(_captures: *const u8, _result_out: *mut u8) {
    let message = paco_str("task failed");
    unsafe { paco_rt_panic(&message, c"input.paco".as_ptr(), 1, 1, 0, 0) };
}

fn join_message(handle: *mut paco_runtime_ffi::JoinHandleOpaque) -> (i32, String) {
    let mut message = PacoStr { ptr: std::ptr::null(), len: 0 };
    let status = unsafe { paco_rt_join(handle, std::ptr::null_mut(), 0, &mut message) };
    let text = if message.len > 0 {
        String::from_utf8_lossy(unsafe { std::slice::from_raw_parts(message.ptr, message.len as usize) }).into_owned()
    } else {
        String::new()
    };
    (status, text)
}

#[test]
fn a_paco_panic_in_a_task_is_joined_as_an_error_with_its_message() {
    ensure_init();
    let handle = unsafe { paco_rt_spawn(paco_panics, std::ptr::null(), 0, 0) };
    assert_eq!(join_message(handle), (1, "task failed".to_string()));
    let handle = unsafe { paco_rt_spawn_blocking(paco_panics, std::ptr::null(), 0, 0) };
    assert_eq!(join_message(handle), (1, "task failed".to_string()));
}
