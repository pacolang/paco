# Paco — Design Context

> This file tracks the current design state: settled decisions and open questions.
> Update it whenever a decision moves from open to settled, and keep it in sync
> with `docs/design/spec.md` (§17 and §18) and the RFCs at
> `https://github.com/pacolang/rfcs`.

## Settled Decisions

| Topic | Summary | RFC |
|-------|---------|-----|
| Memory | Ownership + move semantics, aggressive lifetime inference. `Rc`/`Arc` as explicit escape hatch. | [RFC 0001](https://github.com/pacolang/rfcs/blob/main/text/0001-memory-model.md) |
| Methods | Defined inside `struct`/`enum`. `methods T {}` only for external extensions. Explicit receivers (`&self`, `&mut self`, `self`). | [RFC 0002](https://github.com/pacolang/rfcs/blob/main/text/0002-struct-methods.md) |
| Backend | Cranelift for dev (`paco build`), LLVM for release (`paco build --release`). Common Paco IR. | [RFC 0003](https://github.com/pacolang/rfcs/blob/main/text/0003-compilation-backend.md) |
| Concurrency | M:N lightweight tasks (`spawn`), channels, automatic suspension, no `async`/`await`. `iter` for synchronous generators. Per-task panic isolation. | [RFC 0004](https://github.com/pacolang/rfcs/blob/main/text/0004-concurrency.md) |
| Metaprogramming | Special traits + `comptime`. No runtime metatables. | [RFC 0005](https://github.com/pacolang/rfcs/blob/main/text/0005-metaprogramming-and-packages.md) |
| Packages | Decentralized, URL + version-control tag, no central registry. Manifest `paco.mod`. Tooling deferred. | [RFC 0005](https://github.com/pacolang/rfcs/blob/main/text/0005-metaprogramming-and-packages.md) |
| Collection construction | `Vec::new()`, `Map::new()`, etc. Associated functions only. No shorthand literal syntax. | [RFC 0006](https://github.com/pacolang/rfcs/blob/main/text/0006-collection-construction.md) |
| Error conversion | `?` calls `From::from(e)` automatically when error types differ. `From<T>` lives in the prelude. Implicit trait satisfaction — no `implements` clause needed. | [RFC 0007](https://github.com/pacolang/rfcs/blob/main/text/0007-error-conversion-from-trait.md) |
| Struct mutability | Binding-level only (`let mut`). The whole struct is mutable or immutable — no per-field `mut` modifiers. Interior mutability via `Rc<T>`/`Arc<T>` when needed. | [RFC 0008](https://github.com/pacolang/rfcs/blob/main/text/0008-struct-mutability.md) |
| Syntax macros | `comptime` is the sole metaprogramming mechanism. No syntax macros at this stage. Decision revisited once traits and dispatch are further along, if practical gaps emerge. | [RFC 0009](https://github.com/pacolang/rfcs/blob/main/text/0009-no-syntax-macros.md) |
| String slicing | No `s[n..m]` on strings. Use `s.get(0..n) -> Option<&string>` for UTF-8-safe slicing; `s.as_bytes()[0..n]` for raw bytes. No implicit panic. | [RFC 0010](https://github.com/pacolang/rfcs/blob/main/text/0010-string-slicing.md) |
| Data analysis | Standard library built on `comptime` + traits. `DataFrame<Schema>` and `Matrix<T>` are library types — not compiler-known. Language core provides only the mechanisms (traits, `comptime`, `#[repr]`). Narrowed by RFC 0030: "library" means an official library in its own `pacolang` repository, and `Matrix`/`DataFrame` are marked for extraction out of `stdlib`. | [RFC 0011](https://github.com/pacolang/rfcs/blob/main/text/0011-data-analysis-stdlib.md) |
| **Golden rule** | Building AI systems end to end breaks ties between conflicting designs. **No Python interoperation** — the interop boundary is the C ABI; the framework layer is rewritten in Paco. Supersedes the "concurrent services" golden rule. | [RFC 0013](https://github.com/pacolang/rfcs/blob/main/text/0013-ai-systems-north-star.md) |
| **Positioning** | General purpose: services, CLIs, games, critical flows, compilers and AI systems share one foundation. A foundation gap is closed whatever domain exposes it. | [RFC 0028](https://github.com/pacolang/rfcs/blob/main/text/0028-general-purpose-ai-tie-breaker.md) |
| Reduced-precision floats | `f16` and `bf16` are full arithmetic types; `f8e4m3`/`f8e5m2` are storage and interchange only. No implicit conversion between float types, in either direction. | [RFC 0014](https://github.com/pacolang/rfcs/blob/main/text/0014-reduced-precision-floats.md) |
| Modules | A directory is a module; every file opens with `module <name>`. Imports bind whole modules — no selective or wildcard import; call sites stay qualified. | [RFC 0015](https://github.com/pacolang/rfcs/blob/main/text/0015-modules-and-visibility.md) |
| Visibility | Private by default, `pub` exports. `pub` on a type exports the type only — fields and methods need their own `pub`. Not Go's capitalisation rule. | [RFC 0015](https://github.com/pacolang/rfcs/blob/main/text/0015-modules-and-visibility.md) |
| Constants | `const` with a mandatory type annotation, evaluated at compile time, no address. **No mutable global state** — no `static mut`, no module-level `var`. | [RFC 0016](https://github.com/pacolang/rfcs/blob/main/text/0016-constants-and-global-state.md) |
| FFI | `extern "C"` blocks (implicitly `unsafe fn`), `unsafe { }` expression blocks, raw pointers `*const T`/`*mut T`, `#[repr(C)]` layout. Exporting via `pub extern "C" fn`. | [RFC 0017](https://github.com/pacolang/rfcs/blob/main/text/0017-ffi-and-unsafe.md) |
| Autodiff | Native reverse mode over MIR after type and borrow checking; the same gradients under `paco run`, Cranelift and LLVM; custom derivatives with `#[derivative(of = f)]`. Supersedes Enzyme (RFC 0024) and LLVM routing (RFC 0025). | [RFC 0026](https://github.com/pacolang/rfcs/blob/main/text/0026-native-autodiff.md) |
| Execution | `paco run` compiles with Cranelift (debug) and runs the binary — one semantics with `paco build`; content-addressed build cache (`PACO_CACHE`, `paco clean --cache`); `comptime` runs on a sandboxed MIR interpreter sharing the runtime's arithmetic and formatting; compiled panics report `file:line:column`, with a stack trace in debug. The AST interpreter is removed. | [RFC 0027](https://github.com/pacolang/rfcs/blob/main/text/0027-run-via-codegen.md) |
| Ecosystem | One `pacolang` organization, one core repository (`paco`: compiler, runtime, `stdlib`, one version) and one repository per official library, each with its own semver and a compiler-version range in its `paco.mod`. `stdlib` admission needs one of four checkable reasons (`docs/ecosystem.md`); the compiler and runtime never depend on or name a library type. | [RFC 0030](https://github.com/pacolang/rfcs/blob/main/text/0030-repository-organization-and-stdlib-scope.md) |

## Open Questions

> Never empty. Settled decisions are in the table above and in
> `https://github.com/pacolang/rfcs`. Full discussion in `docs/design/spec.md` §20.

| # | Question | Blocking | Status |
|---|----------|----------|--------|
| 1 | **Immutable `static`** for large tables needing a stable address. | — | Deferred until the first real table that needs it exists |
| 2 | **Backend parity testing** — the conformance suite must run under both backends *and* both overflow modes. | The optimizing backend work | Open — uncontentious, just unwritten |
| 3 | **Is Paco actually easy for a model to write?** No measurement exists. | Every ergonomic claim | A benchmark for this is planned tooling work |
