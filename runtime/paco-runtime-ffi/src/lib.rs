//! Type-erased `extern "C"` FFI surface over `paco-runtime`, callable from
//! natively compiled Paco code (`paco-codegen-cranelift`). `paco-runtime`'s
//! own public API is generic over `T`; native codegen has no generics at
//! the machine-code level, so every value crossing this boundary — a
//! `spawn` thunk's captures, a task's result, a channel element, a
//! generator's yielded value — is raw bytes. The caller (generated code,
//! which knows every Paco type's exact layout from `paco-mir::layout`)
//! allocates and owns any buffer it passes in; these functions treat it as
//! opaque.
//!
//! Every handle is an `Arc` raw pointer: `paco_rt_*_retain` adds an owner,
//! `paco_rt_*_release` drops one, and the last release frees the handle.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use corosensei::{Coroutine, CoroutineResult, Yielder};

mod autodiff;
mod helpers;
mod math;
mod net;
mod panic;
mod trace;
pub use helpers::*;
pub use net::*;
pub use panic::*;

static RUNTIME: OnceLock<paco_runtime::Runtime> = OnceLock::new();

fn runtime() -> &'static paco_runtime::Runtime {
    RUNTIME.get().expect("paco_rt_init was not called before this entry point")
}

/// Initializes the process-global runtime. Must be called exactly once,
/// before any other `paco_rt_*` entry point; [`paco_rt_main`] calls it
/// before the program's `main` runs.
#[unsafe(no_mangle)]
pub extern "C" fn paco_rt_init() {
    let worker_count = std::thread::available_parallelism().map_or(1, |count| count.get());
    RUNTIME
        .set(paco_runtime::Runtime::new(worker_count))
        .unwrap_or_else(|_| panic!("paco_rt_init was called more than once"));
}

/// An owned, 16-byte-aligned byte buffer — safely over-aligned for every
/// layout `paco-mir::layout` produces today (max alignment 8, for
/// `i64`/`u64`/`f64`/pointers), without hand-rolled `std::alloc`
/// bookkeeping. Backed by `Vec<u128>` so it is `Send` for free.
struct Bytes(Vec<u128>);

impl Bytes {
    fn zeroed(len: usize) -> Self {
        Self(vec![0u128; len.div_ceil(16)])
    }

    fn as_mut_ptr(&mut self) -> *mut u8 {
        self.0.as_mut_ptr().cast()
    }

    fn as_ptr(&self) -> *const u8 {
        self.0.as_ptr().cast()
    }
}

// ---------------------------------------------------------------------
// spawn / join
// ---------------------------------------------------------------------

pub struct JoinHandleOpaque(paco_runtime::JoinHandle<Bytes>);

/// Spawns `thunk` as a real task on the scheduler.
///
/// `thunk` is `"C-unwind"`, not plain `"C"`: `paco_runtime::spawn`'s body
/// already wraps its closure in `catch_unwind` to turn a panicking task into
/// `Err(TaskPanic)` rather than tearing down the whole scheduler (see
/// `paco-runtime::join::spawn`) — plain `"C"` makes unwinding across the
/// call an abort by definition (Rust's FFI-unwind safety rule), which would
/// defeat that entirely. Cranelift-generated callees never actually unwind
/// (a compiled trap kills the process directly, independent of this
/// declared ABI), so this costs real compiled code nothing; it only matters
/// for a thunk that can genuinely panic, such as this crate's own Rust-only
/// ABI tests.
///
/// # Safety
/// `thunk` must be a valid function pointer that reads at most
/// `captures_len` bytes from its first argument and writes exactly
/// `result_len` bytes to its second argument before returning (or panics,
/// per the ABI note above). `captures` must point to `captures_len` valid,
/// readable bytes for the duration of this call (they are copied out
/// before it returns).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn paco_rt_spawn(
    thunk: unsafe extern "C-unwind" fn(*const u8, *mut u8),
    captures: *const u8,
    captures_len: usize,
    result_len: usize,
) -> *mut JoinHandleOpaque {
    let mut captures_buf = Bytes::zeroed(captures_len);
    if captures_len > 0 {
        unsafe { std::ptr::copy_nonoverlapping(captures, captures_buf.as_mut_ptr(), captures_len) };
    }
    let handle = paco_runtime::spawn(runtime(), move || {
        let mut result_buf = Bytes::zeroed(result_len);
        let captures_ptr = if captures_len == 0 { std::ptr::null() } else { captures_buf.as_ptr() };
        unsafe { thunk(captures_ptr, result_buf.as_mut_ptr()) };
        result_buf
    });
    Arc::into_raw(Arc::new(JoinHandleOpaque(handle))).cast_mut()
}

