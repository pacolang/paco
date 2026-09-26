//! The tape a gradient records into. Compiled augmented primals push the
//! branches they take and the values they overwrite; pullbacks pop them in
//! reverse. While a tape records, `paco_free` defers every free to the
//! tape's end, so a pullback can still read memory its primal dropped.
//! Adjoints of heap elements live in shadow buffers keyed by the primal
//! buffer's address.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::c_void;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Slice {
    ptr: *mut u8,
    len: i64,
}

#[derive(Default)]
pub struct Tape {
    values: Vec<u8>,
    adjoints: Vec<u8>,
    shadows: HashMap<usize, Vec<u64>>,
    deferred: Vec<*mut c_void>,
    /// Boxed: a tape's handle is its address, still used after it retires.
    #[allow(clippy::vec_box)]
    retired: Vec<Box<Tape>>,
    /// Largest size the value stack reached, in bytes.
    peak: usize,
}

thread_local! {
    // ponytail: per OS thread; a task that yields inside a differentiated
    // function would share it with the tasks it yields to. Per-task tapes
    // if gradients ever run across I/O suspension points.
    static RECORDING: RefCell<Vec<*mut Tape>> = const { RefCell::new(Vec::new()) };
}

/// Hands `ptr` to the innermost recording tape instead of freeing it.
pub(crate) fn defer_free(ptr: *mut c_void) -> bool {
    RECORDING.with(|recording| match recording.borrow().last() {
        Some(&tape) if !ptr.is_null() => {
            unsafe { (*tape).deferred.push(ptr) };
            true
        }
        _ => false,
    })
}

fn tape<'a>(handle: i64) -> &'a mut Tape {
    unsafe { &mut *(handle as *mut Tape) }
}

fn push(stack: &mut Vec<u8>, bytes: &[u8]) {
    stack.extend_from_slice(bytes);
}

fn pop_value<const N: usize>(tape: &mut Tape) -> [u8; N] {
    tape.peak = tape.peak.max(tape.values.len());
    pop(&mut tape.values)
}

fn pop<const N: usize>(stack: &mut Vec<u8>) -> [u8; N] {
    let start = stack.len().checked_sub(N).expect("autodiff tape underflow");
    let bytes = stack[start..].try_into().expect("N bytes");
    stack.truncate(start);
    bytes
}

#[unsafe(no_mangle)]
pub extern "C" fn paco_ad_tape_new() -> i64 {
    let tape = Box::into_raw(Box::<Tape>::default());
    RECORDING.with(|recording| recording.borrow_mut().push(tape));
    tape as i64
}

fn stop_recording(handle: i64) {
    RECORDING.with(|recording| {
        let mut recording = recording.borrow_mut();
        if let Some(index) = recording.iter().rposition(|&tape| tape as i64 == handle) {
            recording.remove(index);
        }
    });
}

fn release(tape: Tape) {
    let Tape { deferred, retired, .. } = tape;
    for inner in retired {
        release(*inner);
    }
    for ptr in deferred {
        unsafe { crate::helpers::raw_free_now(ptr) };
    }
}

fn flag(name: &'static str, cell: &'static std::sync::OnceLock<bool>) -> bool {
    *cell.get_or_init(|| std::env::var_os(name).is_some_and(|value| value == "1"))
}

/// Test-only: `PACO_AD_LEAK_TAPE=1` leaks every tape, so the leak checks can
/// show they would catch a tape that is never freed.
fn leak_tapes() -> bool {
    static LEAK: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    flag("PACO_AD_LEAK_TAPE", &LEAK)
}

/// `PACO_AD_STATS=1` prints each tape's peak size when it is freed.
fn print_stats() -> bool {
    static STATS: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    flag("PACO_AD_STATS", &STATS)
}

#[unsafe(no_mangle)]
pub extern "C" fn paco_ad_tape_free(handle: i64) {
    stop_recording(handle);
    if print_stats() {
        let tape = tape(handle);
        let shadows: usize = tape.shadows.values().map(|shadow| shadow.len() * 8).sum();
        eprintln!("autodiff tape: {} bytes peak, {shadows} bytes of shadows, {} deferred frees", tape.peak, tape.deferred.len());
    }
    if !leak_tapes() {
        release(*unsafe { Box::from_raw(handle as *mut Tape) });
    }
}

