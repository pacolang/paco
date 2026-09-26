use std::ffi::c_void;

use paco_runtime::text;
use std::io::{BufWriter, Stdout, Write};
use std::sync::{LazyLock, Mutex};

#[repr(C)]
#[derive(Clone, Copy)]
pub struct PacoStr {
    pub ptr: *const u8,
    pub len: i64,
}

impl PacoStr {
    pub(crate) unsafe fn bytes(&self) -> &[u8] {
        if self.len <= 0 { &[] } else { unsafe { std::slice::from_raw_parts(self.ptr, self.len as usize) } }
    }
}

#[cfg(not(feature = "system-alloc"))]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[cfg(not(feature = "system-alloc"))]
unsafe extern "C" {
    #[link_name = "mi_malloc"]
    fn raw_malloc(size: usize) -> *mut c_void;
    #[link_name = "mi_calloc"]
    fn raw_calloc(count: usize, size: usize) -> *mut c_void;
    #[link_name = "mi_realloc"]
    fn raw_realloc(ptr: *mut c_void, size: usize) -> *mut c_void;
    #[link_name = "mi_free"]
    fn raw_free(ptr: *mut c_void);
}

#[cfg(feature = "system-alloc")]
unsafe extern "C" {
    #[link_name = "malloc"]
    fn raw_malloc(size: usize) -> *mut c_void;
    #[link_name = "calloc"]
    fn raw_calloc(count: usize, size: usize) -> *mut c_void;
    #[link_name = "realloc"]
    fn raw_realloc(ptr: *mut c_void, size: usize) -> *mut c_void;
    #[link_name = "free"]
    fn raw_free(ptr: *mut c_void);
}

#[unsafe(no_mangle)]
pub extern "C" fn paco_alloc(size: usize) -> *mut c_void {
    unsafe { raw_malloc(size) }
}

#[unsafe(no_mangle)]
pub extern "C" fn paco_calloc(count: usize, size: usize) -> *mut c_void {
    unsafe { raw_calloc(count, size) }
}

/// # Safety
/// `ptr` must be null or come from a `paco_*` allocation function.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn paco_realloc(ptr: *mut c_void, size: usize) -> *mut c_void {
    unsafe { raw_realloc(ptr, size) }
}

/// # Safety
/// `ptr` must be null or come from a `paco_*` allocation function.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn paco_free(ptr: *mut c_void) {
    if !crate::autodiff::defer_free(ptr) {
        unsafe { raw_free(ptr) }
    }
}

/// # Safety
/// `ptr` must be null or come from a `paco_*` allocation function.
pub(crate) unsafe fn raw_free_now(ptr: *mut c_void) {
    unsafe { raw_free(ptr) }
}

static STDOUT: LazyLock<Mutex<BufWriter<Stdout>>> = LazyLock::new(|| Mutex::new(BufWriter::new(std::io::stdout())));

pub(crate) fn print_line_bytes(bytes: &[u8]) {
    print_line(bytes);
}

fn print_line(bytes: &[u8]) {
    let mut out = STDOUT.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let _ = out.write_all(bytes);
    let _ = out.write_all(b"\n");
}

pub fn flush_stdout() {
    let _ = STDOUT.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).flush();
}

/// Copies `src` to `dst`, inlining copies of up to 32 bytes: musl's
/// `memcpy` pays a `rep movs` start-up cost that dominates short strings.
unsafe fn copy_bytes(src: &[u8], dst: *mut u8) {
    let (len, from) = (src.len(), src.as_ptr());
    unsafe {
        match len {
            0 => {}
            1..4 => {
                for (index, &byte) in src.iter().enumerate() {
                    dst.add(index).write(byte);
                }
            }
            4..8 => {
                dst.cast::<u32>().write_unaligned(from.cast::<u32>().read_unaligned());
                dst.add(len - 4).cast::<u32>().write_unaligned(from.add(len - 4).cast::<u32>().read_unaligned());
            }
            8..=16 => {
                dst.cast::<u64>().write_unaligned(from.cast::<u64>().read_unaligned());
                dst.add(len - 8).cast::<u64>().write_unaligned(from.add(len - 8).cast::<u64>().read_unaligned());
            }
            17..=32 => {
                dst.cast::<u128>().write_unaligned(from.cast::<u128>().read_unaligned());
                dst.add(len - 16).cast::<u128>().write_unaligned(from.add(len - 16).cast::<u128>().read_unaligned());
            }
            _ => std::ptr::copy_nonoverlapping(from, dst, len),
        }
    }
}

