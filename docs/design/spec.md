# Paco — Language Specification (v0.1 — design draft)

> Status: living draft. Everything here is negotiable. This document records
> mutually consistent design decisions, not a final standard.

## 0. Philosophy

Paco draws from three places:

- Simplicity and low mental cost: a light, readable syntax.
- Concurrency by messaging, single binary, implicit interfaces, integrated tooling.
- Ownership, memory safety, strong enums and pattern matching, errors as values.

The golden rule when two decisions conflict: **the one that serves "concurrent
services" wins, without hurting the other targets** (desktop, games, backend,
data analysis).

Three principles guide the design:

1. **Opinionated, but with freedom.** There is a recommended way to do each thing
   (a single formatter, one idiomatic style), but escape hatches exist and are explicit.
2. **Visible cost.** No hidden allocation, copying, or dynamic behavior. If it
   costs, it shows up in the code.
3. **Low mental cost by default.** Complexity (lifetimes, reference counting) only
   appears when you actually need it.

---

## 1. Settled foundational decisions

| Topic | Decision |
|-------|----------|
| Memory | Ownership + move semantics, **aggressive lifetime inference** (rarely annotated). `Rc`/`Arc` as an ergonomic escape hatch. |
| Focus | General purpose, with concurrency/services as the guiding star. Good for computation. |
| Metaprogramming | **Traits + comptime** (no dynamic runtime metatables). |
| Syntax | Clean and light (few symbols, no mandatory `;`), but error handling via `Result`/`?`. |
| Explicit lifetimes | `'a`, only when inference fails — no verbosity. |
| Concurrency | Unified lightweight tasks: `spawn` + channels, automatic suspension (no `async`/`await`). A synchronous `iter` generator as a secondary tool. |
| Backend | Cranelift (dev, fast) + LLVM (release, optimized binary). |
| Strings | UTF-8 guaranteed. |
| Methods | Defined inside the `struct`/`enum`; `methods T {}` to extend a type from elsewhere. No inheritance. |
| Packages | Decentralized, URL + version-control tag (no central registry); manifest `paco.mod`. |
| Name | Paco. |

---

## 2. Basic syntax and mental model (features 1 and 4)

```paco
// Line comment
/* Block comment */

// Immutable by default. `mut` makes it mutable.
let x = 10
let mut y = 20

// Inferred types, but annotatable.
let name: string = "Ana"

// Functions
fn add(a: int, b: int) -> int {
    a + b   // last expression is the return, no mandatory `return`
}

// Function without a return value
fn log(msg: string) {
    print(msg)
}
```

Low-mental-cost decisions:

- **Immutable by default**, explicit `mut`.
- **No mandatory semicolons.**
- **Last expression is the return**, but `return` exists for early exit.
- **One canonical formatter** (`paco fmt`) — zero arguing about style.

---

## 3. Ownership, borrowing, and lifetimes (features 5 and 6)

```paco
fn main() {
    let s = String::new("hi")
    consume(s)        // `s` is moved; using it afterward is a compile error
}

fn consume(s: String) { /* now the owner */ }
```

### Borrowing

Two forms of borrow, **with no lifetime annotation in most cases**:

```paco
fn length(s: &String) -> int { s.len() }       // shared borrow
fn append(s: &mut String, c: char) { s.push(c) } // mutable borrow
```

Aliasing rule: **either many shared `&` borrows, or a single `&mut`.** This is
what guarantees safety without a GC.

### The key difference: lifetime inference

The compiler infers lifetimes in **all common cases**. You only write an explicit
lifetime when there is genuine ambiguity between multiple input references — and
the compiler tells you exactly when that happens, with a suggested fix.

```paco
// No annotation — the compiler infers the return lives as long as `s`.
fn first_word(s: &string) -> &string { /* ... */ }
```

> When an annotation IS required (rare), the syntax is in §16.

### Escape hatch: reference counting

When ownership gets in the way (graphs, shared structures), use `Rc<T>`
(single-thread) or `Arc<T>` (multi-thread). Explicit, so the cost is visible.

