# Paco — Functional Requirements and Implementation Roadmap (v0.1)

> Companion to the design document (`spec.md`). While the spec describes **what**
> the language is, this document describes **what needs to be built** to make it
> real, and **in what order**. This is an engineering document, not a design one.

---

## 0. How to read this document

- **§1–§2** define scope and the implementation language of the compiler itself.
- **§3** lists the functional requirements per compiler component.
- **§4** is the most important part: the **phased roadmap**, which turns a
  seemingly impossible project into a sequence of milestones that ship working software.
- **§5** is the risk register — where the genuinely hard parts are.
- **§6–§7** cover testing and dependencies.

Requirements use **MUST** (mandatory for the milestone), **SHOULD** (desirable),
and **MAY** (optional/future), RFC-style.

---

## 1. Scope and non-goals

### In scope (what the `paco` compiler must do)
- Compile `.paco` source to a single native binary.
- Full static checking: types, ownership, borrowing, match exhaustiveness.
- Dual backend: a fast dev backend (Cranelift) and an optimizing one (LLVM).
- Embedded runtime: M:N task scheduler, channels.
- Tools: `fmt`, `test`, `build`, `doc`.
- Dependency fetching: resolve `paco.mod` dependencies from their git URLs
  pinned to tags, with a lock file (§3.11).

### Out of scope (for now — do not build yet)
- IDE / language server (LSP) — desirable later, not in the core.
- Hot-reload, interactive REPL — they don't fit the compiled model.
- ~~C interop / FFI~~ — **no longer out of scope.** RFC 0013 makes the C ABI
  Paco's only interop boundary, and RFC 0017 specifies `extern`, `unsafe` and raw
  pointers. FFI is what makes BLAS, CUDA and NCCL reachable, so the language is
  useful before its library ecosystem exists. It needs a phase; it does not yet
  have one; see the roadmap (Phase 2b for declarations, Phase 7 for codegen).

> Keeping scope tight in the first milestones is what separates language projects
> that reach "it runs" from those that die of ambition.

---

## 2. Implementation language of the compiler

Reasons: Paco shares Rust's model (enums, pattern matching, ownership), so
compiler concepts map directly; the Rust ecosystem has the two key pieces ready —
the dev codegen backend (`cranelift-codegen`) and bindings for the optimizing
backend (`inkwell` for LLVM); and parsing tools (`logos` for the lexer,
`chumsky`/`lalrpop` for the parser) are mature.

Alternatives considered: OCaml (classic for compilers, great type system, but a
weaker codegen ecosystem) and Zig (interesting, but too young to bet the
foundation on). Rust is the lowest-risk choice.

> Bootstrap decision: the compiler starts written in Rust. **Self-hosting**
> (rewriting the Paco compiler in Paco) is a *long-term* goal, only viable once
> the language is mature. It is not a priority.

---

## 3. Functional requirements per component

The compiler is a pipeline. Each component has an input, an output, and a single
responsibility. The order below is the order a file flows through the compiler.

### 3.1 Lexer
- MUST turn UTF-8 source text into a sequence of tokens.
- MUST track each token's position (line, column, offset) for error messages.
- MUST recognize: literals (int, float, UTF-8 string, char, bool), identifiers,
  keywords (`let`, `mut`, `fn`, `struct`, `enum`, `trait`, `match`, `spawn`,
  `iter`, `comptime`, `use`...), operators, delimiters, lifetimes (`'a`).
- MUST handle line (`//`) and block (`/* */`) comments.
- SHOULD report lexical errors (invalid character, unterminated string) without
  stopping at the first — collect several to show at once.

### 3.2 Parser
- MUST build an AST (abstract syntax tree) from the tokens.
- MUST implement Paco's grammar (see the note on the formal grammar in §3.10).
- MUST perform **error recovery**: on an error, synchronize and continue to report
  multiple errors per compilation (not one at a time).
- MUST preserve spans (positions) on every AST node.
- The AST MUST distinguish, from the start: item declarations (fn, struct, enum,
  trait), expressions, patterns, and types.

### 3.3 Name resolution
- MUST resolve each identifier to its binding (variable, function, type, field).
- MUST implement lexical scoping rules (blocks, shadowing — `let x` redefining
  `x` in the same scope is allowed).
- MUST detect undeclared names and illegal duplicates.
- MUST build a per-scope symbol table.

