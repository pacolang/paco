# Paco — Language Specification (v0.1 — design draft)

> Status: living draft. Everything here is negotiable. This document records
> mutually consistent design decisions, not a final standard.
>
> Normative companions: `docs/grammar/grammar.ebnf` (syntax),
> `docs/grammar/tokens.md` (lexis), `https://github.com/pacolang/rfcs` (rationale),
> `context.md` (index of what is settled and what is open).
> **Open questions live in §20 and that section is never empty.**

## 0. Philosophy

**Paco is a general-purpose, compiled language** with memory safety without a
garbage collector, lightweight concurrency and a single-binary deployment.
Services and APIs, command-line tools, games and interactive applications,
critical flows, compilers and language tooling, and AI systems are all
first-class uses of the same foundation.

AI systems get special weight: Paco aims to cover them end to end (data
loading, preprocessing, kernel authoring, training, inference and serving) in
one language, in one binary, with no Python in the loop and without dropping to
C++ or CUDA C for performance.

It draws from three places:

- Simplicity and low mental cost: a light, readable syntax.
- Concurrency by messaging, single binary, implicit interfaces, integrated tooling.
- Ownership, memory safety, strong enums and pattern matching, errors as values.

The golden rule when two decisions conflict: **the one that serves building AI
systems end to end wins**, provided it breaks neither memory safety nor the
single-binary promise. The rule applies only to a conflict: a feature that
serves another domain without costing AI work is judged on its own merits, and
a missing foundation feature is a gap to close whatever domain exposes it.
Concurrency is a first-class strength, but it is not the tie-breaker. See
RFC 0013 and RFC 0028.

Two consequences of that rule are worth stating up front, because they shape
everything else:

- **Paco does not interoperate with Python.** The interop boundary is the C ABI
  (§18), not CPython. Everything above it — autograd, layers, optimisers, data
  loading — is written in Paco. The cost is a library ecosystem that must be
  built; the benefit is that no Python semantics leak into the language.
- **Speed is not the differentiator.** "Faster than Python" is a claim every
  contender makes. The wedge Paco is aiming at is **compile-time shape
  checking** — rejecting a tensor shape mismatch before the program runs, rather
  than forty minutes into a training job. That is const generics with a `Dyn`
  marker for the dimensions nobody knows ahead of time (RFC 0023).

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
| Focus | **General purpose.** Services, CLIs, games, critical flows, compilers and AI systems share one foundation. **AI systems end to end is the tie-breaker** when two designs conflict. |
| Python | **No interoperation.** The interop boundary is the C ABI; the framework layer is rewritten in Paco. |
| Numeric types | `f16` and `bf16` are full arithmetic types; `f8e4m3`/`f8e5m2` are storage-only. No implicit conversion between float types, in either direction. |
| Modules | A directory is a module; every file declares `module <name>`. Imports bind whole modules — no selective or wildcard import. |
| Visibility | Private by default; `pub` exports. `pub` on a type exports the type only — fields and methods need their own `pub`. |
| Constants | `const` with a mandatory type annotation, evaluated at compile time. **No mutable global state** — no `static mut`, no module-level `var`. |
| FFI | `extern "C"` blocks, `unsafe { }` expression blocks, raw pointers `*const T`/`*mut T`, `#[repr(C)]` layout. |
| Metaprogramming | **Traits + comptime** (no dynamic runtime metatables). |
| Syntax | Clean and light, Rust-style `;` statement terminators, error handling via `Result`/`?`. |
| Explicit lifetimes | `'a`, only when inference fails — no verbosity. |
| Concurrency | Unified lightweight tasks: `spawn` + channels, automatic suspension (no `async`/`await`). A synchronous `iter` generator as a secondary tool. |
| Backend | Cranelift (dev, fast) + LLVM (release, optimized binary). |
| Strings | UTF-8 guaranteed. |
| Methods | Defined inside the `struct`/`enum`; `methods T {}` to extend a type from elsewhere. No inheritance. |
| Packages | Decentralized, URL + version-control tag (no central registry); manifest `paco.mod`. |
| Collection construction | `Vec::new()`, `Map::new()`, etc. — associated functions only, no shorthand literal syntax. |
| Error conversion | `?` calls `From::from(e)` automatically; `From<T>` lives in the prelude; implicit satisfaction. |
| Struct mutability | Controlled by the binding (`let mut`); the whole struct is mutable or immutable — no per-field modifiers. |
| Syntax macros | `comptime` is the sole metaprogramming mechanism; no syntax macros (deferred post-Phase 6). |
| String slicing | No `s[n..m]` on strings. Use `s.get(0..n) -> Option<&string>` for safe slicing; `s.as_bytes()[0..n]` for raw bytes. |
| Data analysis | Standard library (`src/math/`) built on `comptime` + traits. No language-core types. `DataFrame<Schema>`, `Matrix<T>` are library types. |
| Slices | `[]T` is an **owned**, fixed-length buffer; `&[]T` / `&mut []T` is the view. `Vec<T>` is the growable one. |
| Indexing | `a[i]`, `a[i, j]`, `a[i, j, k]` — desugars to the `Index` trait. Not valid on strings. |
| Receivers | `&self`, `&mut self`, `self` — the borrow sigil goes before `self`, as in Rust. |
| Attributes | `#[test]`, `#[derive(...)]`, `#[repr(C)]`. `@` means pattern binding only. |
| Name | Paco. |

---

## 2. Basic syntax and mental model (features 1 and 4)

```paco
// Line comment
/* Block comment */

// Immutable by default. `mut` makes it mutable.
let x = 10;
let mut y = 20;

// Inferred types, but annotatable.
let name: string = "Ana";

// Functions
fn add(a: i64, b: i64) -> i64 {
    a + b   // last expression is the return, no mandatory `return`
}

// Function without a return value
fn log(msg: string) {
    print(msg)
}

// Arguments may be passed positionally or by name.
let (tx, rx) = channel<i64>(capacity: 8);
```

Low-mental-cost decisions:

- **Immutable by default**, explicit `mut`.
- **Statements end in `;`, exactly as in Rust.** Newlines are whitespace. The
  last expression of a block, without `;`, is its value; an expression ending
  in a block (`if`, `match`, `while`, `for`, `loop`, `{ }`) used as a statement
  needs no `;`.
- **Last expression is the return**, but `return` exists for early exit.
- **Named arguments are optional at the call site.** `f(capacity: 8)` and
  `f(8)` are both valid; a named argument must match the parameter's declared
  name. Naming is a readability choice, not part of the signature — there are no
  keyword-only parameters and no defaults.
- **One canonical formatter** (`paco fmt`) — zero arguing about style.

---

## 3. Ownership, borrowing, and lifetimes (features 5 and 6)

```paco
fn main() {
    let s = load_text();   // `s: string` — an owned value
    consume(s)            // `s` is moved; using it afterward is a compile error
}

fn consume(s: string) { /* now the owner */ }
```

> **On string types.** Paco has exactly two: `string`, the primitive — owned,
> immutable, guaranteed UTF-8 — and `StringBuf`, a standard-library type for
> building one incrementally. There is no type called `String`. See §10.

### Copy types: which values move, and which duplicate

"Ownership and move by default" applies to values that own something. It does
not apply to values that are just bits:

```paco
let x = 10;
let y = x;        // `x` is COPIED, not moved
print(x);         // fine — `x` is still valid

let s = load_text();
let t = s;        // `s` is MOVED
print(s)         // ERROR: use after move
```

A type is **`Copy`** when duplicating it is a bit-for-bit copy with nothing to
free. The following are `Copy`:

- every numeric primitive — `i8`–`i64`, `u8`–`u64`, `f16`,
  `bf16`, `f32`, `f64`, `f8e4m3`, `f8e5m2`
- `bool`, `char`, `byte`
- shared borrows `&T` — a view is just an address
- raw pointers `*const T`, `*mut T`
- tuples and arrays whose elements are all `Copy`

The following are **not** `Copy`, and move:

- `string`, `StringBuf`, `Vec<T>`, `[]T` — they own a heap buffer
- `&mut T` — duplicating a unique borrow would break the aliasing rule outright
- every user-defined `struct` and `enum`, **by default**

