# Metaprogramming Boundary: `comptime` Only, No Syntax Macros

* Status: accepted
* Deciders: Core Developers
* Date: 2026-05-31

## Context and Problem Statement

`comptime` (ADR 0005) handles metaprogramming by running Paco code at compile
time: it inspects type structure, generates methods, and drives `@derive`.
However, `comptime` operates on *typed, resolved* language constructs — it cannot
transform arbitrary token streams or syntax before the parser runs.

Syntactic macros (e.g., Rust's `macro_rules!`, procedural macros, Lisp macros)
operate at the token/AST level, enabling embedded DSLs (SQL, HTML templates,
regexp literals compiled to automata) and utilities that capture the textual
representation of an expression (e.g., `assert_eq!(a, b)` reporting the source
text of `a` and `b` in its error message).

The question is whether Paco should support syntax macros in addition to
`comptime`, and if so, when.

## Decision Drivers

* **Language surface area**: syntax macros add a second, separate language that
  users must learn; they complicate parsing, tooling, and error messages.
* **"Low mental cost by default"**: one metaprogramming model is easier to teach
  than two.
* **Practical need**: the target use cases (concurrent services, games, data
  analysis) are well served by `comptime` + `@derive` for the majority of
  patterns. DSLs can be served by library-level parsers or external tooling.
* **Reversibility**: omitting syntax macros now and adding them later is possible.
  Adding them and then discovering they create tooling/hygiene problems is
  expensive to undo.
* **Phase discipline**: the language is still in the design and early
  implementation phase. Deferring complex features prevents scope creep.

## Considered Options

* **Option A: `comptime` only, no syntax macros** — `comptime` is the single
  metaprogramming mechanism. Embedded DSLs use string literals or library-level
  parsers. This decision is revisited after Phase 6 once practical gaps emerge.
* **Option B: `comptime` + declarative syntax macros** — add a `macro` keyword
  with pattern-based transformation (like Rust's `macro_rules!`).
* **Option C: `comptime` + procedural syntax macros** — add macros that receive
  a token stream or AST fragment and return transformed code.

## Decision Outcome

Chosen option: **Option A** — `comptime` is the sole metaprogramming mechanism.
No syntax macros are introduced at this stage.

This decision is explicitly **provisional**: it will be revisited after Phase 6
(traits and dispatch) once there is practical, accumulated evidence of what
`comptime` cannot cover in Paco's target use cases. Any future addition of
syntax macros will require a new ADR superseding this one.

### What `comptime` covers

* `@derive(Display, Serialize, Eq, Clone)` — generates methods from field structure.
* Compile-time validation and code generation parameterized by types.
* Operator overloading via special traits (`Add`, `Index`, `Display`, `Iter`...).
* Conditional compilation based on target or configuration values.

### What is deferred (intentionally not solved today)

* Embedded syntax DSLs (SQL, HTML templates, shader code).
* Capturing expression source text in diagnostics (e.g., rich `assert_eq`).
* Token-stream transformation before name resolution.

These patterns are not common in Paco's primary use cases. When they arise,
library-level solutions (runtime parsers, build-time code generators external to
the compiler) serve as interim alternatives.

### Consequences

* **Good (Small surface area)**: one metaprogramming model to learn, document,
  and implement. Formatting, IDE support, and error messages are simpler.
* **Good (Phase discipline)**: compiler implementation can focus on `comptime`
  without designing a second, interacting system.
* **Good (Reversibility)**: deferral is explicitly noted; the decision can be
  reopened with concrete evidence.
* **Bad (Capability gap)**: some patterns that syntax macros handle elegantly
  (rich assertions, embedded DSLs) require workarounds or external tools.
* **Mitigation**: Paco's prelude and standard library will provide idiomatic
  solutions for the most common cases (e.g., a structured `assert_eq` that
  produces useful output using language-level reflection via `comptime`).

## Pros and Cons of the Options

### Option A: `comptime` only

* Good: Single model, minimal surface area, phase-appropriate.
* Bad: Cannot transform syntax before parsing; embedded DSLs need workarounds.

### Option B: Declarative syntax macros (`macro_rules!` style)

* Good: Powerful pattern-matching over token trees; familiar to Rust programmers.
* Bad: Macro hygiene is complex; error messages inside macros are notoriously
  confusing; adds a second language-within-a-language.

### Option C: Procedural syntax macros

* Good: Maximum expressiveness; arbitrary token transformation.
* Bad: Requires a stable public AST/token API from the compiler; tightly couples
  library authors to compiler internals; heavy implementation burden.