### 3.4 Type checking and inference
- MUST infer types where not annotated (`let x = 10` -> `int`).
- MUST check types where annotated, reporting mismatches.
- MUST support generics with monomorphization (resolve `Vec<T>` for each `T` used).
- MUST check **implicit** trait satisfaction: a type satisfies `Trait` if it has
  all the required methods — with no `implements` declaration. This check is
  static (not runtime duck typing).
- MUST distinguish static dispatch (generics, zero cost) from dynamic (`dyn Trait`,
  vtable) and generate the right one.
- SHOULD use bidirectional inference (propagate the expected type down, infer up)
  — a good base for enums and closures.

### 3.5 Pattern-matching checking
- MUST check **exhaustiveness**: a `match` that doesn't cover every case is an error.
- MUST detect unreachable arms (a pattern already covered by an earlier one).
- MUST support: enum patterns with bindings, structs, tuples, ranges (`1..=9`),
  guards (`if cond`), `@` bindings, the `_` wildcard.

### 3.6 Ownership and borrow analysis — **the critical component**
- MUST track ownership: each value has one owner; moving invalidates the original.
- MUST detect use-after-move at compile time.
- MUST enforce the aliasing rule: either N shared `&` borrows, or one `&mut`.
- MUST implement **lifetime inference** that covers the common cases without
  annotation (this is the differentiator and the biggest risk — see §5).
- MUST require an explicit `'a` annotation only when inference is genuinely
  ambiguous, with a message suggesting the exact annotation.
- MUST insert deterministic cleanup (RAII): destroy values at scope end.
- MUST verify that channel-sent data is moved (concurrency safety).