```paco
let node = Rc::new(Node { value: 1 })
let another_ref = node.clone()   // bumps the counter, doesn't copy the data
```

---

## 4. Explicit errors and absence (feature 7)

No exceptions, no `null`. Two core types:

```paco
// Absence
enum Option<T> {
    Some(T),
    None,
}

// Recoverable error
enum Result<T, E> {
    Ok(T),
    Err(E),
}
```

### Ergonomic propagation

The `?` operator propagates an error/absence:

```paco
fn read_config() -> Result<Config, Error> {
    let text = read_file("config.toml")?   // if Err, returns early
    let cfg  = parse(text)?
    Ok(cfg)
}
```

For absence:

```paco
fn first_admin(us: &[User]) -> Option<&User> {
    let u = us.iter().find(|u| u.admin)?
    Some(u)
}
```

Decision: **no implicit panics.** `panic` exists, but only for unrecoverable bugs
(invariant violations), never for normal control flow.

---

## 5. Pattern matching and strong enums (features 8 and 9)

Enums carry data (sum types), and `match` is exhaustive.

```paco
enum Shape {
    Circle(radius: f64),
    Rectangle(width: f64, height: f64),
    Point,
}

fn area(s: Shape) -> f64 {
    match s {
        Shape::Circle(r)             => 3.14159 * r * r,
        Shape::Rectangle(width, height) => width * height,
        Shape::Point                 => 0.0,
    }
}
```

`match` features:

- **Mandatory exhaustiveness** — forgetting a case is a compile error.
- **Guards**: `Shape::Circle(r) if r > 0.0 => ...`
- **`@` bindings**: `n @ 1..=9 => ...`
- **Destructuring** of structs, tuples, and slices.
- `if let` / `while let` as sugar for single cases.

```paco
if let Some(u) = first_admin(&users) {
    print(u.name)
}
```

When matching on something borrowed, the `&` goes **before** the expression
(`match &value { ... }`, `for x in &list`), reading more naturally than a suffix.

---

## 6. Traits and implicit interfaces (features 3 and part of 2)

**A type satisfies an interface without declaring that it implements it** — it
just needs the methods. Methods are defined **inside** the type's block; a
separate `methods T { ... }` block extends a type defined elsewhere.

```paco
trait Sink {
    fn write(self&mut, data: []byte) -> Result<int, Error>
}

// No "implements Sink" clause. If File has the method, it satisfies Sink.
struct File {
    path: string,

    fn write(self&mut, data: []byte) -> Result<int, Error> {
        // ...
        Ok(data.len())
    }
}

// Accepts anything that knows how to write.
fn save(w: &dyn Sink, data: []byte) {
    w.write(data)?
}
```

Syntax notes:

- Receivers: `self&` (shared borrow, reads — **the common case**), `self&mut`
  (mutable borrow), `self` (consumes by move — rare; only for methods that turn
  the object into something else, e.g. `into_bytes`).
- **No hidden default.** `self` alone always means move, never a silent borrow —
  consistent with "visible cost" and "explicit ownership". The compiler warns if
  you write `self` (move) on a method that clearly only reads, suggesting `self&`.
- Slices are `[]byte`, `[]int`.
- Defining methods **inside** the struct is the canonical form; a separate
  `methods T { ... }` block is **only** for extending a type from another module.

Important decision: interfaces are **implicitly satisfied** but **statically
checked**. You get decoupling without runtime duck-typing cost. For dynamic
polymorphism use `dyn Trait` (visible cost: vtable); for static use generics
(zero cost, monomorphization).

> This is NOT classic OOP: there is no inheritance, mutability is governed by
> ownership, and dispatch is static by default. Behavior is a *capability* a type
> satisfies (via traits), not something inherited from a hierarchy. See ADR 0002.

---

## 7. Metaprogramming: special traits + comptime (feature 2)

Instead of metatables, two mechanisms:

### Operator overloading via traits

```paco
struct Vec2 {
    x: f64,
    y: f64,

    fn add(self, other: Vec2) -> Vec2 {   // satisfies the `Add` trait
        Vec2 { x: self.x + other.x, y: self.y + other.y }
    }
}

let v = v1 + v2   // statically resolved to the call above
```