/// [`paco_rt_spawn`], but runs `thunk` on the blocking pool.
///
/// # Safety
/// Same contract as [`paco_rt_spawn`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn paco_rt_spawn_blocking(
    thunk: unsafe extern "C-unwind" fn(*const u8, *mut u8),
    captures: *const u8,
    captures_len: usize,
    result_len: usize,
) -> *mut JoinHandleOpaque {
    let mut captures_buf = Bytes::zeroed(captures_len);
    if captures_len > 0 {
        unsafe { std::ptr::copy_nonoverlapping(captures, captures_buf.as_mut_ptr(), captures_len) };
    }
    let handle = runtime().spawn_blocking(move || {
        let mut result_buf = Bytes::zeroed(result_len);
        let captures_ptr = if captures_len == 0 { std::ptr::null() } else { captures_buf.as_ptr() };
        unsafe { thunk(captures_ptr, result_buf.as_mut_ptr()) };
        result_buf
    });
    Arc::into_raw(Arc::new(JoinHandleOpaque(handle))).cast_mut()
}

/// Blocks until `handle`'s task finishes, filling `result_out` with
/// `result_len` bytes on success. Returns `0` on success, `1` if the task
/// panicked, with the panic message written to `message_out` when it is not
/// null (`result_out` is left unwritten). `handle` stays owned by the
/// caller.
///
/// # Safety
/// `handle` must be a live pointer previously returned by
/// [`paco_rt_spawn`], `result_out` must point to at least `result_len`
/// valid, writable bytes, and `message_out` must be null or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn paco_rt_join(
    handle: *mut JoinHandleOpaque,
    result_out: *mut u8,
    result_len: usize,
    message_out: *mut PacoStr,
) -> i32 {
    let handle = unsafe { &*handle };
    match handle.0.join() {
        Ok(bytes) => {
            if result_len > 0 {
                unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), result_out, result_len) };
            }
            0
        }
        Err(panic) => {
            if !message_out.is_null() {
                write_str(unsafe { &mut *message_out }, panic.message.as_bytes());
            }
            1
        }
    }
}

// ---------------------------------------------------------------------
// channels
// ---------------------------------------------------------------------

pub struct SenderOpaque(paco_runtime::Sender<Vec<u8>>);
pub struct ReceiverOpaque(paco_runtime::Receiver<Vec<u8>>);

/// Writes the new pair's handles through `sender_out`/`receiver_out` rather
/// than returning a 2-pointer struct by value: a small `repr(C)` struct
/// return crosses into ABI territory (register- vs. memory-class
/// decomposition) that `paco-mir`/`paco-codegen-cranelift` have no need to
/// model elsewhere in this FFI surface — every other multi-output entry
/// point here (`paco_rt_join`, `paco_rt_recv`) already writes through an
/// output pointer instead, so this keeps the whole surface one convention.
///
/// # Safety
/// `sender_out` and `receiver_out` must point to valid, writable pointer-
/// sized storage.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn paco_rt_channel(
    capacity: usize,
    sender_out: *mut *mut SenderOpaque,
    receiver_out: *mut *mut ReceiverOpaque,
) {
    let (sender, receiver) = paco_runtime::channel(runtime(), capacity);
    unsafe {
        *sender_out = Arc::into_raw(Arc::new(SenderOpaque(sender))).cast_mut();
        *receiver_out = Arc::into_raw(Arc::new(ReceiverOpaque(receiver))).cast_mut();
    }
}

/// Sends `value_len` bytes from `value` on `sender`. Returns `0` on
/// success, `1` if the channel is closed.
///
/// # Safety
/// `sender` must be a live pointer from [`paco_rt_channel`], and `value`
/// must point to `value_len` valid, readable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn paco_rt_send(sender: *mut SenderOpaque, value: *const u8, value_len: usize) -> i32 {
    let sender = unsafe { &*sender };
    let bytes = if value_len == 0 {
        Vec::new()
    } else {
        unsafe { std::slice::from_raw_parts(value, value_len) }.to_vec()
    };
    match sender.0.send(bytes) {
        Ok(()) => 0,
        Err(_) => 1,
    }
}

/// Receives one value into `value_out` (`value_len` bytes). Returns `0` on
/// success, `1` if the channel is closed and empty.
///
/// # Safety
/// `receiver` must be a live pointer from [`paco_rt_channel`], and
/// `value_out` must point to at least `value_len` valid, writable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn paco_rt_recv(receiver: *mut ReceiverOpaque, value_out: *mut u8, value_len: usize) -> i32 {
    let receiver = unsafe { &*receiver };
    match receiver.0.recv() {
        Ok(bytes) => {
            if value_len > 0 {
                let len = value_len.min(bytes.len());
                unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), value_out, len) };
            }
            0
        }
        Err(_) => 1,
    }
}