/// A tape freed inside a pullback being differentiated: kept until `outer`
/// ends, because the outer pullback reads its adjoints and shadows.
#[unsafe(no_mangle)]
pub extern "C" fn paco_ad_tape_retire(outer: i64, inner: i64) {
    stop_recording(inner);
    tape(outer).retired.push(unsafe { Box::from_raw(inner as *mut Tape) });
}

#[unsafe(no_mangle)]
pub extern "C" fn paco_ad_push_f64(handle: i64, value: f64) {
    push(&mut tape(handle).values, &value.to_le_bytes());
}

#[unsafe(no_mangle)]
pub extern "C" fn paco_ad_pop_f64(handle: i64) -> f64 {
    f64::from_le_bytes(pop_value(tape(handle)))
}

#[unsafe(no_mangle)]
pub extern "C" fn paco_ad_push_f32(handle: i64, value: f32) {
    push(&mut tape(handle).values, &value.to_le_bytes());
}

#[unsafe(no_mangle)]
pub extern "C" fn paco_ad_pop_f32(handle: i64) -> f32 {
    f32::from_le_bytes(pop_value(tape(handle)))
}

#[unsafe(no_mangle)]
pub extern "C" fn paco_ad_push_i64(handle: i64, value: i64) {
    push(&mut tape(handle).values, &value.to_le_bytes());
}

#[unsafe(no_mangle)]
pub extern "C" fn paco_ad_pop_i64(handle: i64) -> i64 {
    i64::from_le_bytes(pop_value(tape(handle)))
}

/// # Safety
/// `source` points to `size` readable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn paco_ad_push_bytes(handle: i64, source: *const u8, size: i64) {
    let bytes = unsafe { std::slice::from_raw_parts(source, size as usize) };
    push(&mut tape(handle).values, bytes);
}

/// # Safety
/// `target` points to `size` writable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn paco_ad_pop_bytes(handle: i64, target: *mut u8, size: i64) {
    let tape = tape(handle);
    tape.peak = tape.peak.max(tape.values.len());
    let values = &mut tape.values;
    let start = values.len().checked_sub(size as usize).expect("autodiff tape underflow");
    unsafe { std::ptr::copy_nonoverlapping(values[start..].as_ptr(), target, size as usize) };
    values.truncate(start);
}

/// Copies `size` bytes without running any ownership logic: how generated
/// code hands a value over without a clone or a drop.
///
/// # Safety
/// Both pointers reach `size` bytes; the ranges do not overlap.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn paco_ad_copy(target: *mut u8, source: *const u8, size: i64) {
    unsafe { std::ptr::copy_nonoverlapping(source, target, size as usize) };
}

/// Zeroed scratch memory for a value generated code must hold without
/// owning it: moving a value into it clears the source's drop flag, and
/// `paco_ad_unhold` releases the memory without dropping what it holds.
#[unsafe(no_mangle)]
pub extern "C" fn paco_ad_hold(size: i64) -> *mut c_void {
    crate::helpers::paco_calloc(1, size.max(1) as usize)
}

/// # Safety
/// `ptr` comes from `paco_ad_hold`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn paco_ad_unhold(ptr: *mut c_void) {
    unsafe { crate::helpers::paco_free(ptr) }
}

#[unsafe(no_mangle)]
pub extern "C" fn paco_ad_adj_push_f64(handle: i64, value: f64) {
    push(&mut tape(handle).adjoints, &value.to_le_bytes());
}

#[unsafe(no_mangle)]
pub extern "C" fn paco_ad_adj_pop_f64(handle: i64) -> f64 {
    f64::from_le_bytes(pop(&mut tape(handle).adjoints))
}

#[unsafe(no_mangle)]
pub extern "C" fn paco_ad_adj_push_f32(handle: i64, value: f32) {
    push(&mut tape(handle).adjoints, &value.to_le_bytes());
}