A user type opts in with `#[derive(Copy)]`, which the compiler accepts only when
every field is itself `Copy`. The default is move because copying is a cost, and
a type that grows a non-`Copy` field later should stop being `Copy` loudly rather
than silently. A type that fails the rule is reported as `PACO-E0354`, naming the
first non-`Copy` field.

Whether a binding copies depends only on its type, never on how its initializer is
written: `let k = if x < 0 { -x } else { x };` and `let p = &v[i];` copy exactly as
their annotated spellings do. Inside a generic item, a parameter bounded by `Copy`
copies; one without the bound moves. `T: Copy` is satisfied only by `Copy` types, so
a struct without the derive, a `string` or a `[]T` is rejected where it is required.

A value moved inside a loop is valid on the next iteration when every path through
the body gives it a new value first (`s = relay(s);`). A move that some path leaves
unreassigned is still an error.

### Slices: `[]T` owns, `&[]T` views

`[]T` is an **owned**, fixed-length heap buffer — a shrink-wrapped `Vec<T>` with
no spare capacity and no growth. It owns its elements and frees them when it
drops. The view is `&[]T` (shared) or `&mut []T` (mutable).

```paco
pub struct Linear {
    w: []f32,                            // the struct OWNS its weights
    b: []f32,
}

fn dot(x: &[]f32, y: &[]f32) -> f32 { /* reads, owns nothing */ }

let view = &buf[0..n];                    // slicing a range yields &[]T
```

Making `[]T` owned is what lets the ordinary borrow operator produce the view.
`&` means "a view of" everywhere else in Paco, and it means the same here — so
the language needs no separate slice-view type.

| Type | Owns | Grows |
|---|---|---|
| `Vec<T>` | yes | yes |
| `[]T` | yes | no |
| `&[]T` / `&mut []T` | no | no |

`&[]T` is represented as a fat pointer — data pointer plus length — so indexing
through a borrow costs one indirection, not two. See RFC 0020.

### Borrowing

Two forms of borrow, **with no lifetime annotation in most cases**:

```paco
fn length(s: &string) -> i64 { s.len() }            // shared borrow
fn append(b: &mut StringBuf, c: char) { b.push(c) } // mutable borrow
```

Aliasing rule: **either many shared `&` borrows, or a single `&mut`.** This is
what guarantees safety without a GC.

Closures are values of a function type, written `fn(A, B) -> R` (`fn(A)` when
the result is `()`). They can be stored in annotated variables and fields,
passed as arguments and returned. A closure moves the owned values it
captures; a closure that captures a borrow keeps that borrow alive while the
closure lives. A closure returned from a function follows the same rule as
any other value holding a borrow (below): it may capture a borrow derived
from a reference parameter, but never a borrow of the function's own locals:

```paco
fn make_greeter(name: string) -> fn(string) -> string {
    |greeting: string| string_concat(&greeting, &name) // owns `name`
}
```

### The key difference: lifetime inference

The compiler infers lifetimes in **all common cases**. You only write an explicit
lifetime when there is genuine ambiguity between multiple input references — and
the compiler tells you exactly when that happens, with a suggested fix.

```paco
// No annotation — the compiler infers the return lives as long as `s`.
fn first_word(s: &string) -> &string { /* ... */ }
```

> When an annotation IS required (rare), the syntax is in §16.

### Borrows held inside values

A struct field, enum variant, tuple, `Option`, collection element or closure
capture may hold a borrow, again with no annotation. Such a value is inferred
to borrow from everything it was built from, and a function whose result may
hold a borrow borrows from its reference arguments (from `self` for a
method). The compiler rejects every way such a value could outlive the owner
it borrows from:

- returning it from the function that owns the borrowed local — as a tail
  expression or through `return`, bare or wrapped in a struct, variant,
  tuple or collection;
- storing it into a longer-lived place — `v.push(&t)`, `h.r = &t`,
  `outer = &t` — when that place is used after `t`'s scope ends, or lives
  outside the loop whose body declares `t`;
- storing it through a reference parameter (`fn fill(v: &mut Vec<&i64>)`),
  which always outlives the function's own locals.

```paco
struct View { r: &i64 }

fn wrap(p: &Point) -> View { View { r: &p.x } }   // fine: borrows the caller's data

fn broken() -> View {
    let t = 5;
    View { r: &t }                                // ERROR: `t` dies here
}
```

### Escape hatch: reference counting

When ownership gets in the way (graphs, shared structures), use `Rc<T>`
(single-thread) or `Arc<T>` (multi-thread). Explicit, so the cost is visible.

```paco
let node = Rc::new(Node { value: 1 });
let another_ref = node.clone();   // bumps the counter, doesn't copy the data
```

Dropping the last `Rc`/`Arc` (or `Cell`, `RefCell`, `Mutex`, `RwLock`) handle
drops the shared value.

### Drop

A value that owns something is dropped exactly once, when its owner stops
owning it:

- a binding at the end of its scope — on normal exit, `return`, `break`,
  `continue` or `?` — in reverse declaration order;
- the old value of a place, when an assignment overwrites it;
- a temporary, at the end of the statement that created it.

A moved-out binding is not dropped, including when the move happened on only
some paths. Reading a non-`Copy` value out of a field, an element or a borrow
yields a copy, which is dropped on its own. A panic does not run drops.

A type opts into custom cleanup with the prelude trait `Drop`:

```paco
struct Span {
    name: string,

    fn drop(&mut self) {
        print(self.name);   // runs when the span leaves scope
    }
}
```

`drop` runs before the value's fields are dropped; fields drop in declaration
order, enum payloads and slice elements in order.

### Collection construction

All standard-library collection types are constructed through an *associated
function* called `new`, following the same convention as `Rc::new`:

```paco
let mut v = Vec::new();
v.push(1);
v.push(2);

let mut m = Map::new();
m.insert("host", "localhost");

// Idiomatic alternative — build from an iterator:
let squares = (1..=5).map(|n| n * n).collect<Vec<i64>>();
```

There is no shorthand literal syntax for constructing collections. This keeps the
grammar uniform: every type, whether from the standard library or user-defined,
follows the same `Type::new(...)` pattern. Heap allocation is always a visible
function call.

`new` is an *associated function* (no receiver) defined inside the type's block,
consistent with the method-placement convention in RFC 0002.

> See RFC 0006 for the full rationale.

### Struct mutability

Mutability is a property of the **binding**, not of the type or its fields.
A struct is entirely mutable or entirely immutable depending on how it is bound:

```paco
let cfg = Config { host: "localhost", port: 8080 };
cfg.port = 9090;    // ERROR: `cfg` is an immutable binding

let mut cfg2 = Config { host: "localhost", port: 8080 };
cfg2.port = 9090;   // OK
cfg2.host = "prod" // OK — the whole struct is mutable
```

There are no per-field `mut` modifiers. One rule covers everything: bind with
`let mut` to mutate. Method receivers follow the same logic — `&mut self` is the
explicit request for mutable access; the compiler requires the caller to hold a
mutable binding or borrow.

For the pattern of "one field that changes while the rest stays constant," the
idiomatic solution is explicit interior mutability (`Rc<T>`, `Arc<T>`), which
makes the cost visible rather than hiding it in a field declaration. Since `Rc`
and `Arc` enforce immutability of shared contents, developers wrap the target
data inside standard library interior mutability containers:
- `Cell<T>`: For simple, copyable types (no runtime check).
- `RefCell<T>`: For general types under single-threaded `Rc` (monitored via compile-time/runtime borrow checking).
- `Mutex<T>` / `RwLock<T>`: For multi-threaded `Arc` access, ensuring synchronization.

> See RFC 0008 for the full rationale.

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
    let text = read_file("config.toml")?;   // if Err, returns early
    let cfg  = parse(text)?;
    Ok(cfg)
}
```

For absence:

```paco
fn first_admin(us: &[]User) -> Option<&User> {
    let u = us.iter().find(|u| u.admin)?;
    Some(u)
}
```

Decision: **no implicit panics.** `panic` exists, but only for unrecoverable bugs
(invariant violations), never for normal control flow.

### Automatic error conversion (`From<T>`)

When the `?` operator needs to convert an error from one type to another, Paco
calls `From::from(e)` automatically. A `From<T>` trait lives in the prelude:

```paco
trait From<Src> {
    fn from(e: Src) -> Self;
}
```

Implement it by defining a `from` associated function inside your error enum or
struct. Implicit trait satisfaction applies — no `implements` clause is needed:

```paco
enum AppError {
    Io(IoError),
    Parse(ParseError),

