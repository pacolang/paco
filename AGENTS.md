# Paco — agent context
 
**Read this file every time you touch Paco code in this repo.**
 
Paco is at the design/bootstrap stage. The commands and tools described here are
the target behavior; not everything exists yet. When the real code diverges from
this file, the code wins — and this file should be updated.
 
## 1. Required reading
 
Before generating, editing, or reviewing any `.paco` file:
 
1. Read `docs/design/spec.md` (what the language is). Do not reopen a
   decision already recorded in an ADR without justification.
2. Scan the module you'll modify (under `src/`) and the programs in `examples/`
   for existing conventions, then match them.
This is not optional. Paco has its own idioms — guessing leads to code that
doesn't compile or that contradicts the design.
 
## 2. Language quick-reference
 
- **Errors as values**: `Result<T, E>` and `Option<T>`. No exceptions, no `null`.
  Propagate with the postfix `?`. `panic` is only for unrecoverable bugs, never
  control flow.
- **Memory**: ownership + move by default. Borrows `&` (shared/immutable) and
  `&mut` (mutable); aliasing rule: N `&` XOR one `&mut`. Lifetimes are inferred —
  only annotate `'a` when the compiler asks. Escape hatch: `Rc<T>` (single
  thread) / `Arc<T>` (multi-thread). Deterministic cleanup (RAII) at scope exit.
- **Concurrency**: lightweight M:N tasks via `spawn f(args)`. No `async`/`await` —
  the runtime suspends on I/O automatically. `spawn` returns a handle;
  `h.join() -> Result` recovers the value or the task's isolated panic (a panic
  in a task does NOT bring down the process). Channels: `channel<T>(capacity: n)`,
  `tx.send(x)?`, `rx.recv()`, `tx.close()`, and `select { ... }`.
- **Synchronous generators**: `iter fn name() -> T { ... yield x ... }`. Pulled by
  the consumer (`for x in name()`), with no task cost. `yield` only appears here.
- **Methods**: defined **inside** the `struct`/`enum` block. Receivers:
  `self&` (reads — the common case), `self&mut` (mutates), `self` (consumes —
  rare). To extend a type defined in another module, use a separate
  `methods T { ... }` block. **No inheritance** — compose with traits and structs.
- **Traits + dyn**: **implicit** satisfaction (a type satisfies a trait if it has
  the methods; no `implements` clause), checked **statically**. Use `dyn Trait`
  for dynamic dispatch (vtable, visible cost).
- **Generics**: `fn f<T>(v: T)`, bounds via `T: Trait`. Monomorphized (zero cost).
- **Metaprogramming**: `comptime` (compile-time execution, type introspection,
  code generation) + special traits (`Add`, `Index`, `Display`, `Iter`...).
  Derives via attribute: `@derive(Display, Clone, Eq)`.
- **Tests**: `@test`, `@bench`, `@should_panic` above the function.
- **Strings**: always valid UTF-8. `s.len()` counts **bytes**; `s.chars()`
  iterates characters; `s.bytes()` iterates bytes. Byte slicing validates
  character boundaries. `==` on strings compares by **value** (contents), not by
  pointer.
- **Doc comments**: `///` above the declaration; examples in fenced ```paco blocks.
## 3. Project commands
 
```
paco new <name>             create a project
paco run                    build + run
paco build                  build (dev backend) -> single binary
paco build --release        build (optimizing backend)
paco build --target=<triple>  cross-compilation
paco check <file>           parse + types + borrow check, no codegen
paco test [path]            run @test functions
paco bench [path]           run @bench functions
paco fmt <file> [--write]   canonical formatter (non-negotiable)
paco doc                    generate documentation
paco clean                  wipe build artifacts
```
 
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
`use example.com/team/json`. Standard-library modules use the short `std::` path.
 
## 4. House rules
 
- Don't use `try`/`catch` or exceptions; use `Result`/`Option` + `?`.
- Don't use `null`; absence is `Option::None`.
- Don't write `self` (move) on a method that only reads — use `self&`. The
  compiler warns.
- Don't share data across tasks without `Arc` + explicit synchronization; what
  goes through a channel is **moved**.
- Don't build inheritance hierarchies; they don't exist. Share behavior via traits.
- Prefer defining methods inside the `struct`. Use a separate `methods T {}`
  block **only** to extend a type from another module.
- `match` is exhaustive — covering every case is not style, it's a compiler
  requirement.
- When slicing strings, be conscious of byte vs character; don't assume 1 byte = 1 char.
## 5. Lints (compiler-enforced patterns)
 
`paco check` and the future LSP run these checks. Each fires a lint code that
`@allow("<code>")` on the enclosing function/block silences. Don't suppress
without a justification in a comment — the lint usually points at a real bug.
 
- `use-after-move` — using a value after it has been moved.
- `unhandled-result` — calling a `Result`-returning function and silently
  discarding the value swallows the error. Use `f(...)?`, `let _ = f(...)`, or
  bind the result.
- `ignored-option` — reading the contents of an `Option` without a preceding
  `match`/`if let` reads the `None` case improperly.
- `needless-move-self` — a `self` (move) receiver where `self&` would suffice.
- `non-exhaustive-match` — a `match` that doesn't cover every case (a hard error
  in practice).
- `shared-without-sync` — sending non-`Arc` shared data across tasks.
- `unclosed-channel` — a channel without `close()` on some path leaves receivers
  parked forever on `recv()`.
- `string-byte-boundary` — slicing a string at an offset that falls in the middle
  of a character.
Default forms to follow:
 
```paco
// fallible call
let cfg = read_config()?
 
// option with a check
match lookup(key) {
    Some(v) => use_it(v),
    None    => return Err(Error::NotFound),
}
 
// task with isolated panic
let h = spawn work()
match h.join() {
    Ok(value)  => use_it(value),
    Err(panic) => log("task died: " + panic.message()),
}
 
// channel (closes at scope end via RAII, or explicitly)
let (tx, rx) = channel<int>(capacity: 8)
spawn {
    for i in 0..10 { tx.send(i)? }
    tx.close()
}
for v in rx { print(v) }
 
// method inside the struct
struct Point {
    x: f64,
    y: f64,
 
    fn distance(self&, other: &Point) -> f64 {
        math::sqrt((self.x - other.x).pow(2) + (self.y - other.y).pow(2))
    }
}
```