#[unsafe(no_mangle)]
pub extern "C" fn paco_ad_adj_pop_f32(handle: i64) -> f32 {
    f32::from_le_bytes(pop(&mut tape(handle).adjoints))
}

/// Writes to `out` a slice over the zero-initialised adjoints of the
/// elements of `primal`, created on first use.
///
/// # Safety
/// `primal` and `out` point to slice descriptors.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn paco_ad_shadow(handle: i64, primal: *const Slice, elem_size: i64, out: *mut Slice) {
    let primal = unsafe { *primal };
    let words = ((primal.len.max(0) * elem_size) as usize).div_ceil(8);
    let shadow = tape(handle).shadows.entry(primal.ptr as usize).or_insert_with(|| vec![0; words]);
    if shadow.len() < words {
        shadow.resize(words, 0);
    }
    unsafe { *out = Slice { ptr: shadow.as_mut_ptr().cast(), len: primal.len } };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_come_back_last_in_first_out() {
        let handle = paco_ad_tape_new();
        paco_ad_push_f64(handle, 1.5);
        paco_ad_push_i64(handle, 7);
        let bytes = [1u8, 2, 3];
        unsafe { paco_ad_push_bytes(handle, bytes.as_ptr(), 3) };
        paco_ad_push_f32(handle, 2.5);
        assert_eq!(paco_ad_pop_f32(handle), 2.5);
        let mut back = [0u8; 3];
        unsafe { paco_ad_pop_bytes(handle, back.as_mut_ptr(), 3) };
        assert_eq!(back, bytes);
        assert_eq!(paco_ad_pop_i64(handle), 7);
        assert_eq!(paco_ad_pop_f64(handle), 1.5);
        assert_eq!(tape(handle).peak, 8 + 8 + 3 + 4);
        paco_ad_tape_free(handle);
    }

    #[test]
    fn a_shadow_starts_at_zero_and_is_found_again_by_address() {
        let handle = paco_ad_tape_new();
        let mut data = [1.0f64, 2.0, 3.0];
        let primal = Slice { ptr: data.as_mut_ptr().cast(), len: 3 };
        let mut first = Slice { ptr: std::ptr::null_mut(), len: 0 };
        unsafe { paco_ad_shadow(handle, &primal, 8, &mut first) };
        assert_eq!(first.len, 3);
        let values = unsafe { std::slice::from_raw_parts_mut(first.ptr.cast::<f64>(), 3) };
        assert_eq!(values, [0.0; 3]);
        values[1] = 4.0;
        let mut again = Slice { ptr: std::ptr::null_mut(), len: 0 };
        unsafe { paco_ad_shadow(handle, &primal, 8, &mut again) };
        assert_eq!(again.ptr, first.ptr);
        assert_eq!(unsafe { *again.ptr.cast::<f64>().add(1) }, 4.0);
        paco_ad_tape_free(handle);
    }

    #[test]
    fn frees_wait_for_the_recording_tape_and_retired_tapes_for_the_outer_one() {
        let outer = paco_ad_tape_new();
        let inner = paco_ad_tape_new();
        let buffer = crate::helpers::paco_alloc(16);
        unsafe { crate::helpers::paco_free(buffer) };
        assert_eq!(tape(inner).deferred, [buffer]);
        paco_ad_tape_retire(outer, inner);
        let other = crate::helpers::paco_alloc(16);
        unsafe { crate::helpers::paco_free(other) };
        assert_eq!(tape(outer).deferred, [other]);
        assert_eq!(tape(outer).retired.len(), 1);
        paco_ad_tape_free(outer);
        assert!(!defer_free(other), "no tape records after the outermost one is freed");
    }

    #[test]
    fn adjoints_have_a_stack_of_their_own() {
        let handle = paco_ad_tape_new();
        paco_ad_push_f64(handle, 1.0);
        paco_ad_adj_push_f64(handle, 2.0);
        assert_eq!(paco_ad_adj_pop_f64(handle), 2.0);
        assert_eq!(paco_ad_pop_f64(handle), 1.0);
        paco_ad_tape_free(handle);
    }
}
