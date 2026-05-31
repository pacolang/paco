# Paco — Design Context

> This file tracks the current design state: settled decisions and open questions.
> Update it whenever a decision moves from open to settled, and keep it in sync
> with `docs/design/spec.md` (§17 and §18) and the ADRs.

## Settled Decisions

| Topic | Summary | ADR |
|-------|---------|-----|
| Memory | Ownership + move semantics, aggressive lifetime inference. `Rc`/`Arc` as explicit escape hatch. | [ADR 0001](docs/design/decisions/0001-memory-model.md) |
| Methods | Defined inside `struct`/`enum`. `methods T {}` only for external extensions. Explicit receivers (`self&`, `self&mut`, `self`). | [ADR 0002](docs/design/decisions/0002-struct-methods.md) |
| Backend | Cranelift for dev (`paco build`), LLVM for release (`paco build --release`). Common Paco IR. | [ADR 0003](docs/design/decisions/0003-compilation-backend.md) |
| Concurrency | M:N lightweight tasks (`spawn`), channels, automatic suspension, no `async`/`await`. `iter` for synchronous generators. Per-task panic isolation. | [ADR 0004](docs/design/decisions/0004-concurrency.md) |
| Metaprogramming | Special traits + `comptime`. No runtime metatables. | [ADR 0005](docs/design/decisions/0005-metaprogramming-and-packages.md) |
| Packages | Decentralized, URL + version-control tag, no central registry. Manifest `paco.mod`. Tooling deferred. | [ADR 0005](docs/design/decisions/0005-metaprogramming-and-packages.md) |
| Collection construction | `Vec::new()`, `Map::new()`, etc. Associated functions only. No shorthand literal syntax. | [ADR 0006](docs/design/decisions/0006-collection-construction.md) |
| Error conversion | `?` calls `From::from(e)` automatically when error types differ. `From<T>` lives in the prelude. Implicit trait satisfaction — no `implements` clause needed. | [ADR 0007](docs/design/decisions/0007-error-conversion-from-trait.md) |
| Struct mutability | Binding-level only (`let mut`). The whole struct is mutable or immutable — no per-field `mut` modifiers. Interior mutability via `Rc<T>`/`Arc<T>` when needed. | [ADR 0008](docs/design/decisions/0008-struct-mutability.md) |
| Syntax macros | `comptime` is the sole metaprogramming mechanism. No syntax macros at this stage. Decision revisited after Phase 6 if practical gaps emerge. | [ADR 0009](docs/design/decisions/0009-no-syntax-macros.md) |
| String slicing | No `s[n..m]` on strings. Use `s.get(0..n) -> Option<&string>` for UTF-8-safe slicing; `s.as_bytes()[0..n]` for raw bytes. No implicit panic. | [ADR 0010](docs/design/decisions/0010-string-slicing.md) |
| Data analysis | Standard library (`src/math/`) built on `comptime` + traits. `DataFrame<Schema>` and `Matrix<T>` are library types — not compiler-known. Language core provides only the mechanisms (traits, `comptime`, `@repr`). | [ADR 0011](docs/design/decisions/0011-data-analysis-stdlib.md) |

## Open Questions

There are no open design decisions. All foundational questions have been settled.
The next milestone is the EBNF grammar and Phase 0 of compiler implementation.