pub(crate) fn write_str(out: &mut PacoStr, bytes: &[u8]) {
    let copy = paco_alloc(bytes.len().max(1)).cast::<u8>();
    unsafe { copy_bytes(bytes, copy) };
    out.ptr = copy;
    out.len = bytes.len() as i64;
}

#[unsafe(no_mangle)]
pub extern "C" fn paco_print_int(value: i64) {
    print_line(text::int_to_string(value).as_bytes());
}

#[unsafe(no_mangle)]
pub extern "C" fn paco_print_uint(value: u64) {
    print_line(text::uint_to_string(value).as_bytes());
}

#[unsafe(no_mangle)]
pub extern "C" fn paco_print_bool(value: i8) {
    print_line(text::bool_to_string(value != 0).as_bytes());
}

#[unsafe(no_mangle)]
pub extern "C" fn paco_print_float(value: f64, code: i32) {
    print_line(paco_runtime::format_float_code(value, code).as_bytes());
}

#[unsafe(no_mangle)]
pub extern "C" fn paco_print_char(c: u32) {
    let (buf, len) = text::encode_char(c);
    print_line(&buf[..len]);
}

/// # Safety
/// `s` must point to a valid `PacoStr`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn paco_print_str(s: *const PacoStr) {
    print_line(unsafe { (*s).bytes() });
}

/// # Safety
/// `s` must point to a valid `PacoStr`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn paco_stderr_write(s: *const PacoStr) {
    flush_stdout();
    let mut err = std::io::stderr().lock();
    let _ = err.write_all(unsafe { (*s).bytes() });
    let _ = err.write_all(b"\n");
}

/// # Safety
/// Both arguments must point to valid `PacoStr`s.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn paco_string_eq(a: *const PacoStr, b: *const PacoStr) -> i8 {
    unsafe { ((*a).bytes() == (*b).bytes()) as i8 }
}

/// # Safety
/// `a` and `b` must point to valid `PacoStr`s and `out` to writable memory.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn paco_string_concat(a: *const PacoStr, b: *const PacoStr, out: *mut PacoStr) {
    let (a, b) = unsafe { ((*a).bytes(), (*b).bytes()) };
    let copy = paco_alloc(a.len() + b.len() + 1).cast::<u8>();
    unsafe {
        copy_bytes(a, copy);
        copy_bytes(b, copy.add(a.len()));
        *out = PacoStr { ptr: copy, len: (a.len() + b.len()) as i64 };
    }
}

/// # Safety
/// `s` must point to a valid `PacoStr`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn paco_string_next_char_boundary(s: *const PacoStr, pos: i64) -> i64 {
    text::next_char_boundary(unsafe { (*s).bytes() }, pos)
}

/// # Safety
/// `s` must point to a valid `PacoStr` and `out` to writable memory.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn paco_string_char_at(s: *const PacoStr, pos: i64, out: *mut u32) -> i32 {
    let Some(c) = text::char_at(unsafe { (*s).bytes() }, pos) else { return 0 };
    unsafe { *out = c };
    1
}

/// # Safety
/// `s` must point to a valid `PacoStr` and `out` to writable memory.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn paco_string_byte_at(s: *const PacoStr, pos: i64, out: *mut i64) -> i32 {
    let Some(byte) = text::byte_at(unsafe { (*s).bytes() }, pos) else { return 0 };
    unsafe { *out = byte };
    1
}

/// # Safety
/// `s` must point to a valid `PacoStr` and `out` to writable memory.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn paco_string_slice_utf8(s: *const PacoStr, start: i64, end: i64, out: *mut PacoStr) -> i32 {
    let Some(slice) = text::slice_utf8(unsafe { (*s).bytes() }, start, end) else { return 0 };
    write_str(unsafe { &mut *out }, slice);
    1
}