    fn from(e: IoError) -> Self    { AppError::Io(e) }
    fn from(e: ParseError) -> Self { AppError::Parse(e) }
}

fn load() -> Result<Config, AppError> {
    let text = read_file("config.toml")?;   // IoError → AppError::Io automatically
    let cfg  = parse(text)?;                // ParseError → AppError::Parse automatically
    Ok(cfg)
}
```

No `.map_err(...)` needed. If no `From` implementation covers the required
conversion, the compiler reports a type error at the `?` site.

> See RFC 0007 for the full rationale.

---

## 5. Pattern matching and strong enums (features 8 and 9)

Enums carry data (sum types), and `match` is exhaustive.

```paco
const PI: f64 = 3.141592653589793;

enum Shape {
    Circle(radius: f64),
    Rectangle(width: f64, height: f64),
    Point,
}

fn area(s: Shape) -> f64 {
    match s {
        Shape::Circle(r)                => PI * r * r,
        Shape::Rectangle(width, height) => width * height,
        Shape::Point                    => 0.0,
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
    fn write(&mut self, data: &[]byte) -> Result<i64, Error>;
}

// No "implements Sink" clause. If File has the method, it satisfies Sink.
pub struct File {
    path: string,

    pub fn write(&mut self, data: &[]byte) -> Result<i64, Error> {
        // ...
        Ok(data.len())
    }
}

// Accepts anything that knows how to write. The receiver is `&mut self`, so the
// parameter must be `&mut dyn` — and the error is propagated, so the function
// returns a Result.
pub fn save(w: &mut dyn Sink, data: &[]byte) -> Result<i64, Error> {
    w.write(data)
}
```

An abstract trait method ends in `;`. A method with a default body ends in its
block. That distinction is what tells the parser which one it is reading.

Syntax notes:

- Receivers: `&self` (shared borrow, reads — **the common case**), `&mut self`
  (mutable borrow), `self` (consumes by move — rare; only for methods that turn
  the object into something else, e.g. `into_bytes`).
- **No hidden default.** `self` alone always means move, never a silent borrow —
  consistent with "visible cost" and "explicit ownership". The compiler warns if
  you write `self` (move) on a method that clearly only reads, suggesting `&self`.
- Slices are `[]byte`, `[]i64`.
- Defining methods **inside** the struct is the canonical form; a separate
  `methods T { ... }` block is **only** for extending a type from another module.
  If extending a generic type, generic parameters must be explicitly declared:
  `methods<T> Vec<T> where T: Display { ... }`.

### Passing a concrete type where a `dyn Trait` is expected

```paco
let model = Linear::new(w, b);
serve(&model, rx)                 // serve expects &dyn Model
```

A borrow of a concrete type coerces to a borrow of a trait object — `&T` to
`&dyn Trait`, and `&mut T` to `&mut dyn Trait` — whenever `T` satisfies that
trait. This is the one implicit conversion in the language, and it is not free:
the coercion attaches a vtable pointer, which is the visible cost `dyn` already
advertises. It happens only at a borrow, never on an owned value.

Important decision: interfaces are **implicitly satisfied** but **statically
checked**. You get decoupling without runtime duck-typing cost. For dynamic
polymorphism use `dyn Trait` (visible cost: vtable); for static use generics
(zero cost, monomorphization).

> This is NOT classic OOP: there is no inheritance, mutability is governed by
> ownership, and dispatch is static by default. Behavior is a *capability* a type
> satisfies (via traits), not something inherited from a hierarchy. See RFC 0002.

---

## 7. Metaprogramming: special traits + comptime (feature 2)

Instead of metatables, two mechanisms:

### Operator overloading via traits

```paco
struct Vec2 {
    x: f64,
    y: f64,

    fn add(&self, other: &Vec2) -> Vec2 {   // satisfies the `Add` trait
        Vec2 { x: self.x + other.x, y: self.y + other.y }
    }
}

let v = v1 + v2;   // statically resolved to the call above
```

"Magic" traits covering what metatables did: `Add`, `Index`, `Call`, `Display`,
`Iter`, etc.

**Primitive types satisfy the relevant magic traits natively**, built into the
compiler rather than through a `methods` block in the standard library. `1 + 2`,
`a == b` and `n.to_string()` work on primitives because the compiler knows they
do, not because someone wrote `methods i64 { ... }`. A user-defined type
satisfies the same traits the ordinary way, by having the methods (§6), and
becomes indistinguishable at the use site.

An overloaded binary operator `a op b` is the call `a.method(b)`, and its operands
follow the method's parameters: the left operand is borrowed for a `&self`
receiver, the right operand is moved when the parameter is `Self` by value and the
type is not `Copy`, and borrowed when the parameter is `&Self` (the form above). The
ordering operators `<`, `<=`, `>`, `>=` on a type with `fn cmp(&self, other) -> i64`
evaluate `a.cmp(b)` against `0`, as `Ord` requires; a type without `cmp` cannot be
ordered.

The bitwise operators `&`, `|`, `^`, `<<`, `>>` and prefix `~` work on every integer
type and are not overloadable. `&`, `|` and `^` take two operands of the same type;
a shift amount may be any integer type. `>>` is arithmetic on signed types and
logical on unsigned ones. A shift amount outside `0..bits` panics in debug builds
and is masked to the width in release builds, matching the overflow rule of §9.

### `comptime` — compile-time execution

```paco
// Generates code / inspects types with no runtime cost.
comptime fn derive_serialization(T: type) -> Code {
    // walk T's fields and generate the serializer
}

// Typical use: derives
#[derive(Serialize, Eq, Clone)]
struct User {
    name: string,
    age:  i64,
}
```

`comptime` is what gives "data-analysis" power: code generation for parsing, ORM,
serialization — all resolved at compile time. No runtime cost, no unpredictability.

`comptime` code runs in a sandbox — no I/O, no FFI, no tasks, no clock, and a
bounded number of steps — and computes exactly what the same code computes at
run time: the same integer overflow rules as a debug build (an overflow is a
compile error), the same float results and the same printed text. It runs the
same way under `paco run`, `paco build` and `paco build --release` (RFC 0027).

### No syntax macros

There are no syntax macros (no `macro_rules!` or procedural macros that operate
on token streams). `comptime` is the sole metaprogramming mechanism. This keeps
a single, learnable model: all code generation is written in Paco itself, runs
during compilation, and is type-checked like any other code.

This decision is explicitly provisional and will be revisited after Phase 6
(traits and dispatch), once there is practical evidence of what `comptime` cannot
cover in Paco's target use cases.

> See RFC 0009 for the full rationale.

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
let h = spawn risky();            // `spawn` returns a handle

match h.join() {                 // wait for the task and recover the result
    Ok(value)  => use_it(value),
    Err(panic) => log("task died: " + panic.message()),
}
```

Notes:

- If you ignore the handle (`spawn f()` without keeping it), a task panic is
  **logged and the task dies silently** — the process stays alive.
- `main` is the exception: a panic in `main` ends the process (no one can recover)
  with exit status 101.
- A panic prints `panic at <file>:<line>:<column>: <message>` on stderr, naming
  the Paco expression that panicked — an explicit `panic`, an out-of-bounds
  index, division by zero, overflow in a debug build or an `unwrap` of
  `None`/`Err`. Debug builds (including `paco run`) follow it with the chain of
  Paco function calls that led there.
- Because the result comes back typed in a `Result`, you are *encouraged* to handle
  it — it isn't a loose, easy-to-forget recover.

### Channels (communication between tasks)

CSP: "don't communicate by sharing memory; share memory by communicating."

```paco
let (tx, rx) = channel<i64>(capacity: 8);

// `?` needs a Result context, so the producer is a function, not a bare block.
fn produce(tx: Sender<i64>) -> Result<i64, Error> {
    for i in 0..10 { tx.send(i)? }
    tx.close();
    Ok(10)
}

let producer = spawn produce(tx);

for value in rx {        // iterates until the channel closes
    print(value)
}

// `select` over multiple channels. `Duration` lives in stdlib::time, so a file
// using a timeout arm imports it: `use stdlib::time`.
select {
    v = rx1.recv() => handle(v),
    v = rx2.recv() => handle(v),
    timeout(time::Duration::seconds(1)) => print("took too long"),
}

// Non-blocking select using the `default` fallback
select {
    v = rx1.recv() => handle(v),
    default        => print("no data available"),
}
```

Safety decision: the ownership system guarantees that data sent over a channel is
**moved** (not accidentally shared), eliminating data races at compile time.
Types shared across threads must be `Arc` + explicit synchronization.

> [!NOTE]
> **Compiler select mechanics:** While `rx.recv()` looks like a standard function call, the compiler recognizes it specially inside `select` arms. Instead of executing it immediately (which would block execution before `select` multiplexes), the compiler desugars `select` into calls to the runtime scheduling API (`paco_rt_select` registration), passing the channel references.
> Non-blocking behavior is achieved via the `default` branch, which runs immediately if no registered channel has pending data.

### Lightweight synchronous generators (`iter`) — secondary tool

For the hot paths in **games and data analysis** where you want to produce a
sequence on demand *without* the weight of a task + channel, there is `iter`: a
purely synchronous generator, "pulled" by the consumer. No allocation, no
scheduler, zero cost. It's the only place `yield` appears.

```paco
iter fn fibonacci() -> i64 {
    let mut a = 0;
    let mut b = 1;
    loop {
        yield a;              // pause; hand `a` back to whoever is iterating
        let next = a + b;
        a = b;
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
  `paco build --release` uses LLVM. `paco run` is `paco build` followed by
  running the binary, so it behaves exactly like the program you ship; an
  unchanged program is not rebuilt, because binaries are kept in a build
  cache keyed by the content of every input (`PACO_CACHE`, cleared by
  `paco clean --cache`). See RFC 0027.
- **Zero-cost abstractions**: iterators, `Option`, closures without allocation
  when possible.
- **Explicit data layout** when needed (`#[repr]`), important for games and data.

### Good for computation (data-analysis support)

Specific decisions to make the language strong with numbers:

- **Explicit numeric types with no surprises**: `i8..i64`, `u8..u64`, `f16`,
  `bf16`, `f32`, `f64`, `f8e4m3`, `f8e5m2` — every width spelled out, no
  generic `int`/`uint` default to reach for without thinking. No silent
  implicit coercion (visible cost).
- **Float math is built in**: every float type with arithmetic has `sqrt`,
  `exp`, `ln`, `sin`, `cos`, `tanh`, `abs`, `powf(y)`, `min(y)` and `max(y)`
  (`x.sqrt()`), with bit-identical results in every build mode and at compile
  time. `min`/`max` return NaN if either operand is NaN and order `-0.0` below
  `0.0`; `f16`/`bf16` compute in `f64` and round. FP8 has none (`PACO-E0339`).
  Each has a derivative rule, so gradients flow through it.
- **Operators on arrays/slices via traits** (`Add`, `Mul`...), allowing clean math
  notation on vectors and matrices with no runtime cost.
- **Overflow checked in debug, wrapping in release.** `paco build` panics on
  overflow; `paco build --release` wraps (two's complement, what the hardware
  does). Wrapping is the default because counter-based PRNGs — Philox and
  Threefry, what JAX and PyTorch use for reproducible randomness — are built on
  it and collapse under saturating arithmetic. Every integer type also carries
  `wrapping_*`, `saturating_*`, `checked_*` and `overflowing_*` for `add`, `sub`
  and `mul` (`x.wrapping_add(y)`, `x.checked_mul(y) -> Option<T>`,
  `x.overflowing_sub(y) -> (T, bool)`), which behave the same in every build mode,
  so intent can be written down. See RFC 0021.
- **`comptime`** (§7) generates specialized computation kernels at compile time.
  It is the mechanism behind column-typed data frames and fixed-shape matrices;
  the core provides only the mechanisms (traits, `comptime`, `#[repr]`, const
  generics), and the concrete numeric types live in libraries outside `stdlib`
  (`github.com/pacolang/tensor`, `github.com/pacolang/math`). See RFC 0011.
  A type that carries its dimensions in its type (see "Shapes" below) makes
  `a + b` on two different static shapes, or a `matmul` whose inner dimensions
  disagree, a compile-time error.

### Shapes: `const`, `dim` and `Dyn`

A generic parameter can be a value instead of a type (RFC 0023, RFC 0029). A
dimension is one of three kinds:

| Kind | Written | Known | Checked |
|------|---------|-------|---------|
| Static | `const N: int`, a literal | at compile time | by the type checker; one instantiation per value |
| Symbolic | `dim B`, a local witness, `?B` | at run time; its identity is in the type | by the type checker, on names; erased afterwards |
| Anonymous | `Dyn` | at run time only | only by explicit code |

The mechanism is not tied to any type. Any struct with const or `dim`
parameters is shape-checked the same way — a buffer, an image, an audio frame,
a grid or a library's tensor:

```paco
struct Grid<T: Numeric, const D: int...> { .. }   // a program's own type

fn matmul<dim M, dim K, dim N>(a: &Grid<f32, M, K>, b: &Grid<f32, K, N>) -> Grid<f32, M, N>;

// The batch is known only at run time, the features are static.
fn forward<dim B>(x: &Grid<f32, B, 768>, w: &Grid<f32, 768, 3072>) -> Grid<f32, B, 3072>;
```

- `const N: int` is one static dimension; `const D: int...` is a pack — at most
  one, always last. `D...` passes a whole pack on, and may follow fixed
  dimensions: `methods<T, const R: int...> Grid<T, Dyn, R...>`. Each distinct
  set of const arguments is a separate instantiation; an item instantiated with
  more than its limit (256, or `#[instantiation_limit(N)]`) is rejected with the
  offending constants listed (`PACO-E0337`).
- `dim B` is a symbolic dimension: it accepts a static, symbolic or `Dyn`
  argument, reads as an `i64` inside the item, and is passed as a hidden `i64`
  argument. It never creates an instantiation and never counts toward the
  limit. A `const` parameter cannot receive a symbolic dimension
  (`PACO-E0347`); declare it `dim` instead.
- Inside the item, a dimension parameter reads as a value: `N` and `B` are
  `i64`, and a pack `D` is a `[]i64`. A position instantiated with a named
  run-time dimension reads its value; an anonymous `Dyn` position reads `-1`.
- Arguments are inferred from the argument types, or written explicitly:
  `widen<2>(t)`. A parameter is inferred only from a position where it appears
  alone, or from `N + c` with a literal `c`; anything else needs an explicit
  argument (`PACO-E0344`).
- **Equality is polynomial.** Dimension expressions are compared in a
  canonical polynomial form over the integers: `B + B` is `2 * B`,
  `(N + 1) * 2` is `2 * N + 2`, `28 * 28` is `784`. `/` and `%` are opaque:
  `N / 2` equals only `N / 2`. Divisibility, bounds and inequalities are never
  proved by the type checker; code checks them explicitly.
- Two dimensions that are provably different are a type error naming both
  (`PACO-E0336`). Two dimensions that must be equal but cannot be proved equal
  — two `Dyn`, or two different names — are a different error
  (`PACO-E0342`), never a hidden run-time check. `Dyn` never unifies with a
  constant, with a name or with another `Dyn`. A value with a named dimension
  may still be passed where a parameter is declared `Dyn`; a static one may
  not, since static and run-time extents are stored differently.

**Local witnesses** name a run-time extent once, at the boundary where data
enters the program:

```paco
use stdlib::dims;

fn step(x: Grid<f32, Dyn, 784>, y: Grid<i64, Dyn>) -> Result<f32, dims::DimError> {
    let batch = x.dim(0);                               // an i64 and a rigid name
    let x: Grid<f32, batch, 784> = x.with_dims()?;      // no comparison needed
    let y: Grid<i64, batch> = y.with_dims()?;           // compared once, here
    let h = x + &x;                                     // no Result, no check
    Ok(loss(&h, &y))
}
```

- The mechanism works on any type that satisfies `stdlib::dims::Shaped` — it has
  `fn extent(&self, axis: i64) -> i64` and its `&mut self` methods never change
  an extent its type names.
- `v.dim(i)` on an immutable binding, with a literal axis, returns the extent
  and binds a rigid name to it. On a static axis it is the constant.
  `let t = dims::witness(len)?;` names a plain length the same way (a negative
  one is an `Err`). Binding a value to an immutable name gives each of its
  `Dyn` extents an anonymous name (`x.dim0`), so `x + &x` needs no check; a
  `let mut` binding does not.
- `v.with_dims()` moves `v` into the annotated type without copying, comparing
  each dynamic extent with its target once and returning `Err(DimError)` on a
  mismatch or a negative extent; its `display()` names the expected dimension,
  the line that bound it and the extent found. `v.as_dims()` does the same for
  a borrow. An extent that the witness was read from needs no comparison. A
  static position stays static and a dynamic one dynamic (`PACO-E0348`).
  `unsafe { v.assume_dims() }` asserts a relation a library maintains (two
  views of one cache); it is checked only in debug builds.
- After the boundary, operations whose dimensions are provably equal return
  their result directly: no `Result`, no panic, no check at run time. When
  equality is not proved, the operation does not compile, and the diagnostic
  offers `with_dims` or the `checked_*` form of the operation, which returns
  `Result`. A use of `Dyn` that needs no equality (the batch of a `matmul` whose
  inner dimension is static) compiles.
- **Existentials.** A type naming a witness outside the witness's scope must
  say so with `?B`: `fn nonzero(v: Grid<f32, Dyn>) -> Grid<f32, ?n>`, or
  `struct Batch { x: Grid<f32, ?b, 784>, y: Grid<i64, ?b> }`, where both fields
  share the same `?b` and constructing a `Batch` requires them to be proved
  equal. The caller opens it with `let` and gets a fresh name (`kept.n`), also
  in each loop iteration. A named dimension is never weakened to `Dyn`
  implicitly, nor carried out of the scope that bound it (`PACO-E0343`);
  `v.erase_dims()` forgets it explicitly.
- A value whose type names a symbolic dimension keeps its extent: operations
  that change extents consume the value and return an existential, and a
  borrowed view prevents the change while it lives.
- Broadcasting is explicit. A library marks its method
  `#[broadcasts(D, T)] fn broadcast_to<const T: int...>(&self) -> Grid<E, T...>`,
  and the checker requires each source axis, aligned from the right, to equal
  its target or to be the literal `1`; a dynamic extent never becomes `1`
  implicitly (`PACO-E0346`). `v.checked_broadcast_to()` returns `Result` when it
  cannot be proved.
- Extents are never negative, and arithmetic on dimensions is checked for
  overflow in every build, including `--release`.

`paco shapes file.paco` prints the shape of every `let`, with the origin of each
name. Dimension diagnostics show where each name was bound, the expression as
written and its normal form, and at most two fixes, machine-applicable under
`--format=json`; `paco explain <code>` prints what a code means.

Type parameters take trait bounds (`T: Add + Mul`); a bound is checked where
the item is used, and allows the operators it names inside a generic body.
Arithmetic on FP8 elements does not satisfy `Add`, `Sub` or `Mul`, so any
operation bounded by them rejects an `f8e4m3` element type.

### Gradients

A function marked `#[differentiable]` takes and returns floating-point scalars
(`f64`, `f32`, `f16`, `bf16`) or values of types that satisfy
`stdlib::autodiff::Differentiable` (or borrows and tuples of them). The trait,
after Swift's, names the type of a value's gradient:

```paco
pub trait Differentiable {
    type Tangent;
    fn zero_tangent(&self) -> Self::Tangent;
    fn move_by(&mut self, offset: &Self::Tangent);
}

struct Vec2 {
    x: f64,
    y: f64,
    type Tangent = Vec2;

    fn zero_tangent(&self) -> Vec2 { Vec2 { x: 0.0, y: 0.0 } }
    fn move_by(&mut self, offset: &Vec2) { self.x = self.x + offset.x; self.y = self.y + offset.y }
}
```

The float types satisfy it natively with `Tangent = Self`; FP8 does not, and
the compiler recognizes no other type by name — a library's tensor is
differentiable because it has these items. `stdlib::autodiff::grad(f, inputs)`
returns `(f(inputs), gradients)`, each gradient of its input's `Tangent` type
(with the same dimension names; dimensions themselves are never
differentiated). When `Tangent` is the type itself, the gradient of each field
is routed to the same field; otherwise a function that reads the fields needs
its own derivative (`PACO-E0815`).

Gradients are computed by the compiler itself (RFC 0026): after type and borrow
checking, the MIR of the differentiated function and everything it calls is
transformed into an augmented primal and a pullback, which run identically
under `paco run`, `paco build` and `paco build --release`. Control flow and
loops are differentiated by recording the branches taken; mutation through
`&mut` is differentiated by saving values the pullback needs before they are
overwritten. A `&mut` parameter is both an input and an output: its initial
value receives a gradient, and when the function returns `()` its final value
is the function's result (it must have exactly one such parameter,
`PACO-E0813`). `grad` differentiates a function whose result is a
floating-point scalar; a gradient of a gradient is allowed.

