#![cfg(unix)]

use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use paco_runtime::{Interest, Runtime, spawn};

#[test]
fn a_task_waiting_on_a_raw_fd_wakes_only_once_the_peer_writes() {
    let runtime = Arc::new(Runtime::new(1));
    let (reader, mut writer) = UnixStream::pair().unwrap();
    reader.set_nonblocking(true).unwrap();
    let reader_fd = reader.as_raw_fd();
    let woke = Arc::new(AtomicBool::new(false));

    let runtime_for_task = runtime.clone();
    let woke_for_task = woke.clone();
    let handle = spawn(&runtime, move || {
        runtime_for_task.wait_fd(reader_fd, Interest::Readable).unwrap();
        woke_for_task.store(true, Ordering::Release);
        let mut reader = reader;
        let mut byte = [0u8; 1];
        reader.read_exact(&mut byte).unwrap();
        byte[0]
    });

    std::thread::sleep(Duration::from_millis(50));
    assert!(!woke.load(Ordering::Acquire), "the task woke before the peer wrote anything");

    writer.write_all(&[42]).unwrap();

    let byte = handle.join().unwrap();
    assert_eq!(byte, 42);
    assert!(woke.load(Ordering::Acquire));
}

#[test]
fn a_hundred_tasks_waiting_on_separate_pipes_all_wake() {
    let runtime = Arc::new(Runtime::new(1));
    let start = Instant::now();

    let handles: Vec<_> = (0..100)
        .map(|_| {
            let (reader, mut writer) = UnixStream::pair().unwrap();
            reader.set_nonblocking(true).unwrap();
            let reader_fd = reader.as_raw_fd();
            let runtime_for_task = runtime.clone();
            std::thread::spawn(move || writer.write_all(&[1]).unwrap());
            spawn(&runtime, move || {
                runtime_for_task.wait_fd(reader_fd, Interest::Readable).unwrap();
                let mut reader = reader;
                let mut byte = [0u8; 1];
                reader.read_exact(&mut byte).unwrap();
            })
        })
        .collect();

    for handle in handles {
        handle.join().unwrap();
    }

    assert!(start.elapsed() < Duration::from_secs(1), "{:?}", start.elapsed());
}