/// Copies the string's bytes into a new `[]byte`.
///
/// # Safety
/// `s` must point to a valid `PacoStr` and `out` to writable memory.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn paco_string_to_bytes(s: *const PacoStr, out: *mut PacoStr) {
    let bytes = unsafe { (*s).bytes() };
    let copy = paco_calloc(bytes.len().max(1), 1).cast::<u8>();
    unsafe {
        copy_bytes(bytes, copy);
        *out = PacoStr { ptr: copy, len: bytes.len() as i64 };
    }
}

/// # Safety
/// `bytes` must point to a valid `[]byte` descriptor and `out` to writable memory.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn paco_string_from_bytes(bytes: *const PacoStr, start: i64, end: i64, out: *mut PacoStr) -> i32 {
    let Some(range) = text::from_utf8_range(unsafe { (*bytes).bytes() }, start, end) else { return 0 };
    write_str(unsafe { &mut *out }, range);
    1
}

/// Writes the string's bytes into `dst` at `at`; writes nothing and
/// returns 0 when they do not fit.
///
/// # Safety
/// `dst` must point to a valid, writable `[]byte` descriptor and `s` to a valid `PacoStr`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn paco_bytes_write_string(dst: *const PacoStr, at: i64, s: *const PacoStr) -> u8 {
    let (dst, bytes) = unsafe { (&*dst, (*s).bytes()) };
    if at < 0 || at.checked_add(bytes.len() as i64).is_none_or(|end| end > dst.len) {
        return 0;
    }
    unsafe { copy_bytes(bytes, dst.ptr.cast_mut().add(at as usize)) };
    1
}

/// Sorts the first `len` elements of a primitive slice (see
/// `paco_runtime::sort::sort_raw` for `kind`).
///
/// # Safety
/// `slice` must point to a valid, writable slice descriptor of `kind` elements.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn paco_slice_sort(slice: *const PacoStr, len: i64, kind: i32) {
    let slice = unsafe { &*slice };
    let len = len.clamp(0, slice.len.max(0)) as usize;
    unsafe { paco_runtime::sort::sort_raw(slice.ptr.cast_mut(), len, kind) };
}

/// # Safety
/// `s` must point to a valid `PacoStr`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn paco_string_hash(s: *const PacoStr) -> u64 {
    text::hash_bytes(unsafe { (*s).bytes() })
}

/// # Safety
/// `path` must point to a valid `PacoStr` and `out` to writable memory.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn paco_fs_read_to_string(path: *const PacoStr, out: *mut PacoStr) -> i32 {
    let Ok(path) = std::str::from_utf8(unsafe { (*path).bytes() }) else { return 0 };
    if path.contains('\0') {
        return 0;
    }
    match std::fs::read(path) {
        Ok(bytes) if std::str::from_utf8(&bytes).is_ok() => {
            write_str(unsafe { &mut *out }, &bytes);
            1
        }
        _ => 0,
    }
}

/// # Safety
/// `out` must point to writable memory.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn paco_int_to_string(value: i64, out: *mut PacoStr) {
    write_str(unsafe { &mut *out }, text::int_to_string(value).as_bytes());
}

/// # Safety
/// `out` must point to writable memory.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn paco_uint_to_string(value: u64, out: *mut PacoStr) {
    write_str(unsafe { &mut *out }, text::uint_to_string(value).as_bytes());
}

/// # Safety
/// `out` must point to writable memory.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn paco_bool_to_string(value: i8, out: *mut PacoStr) {
    write_str(unsafe { &mut *out }, text::bool_to_string(value != 0).as_bytes());
}

/// # Safety
/// `out` must point to writable memory.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn paco_float_to_string(value: f64, code: i32, out: *mut PacoStr) {
    write_str(unsafe { &mut *out }, paco_runtime::format_float_code(value, code).as_bytes());
}

/// # Safety
/// `out` must point to writable memory.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn paco_char_to_string(value: u32, out: *mut PacoStr) {
    let (buf, len) = text::encode_char(value);
    write_str(unsafe { &mut *out }, &buf[..len]);
}