"Magic" traits covering what metatables did: `Add`, `Index`, `Call`, `Display`,
`Iter`, etc.

### `comptime` — compile-time execution

```paco
// Generates code / inspects types with no runtime cost.
comptime fn derive_serialization(T: type) -> Code {
    // walk T's fields and generate the serializer
}

// Typical use: derives
@derive(Serialize, Eq, Clone)
struct User {
    name: string,
    age:  int,
}
```

`comptime` is what gives "data-analysis" power: code generation for parsing, ORM,
serialization — all resolved at compile time. No runtime cost, no unpredictability.

---

## 8. Concurrency: tasks, generators, and channels (features 3, 11, 12)

> Note: "feature 3" appears twice in the original list (implicit interfaces and a
> goroutine equivalent). Interfaces are covered in §6 and concurrency here.

**Model decision: unified lightweight-task concurrency.** There is no
`async`/`await` and no visible `yield` for concurrency. You write normal
sequential code; the runtime suspends a task **automatically and invisibly** when
it blocks (I/O, channel) and runs another. This is what delivers "low mental
cost" (feature 4) — no "function color" problem.

### Lightweight tasks (the goroutine equivalent)

```paco
spawn compute(data)        // fires a lightweight task, scheduled M:N
```

Any function can be a task. No need to mark it `async`. When `compute` makes a
blocking call, the scheduler simply runs another task in the meantime. Tasks are
cheap (stacks that grow on demand), so spawning thousands is normal.

### Panic in tasks: isolate, don't crash

**Decision: a panic inside a task brings down only that task, not the whole
process.** A request with a bug must not take the entire server with it.

The panic is captured at the task boundary and turned into a `Result` that the
spawner can inspect via the task's *handle*:

```paco
let h = spawn risky()            // `spawn` returns a handle

match h.join() {                 // wait for the task and recover the result
    Ok(value)  => use_it(value),
    Err(panic) => log("task died: " + panic.message()),
}
```

Notes:

- If you ignore the handle (`spawn f()` without keeping it), a task panic is
  **logged and the task dies silently** — the process stays alive.
- `main` is the exception: a panic in `main` ends the process (no one can recover).
- Because the result comes back typed in a `Result`, you are *encouraged* to handle
  it — it isn't a loose, easy-to-forget recover.

### Channels (communication between tasks)

CSP: "don't communicate by sharing memory; share memory by communicating."

```paco
let (tx, rx) = channel<int>(capacity: 8)

spawn {
    for i in 0..10 { tx.send(i)? }
    tx.close()
}

for value in rx {        // iterates until the channel closes
    print(value)
}

// `select` over multiple channels
select {
    v = rx1.recv() => handle(v),
    v = rx2.recv() => handle(v),
    timeout(1s)    => print("took too long"),
}
```

Safety decision: the ownership system guarantees that data sent over a channel is
**moved** (not accidentally shared), eliminating data races at compile time.
Types shared across threads must be `Arc` + explicit synchronization.

### Lightweight synchronous generators (`iter`) — secondary tool

For the hot paths in **games and data analysis** where you want to produce a
sequence on demand *without* the weight of a task + channel, there is `iter`: a
purely synchronous generator, "pulled" by the consumer. No allocation, no
scheduler, zero cost. It's the only place `yield` appears.

```paco
iter fn fibonacci() -> int {
    let mut a = 0
    let mut b = 1
    loop {
        yield a              // pause; hand `a` back to whoever is iterating
        let next = a + b
        a = b
        b = next
    }
}

// The consumer controls the pace:
for n in fibonacci().take(10) {
    print(n)
}
```

The mental distinction is clear: **`iter` = a synchronous sequence you pull**
(fast, local, no concurrency). **`spawn` + channel = concurrent work** the runtime
schedules. Both use the same suspension mechanism underneath, but you never have
to think about that — you choose by intent.

---

## 9. Ergonomics + performance (feature 10)

How we deliver ergonomics without losing performance:

