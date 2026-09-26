use std::ffi::{CStr, c_char};
use std::io::Write;

use crate::helpers::{PacoStr, flush_stdout};

/// Where a panic happened: the source position, and in debug builds the
/// panicking function's frame pointer and address (0 otherwise).
struct PanicSite {
    file: *const c_char,
    line: u32,
    column: u32,
    frame: usize,
    function: usize,
}

fn report(message: &str, site: &PanicSite) {
    flush_stdout();
    let file = if site.file.is_null() { "<unknown>".into() } else { unsafe { CStr::from_ptr(site.file) }.to_string_lossy() };
    let mut stderr = std::io::stderr().lock();
    let _ = writeln!(stderr, "panic at {file}:{}:{}: {message}", site.line, site.column);
    if site.frame != 0 {
        crate::trace::write(&mut stderr, site.frame, site.function, (&file, site.line, site.column));
    }
}

/// Leaves the innermost task or generator, or ends the process when there
/// is none.
pub(crate) fn raise(message: String) -> ! {
    let _ = paco_runtime::abandon(message);
    flush_stdout();
    std::process::exit(101)
}

fn panic_with(message: String, site: PanicSite) -> ! {
    report(&message, &site);
    raise(message)
}

/// # Safety
/// `message` must point to a valid string; `file` must be null or a
/// NUL-terminated string; `frame` and `function` are 0 or the panicking
/// function's frame pointer and address.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn paco_rt_panic(
    message: *const PacoStr,
    file: *const c_char,
    line: u32,
    column: u32,
    frame: usize,
    function: usize,
) -> ! {
    let message = String::from_utf8_lossy(unsafe { (*message).bytes() }).into_owned();
    panic_with(message, PanicSite { file, line, column, frame, function })
}

/// # Safety
/// As [`paco_rt_panic`], with `message` a NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn paco_rt_panic_str(
    message: *const c_char,
    file: *const c_char,
    line: u32,
    column: u32,
    frame: usize,
    function: usize,
) -> ! {
    let message = unsafe { CStr::from_ptr(message) }.to_string_lossy().into_owned();
    panic_with(message, PanicSite { file, line, column, frame, function })
}

/// # Safety
/// As [`paco_rt_panic`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn paco_rt_panic_bounds(
    index: i64,
    len: i64,
    file: *const c_char,
    line: u32,
    column: u32,
    frame: usize,
    function: usize,
) -> ! {
    let message = format!("index {index} out of bounds for length {len}");
    panic_with(message, PanicSite { file, line, column, frame, function })
}

/// A runtime failure with no Paco source location.
pub(crate) fn panic_without_location(message: String) -> ! {
    report(&message, &PanicSite { file: std::ptr::null(), line: 0, column: 0, frame: 0, function: 0 });
    raise(message)
}