pub use paco_runtime::{FLOAT_CODE_F32, FLOAT_CODE_F64};

#[unsafe(no_mangle)]
pub extern "C" fn paco_float_to_f64(bits: u32, code: i32) -> f64 {
    paco_runtime::float_to_f64(bits, code)
}

#[unsafe(no_mangle)]
pub extern "C" fn paco_float_from_f64(value: f64, code: i32) -> u32 {
    paco_runtime::float_from_f64(value, code)
}

unsafe extern "C" {
    static __paco_entry_returns_value: i8;
    fn __paco_entry() -> i64;
}

/// A real Paco program's own compiled object always defines
/// `__paco_entry`/`__paco_entry_returns_value` (paco-codegen-cranelift/llvm
/// emit them), so `paco-link` never needs these. `cargo test`/`cargo build`
/// of this crate on its own does need them, since it pulls in `paco_rt_main`
/// without a program behind it; `runtime/.cargo/config.toml` supplies
/// `/alternatename:` linker flags on the msvc target pointing at these.
#[cfg(windows)]
#[unsafe(no_mangle)]
extern "C" fn paco_rt_no_program() -> i64 {
    0
}

#[cfg(windows)]
#[unsafe(no_mangle)]
static paco_rt_no_program_value: i8 = 0;

static ARGS: std::sync::OnceLock<Vec<Vec<u8>>> = std::sync::OnceLock::new();

#[unsafe(no_mangle)]
pub extern "C" fn paco_arg_count() -> i64 {
    ARGS.get().map_or(0, Vec::len) as i64
}

/// # Safety
/// `out` must point to writable memory.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn paco_arg_at(index: i64, out: *mut PacoStr) {
    let arg = usize::try_from(index).ok().and_then(|index| ARGS.get()?.get(index));
    write_str(unsafe { &mut *out }, arg.map_or(&[][..], Vec::as_slice));
}

/// The process entry the linker routes `main` to: starts the runtime, runs
/// the compiled program and turns its result into the exit code.
///
/// # Safety
/// `argv` must hold `argc` NUL-terminated strings, as the C runtime passes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn paco_rt_main(argc: i32, argv: *const *const u8, _envp: *const *const u8) -> i32 {
    let args = (0..argc.max(0) as usize)
        .map(|index| unsafe { std::ffi::CStr::from_ptr((*argv.add(index)).cast()) }.to_bytes().to_vec())
        .collect();
    let _ = ARGS.set(args);
    crate::paco_rt_init();
    let result = unsafe { __paco_entry() };
    flush_stdout();
    if unsafe { std::ptr::read_volatile(&raw const __paco_entry_returns_value) } != 0 { result as i32 } else { 0 }
}

#[cfg(all(target_env = "gnu", target_arch = "x86_64"))]
#[unsafe(naked)]
#[unsafe(no_mangle)]
pub extern "C" fn paco_rt_start() -> ! {
    core::arch::naked_asm!(
        "xor ebp, ebp",
        "mov r9, rdx",
        "pop rsi",
        "mov rdx, rsp",
        "and rsp, -16",
        "push rax",
        "push rsp",
        "xor r8d, r8d",
        "xor ecx, ecx",
        "lea rdi, [rip + {main}]",
        "call qword ptr [rip + __libc_start_main@GOTPCREL]",
        "hlt",
        main = sym paco_rt_main,
    )
}

#[cfg(all(target_env = "gnu", target_arch = "aarch64"))]
#[unsafe(naked)]
#[unsafe(no_mangle)]
pub extern "C" fn paco_rt_start() -> ! {
    core::arch::naked_asm!(
        "mov x29, #0",
        "mov x30, #0",
        "mov x5, x0",
        "ldr x1, [sp]",
        "add x2, sp, #8",
        "mov x6, sp",
        "mov x3, #0",
        "mov x4, #0",
        "adrp x0, {main}",
        "add x0, x0, :lo12:{main}",
        "bl __libc_start_main",
        "brk #0",
        main = sym paco_rt_main,
    )
}