- **Monomorphized generics** (zero cost).
- **No GC** on the default path — RAII/ownership frees memory deterministically.
- **Inlining and optimizations** via a dual backend: **Cranelift** for dev builds
  (fast compilation, agile cycle — important for games and data) and **LLVM** for
  release builds (heavily optimized binary). `paco build` uses Cranelift;
  `paco build --release` uses LLVM.
- **Zero-cost abstractions**: iterators, `Option`, closures without allocation
  when possible.
- **Explicit data layout** when needed (`@repr`), important for games and data.

### Good for computation (data-analysis support)

Specific decisions to make the language strong with numbers:

- **Explicit numeric types with no surprises**: `i8..i64`, `u8..u64`, `f32`,
  `f64`, plus `int`/`uint` (word-sized). No silent implicit coercion (visible cost).
- **Operators on arrays/slices via traits** (`Add`, `Mul`...), allowing clean math
  notation on vectors and matrices with no runtime cost.
- **Overflow checked in debug, defined in release**: bugs show up early,
  performance at the end.
- **`comptime`** (§7) generates specialized computation kernels at compile time —
  the basis for performant DataFrames/linear algebra in a library.

---

## 10. Strings — UTF-8 guaranteed (supports features 4 and 10)

Strings are **always valid UTF-8**, not raw bytes. This eliminates a whole class
of encoding bugs, at the cost of slicing needing to be char/byte aware.

```paco
let s = "café"           // always valid UTF-8
s.len()                  // 5 (bytes) — explicitly counts bytes
s.chars().count()        // 4 (Unicode characters)

for c in s.chars() { ... }       // iterate by character
for b in s.bytes() { ... }       // iterate by byte
```

Decisions:

- `string` is immutable and UTF-8; `StringBuf` (or `[]byte`) for mutable building.
- Byte indexing (`s[0..3]`) **validates character boundaries** and errors if it
  cuts in the middle of a code point — no silent malformed string.
- `==` on strings compares by **value** (contents), not by pointer.
- Separate types keep the cost visible: you always know whether you're dealing
  with bytes, code points, or graphemes (the latter via a library).

---

## 11. Single binary (feature 14)

`paco build` produces **one static executable**, with no external runtime
dependencies, easy to distribute. The concurrency runtime (M:N scheduler) is
embedded in the binary. First-class cross-compilation (`paco build --target`).

---

## 12. Tests with decorators (feature 15)

Tests live alongside the code, marked by an attribute.

```paco
@test
fn tests_add() {
    assert_eq(add(2, 3), 5)
}

@test
@should_panic
fn tests_divide_by_zero() {
    divide(1, 0)
}

// Benchmarks
@bench
fn bench_parse(b: &mut Bencher) {
    b.iter(|| parse(input))
}
```

`paco test` discovers and runs everything. No external framework in the basic case.

---

## 13. Module system and tooling (supports feature 1 "opinionated")

- `paco fmt` — canonical formatter (non-negotiable).
- `paco test` — built-in test/benchmark runner.
- `paco build` — single binary.
- `paco doc` — documentation from comments.
- **Decentralized dependencies**: a dependency is the URL of its source repository
  pinned to a version-control tag (semantic versioning). There is no central
  registry. The manifest is `paco.mod` with a lock file; external modules are
  imported by their URL-like module path (`use example.com/team/json`). See
  ADR 0005. Implementation is deferred to a later milestone.

---

## 14. Coverage table of the 15 requested features

| # | Requested feature | Where | How |
|---|-------------------|-------|-----|
| 1 | Opinionated but with freedom | §2, §13 | Single formatter + explicit escape hatches |
| 2 | Metatables | §7 | Special traits + comptime (not runtime) |
| 3 | Coroutines | §8 | `iter fn ... yield` (lightweight synchronous generator) |
| 4 | Low mental cost | §2, §8 | Immutable by default, light syntax, no async/await |
| 5 | Explicit ownership | §3 | Ownership + move semantics |
| 6 | Easier borrowing | §3 | Aggressive lifetime inference + `Rc`/`Arc` |
| 7 | Explicit errors and absence | §4 | `Result`, `Option`, `?`, no `null`/exceptions |
| 8 | Pattern matching | §5 | Exhaustive `match`, guards, bindings |
| 9 | Strong enums | §5 | Enums with data (sum types) |
| 10 | Ergonomics + performance | §9 | Monomorphization, no GC, dual backend, zero cost |
| 11 | Goroutine equivalent | §8 | `spawn` + M:N scheduler, automatic suspension |
| 12 | Channels | §8 | `channel`, `select`, ownership prevents races |
| 13 | Implicit interfaces | §6 | Implicitly satisfied traits, statically checked |
| 14 | Single binary | §11 | Static `paco build` |
| 15 | Test decorators | §12 | `@test`, `@bench`, `@should_panic` |

