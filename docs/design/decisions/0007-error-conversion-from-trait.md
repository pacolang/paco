# Automatic Error Conversion: `From<T>` Trait and `?` Integration

* Status: accepted
* Deciders: Core Developers
* Date: 2026-05-26

## Context and Problem Statement

When propagating errors with the `?` operator in a function that returns
`Result<T, E>`, the error type produced by the called function frequently differs
from the `E` declared in the calling function's return type. Without an automatic
conversion mechanism, every such call requires an explicit adapter:

```paco
let text = read_file("config.toml").map_err(|e| AppError::Io(e))?
let cfg  = parse(text).map_err(|e| AppError::Parse(e))?
```

This pattern is mechanical, repetitive, and noisy in any function that calls into
multiple lower-level APIs, which is the common case for service and application
code. The language needs a principled solution that eliminates the boilerplate
without hiding the conversion from the programmer.

## Decision Drivers

* **Ergonomics**: Remove mechanical `.map_err(...)` boilerplate from the common
  case of propagating a lower-level error into a higher-level error type.
* **Explicit control**: The conversion must be authored by the programmer (as a
  `from` method), not silently synthesized by the compiler.
* **Consistency**: Paco already satisfies traits implicitly from method signatures
  (ADR 0002); the same principle should apply to error conversion.
* **Prelude availability**: The trait must be usable in every file without an
  explicit `use` statement.

## Considered Options

* **Option 1: No automatic conversion** — require `.map_err(|e| DstError::from(e))?`
  or equivalent at every call site.
* **Option 2: `From<T>` in the prelude, auto-called by `?`** — the `?` operator
  invokes `From::from(e)` automatically when source and target error types differ.
  The programmer opts in by implementing the `from` associated function.
* **Option 3: Compiler-synthesized conversions** — the compiler infers error
  wrappers automatically without any programmer-written conversion code.

## Decision Outcome

Chosen option: **Option 2**.

A `From<T>` trait lives in the prelude (available without a `use` statement):

```paco
trait From<Src> {
    fn from(e: Src) -> Self
}
```

When `?` is applied to a `Result<T, SrcError>` inside a function returning
`Result<U, DstError>` (where `SrcError ≠ DstError`), the compiler automatically
emits a call to `DstError::from(src_error)`. If no such implementation exists,
the call is a type error.

**Implicit trait satisfaction applies**: the compiler recognizes the
implementation from the `from` method signature alone, with no `implements`
clause required.

```paco
enum AppError {
    Io(IoError),
    Parse(ParseError),

    fn from(e: IoError) -> Self    { AppError::Io(e) }
    fn from(e: ParseError) -> Self { AppError::Parse(e) }
}

fn load() -> Result<Config, AppError> {
    let text = read_file("config.toml")?   // IoError → AppError::Io automatically
    let cfg  = parse(text)?                // ParseError → AppError::Parse automatically
    Ok(cfg)
}
```

No `.map_err(...)` is needed. If no `From` implementation covers the required
conversion, the compiler reports a type error at the `?` site.

### Consequences

* **Good (Ergonomics)**: Error propagation is concise; `?` alone is sufficient in
  the vast majority of cases across module boundaries.
* **Good (Explicit)**: Conversions are programmer-authored; the compiler generates
  nothing the programmer did not write.
* **Good (Consistent)**: Extends the existing implicit trait satisfaction model to
  error conversion, teaching one rule rather than two.
* **Bad (Method accumulation)**: An error enum that wraps many source types will
  accumulate many `from` methods inside its block; this is explicit but can make
  the block long.
* **Mitigation**: Idiomatic Paco defines a dedicated, narrow error enum per module
  rather than a single catch-all error type, keeping the number of `from` impls
  small per type.

## Pros and Cons of the Options

### Option 1: No automatic conversion

* Good: Maximally explicit; every conversion is visible at the call site.
* Bad: Produces repetitive, mechanical boilerplate on nearly every `?` call when
  error types cross module boundaries.

### Option 2: `From<T>` in prelude, auto-called by `?`

* Good: Eliminates boilerplate while keeping conversions explicitly authored.
* Good: Familiar pattern (mirrors Rust's `From`/`Into`); easy to teach.
* Bad: Introduces a compiler intrinsic coupling between `?` and `From<T>`.

### Option 3: Compiler-synthesized conversions

* Good: Maximum ergonomics with zero programmer-written conversion code.
* Bad: Violates "visible cost" — the compiler generates code the programmer never
  wrote, obscuring the type-conversion flow.