/// # Safety
/// `sender` must be a live pointer from [`paco_rt_channel`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn paco_rt_sender_close(sender: *mut SenderOpaque) {
    unsafe { &*sender }.0.close();
}

/// # Safety
/// `receiver` must be a live pointer from [`paco_rt_channel`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn paco_rt_receiver_close(receiver: *mut ReceiverOpaque) {
    unsafe { &*receiver }.0.close();
}

/// # Safety
/// `sender` must be a live pointer from [`paco_rt_channel`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn paco_rt_sender_is_ready(sender: *mut SenderOpaque) -> i32 {
    i32::from(unsafe { &*sender }.0.is_ready())
}

/// # Safety
/// `receiver` must be a live pointer from [`paco_rt_channel`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn paco_rt_receiver_is_ready(receiver: *mut ReceiverOpaque) -> i32 {
    i32::from(unsafe { &*receiver }.0.is_ready())
}

// ---------------------------------------------------------------------
// generators (`iter fn`)
// ---------------------------------------------------------------------

/// The resume input is `true` when the generator is being dropped.
type GenCoroutine = Coroutine<bool, (), (), corosensei::stack::DefaultStack>;
type GenThunk = unsafe extern "C-unwind" fn(*const u8, i64);

pub struct GeneratorOpaque {
    coroutine: Mutex<Option<GenCoroutine>>,
    yielder: Box<AtomicUsize>,
    _captures: Bytes,
}

unsafe fn suspend_generator(slot: *const ()) {
    let yielder = unsafe { &*slot.cast::<AtomicUsize>() }.load(Ordering::Acquire) as *const Yielder<bool, ()>;
    unsafe { &*yielder }.suspend(());
}

impl GeneratorOpaque {
    /// A panic in the generator's body is raised again in its caller.
    fn resume(&self, coroutine: &mut GenCoroutine, cancel: bool) -> std::thread::Result<CoroutineResult<(), ()>> {
        let outer = CURRENT_YIELDER.with(|cell| cell.replace(self.yielder.load(Ordering::Acquire) as *const ()));
        let isolated = paco_runtime::isolate(std::ptr::from_ref(self.yielder.as_ref()).cast(), suspend_generator);
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| coroutine.resume(cancel)));
        drop(isolated);
        CURRENT_YIELDER.with(|cell| cell.set(outer));
        if let Some(message) = paco_runtime::take_abandoned() {
            unsafe { coroutine.force_reset() };
            panic::raise(message);
        }
        outcome
    }
}

impl Drop for GeneratorOpaque {
    fn drop(&mut self) {
        let Some(mut coroutine) = self.coroutine.get_mut().unwrap_or_else(std::sync::PoisonError::into_inner).take()
        else {
            return;
        };
        if coroutine.done() {
            return;
        }
        // A cancelled thunk returns from its suspension point, dropping its
        // live locals; one that yields anyway has its stack discarded.
        if let Ok(CoroutineResult::Yield(())) = self.resume(&mut coroutine, true) {
            unsafe { coroutine.force_reset() };
        }
    }
}

// Safety: as for `paco-runtime::task::TaskEntry`, only one thread ever
// resumes a given coroutine at a time, serialized by `coroutine`'s mutex.
unsafe impl Send for GeneratorOpaque {}
unsafe impl Sync for GeneratorOpaque {}

thread_local! {
    static CURRENT_YIELDER: std::cell::Cell<*const ()> = const { std::cell::Cell::new(std::ptr::null()) };
    static CURRENT_YIELD_BUF: std::cell::RefCell<Option<Vec<u8>>> = const { std::cell::RefCell::new(None) };
}

/// Wraps `thunk` in a coroutine without running it — `.next()` drives it.
///
/// # Safety
/// `thunk` must be a valid function pointer reading at most `captures_len`
/// bytes from its argument; it must call [`paco_rt_generator_yield`] (not
/// return normally with a value) to hand values back. `captures` must
/// point to `captures_len` valid, readable bytes for the duration of this
/// call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn paco_rt_generator_new(
    thunk: GenThunk,
    captures: *const u8,
    captures_len: usize,
) -> *mut GeneratorOpaque {
    let mut captures_buf = Bytes::zeroed(captures_len);
    if captures_len > 0 {
        unsafe { std::ptr::copy_nonoverlapping(captures, captures_buf.as_mut_ptr(), captures_len) };
    }
    let captures_addr = if captures_len == 0 { 0 } else { captures_buf.as_ptr() as usize };
    let yielder = Box::new(AtomicUsize::new(0));
    let slot = std::ptr::from_ref(yielder.as_ref()) as usize;
    let coroutine = GenCoroutine::new(move |yielder: &Yielder<bool, ()>, cancel: bool| {
        let ptr: *const () = std::ptr::from_ref(yielder).cast();
        unsafe { &*(slot as *const AtomicUsize) }.store(ptr as usize, Ordering::Release);
        CURRENT_YIELDER.with(|cell| cell.set(ptr));
        unsafe { thunk(captures_addr as *const u8, i64::from(cancel)) };
    });
    Arc::into_raw(Arc::new(GeneratorOpaque {
        coroutine: Mutex::new(Some(coroutine)),
        yielder,
        _captures: captures_buf,
    }))
    .cast_mut()
}