A function can supply its own derivative, which is used instead of
differentiating its body — a library's tensor operations do, and a function
backed by `extern` code must:

```paco
fn norm(v: &Vec2) -> f64 {
    (v.x * v.x + v.y * v.y).sqrt()
}

struct NormPullback {
    v: Vec2,
    n: f64,
    type Seed = f64;
    type Gradients = Vec2;

    fn pullback(self, seed: f64) -> Vec2 {
        Vec2 { x: seed * self.v.x / self.n, y: seed * self.v.y / self.n }
    }
}

#[derivative(of = norm)]
fn norm_derivative(v: &Vec2) -> (f64, NormPullback) {
    let n = norm(v);
    (n, NormPullback { v: Vec2 { x: v.x, y: v.y }, n: n })
}
```

The derivative takes the original function's parameters and returns its result
together with a value satisfying `stdlib::autodiff::Pullback`, whose
`pullback(self, seed)` maps the gradient of the result (`Seed`) to the
gradients of the differentiable parameters (`Gradients`, a tuple when there are
several). It may be declared in any module, including a fetched library, which
is how a library registers the derivatives of its own operations (`PACO-E0814`
for a malformed one). A call to an `extern` function cannot be differentiated
unless such a derivative exists (`PACO-E0810`); that, and every other
non-differentiable construct reached from `grad` — a raw-pointer access, a
value sent to another task, a borrow whose target is not known at compile time
(`PACO-E0811`), a call through a function value (`PACO-E0812`) — is a
compile-time error naming the expression and the call chain that reached it.