---

## 15. "Everything together" example

```paco
@derive(Display)
enum Task {
    Pending(description: string),
    Done,
}

trait Process {
    fn process(self&) -> Result<string, Error>
}

enum Task {
    Pending(description: string),
    Done,

    fn process(self&) -> Result<string, Error> {
        match self {
            Task::Pending(d) => Ok("doing: " + d),
            Task::Done       => Err(Error::AlreadyDone),
        }
    }
}

fn main() {
    let (tx, rx) = channel<Task>(capacity: 4)

    spawn {
        tx.send(Task::Pending("write spec"))?
        tx.close()
    }

    for task in rx {
        match task.process() {
            Ok(msg) => print(msg),
            Err(e)  => print("error: " + e.to_string()),
        }
    }
}
```

---

## 16. Explicit lifetimes (when inference fails)

The rule: you almost never write a lifetime. But when there is genuine ambiguity
between multiple input references, the compiler stops and asks — with a clear
message. The syntax, only in those rare cases:

```paco
// Inference covers 99% — no annotation:
fn first(s: &string) -> &string { ... }

// Ambiguous: which input does the return follow? Then you annotate.
fn longest<'a>(x: &'a string, y: &'a string) -> &'a string {
    if x.len() > y.len() { x } else { y }
}
```

Deliberate differences to reduce verbosity:

- Lifetimes in structs are rarely needed (stronger inference heuristics).
- No `'static` scattered through common code — the compiler deduces it.
- Error messages **suggest the exact annotation** to paste, so you don't reason
  about lifetimes from scratch — just confirm.

---

## 17. Settled decisions

See `docs/design/decisions/` for the full ADRs. Summary:

- Syntax: clean and light, with error handling via `Result`/`?`.
- Methods: inside the `struct`/`enum`. Receivers: `self&` (common, reads),
  `self&mut` (mutates), `self` (rare, consumes). No hidden default; the compiler
  suggests `self&` when it fits (§6, ADR 0002).
- Lifetimes: `'a`, only when inference fails (§16, ADR 0001).
- Backend: Cranelift for dev (`paco build`), LLVM for release
  (`paco build --release`) (§9, ADR 0003).
- Strings: UTF-8 guaranteed, value equality (§10).
- Concurrency: unified lightweight tasks (`spawn` + channels, no async/await),
  with synchronous `iter` as a secondary tool; per-task panic isolation
  (§8, ADR 0004).
- Computation: explicit numeric types, operators on arrays, overflow checked in
  debug (§9).
- Packages: decentralized, URL + version-control tag, no central registry
  (§13, ADR 0005).

---

## 18. Open decisions (next conversations)

1. **Data analysis**: how far to take it? DataFrames/linear algebra in the
   language core or in a standard library built on `comptime`?
2. **Struct mutability**: per-field granular or only at the binding (`let mut`)?
3. **String slicing**: does the character-boundary validation error at runtime, or
   require an explicit API (`s.get(0..3) -> Option`)?
4. **Syntactic macros**: beyond `comptime`, will there be syntax macros, or does
   `comptime` cover everything?
5. **Error conversion**: an automatic error-conversion mechanism (a `From`-style
   trait used by `?`) to remove the repeated `.map_err(...)` seen in the examples.
6. **Collection construction syntax**: clarify how nested collection types are
   constructed (the `[][]Cell` / `[]f64::new()` friction from the CSV example).
