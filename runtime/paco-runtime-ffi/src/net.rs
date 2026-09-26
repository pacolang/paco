use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;

use crate::helpers::{PacoStr, print_line_bytes, write_str};

pub struct ListenerOpaque(TcpListener);
pub struct StreamOpaque(TcpStream);

fn fail(what: &str, error: std::io::Error) -> ! {
    crate::panic::panic_without_location(format!("{what} failed: {error}"))
}

/// Listens on every interface at `port`, printing `LISTENING <port>`.
#[unsafe(no_mangle)]
pub extern "C" fn paco_rt_tcp_listen(port: i64) -> *mut ListenerOpaque {
    let listener = TcpListener::bind(("0.0.0.0", port.clamp(0, i64::from(u16::MAX)) as u16))
        .and_then(|listener| listener.set_nonblocking(true).map(|()| listener))
        .unwrap_or_else(|error| fail("tcp_listen", error));
    let bound = listener.local_addr().map(|address| address.port()).unwrap_or_else(|error| fail("tcp_listen", error));
    print_line_bytes(format!("LISTENING {bound}").as_bytes());
    Arc::into_raw(Arc::new(ListenerOpaque(listener))).cast_mut()
}

/// Waits for the next connection, suspending only the calling task.
///
/// # Safety
/// `listener` must be a live pointer from [`paco_rt_tcp_listen`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn paco_rt_tcp_accept(listener: *mut ListenerOpaque) -> *mut StreamOpaque {
    let listener = &unsafe { &*listener }.0;
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                stream.set_nonblocking(true).unwrap_or_else(|error| fail("accept", error));
                return Arc::into_raw(Arc::new(StreamOpaque(stream))).cast_mut();
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                crate::runtime().wait_readable(listener).unwrap_or_else(|error| fail("accept", error));
            }
            Err(error) => fail("accept", error),
        }
    }
}

/// Reads at most `max_len` bytes, suspending only the calling task.
///
/// # Safety
/// `stream` must be a live pointer from [`paco_rt_tcp_accept`] and `out`
/// writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn paco_rt_tcp_read(stream: *mut StreamOpaque, max_len: i64, out: *mut PacoStr) {
    let stream = &unsafe { &*stream }.0;
    let mut buf = vec![0u8; max_len.max(0) as usize];
    loop {
        match (&*stream).read(&mut buf) {
            Ok(n) => {
                let text = String::from_utf8_lossy(&buf[..n]);
                write_str(unsafe { &mut *out }, text.as_bytes());
                return;
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                crate::runtime().wait_readable(stream).unwrap_or_else(|error| fail("read", error));
            }
            Err(error) => fail("read", error),
        }
    }
}

/// Writes all of `data`, suspending only the calling task.
///
/// # Safety
/// `stream` must be a live pointer from [`paco_rt_tcp_accept`] and `data`
/// a valid string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn paco_rt_tcp_write(stream: *mut StreamOpaque, data: *const PacoStr) {
    let stream = &unsafe { &*stream }.0;
    let bytes = unsafe { (*data).bytes() };
    let mut offset = 0;
    while offset < bytes.len() {
        match (&*stream).write(&bytes[offset..]) {
            Ok(n) => offset += n,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                crate::runtime().wait_writable(stream).unwrap_or_else(|error| fail("write", error));
            }
            Err(error) => fail("write", error),
        }
    }
}