### Multidimensional indexing

An index expression takes one subscript or several:

```paco
let x = v[i];         // Index<i64>
let y = m[i, j];      // Index<(i64, i64)>
let z = t[i, j, k];   // Index<(i64, i64, i64)>
```

Each form desugars to the `Index` trait, with the subscript tuple as its type
parameter. A type becomes indexable by satisfying it:

```paco
trait Index<Idx> {
    type Output;
    fn index(&self, i: Idx) -> &Self::Output;
}
```

`type Output;` is an **associated type**: declared by the trait, supplied by the
type that satisfies it. Indexing is not valid on strings (§10, RFC 0010).

### Reduced-precision floating point

AI workloads do not run on `f32` and `f64` alone, so Paco makes the formats they
do run on primitive types (RFC 0014):

| Type | Layout | Arithmetic |
|------|--------|-----------|
| `f16` | IEEE 754 binary16 (1-5-10) | Yes |
| `bf16` | bfloat16 (1-8-7) — `f32`'s exponent range | Yes |
| `f8e4m3` | OCP FP8 (1-4-3) | **No** |
| `f8e5m2` | OCP FP8 (1-5-2) | **No** |

`f8e4m3` and `f8e5m2` carry **no arithmetic operators at all**. They are storage
and interchange formats: convert with `as` to compute. This is not a restriction
we are working around — it reflects the hardware, where FP8 is consumed by
matrix-multiply units as an input format and accumulated in something wider.
Exposing scalar FP8 arithmetic would promise something no target delivers.

**There is no implicit conversion between any two float types, in either
direction.** Widening is as explicit as narrowing:

```paco
let a: bf16 = 1.5;
let b: f32  = a;           // ERROR: no implicit widening
let c: f32  = a as f32;    // OK
let d: bf16 = c as bf16;   // OK — narrowing, precision loss is visible

// FP8 is storage; widen to compute.
let w: f8e4m3 = load_weight();
let acc: f32  = (w as f32) * (c as f32);
```