/// Called from within a running generator thunk (`Expr::Yield`'s lowering)
/// to suspend and hand a value back to the pending [`paco_rt_generator_next`]
/// call. Returns `1` when the generator is resumed only to be dropped: the
/// thunk must then return without yielding again.
///
/// # Safety
/// Must be called only from a thunk currently running under
/// [`paco_rt_generator_new`]'s coroutine. `elem` must point to `elem_len`
/// valid, readable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn paco_rt_generator_yield(elem: *const u8, elem_len: usize) -> i32 {
    let bytes = if elem_len == 0 {
        Vec::new()
    } else {
        unsafe { std::slice::from_raw_parts(elem, elem_len) }.to_vec()
    };
    let ptr = CURRENT_YIELDER.with(std::cell::Cell::get);
    assert!(!ptr.is_null(), "paco_rt_generator_yield called outside a running generator");
    let yielder = unsafe { &*ptr.cast::<Yielder<bool, ()>>() };
    CURRENT_YIELD_BUF.with(|cell| *cell.borrow_mut() = Some(bytes));
    i32::from(yielder.suspend(()))
}

/// Resumes `handle`. Returns `0` with `elem_out` filled (`elem_len` bytes)
/// if the thunk yielded a value, `1` if the thunk ran to completion (no
/// more values — every later call also returns `1`), `2` if the thunk
/// panicked (also terminal — every later call returns `1`, matching a
/// finished generator, since there is nothing left to resume).
///
/// # Safety
/// `handle` must be a live pointer from [`paco_rt_generator_new`], and
/// `elem_out` must point to at least `elem_len` valid, writable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn paco_rt_generator_next(
    handle: *mut GeneratorOpaque,
    elem_out: *mut u8,
    elem_len: usize,
) -> i32 {
    let handle = unsafe { &*handle };
    let mut guard = handle.coroutine.lock().unwrap();
    let Some(coroutine) = guard.as_mut() else {
        return 1;
    };
    match handle.resume(coroutine, false) {
        Ok(CoroutineResult::Yield(())) => {
            let bytes = CURRENT_YIELD_BUF
                .with(|cell| cell.borrow_mut().take())
                .expect("generator yielded without paco_rt_generator_yield setting a value");
            if elem_len > 0 {
                let len = elem_len.min(bytes.len());
                unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), elem_out, len) };
            }
            0
        }
        Ok(CoroutineResult::Return(())) => {
            *guard = None;
            1
        }
        Err(_) => {
            *guard = None;
            2
        }
    }
}

// ---------------------------------------------------------------------
// ownership
// ---------------------------------------------------------------------

macro_rules! refcounted {
    ($opaque:ty, $retain:ident, $release:ident) => {
        /// # Safety
        /// `handle` must be null or a live handle of this kind.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $retain(handle: *mut $opaque) {
            if !handle.is_null() {
                unsafe { Arc::increment_strong_count(handle.cast_const()) };
            }
        }

        /// # Safety
        /// `handle` must be null or a live handle of this kind; the caller
        /// gives up its ownership.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $release(handle: *mut $opaque) {
            if !handle.is_null() {
                unsafe { drop(Arc::from_raw(handle.cast_const())) };
            }
        }
    };
}

refcounted!(JoinHandleOpaque, paco_rt_join_handle_retain, paco_rt_join_handle_release);
refcounted!(SenderOpaque, paco_rt_sender_retain, paco_rt_sender_release);
refcounted!(ReceiverOpaque, paco_rt_receiver_retain, paco_rt_receiver_release);
refcounted!(GeneratorOpaque, paco_rt_generator_retain, paco_rt_generator_release);
refcounted!(ListenerOpaque, paco_rt_tcp_listener_retain, paco_rt_tcp_listener_release);
refcounted!(StreamOpaque, paco_rt_tcp_stream_retain, paco_rt_tcp_stream_release);
