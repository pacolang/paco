# Data Analysis: Standard Library Built on `comptime`, Not Language Core

* Status: accepted
* Deciders: Core Developers
* Date: 2026-05-31

## Context and Problem Statement

Paco targets "good for computation" as one of its goals alongside concurrent
services, games, and general-purpose development. This raises the question of
how deep the language commitment to data analysis should go: should types like
`DataFrame` or `Matrix` live in the language core (as first-class syntax or
compiler-known types), or should they be implemented in a standard library that
uses the general mechanisms the language already provides?

## Decision Drivers

* **Precedent**: both major languages dominant in their respective domains
  (C++ for systems/performance, Python for data science) reached the same
  conclusion independently — the language provides mechanisms; libraries provide
  concrete types.
* **`comptime` is the right mechanism**: Paco's compile-time execution is
  directly analogous to C++ expression templates (used by Eigen) and can generate
  specialized, zero-overhead computation kernels from type structure. No special
  compiler knowledge of "DataFrame" is needed.
* **Avoiding language bloat**: embedding domain-specific types in the language
  core creates privileged types that cannot be replaced or extended by users,
  contradicts the "uniform surface" principle, and bloats the compiler.
* **Phase discipline**: the compiler implementation roadmap (Phases 0–12) is
  already large. Adding data-analysis primitives to the language core would
  require compiler changes; adding them to the standard library requires only
  Paco code built on features already planned.

## Considered Options

* **Option A: Language core** — `DataFrame`, `Matrix`, and related types are
  compiler-known, with dedicated syntax (e.g., matrix literals, vectorized
  broadcast operators).
* **Option B: Standard library built on `comptime` + traits** — the language
  provides the mechanisms; `src/math/` and `src/collections/` in the standard
  library implement the concrete types using those mechanisms. No compiler change
  required beyond what is already planned.
* **Option C: External ecosystem only** — data-analysis types are left entirely
  to third-party libraries; the standard library provides no support.

## Decision Outcome

Chosen option: **Option B** — data analysis lives in the standard library,
built on mechanisms the language already provides. No language-core changes
are required.

### The mechanism → library split

| Layer | What Paco provides |
|-------|--------------------|
| **Language core** | Operator overloading via traits (`Add`, `Mul`, `Index`, `Iter`), monomorphized generics, `comptime` for code generation, `@repr` for memory layout control |
| **Standard library** (`src/math/`) | `Matrix<T>`, `Vec<T>` (numeric sense), `DataFrame<Schema>`, statistical functions |
| **Ecosystem** | Specialized libraries (sparse matrices, GPU offload, domain-specific formats) |

### The `comptime` parallel to C++ expression templates

In C++, Eigen builds zero-overhead matrix expressions by encoding the entire
computation tree as a template type at compile time. The expression `A + B * C`
never allocates intermediate matrices; the compiler fuses everything into a
single loop.

Paco's `comptime` achieves the same result in a more readable way:

```paco
// A comptime-parameterized DataFrame: schema is known at compile time.
// The compiler generates typed accessors and avoids runtime column lookup.
@derive(Schema)
struct LogRow {
    timestamp: i64,
    level:     string,
    message:   string,
}

let df = DataFrame<LogRow>::new()
df.push(LogRow { timestamp: 1_000, level: "INFO", message: "started" })

// Column access is type-checked and zero-overhead — no string lookup at runtime.
let ts: &[i64] = df.column(.timestamp)
```

### The Python `@` operator lesson

Python added the `@` matrix-multiplication operator (PEP 465, Python 3.5) to
the language core — not a `Matrix` type, just an operator hook (`__matmul__`).
This was the only language-level concession to the data ecosystem; everything
else (NumPy, Pandas, SciPy) is library code.

Paco can do the same if evidence emerges that a `MatMul` trait and a `@`
operator improve ergonomics. That is a small, targeted language change that does
not require compiler knowledge of any specific type.

### Consequences

* **Good (No bloat)**: the language core stays small and general. Any user can
  write a `Matrix` type that is indistinguishable from the standard library's.
* **Good (Phase-aligned)**: standard library work is Paco code; it can proceed in
  parallel with compiler development from Phase 2 onward without blocking the
  compiler team.
* **Good (Precedent-backed)**: mirrors the successful approach of C++ (Eigen,
  Arrow) and Python (NumPy, Pandas) — well-understood risks.
* **Good (`comptime` justification)**: gives the `comptime` feature a concrete,
  high-value use case that validates its design from the start.
* **Bad (Bootstrap gap)**: until the standard library is sufficiently built out,
  Paco will lack the data-analysis ergonomics it promises. This is a timeline
  risk, not a design risk.
* **Mitigation**: prioritize `src/math/` with `Matrix<T>` and basic linear
  algebra in Phase 12, and document the `comptime`-based extension pattern early
  so third-party libraries can fill gaps before the standard library does.

## Pros and Cons of the Options

### Option A: Language core

* Good: Deep integration; potential for syntax sugar (matrix literals, broadcast).
* Bad: Bloats the compiler; creates privileged types; costly to change if the API
  proves wrong; contradicts the uniform-surface principle.

### Option B: Standard library via `comptime` + traits

* Good: General, extensible, consistent with language principles; aligns with
  precedent from C++ and Python.
* Bad: Delayed availability; ergonomics depend on library maturity.

### Option C: External ecosystem only

* Good: Zero standard library scope.
* Bad: Fragments the ecosystem early; fails the "good for computation" promise;
  leaves `comptime` without a flagship use case.