### 3.7 `comptime` (compile-time execution)
- MUST execute code marked `comptime` during compilation.
- MUST allow type introspection (walking a struct's fields) for `#[derive]`.
- MUST allow code generation that enters the pipeline as if hand-written.
- Requires an **interpreter over the IR** (§3.8) for the comptime subset of the
  language. It is the compiler's only interpreter: programs themselves always
  run compiled (RFC 0027). Its arithmetic and formatting MUST match compiled
  code.

### 3.8 IR (intermediate representation) and lowering
- MUST lower the typed AST to a simpler, more explicit IR suited to codegen and
  backend-independent optimizations.
- The IR MUST make explicit: destructor calls, method dispatch, coercions.
- SHOULD be designed to serve both backends without duplicating lowering logic.

### 3.9 Codegen
- MUST have a **dev backend** (Cranelift) for `paco build` (fast compilation).
- MUST have an **optimizing backend** (LLVM) for `paco build --release`.
- MUST produce a **single static binary**, with the runtime embedded.
- SHOULD support cross-compilation (`--target`).
- Both backends MUST produce programs with identical semantics (only perf differs).
- `paco run` MUST compile through the dev backend and execute the binary, so
  it behaves exactly like the shipped program, and MUST reuse a
  content-addressed build cache so an unchanged program is not recompiled
  (RFC 0027).
- A panic in compiled code MUST report its message and Paco source location;
  debug builds MUST print a stack trace of Paco functions.
- Status: both backends are implemented. `paco build --release` compiles
  through LLVM 18 at `-O3`; every conformance program runs through both
  backends and both profiles, and their outputs are compared with each other
  and with its reviewed expected output.

### 3.10 Note on the formal grammar
- **Superseded.** This clause deferred the EBNF until the open syntax frictions
  (the position of `&` in borrows, collection syntax, error conversion) were
  resolved. ADRs 0006–0011 resolved them, and `docs/grammar/grammar.ebnf` now
  exists as the normative syntactic reference, alongside `docs/grammar/tokens.md`
  for the lexical one.
- The grammar is a **living** reference, not a frozen one: ADRs 0013–0017 
  extended it, and the open questions in spec §20 will extend it again. A change
  to it MUST be preceded by an ADR.

### 3.11 Modules and dependencies (decentralized)
- MUST resolve a dependency from the URL of its source repository pinned to a
  version-control tag (semantic versioning). There is no central registry.
- MUST read a manifest `paco.mod` and write a lock file pinning exact versions.
- MUST resolve in-code imports by their URL-like module path
  (`use example.com/team/json`), with `stdlib::` reserved for the standard library.
- Implemented for v0.1 by the `git-module-fetch` change.

---

## 4. Phased implementation roadmap

Principle: **always have something that runs.** Each phase ends with a working
compiler that simply covers more of the language. Never go months without running
anything.

### Phase 0 — Foundation (weeks)
- Set up the Rust project, CI, folder structure, test framework.
- Define the AST structure for a minimal subset.
- **Deliverable:** a skeleton that builds and runs empty tests.

### Phase 1 — Minimal executable core (weeks)
- Lexer + parser + name resolution for: `let`, `fn`, `int`/`float`/`bool`/`string`,
  arithmetic, `if`, function calls, `print`.
- **Tree-walking interpreter** (NOT codegen yet) to validate semantics quickly.
- **Deliverable:** run "hello world" and factorial. A huge psychological milestone
  — the language "exists".

### Phase 2 — Type system and data (months)
- Type checking and inference.
- `struct` with methods (confirmed syntax: methods inside the struct), `enum` with data.
- **Deliverable:** programs with custom, statically-checked types.

### Phase 2b — Language surface from RFC 0013–0017
- Modules (a directory is a module), `module` declarations, `pub` visibility.
- `const` declarations and type aliases; no mutable global state.
- Reduced-precision float types: `f16`, `bf16`, `f8e4m3`, `f8e5m2`.
- `extern` / `unsafe` / raw pointers — parsing and type checking only; codegen
  is Phase 7.
- **Deliverable:** the language surface the later phases build on. All of it is
  frontend work that is unblocked today and needs no codegen to be exercised.

### Phase 3 — Pattern matching (weeks–months)
- `match` with exhaustiveness, guards, bindings, ranges.
- `if let` / `while let`.
- **Deliverable:** the CSV example (parsing part) runs in the interpreter.

### Phase 4 — Ownership and move (months)
- Owner tracking, use-after-move detection, RAII.
- Still **no** borrowing — move semantics only.
- **Deliverable:** move errors detected; deterministic cleanup works.

### Phase 5 — Borrowing and lifetime inference (several months) [HIGHEST-RISK PHASE]
- `&`/`&mut` borrows, aliasing rule.
- Lifetime inference for the common cases.
- Explicit `'a` annotation for the ambiguous cases.
- **Deliverable:** the examples compile with guaranteed memory safety.
- **Risk:** see §5. This is where the schedule can blow up.

### Phase 6 — Traits and dispatch (months)
- Trait definition, implicit satisfaction, static checking.
- Static dispatch (monomorphization) and dynamic (`dyn`, vtable).
- Operators via traits (`Add`, `Index`...).
- **Deliverable:** implicit interfaces and operator overloading work.

### Phase 7 — Dev codegen (months)
- Lowering typed AST -> IR -> dev backend -> native binary.
- Drop the interpreter as the main path. (Later, RFC 0027: `paco run` itself
  compiles and executes, and `comptime` runs on its own MIR interpreter.)
- **Deliverable:** `paco build` produces a real native binary. Second huge milestone.

### Phase 8 — Concurrency (months)
- Runtime: M:N scheduler, growable stacks, automatic suspension on I/O.
- `spawn`, channels, `select`, per-task panic isolation (`handle.join()`).
- `iter` (synchronous generators).
- **Deliverable:** the HTTP server example runs concurrently.

### Phase 9 — comptime (months)
- comptime interpreter over MIR, type introspection, `#[derive]`.
- **Deliverable:** `#[derive(Display, Serialize)]` generates code automatically.

### Phase 10 — Optimizing backend and release (months)
- LLVM backend for `--release`, optimizations.
- Cross-compilation.
- **Deliverable:** optimized production binaries.

### Phase 11 — Tooling (parallel to the phases above when possible)
- `paco fmt` (canonical formatter — can start early, in Phase 2).
- `paco test` (runner with `#[test]`, `#[bench]`, `#[should_panic]`).
- `paco doc`.
- **Deliverable:** a complete developer experience.

### Phase 12 — Standard library (ongoing)
- `stdlib::core`, collections, I/O, formatting.
- `stdlib::blas` — safe wrappers over C ABI BLAS entry points (RFC 0017), so
  numeric code has a hardware-backed path before any Paco-native kernel exists.
- **Deliverable:** a language usable for real projects.

### Phase 13 — Numerics
- `Tensor<T, const D...>` with **compile-time shape checking**, built on the
  const generics from Phase 6. A shape mismatch is a type error at the call
  site, not a runtime failure forty minutes into a training run.
- A defined story for genuinely dynamic dimensions (batch size, sequence length).
- Reverse-mode autodiff as a compiler transformation over MIR (RFC 0026),
  available in both backends and therefore in `paco run`.
- **Deliverable:** numeric and ML code is writable without reimplementing the
  foundation. This is where RFC 0013's central bet — compile-time shapes — gets
  paid.

### Phase 14 — GPU: native kernel codegen
- A third backend emitting PTX and SPIR-V from MIR.
- Address spaces in the type system; how ownership and borrowing behave across a
  host/device boundary. **This is unsolved research and the largest single piece
  of work in the roadmap.**
- **Deliverable:** kernels written in Paco, not just called from it (RFC 0019).

> Calling CUDA and BLAS through the C ABI is available from Phase 7. Phase 14 is
> about *replacing* them, and it should not start until there is evidence that
> the FFI path is a real constraint rather than a theoretical one.

> Honest note on timeline: reaching Phase 7 (native binary) is already a
> many-month effort for one dedicated person. The complete Paco is years of work
> and/or a team. The roadmap guarantees you always have something demonstrable and
> growing, instead of a long gap with no return.

---

## 5. Risk register (where the hard parts are)

| Risk | Severity | Why | Mitigation |
|------|----------|-----|------------|
| Aggressive lifetime inference (Phase 5) | **High** | Near research-grade. Inferring "more than the established borrow checker" is ambitious. | Start with inference equivalent to the established one and loosen it. Accept asking for annotations in more cases early on. |
| Implicit trait satisfaction + static check (Phase 6) | Medium | Combining "implicit" with "statically checked" is uncommon; type coherence gets harder. | Study how structural interfaces and coherence are done; possibly restrict ambiguous cases. |
| M:N runtime with automatic suspension (Phase 8) | Medium-high | Suspending a task on I/O with no visible `async` requires deep integration with the runtime and OS I/O. | Model it on a mature lightweight-thread runtime (netpoller). A large body of work by itself. |
| Two backends (dev + optimizing) | Medium | Keeping semantic parity across two generators doubles the bug surface. | A well-designed common IR (§3.8) consumed by both. |
| comptime (Phase 9) | Medium | Executing the language at compile time needs a correct, safe interpreter. | Reuse the IR; limit what comptime can do at first. |
| Total scope | **High** | The sum of everything is enormous; risk of burnout/abandonment. | The phased roadmap is the mitigation. Celebrate every milestone that "runs". |

---

## 6. Testing and validation requirements
- There MUST be a **compiler test suite** from Phase 0, growing with each phase.
- There MUST be "must compile" and "must fail with error X" tests (diagnostic
  tests — fundamental for a compiler).
- There SHOULD be a set of **canonical example programs** (the server, the CSV,
  the game) that must compile and run at each relevant phase — living regression tests.
- There SHOULD be property/fuzz tests on the parser (random input must not crash
  the compiler, only report an error).
- Semantics MUST be identical across both backends and both profiles, and
  compile-time evaluation MUST agree with compiled code on the comptime
  subset; both are checked by differential tests. Because no second
  implementation runs whole programs (RFC 0027), every conformance program's
  expected output MUST be reviewed by hand.

---

## 7. Dependencies and suggested stack (if in Rust)
- **Lexer:** `logos` (fast, derive-based).
- **Parser:** `chumsky` (good error recovery) or hand-written (more control).
- **Diagnostics:** `ariadne` or `codespan-reporting` (pretty error messages with
  carets in the code — important for Paco's "ergonomics").
- **Dev codegen:** `cranelift-codegen`, `cranelift-module`.
- **Optimizing codegen:** `inkwell` (safe LLVM bindings).
- **Concurrency runtime:** hand-built, modeled on a mature lightweight-thread
  scheduler (no off-the-shelf shortcut fits Paco's model).

---

## 8. Further materials I can produce (on request)
1. **Formal lexical grammar** (tokens) — stable, can be done now.
2. **AST definition in Rust** — the structs/enums representing the tree, a great
   concrete starting point for Phase 0–1.
3. **Error-message specification** — what diagnostics should look like (part of
   the ergonomics).
4. **Formal syntactic grammar (EBNF)** — after the open syntax frictions are resolved.
5. **Canonical test-case document** — the programs that validate each phase.
