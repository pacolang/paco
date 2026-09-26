use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::time::{Duration, Instant};

use paco_runtime::{Runtime, spawn};

#[test]
fn a_task_blocked_on_a_socket_read_resumes_when_data_arrives() {
    let runtime = Arc::new(Runtime::new(2));
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();

    let runtime_for_task = runtime.clone();
    let server = spawn(&runtime, move || {
        let (stream, _) = listener.accept().unwrap();
        stream.set_nonblocking(true).unwrap();
        let mut buf = [0u8; 5];
        loop {
            match (&stream).read(&mut buf) {
                Ok(n) => return (buf, n),
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    runtime_for_task.wait_readable(&stream).unwrap();
                }
                Err(e) => panic!("read failed: {e}"),
            }
        }
    });

    std::thread::sleep(Duration::from_millis(50));
    let mut client = TcpStream::connect(addr).unwrap();
    std::thread::sleep(Duration::from_millis(150));
    let sent_at = Instant::now();
    client.write_all(b"hello").unwrap();

    let (buf, n) = server.join().unwrap();
    let elapsed = sent_at.elapsed();
    assert_eq!(&buf[..n], b"hello");
    // resumed promptly on arrival, not via a slow poll loop
    assert!(elapsed < Duration::from_millis(100), "{elapsed:?}");
}
