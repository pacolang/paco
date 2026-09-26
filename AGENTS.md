# Paco — agent context
 
**Read this file every time you touch Paco code in this repo.**
 
Paco is at the design/bootstrap stage. The commands and tools described here are
the target behavior; not everything exists yet. When the real code diverges from
this file, the code wins — and this file should be updated.
 
## 1. Required reading
 
Before generating, editing, or reviewing any `.paco` file:
 
1. Read `docs/design/spec.md` (what the language is). Settled decisions are
   recorded as numbered RFCs in `https://github.com/pacolang/rfcs` — do not
   reopen one without justification.
2. Read `context.md` — it indexes every settled decision (with a link to its
   RFC) *and* the open questions. Something listed as open there is not yours
   to settle silently.
3. Scan the module you'll modify for existing conventions, then match them.
   The compiler lives in `compiler/` (Rust), the runtime in `runtime/` (Rust),
   and the standard library in `stdlib/` (Paco). `tests/conformance/` holds
   the compiler's own test fixtures; runnable example programs live in the
   separate [`pacolang/examples`](https://github.com/pacolang/examples)
   repository.
This is not optional. Paco has its own idioms — guessing leads to code that
doesn't compile or that contradicts the design.
 
## 2. Language quick-reference
 
- **What Paco is for**: general-purpose programming — services, CLIs, games,
  critical flows, compilers and AI systems on one foundation, in one binary.
  **No Python interop**: the boundary is the C ABI. When two designs conflict,
  the one that serves building AI systems end to end wins
  ([RFC 0013](https://github.com/pacolang/rfcs/blob/main/text/0013-ai-systems-north-star.md),
  [RFC 0028](https://github.com/pacolang/rfcs/blob/main/text/0028-general-purpose-ai-tie-breaker.md)).
- **Modules**: a directory is a module. Every file starts with `module <name>`
  matching the directory, then `use` imports, then items — in that order, always.
  Imports bind whole modules (`use stdlib::io` → `io::read_file(...)`); there is no
  selective or wildcard import, and call sites stay qualified.
- **Visibility**: private by default, `pub` exports. `pub` on a `struct`/`enum`
  exports the type only — fields, methods and associated consts each need their
  own `pub`. Enum variants are exported with the enum. Paco does **not** use
  Go's capitalisation rule.
- **Constants**: `const NAME: Type = expr` — the annotation is mandatory and the
  initialiser must be compile-time evaluable. There is **no mutable global
  state**: no `static`, no `static mut`, no module-level `var`. Shared mutable
  state is `Arc<Mutex<T>>`.
- **Numeric types**: `i8..i64`, `u8..u64`, `int`/`uint`, `f16`, `bf16`, `f32`,
  `f64`, `f8e4m3`, `f8e5m2`. `f16`/`bf16` do arithmetic; **FP8 does not** — it is
  storage only, so widen with `as` to compute. **No implicit conversion between
  float types in either direction** — even widening needs an explicit `as`.
- **FFI**: `extern "C" { fn ...; }` declares foreign functions, all implicitly
  `unsafe fn`. Call them inside `unsafe { }`, which is an expression block. Raw
  pointers `*const T` / `*mut T` are FFI-only — never an escape hatch for
  ordinary code. `#[repr(C)]` for layout. Export with `pub extern "C" fn`.
- **Prelude**: `Option`/`Result` and their variants, the operator traits
  (`Add`, `Display`, `Iter`, `Index`, `From`...), `Vec`/`Map`/`Set`/`StringBuf`,
  `Rc`/`Arc`/`Cell`/`RefCell`/`Mutex`/`RwLock`, `channel`/`Sender`/`Receiver`/
  `spawn_blocking`/`TaskPanic`, and `print`/`panic` are in scope without import.
  Everything else is imported — including `Duration` (`stdlib::time`), `stdlib::io`
  and `stdlib::math`. Full table in spec §13
  ([RFC 0022](https://github.com/pacolang/rfcs/blob/main/text/0022-prelude.md)).
- **Integer overflow**: checked in debug, **wrapping** in release
  ([RFC 0021](https://github.com/pacolang/rfcs/blob/main/text/0021-integer-overflow.md)).
  Write `wrapping_add`, `saturating_add`, `checked_add` or `overflowing_add`
  when the behaviour matters — the default is the hardware's, not a promise.
- **Errors as values**: `Result<T, E>` and `Option<T>`. No exceptions, no `null`.
  Propagate with the postfix `?`. `panic` is only for unrecoverable bugs, never
  control flow.
- **Memory**: ownership + move by default. Borrows `&` (shared/immutable) and
  `&mut` (mutable); aliasing rule: N `&` XOR one `&mut`. Lifetimes are inferred —
  only annotate `'a` when the compiler asks. Escape hatch: `Rc<T>` / `Arc<T>`
  using `Cell`, `RefCell`, `Mutex`, or `RwLock` for interior mutability.
  Deterministic cleanup (RAII) at scope exit.
- **Blocking calls**: a foreign call occupies its OS thread. Wrap it in
  `spawn_blocking(|| ...)`, which runs it off the worker pool and returns the
  same handle `spawn` does
  ([RFC 0018](https://github.com/pacolang/rfcs/blob/main/text/0018-blocking-foreign-calls.md)).
- **Concurrency**: lightweight M:N tasks via `spawn f(args)`. No `async`/`await` —
  the runtime suspends on I/O automatically. `spawn` returns a handle;
  `h.join() -> Result` recovers the value or the task's isolated panic (a panic
  in a task does NOT bring down the process). Channels: `channel<T>(capacity: n)`,
  `tx.send(x)?, rx.recv(), tx.close(), and `select { ... }` (with optional `default =>`).
- **Synchronous generators**: `iter fn name() -> T { ... yield x ... }`. Pulled by
  the consumer (`for x in name()`), with no task cost. `yield` only appears here.
- **Methods**: defined **inside** the `struct`/`enum` block. Receivers:
  `&self` (reads — the common case), `&mut self` (mutates), `self` (consumes —
  rare). To extend a type defined in another module, use a separate
  `methods T { ... }` block. **No inheritance** — compose with traits and structs.
- **Traits + dyn**: **implicit** satisfaction (a type satisfies a trait if it has
  the methods; no `implements` clause), checked **statically**. Use `dyn Trait`
  for dynamic dispatch (vtable, visible cost).
- **Generics**: `fn f<T>(v: T)`, bounds via `T: Trait`. Monomorphized (zero cost).
- **Metaprogramming**: `comptime` (compile-time execution, type introspection,
  code generation) + special traits (`Add`, `Index`, `Display`, `Iter`...).
  Derives via attribute: `#[derive(Display, Clone, Eq)]`.
- **Tests**: `#[test]`, `#[bench]`, `#[should_panic]` above the function.
- **Strings**: always valid UTF-8. `s.len()` counts **bytes**; `s.chars()`
  iterates characters; `s.bytes()` iterates bytes. Byte slicing validates
  character boundaries. `==` on strings compares by **value** (contents), not by
  pointer.
- **Doc comments**: `///` above the declaration; examples in fenced ```paco blocks.
## 3. Project commands
 
```
paco new <name>             create a project
paco run [file] [-- args]   build (cached) + run; same binary as `paco build`
paco build                  build (dev backend) -> single binary
paco build --release        build (optimizing backend)
paco build --target=<triple>  cross-compilation
paco check <file>           parse + types + borrow check, no codegen
paco test [path]            run #[test] functions
paco bench [path]           run #[bench] functions
paco fmt <file> [--write]   canonical formatter (non-negotiable)
paco doc                    generate documentation
paco clean [file]           wipe build artifacts
paco clean --cache          empty the build cache
```

`paco run` keeps binaries in a build cache keyed by every input's content:
`PACO_CACHE`, else `$XDG_CACHE_HOME/paco`, else `~/.cache/paco`. A panic
prints `panic at <file>:<line>:<column>: <message>` on stderr and exits with
status 101; debug builds (including `paco run`) follow it with one
`   at <function> (<file>:<line>:<column>)` line per Paco call.
 
### Modules and dependencies (decentralized)
 
There is no central package registry. A dependency is identified by the URL of
its source repository and pinned to a version-control tag (semantic versioning).
The manifest is `paco.mod`; a lock file pins exact resolved versions.
 
```
paco mod init <module-path>   create paco.mod
paco get <url>[@version]      add and fetch a dependency by its source URL
paco mod tidy                 sync paco.mod with the imports used in code
```
 
In code, external modules are imported by their module path (URL-like), e.g.
`use example.com/team/json`. Standard-library modules use the short `stdlib::` path.

**`stdlib` versus a library import.** `stdlib` (short path, no host) is the
small, domain-neutral core: the prelude, `string`, `io`, `sync`, `collections`,
`autodiff`, `dims`. It admits a module only for one of four checkable reasons
(`#[builtin]`, a prelude entry, a `paco_rt_*`/`extern` symbol, or every official
library's public signature needing it) — see `docs/ecosystem.md`, governed by
[RFC 0030](https://github.com/pacolang/rfcs/blob/main/text/0030-repository-organization-and-stdlib-scope.md).
Domain libraries — `Tensor`, `Matrix`, `DataFrame`, BLAS — are imported by
their full path (`use github.com/pacolang/tensor`), fetched with `paco get`,
and versioned independently of the compiler. `stdlib/numerics.paco`,
`stdlib/math.paco` and `stdlib/blas.paco` still ship inside this repository
today but are marked for extraction to `pacolang/tensor`, `pacolang/math` and
`pacolang/blas` — write new domain code as if it already lived in a library:
never add a compiler feature, a diagnostic, or a `stdlib` module because it
happens to be useful for one type.
 
## 4. House rules
 
- Don't use `try`/`catch` or exceptions; use `Result`/`Option` + `?`.
- Don't use `null`; absence is `Option::None`.
- Don't write `self` (move) on a method that only reads — use `&self`. The
  compiler warns.
- Don't share data across tasks without `Arc` + explicit synchronization; what
  goes through a channel is **moved**.
- Don't build inheritance hierarchies; they don't exist. Share behavior via traits.
- Prefer defining methods inside the `struct`. Use a separate `methods T {}`
  block **only** to extend a type from another module.
- `match` is exhaustive — covering every case is not style, it's a compiler
  requirement.
- When slicing strings, be conscious of byte vs character; don't assume 1 byte = 1 char.
- Don't write `String` — the primitive is `string` (owned, immutable, UTF-8) and
  the builder is `StringBuf`. There is no type called `String`.
- Don't rely on implicit float conversion; there is none. Write `x as f32`.
- Don't do arithmetic on `f8e4m3`/`f8e5m2` — they have no operators. Widen first.
- Don't reach for a raw pointer or `unsafe` outside an FFI boundary. If ownership
  is awkward, the answer is `Rc`/`Arc` (spec §3).
- Don't add a global to hold state; there are none. Pass it or share it via `Arc`.
- Don't assume a name is in the prelude. If it isn't in the spec §13 table,
  import its module and qualify the call.
- Don't rely on release overflow behaviour without writing which one you mean.
<!-- consistency-ignore: selective-import -->
- Don't import selectively (`use stdlib::io::{Read}`) — imports bind whole
  modules and call sites stay qualified.
## 5. Lints (compiler-enforced patterns)
 
`paco check` and the future LSP run these checks. Each fires a lint code that
`#[allow("<code>")]` on the enclosing function/block silences. Don't suppress
without a justification in a comment — the lint usually points at a real bug.
 
- `use-after-move` — using a value after it has been moved.
- `unhandled-result` — calling a `Result`-returning function and silently
  discarding the value swallows the error. Use `f(...)?`, `let _ = f(...)`, or
  bind the result.
- `ignored-option` — reading the contents of an `Option` without a preceding
  `match`/`if let` reads the `None` case improperly.
- `needless-move-self` — a `self` (move) receiver where `&self` would suffice.
- `non-exhaustive-match` — a `match` that doesn't cover every case (a hard error
  in practice).
- `shared-without-sync` — sending non-`Arc` shared data across tasks.
- `unclosed-channel` — a channel without `close()` on some path leaves receivers
  parked forever on `recv()`.
- `string-byte-boundary` — slicing a string at an offset that falls in the middle
  of a character.
- `implicit-float-cast` — an assignment or argument that would need a float
  conversion. There is none; write the `as`.
- `fp8-arithmetic` — an arithmetic or ordering operator applied to `f8e4m3` or
  `f8e5m2`. Widen to `f16`/`bf16`/`f32` first.
- `unsafe-outside-ffi` — an `unsafe` block that neither calls a foreign function
  nor dereferences a raw pointer, i.e. one that isn't buying anything.
- `module-name-mismatch` — the `module` declaration doesn't match the directory
  name. `paco fmt` fixes this rather than only reporting it.
- `unqualified-import` — a selective or wildcard import.
- `missing-pub` — an exported type whose public method returns or accepts a
  private type, making the method uncallable from outside the module.
- `blocking-call-on-worker` — an `extern` function called directly from a task
  instead of inside `spawn_blocking`. A warning, not an error: the compiler
  cannot know which foreign functions block
  ([RFC 0018](https://github.com/pacolang/rfcs/blob/main/text/0018-blocking-foreign-calls.md)).

### Diagnostic contract

Every diagnostic carries a **stable code**, a plain-language explanation, the
cause, a suggested fix, and a reference to the governing spec section or RFC.
Codes are permanent: once issued, a code's meaning never changes, so it can be
cited in an issue, suppressed with `#[allow]`, or matched by a tool years later.

Every command that reports diagnostics supports `--json`, because the primary
consumer of a Paco diagnostic is expected to be the program that wrote the code.
Default forms to follow.

**Every file** — module, then imports, then items, in that order:

```paco
module nn;

use stdlib::math;
use example.com/team/tensor as tensor;

pub const EPS: f32 = 1e-6;
```

**Inside a function** — fallible call, and an `Option` with a check:

```paco
let cfg = read_config()?;

match lookup(key) {
    Some(v) => use_it(v),
    None    => return Err(Error::NotFound),
}
```

**A task, with its panic recovered** — a panic in a task does not kill the
process:

```paco
let h = spawn work();
match h.join() {
    Ok(value)  => use_it(value),
    Err(panic) => log("task died: " + panic.message()),
}
```

**A channel** — `?` needs a `Result` context, so the producer is a real function
rather than a bare `spawn { ... }` block:

```paco
fn produce(tx: Sender<int>) -> Result<(), Error> {
    for i in 0..10 { tx.send(i)? }
    tx.close();
    Ok(())
}
```

```paco
let (tx, rx) = channel<int>(capacity: 8);
let producer = spawn produce(tx);
for v in rx { print(v) }
```

**A method inside its struct** — fields stay private unless exported:

```paco
pub struct Point {
    x: f64,
    y: f64,

    pub fn distance(&self, other: &Point) -> f64 {
        let dx = self.x - other.x;
        let dy = self.y - other.y;
        (dx * dx + dy * dy).sqrt()
    }
}
```

**FFI** — a thin safe wrapper over a narrow unsafe core, run off the worker pool:

```paco
extern "C" {
    fn cblas_sdot(n: i32, x: *const f32, incx: i32, y: *const f32, incy: i32) -> f32;
}

pub fn dot(x: &[]f32, y: &[]f32) -> f32 {
    unsafe { cblas_sdot(x.len() as i32, x.as_ptr(), 1, y.as_ptr(), 1) }
}
```

**Mixed precision** — every conversion is written:

```paco
let w: bf16 = load_weight();
let acc: f32 = (w as f32) * scale;
```
