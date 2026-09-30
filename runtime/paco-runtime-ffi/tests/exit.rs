//! `paco_rt_exit` ends the whole process, so it cannot run in-process like
//! an ordinary assertion: this test re-execs its own test binary as a child
//! with an environment variable set, then checks the child's exit code and
//! stdout from the outside (the same pattern `ffi.rs`'s panic tests use).

use std::process::Command;

fn run_child(test: &str) -> std::process::Output {
    Command::new(std::env::current_exe().unwrap())
        .args(["--exact", test, "--nocapture", "--test-threads=1"])
        .env("PACO_RT_EXIT_CHILD", "1")
        .output()
        .unwrap()
}

#[test]
fn exit_flushes_buffered_stdout_then_ends_the_process_with_the_given_code() {
    if std::env::var_os("PACO_RT_EXIT_CHILD").is_some() {
        let text = b"buffered before exit";
        let message = paco_runtime_ffi::PacoStr { ptr: text.as_ptr(), len: text.len() as i64 };
        unsafe { paco_runtime_ffi::paco_print_str(&message) };
        paco_runtime_ffi::paco_rt_exit(42);
    }
    let output = run_child("exit_flushes_buffered_stdout_then_ends_the_process_with_the_given_code");
    assert_eq!(output.status.code(), Some(42));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("buffered before exit"), "{stdout}");
}