Narrowing rounds to nearest, ties to even. Values beyond the target's range
become infinities — defined behaviour, not a panic, because a panic in an inner
loop is unusable.

**Accumulation width belongs to the operation, not the element type.** A
reduction over `[]bf16` does not implicitly accumulate in `bf16`; a standard
library `sum` over `[]bf16` returning `f32` is the expected shape, not a special
case.

### Printing floats

`print`, `display` and `float_to_string` render every float type the same way,
at compile time (`comptime`) and at run time, on every backend:

- **Digits:** the fewest significant digits that read back to the same value
  *in the value's own type*. `0.1` prints `0.1` whether it is an `f64`, `f32`,
  `f16` or `bf16`; `0.1 + 0.2` prints `0.30000000000000004`; `(65504.0 as f16)`
  prints `65500`, because `65500` already rounds back to that `f16`.
- **Layout:** plain decimal when the decimal exponent `e` satisfies
  `-7 < e < 21` (`100000000000000000000`, `0.000001`), otherwise
  `<digits>e<exponent>` with no `+` and no padding (`1e21`, `1e-7`, `5e-324`).
  An integral value has no fractional part: `1.0` prints `1`.
- **Special values:** `inf`, `-inf`, `NaN` (never signed); negative zero prints
  `-0`.

---

## 10. Strings — UTF-8 guaranteed (supports features 4 and 10)

Strings are **always valid UTF-8**, not raw bytes. This eliminates a whole class
of encoding bugs, at the cost of slicing needing to be char/byte aware.

```paco
let s = "café";           // always valid UTF-8
s.len();                  // 5 (bytes) — explicitly counts bytes
s.chars().count();        // 4 (Unicode characters)

for c in s.chars() { ... }       // iterate by character
for b in s.bytes() { ... }       // iterate by byte
```

Decisions:

- `string` is immutable and UTF-8; `StringBuf` (or `[]byte`) for mutable building.
- **No direct range indexing on strings.** Use `s.get(0..n) -> Option<&string>`
  for UTF-8-safe slicing (returns `None` if the range cuts mid-codepoint), and
  `s.as_bytes()[0..n]` for raw byte access. No implicit panic — boundary failures
  are values, not crashes:

```paco
// Safe slicing — returns Option, never panics
match s.get(0..3) {
    Some(sub) => print(sub),    // "caf"
    None      => handle_error(),
}

// Raw byte access — no UTF-8 concern, explicit intent
let raw: []byte = s.as_bytes();
let slice = raw[0..3];   // []byte, always valid
```

- `==` on strings compares by **value** (contents), not by pointer.
- `s.as_bytes()` copies the bytes into a new `[]byte` in one operation, and
  `string_from_bytes(&bytes, start, end)` turns a byte range back into
  `Some(string)` only when it is valid UTF-8. `StringBuf` appends in amortized
  constant time (`push_str`, `push(c)`), and `to_string()` copies the result out.
- Separate types keep the cost visible: you always know whether you're dealing
  with bytes, code points, or graphemes (the latter via a library).

> See RFC 0010 for the full rationale on string slicing.

---

## 11. Single binary (feature 14)

`paco build` produces **one static executable**, with no external runtime
dependencies, easy to distribute. The concurrency runtime (M:N scheduler) is
embedded in the binary. Building needs only the Paco distribution — no C
compiler, linker or system packages.

A program without `extern` blocks is linked statically against musl and runs
on any Linux system of its architecture. A program with `extern` blocks is
linked dynamically against the target system's glibc and the C libraries it
names, since those (GPU drivers, BLAS) ship as shared objects; `--link
static|dynamic` overrides the choice, and a static build of a program with
`extern` blocks is an error.

On macOS and Windows, `paco build` produces native executables linked
against the platform's own libraries (the macOS SDK through the Xcode command
line tools; the MSVC runtime and Windows SDK), which cannot be shipped with
Paco.

Cross-compilation is first-class: `paco build --target aarch64-unknown-linux`
(or `x86_64-unknown-linux`) works from either Linux architecture with nothing
installed, and from macOS. The link mode completes the target to `-musl` or `-gnu`; an
explicit suffix chooses the mode. A dynamic cross build links against the
target system's libraries, given as a directory with `--sysroot <dir>` (or
`PACO_SYSROOT`).

---

## 12. Tests with decorators (feature 15)

Tests live alongside the code, marked by an attribute.

```paco
#[test]
fn tests_add() {
    assert_eq(add(2, 3), 5)
}

#[test]
#[should_panic]
fn tests_divide_by_zero() {
    divide(1, 0)
}

// Benchmarks
#[bench]
fn bench_parse(b: &mut Bencher) {
    b.iter(|| parse(input))
}
```

`paco test` discovers and runs everything. No external framework in the basic case.

---

## 13. Modules, visibility, and tooling

### A directory is a module

Every `.paco` file in a directory belongs to the same module, shares one
namespace, and can see all of that module's declarations regardless of `pub`.
Subdirectories are separate modules — nesting on disk implies nothing about
visibility.

Every file opens with its module declaration, which must match the directory
name, then its imports, then its items. Nothing may precede the module
declaration and imports may not be interleaved with items. A Paco file therefore
always has the same shape:

```paco
module nn;

use stdlib::math;
use example.com/team/tensor as tensor;

pub const EPS: f32 = 1e-6;

pub struct Linear {
    w: tensor::Tensor<f32>,     // private field
    b: tensor::Tensor<f32>,

    pub fn forward(&self, x: &tensor::Tensor<f32>) -> tensor::Tensor<f32> {
        // ...
    }

    fn init_weights(&mut self) { /* private to module nn */ }
}
```

The `module` line is redundant with the directory name by design. It costs one
line and makes every file self-describing: you can open a single file and know
where you are without inspecting the filesystem.

### Visibility: private by default, `pub` exports

- No marker means **private to the module**.
- `pub` exports an item from its module.
- `pub` on a `struct` or `enum` exports the **type only**. Fields, methods, and
  associated constants each need their own `pub`. This is what lets a module
  expose a type whose representation stays private.
- Enum **variants** are exported with the enum and take no `pub` — variants you
  cannot see are variants you cannot match on, which would make exhaustiveness
  meaningless.
- There is exactly one level of visibility. No `pub(crate)`, no `internal`, no
  friend mechanism.

Paco deliberately does **not** use Go's capitalisation rule. Go can afford it
because it writes `CamelCase` everywhere; Paco writes `snake_case` functions, so
exporting `read_config` would force `Read_config` and put the visibility rule in
direct conflict with the canonical formatter. See RFC 0015.

### Imports bind whole modules

```paco
use stdlib::io;                          // referred to as io::
use example.com/team/json;            // referred to as json::
use example.com/team/json as parser;  // alias
```

<!-- consistency-ignore: selective-import -->
There is **no selective import** (`use stdlib::io::{Read, Write}`) and no wildcard
import. Call sites stay qualified — `io::read_file(path)`, never a bare
`read_file`. Two modules may export the same name without conflict, and every
call site states where its callee came from, which removes an entire class of
ambiguity when reading a fragment of code out of context.

### The prelude

The prelude is the only exception to qualified imports. It holds what the
language's own rules force you to use — what the compiler desugars to, what the
operators resolve against, and what the ADRs name as the only correct answer to a
problem the language creates.

| Group | Names |
|---|---|
| Desugaring targets | `Option`, `Some`, `None`, `Result`, `Ok`, `Err` |
| Traits the compiler resolves | `From`, `Into`, `Display`, `Clone`, `Copy`, `Drop`, `Eq`, `Ord`, `Hash`, `Add`, `Sub`, `Mul`, `Div`, `Rem`, `Neg`, `Index`, `Iter`, `Call`, `Numeric` |
| Collections | `Vec`, `Map`, `Set`, `StringBuf` |
| Ownership escape hatches | `Rc`, `Arc`, `Cell`, `RefCell`, `Mutex`, `RwLock` |
| Concurrency | `channel`, `Sender`, `Receiver`, `spawn_blocking`, `TaskPanic` |
| Free functions | `print`, `panic` |

