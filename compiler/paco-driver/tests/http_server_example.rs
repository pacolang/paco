use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};
use std::{fs, path::PathBuf};

use clap::Parser;
use paco_driver::{Cli, DriverOutput, run};

const EXAMPLE_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/conformance/programs/http_server/input.paco");

/// Task 5.1: the checked-in example parses and type-checks with `paco check`.
#[test]
fn check_accepts_the_http_server_example() {
    let cli = Cli::try_parse_from(["paco", "check", EXAMPLE_PATH]).unwrap();

    let output = run(cli).unwrap();

    assert_eq!(output, DriverOutput { stdout: String::new(), stderr: String::new() });
}

/// Task 5.2: run a bounded variant of the same `accept` → `spawn` →
/// `read`/`write` pattern under `paco run` and prove concurrent
/// connections do not block each other. A slow client (sends its request
/// only after a delay) and a fast client (sends immediately) connect
/// around the same time; the fast client's response must arrive well
/// before the slow client even sends its request — the M:N scheduler
/// suspends only the task blocked on the slow socket read, not the whole
/// server. Uses a fixed port (the http-server example's own listener starts
/// only once `.accept()` unblocks, and the program's `print` buffers to a
/// string returned only when the program finishes — there is no way for
/// this test to read back an OS-assigned port while the server is still
/// running, so the port is fixed and chosen to be unlikely to collide).
#[test]
fn run_http_server_handles_a_slow_and_a_fast_connection_concurrently() {
    let source = r#"
fn respond(stream: TcpStream) {
    let request = stream.read(4096);
    stream.write("HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nOK")
}

fn main() {
    let listener = tcp_listen(58417);
    let first = listener.accept();
    let h1 = spawn respond(first);
    let second = listener.accept();
    let h2 = spawn respond(second);
    h1.join();
    h2.join();
}
"#;
    let file = write_temp_paco("http_server_concurrency", source);
    let cli = Cli::try_parse_from(["paco", "run", file.to_str().unwrap()]).unwrap();
    let server = std::thread::spawn(move || run(cli));

    let mut slow = connect_with_retry(58417);
    let mut fast = connect_with_retry(58417);

    fast.write_all(b"GET / HTTP/1.1\r\nHost: x\r\n\r\n").unwrap();
    let fast_start = Instant::now();
    let mut buf = [0u8; 4096];
    let n = fast.read(&mut buf).unwrap();
    let fast_elapsed = fast_start.elapsed();
    assert!(std::str::from_utf8(&buf[..n]).unwrap().starts_with("HTTP/1.1 200"));

    // The slow client only sends its request well after the fast client
    // already got its response — proving the server was not blocked
    // waiting to read from the slow connection.
    assert!(
        fast_elapsed < Duration::from_millis(250),
        "fast client waited {fast_elapsed:?} for a response"
    );

    std::thread::sleep(Duration::from_millis(300));
    slow.write_all(b"GET / HTTP/1.1\r\nHost: x\r\n\r\n").unwrap();
    let n = slow.read(&mut buf).unwrap();
    assert!(std::str::from_utf8(&buf[..n]).unwrap().starts_with("HTTP/1.1 200"));

    let output = server.join().unwrap().unwrap();
    assert!(output.stdout.contains("LISTENING 58417"));
}

fn connect_with_retry(port: u16) -> TcpStream {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match TcpStream::connect(("127.0.0.1", port)) {
            Ok(stream) => return stream,
            Err(_) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(error) => panic!("could not connect to the test server: {error}"),
        }
    }
}

fn write_temp_paco(name: &str, source: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!(
        "paco_http_server_{}_{}_{}.paco",
        name,
        std::process::id(),
        monotonic_suffix()
    ));
    fs::write(&path, source).unwrap();
    path
}

fn monotonic_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}