Not in the prelude, and imported like anything else: `Duration` (`stdlib::time`),
I/O (`stdlib::io`), maths (`stdlib::math`) and gradients (`stdlib::autodiff`). `Tensor`
(`stdlib::numerics`), `Matrix` and `DataFrame` (`stdlib::math`) still ship inside
`stdlib` today, but are marked for extraction to official libraries outside `stdlib`
(`github.com/pacolang/tensor`, `github.com/pacolang/math`) — RFC 0030 keeps them
unprivileged, and a prelude entry is a privilege; see `docs/ecosystem.md` for
the admission criteria and the target organization. The one marker trait the
compiler satisfies natively for them, `Numeric`, is in the prelude with the
other traits it resolves.

`Bencher` is not imported either; the `#[bench]` attribute brings it into scope
the way a parameter would.

`Map<K, V>` and `Set<K>` are hash tables: their keys satisfy `Hash + Eq`, which
every integer type, `bool`, `char` and `string` do natively (floats do not), and a
user type does by declaring `fn hash(&self) -> u64`. `insert`, `get`,
`get_or_insert` and `contains_key` take expected constant time. `Vec<T>` and `[]T`
have `sort()` for `T: Ord` and `sort_by(compare)`, both stable and O(n log n);
`sort()` orders floats by the IEEE total order.

A prelude name may be shadowed by a module-level declaration, and the local
definition wins without a warning. The exception is the operator traits:
shadowing `Add` does not change what `+` means, because the operator binds to the
prelude trait rather than to whatever `Add` currently names. See RFC 0022.

### Tooling

- `paco fmt` — canonical formatter (non-negotiable).
- `paco test` — built-in test/benchmark runner.
- `paco build` — single binary.
- `paco run [file] [-- args]` — build (cached) and run; same binary as
  `paco build` (RFC 0027). The cache is `PACO_CACHE`, else
  `$XDG_CACHE_HOME/paco`, else `~/.cache/paco`.
- `paco clean [file]` — remove build outputs; `paco clean --cache` empties the
  build cache.
- `paco doc` — documentation from comments.
- **Decentralized dependencies**: a dependency is the URL of its source repository
  pinned to a version-control tag (semantic versioning). There is no central
  registry. The manifest is `paco.mod` with a lock file; external modules are
  imported by their URL-like module path (`use example.com/team/json`). See
  RFC 0005. Implementation is deferred to a later milestone.

---

## 14. Coverage of the original 15-feature brief

> **Historical note.** Paco began as a list of fifteen desired features, and the
> section headings above still carry references to their numbers. That list was
> a wish list, not a thesis — it is what produced a golden rule ("concurrent
> services wins") that did not match what the project set out to build. RFC 0013
> replaced it. The table is kept because the features are still covered and the
> cross-references are still useful, not because it still organises the design.

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
| 15 | Test decorators | §12 | `#[test]`, `#[bench]`, `#[should_panic]` |

Added since, under the current golden rule (RFC 0013, RFC 0028):

| Capability | Where | How |
|------------|-------|-----|
| Reduced-precision floats | §9 | `f16`, `bf16` arithmetic; `f8e4m3`, `f8e5m2` storage |
| Shape-checked dimensions | §9 | `const N`, `dim B`, `Dyn`; run-time extents checked once, at the boundary |
| Modules and visibility | §13 | Directory modules, `module` declaration, `pub` |
| Constants | §17 | `const`, compile-time evaluated; no mutable globals |
| Hardware access | §18 | `extern "C"`, `unsafe`, raw pointers, `#[repr(C)]` |

---

## 15. "Everything together" example

A minimal inference server: a producer task feeds requests over a channel, the
main task runs them through a model and reports. It exercises modules, `pub`,
`const`, traits with implicit satisfaction, enums with methods, exhaustive
`match`, `Result`/`?`, `spawn` with a handle, and per-task panic isolation.

```paco
module server;

use stdlib::io;

pub const MAX_BATCH: i64 = 32;

// `Error` is not a prelude type. Every example that propagates one declares it,
// because Paco has no built-in error enum — you define the errors your module
// can produce, and `?` converts between them via `From` (§4).
pub enum Error {
    ShapeMismatch,
    Closed,
}

trait Model {
    fn infer(&self, input: &[]f32) -> Result<[]f32, Error>;
}

pub struct Linear {
    w: []f32,          // private — representation is not part of the contract
    b: []f32,

    pub fn new(w: []f32, b: []f32) -> Linear {
        Linear { w, b }        // field shorthand: `w` is `w: w`
    }

    // No "implements Model" clause: having the method is enough.
    pub fn infer(&self, input: &[]f32) -> Result<[]f32, Error> {
        if input.len() != self.w.len() {
            return Err(Error::ShapeMismatch);
        }
        // ...
        Ok(Vec::new().into_slice())
    }
}

pub enum Request {
    Predict(input: []f32),
    Shutdown,

    pub fn describe(&self) -> string {
        match self {
            Request::Predict(input) => "predict/" + input.len().to_string(),
            Request::Shutdown       => "shutdown",
        }
    }
}

fn produce(tx: Sender<Request>, samples: [][]f32) -> Result<i64, Error> {
    let mut sent = 0;
    for s in samples {
        tx.send(Request::Predict(s))?;
        sent += 1
    }
    tx.send(Request::Shutdown)?;
    tx.close();
    Ok(sent)
}

pub fn serve(model: &dyn Model, rx: Receiver<Request>) -> Result<i64, Error> {
    let mut served = 0;
    for req in rx {
        match req {
            Request::Predict(input) => {
                let output = model.infer(&input)?;
                io::print(output.len().to_string());
                served += 1
            }
            Request::Shutdown => break,
        }
    }
    Ok(served)
}

fn main() {
    let (tx, rx) = channel<Request>(capacity: MAX_BATCH);
    let producer = spawn produce(tx, load_samples());
    let model = Linear::new(load_weights(), load_bias());

    match serve(&model, rx) {
        Ok(n)  => io::print("served " + n.to_string()),
        Err(e) => io::print("failed: " + e.to_string()),
    }

    // A panic in the producer kills that task only; we recover it here.
    match producer.join() {
        Ok(sent)   => io::print("produced " + sent.to_string()),
        Err(panic) => io::print("producer died: " + panic.message()),
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

## 17. Constants and compile-time values

```paco
pub const EPS: f32 = 1e-6;
const TILE: i64 = 64;

pub struct Tensor<T> {
    data: []T,

    pub const RANK: i64 = 2;       // associated constant
}
```

Rules:

- **The type annotation is mandatory.** A constant is API surface; its type is
  part of the module's contract and must not depend on how it was computed.
- **The initialiser must be evaluable at compile time** — a literal, an operation
  over constants, or a `comptime` call (§7).
- **A constant has no address and no runtime initialisation.** It is substituted
  at each use site, so using one costs exactly what writing the literal costs.
- Constants may be declared at module level, or inside a `struct`, `enum`,
  `trait`, or `methods` block.

### No mutable global state

Paco has no `static`, no `static mut`, and no module-level `var`. Mutable state
that outlives a scope is owned by something: passed down as a parameter, or
shared through `Arc<Mutex<T>>` / `Arc<RwLock<T>>` with the cost written at the
point of sharing.

This is what keeps §8's central claim intact — that the ownership system prevents
data races at compile time. A mutable global is reachable from every task at once
and owned by none, so it is exactly the construct the borrow checker cannot
reason about. Go permits package-level `var` and pays for it with a runtime race
detector; Paco does not need one because the construct does not exist.

> Immutable `static` — a value with a stable address, for large tables that
> should not be duplicated at each use site — is a real but narrower need, and is
> deliberately left open (§20). See RFC 0016.

---

## 18. Foreign function interface

Paco does not interoperate with Python. Its boundary with the outside world is
the **C ABI**, and everything above that boundary is written in Paco. This is
what makes accelerators reachable: cuBLAS, cuDNN, NCCL, the CUDA driver API,
ROCm, Metal and every BLAS implementation are C libraries.

### Declaring foreign functions

```paco
module blas;

extern "C" {
    fn cblas_sgemm(
        order: i32, transa: i32, transb: i32,
        m: i32, n: i32, k: i32,
        alpha: f32,
        a: *const f32, lda: i32,
        b: *const f32, ldb: i32,
        beta: f32,
        c: *mut f32, ldc: i32,
    );
}
```

Every function in an `extern` block is implicitly `unsafe fn`: the compiler
cannot verify a foreign signature, so calling one requires an `unsafe` block.

### `unsafe` is an expression block

```paco
pub fn sgemm(a: &[]f32, b: &[]f32, c: &mut []f32, m: i64, n: i64, k: i64) {
    unsafe {
        cblas_sgemm(
            ROW_MAJOR, NO_TRANS, NO_TRANS,
            m as i32, n as i32, k as i32,
            1.0, a.as_ptr(), k as i32,
            b.as_ptr(), n as i32,
            0.0, c.as_mut_ptr(), n as i32,
        )
    }
}
```

`unsafe { ... }` yields its final value, so `let x = unsafe { *p }` is idiomatic
and the unsafe region stays as small as the operation needing it. The idiom is a
thin safe wrapper over a narrow unsafe core.

Inside an `unsafe` block exactly three additional things become legal:
dereferencing a raw pointer, calling an `unsafe fn`, and calling a foreign
function. `unsafe` does **not** disable the borrow checker, permit use after
move, or relax exhaustiveness.

### Raw pointers

`*const T` and `*mut T` exist for FFI. A raw pointer carries no lifetime and no
aliasing guarantee, is exempt from the borrow checker, and may be null. It is not
an escape hatch for ordinary Paco code — `Rc`/`Arc` remain the answer when
ownership is awkward (§3). Converting a borrow to a raw pointer is safe;
dereferencing one is not.

### Exporting to C

```paco
pub extern "C" fn paco_kernel(data: *mut f32, len: i32) { /* ... */ }
```

Paco can be loaded as a shared library by an existing system, so a team can
replace one kernel without rewriting the program around it.

### Layout

`#[repr(C)]` gives a struct the target's C ABI layout. Without it Paco makes no
guarantee about field order or padding and is free to reorder for packing.

### Memory across the boundary

Paco frees Paco memory and C frees C memory. Paco values are allocated by
Paco's own allocator; memory a C library returns must be released with that
library's function (`free`, `cudaFree`, …), never by dropping a Paco value
that points at it, and a pointer to Paco-owned memory handed to C must not
be freed by C.

> **Honest caveat.** With FFI, Paco can segfault. The safety claim becomes "safe
> outside `unsafe`" — weaker than "safe", and worth saying plainly. See RFC 0017.

A foreign call blocks its OS thread, and the M:N scheduler (§8) cannot suspend
opaque native code. Run it on the blocking pool:

```paco
let result = spawn_blocking(|| {
    unsafe { cblas_sgemm(/* ... */) }
}).join()?;
```

`spawn_blocking` returns the same handle `spawn` does, with the same panic
isolation. Calling an `extern` function directly from a task fires the
`blocking-call-on-worker` lint — a warning, not an error, because the compiler
cannot know which foreign functions block. See RFC 0018.

---

## 19. Settled decisions

See `https://github.com/pacolang/rfcs` for the full RFCs. Summary:

- Syntax: clean and light, with error handling via `Result`/`?`.
- Methods: inside the `struct`/`enum`. Receivers: `&self` (common, reads),
  `&mut self` (mutates), `self` (rare, consumes). No hidden default; the compiler
  suggests `&self` when it fits (§6, RFC 0002).
- Lifetimes: `'a`, only when inference fails (§16, RFC 0001).
- Backend: Cranelift for dev (`paco build`), LLVM for release
  (`paco build --release`) (§9, RFC 0003).
- Strings: UTF-8 guaranteed, value equality (§10).
- Concurrency: unified lightweight tasks (`spawn` + channels, no async/await),
  with synchronous `iter` as a secondary tool; per-task panic isolation
  (§8, RFC 0004).
- Computation: explicit numeric types, operators on arrays, overflow checked in
  debug (§9).
- Packages: decentralized, URL + version-control tag, no central registry
  (§13, RFC 0005).
- Collection construction: `Vec::new()`, `Map::new()`, etc. — associated
  functions inside the type block, no literal shorthand (§3, RFC 0006).
- Error conversion: `?` calls `From::from(e)` automatically; `From<T>` trait in
  the prelude; implicit satisfaction (§4, RFC 0007).
- Struct mutability: binding-level only (`let mut`); no per-field modifiers
  (§3, RFC 0008).
- Syntax macros: `comptime` is the sole metaprogramming mechanism; no syntax
  macros at this stage (§7, RFC 0009).
- String slicing: `s.get(0..n) -> Option<&string>` for safe slicing;
  `s.as_bytes()[0..n]` for raw bytes; no `s[n..m]` on strings (§10, RFC 0010).
- Data analysis: standard library (`src/math/`) built on `comptime` + traits;
  no language-core types; `DataFrame<Schema>` and `Matrix<T>` are library types
  (§9, RFC 0011).
- Positioning and golden rule: general purpose; building AI systems end to end
  breaks ties between conflicting designs; no Python interoperation; the
  interop boundary is the C ABI (§0, RFC 0013, RFC 0028).
- Reduced-precision floats: `f16`/`bf16` arithmetic, `f8e4m3`/`f8e5m2` storage
  only; no implicit float conversion in either direction (§9, RFC 0014).
- Modules: a directory is a module; `module <name>` opens every file; imports
  bind whole modules, always qualified (§13, RFC 0015).
- Visibility: private by default, `pub` exports; `pub` on a type exports the type
  only (§13, RFC 0015).
- Constants: `const` with mandatory annotation, compile-time evaluated; no
  mutable global state (§17, RFC 0016).
- FFI: `extern "C"` blocks, `unsafe { }` expression blocks, raw pointers,
  `#[repr(C)]` (§18, RFC 0017).
- Blocking foreign calls: a separate blocking pool, Tokio-style `spawn_blocking`;
  `blocking-call-on-worker` is a lint, not an error (§8, RFC 0018).
- GPU: reached through the C ABI now; native PTX/SPIR-V codegen is a declared
  later direction, not a present capability (§18, RFC 0019).
- Rust alignment: `&self`/`&mut self` receivers, `#[attr]` attributes, `@` for
  pattern binding only (§6, RFC 0020).
- Slices: `[]T` owns; `&[]T`/`&mut []T` is the view (§3, RFC 0020).
- Indexing: `a[i, j]` desugars to `Index<(i64, i64)>`; traits may declare
  associated types (§9, RFC 0020).
- Integer overflow: checked in debug, **wrapping** in release; explicit
  `wrapping_*`/`saturating_*`/`checked_*`/`overflowing_*` forms (§9, RFC 0021).
- Const generics: static dimensions plus a `Dyn` marker; `Dyn` never unifies at
  compile time; weak structural equality on constant expressions (§9, RFC 0023).
- Autodiff: native reverse mode over MIR, identical under `paco run` and both
  backends; custom derivatives through `#[derivative(of = f)]` (§9, RFC 0026,
  superseding RFC 0024).
- Prelude: enumerated in §13 — desugaring targets, operator traits, collections,
  ownership escape hatches, concurrency types, `print`/`panic` (§13, RFC 0022).
- Backend parity: every conformance program runs through both backends in debug
  and release, and all outputs must match each other and the reviewed expected
  output, including release wrapping overflow (RFC 0003, RFC 0021).
- Execution: `paco run` compiles with Cranelift and runs the binary, reusing a
  content-addressed build cache; `comptime` runs on a MIR interpreter whose
  arithmetic and formatting match compiled code; compiled panics report the
  Paco source location (RFC 0027).

---

## 20. Open questions

This section is never empty. A language at v0.1 with open questions is honest;
one without them has stopped looking.

Settled decisions live in §19 and in `https://github.com/pacolang/rfcs`. What follows is
only what is genuinely undecided.

### 1. Immutable `static`

RFC 0016 rules out mutable global state permanently. Whether a large constant
table needs a stable address, rather than substitution at every use site,
is still open — and FFI raised its priority, since passing `*const T` from a
`const` points at a temporary.

Deferred to Phase 12, when the first real table exists.

### 2. Does any of this actually help a model write Paco?

No measurement exists, and the project's premise is that much of the Paco written
will be generated.

Until a benchmark runs — models that never saw Paco in training, scored by
compiling and running, against Python and Rust baselines on the same problems —
**every ergonomic argument in this document is an assertion.** That includes the
ones used to justify `pub` over capitalisation, qualified call sites over
selective imports, and following Rust on receivers and attributes.

A Phase 11 deliverable.
