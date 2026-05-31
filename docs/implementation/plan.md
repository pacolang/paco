# Paco — Complete Implementation Plan

> Companion to `requirements.md` (what to build) and `architecture.md` (how it is
> structured). This document is the **execution plan**: every phase broken down into
> concrete tasks, subagent assignments, parallelism maps, and exit criteria.
>
> Status: living plan. Update it as tasks complete and decisions get made.
> The normative references are `spec.md`, `architecture.md`, and the ADRs.

---

## 0. How to use this plan

### Agent roles

| Role | Responsibility in this project |
|------|-------------------------------|
| `development` | Implements Rust code in `compiler/` crates and the runtime in `runtime/`. |
| `tests` | Writes test files: `.paco` golden tests (compile-pass and compile-fail), run-output tests, unit tests inside crates. |
| `review` | Reviews for spec compliance, Rust safety, ADR consistency, and correctness of diagnostics wording. |
| `devops` | Sets up and maintains the Cargo workspace, CI pipelines, cross-platform builds, toolchain pinning, and release packaging. |
| `development-stdlib` | Writes standard-library code in Paco (not Rust). Starts in Phase 12 but can stub trait definitions from Phase 6. |

A single session may cover one task from one role. Tasks marked **parallel** in the
same phase can be delegated simultaneously — they touch different crates or files.

### Conventions

- **Task IDs**: `P{phase}.{role-prefix}.{seq}` — e.g., `P1.dev.3` is the third
  development task of Phase 1.
- **Entry criteria** must be met before the phase starts. Do not skip them.
- **Exit criteria** are the definition of "done" for the phase. A phase is not
  done until every criterion is green.
- **Risk** lines flag where the schedule is most likely to blow up.

### Inter-phase dependency graph

```
Phase 0  ──► Phase 1  ──► Phase 2  ──► Phase 3
                                │
                                ▼
                          Phase 4  ──► Phase 5  ──► Phase 6  ──► Phase 7
                                                                    │
                                              ┌─────────────────────┤
                                              ▼                     ▼
                                          Phase 8              Phase 10
                                              │
                                              ▼
                                          Phase 9

Phase 11 (Tooling) runs in parallel starting at Phase 2.
Phase 12 (Stdlib) runs in parallel starting at Phase 7.
```

---

## Phase 0 — Foundation

**Goal:** a Rust workspace that compiles, has all crates stubbed with correct
dependency edges, and has a working test harness. Nothing runs yet; the skeleton
is set.

**Duration estimate:** 1–2 weeks.

### Entry criteria

- `spec.md`, `architecture.md`, `grammar.ebnf`, and `tokens.md` are finalized.

### Crates touched (created as stubs)

All 15 crates from `architecture.md §4`:
`paco-span`, `paco-diag`, `paco-syntax`, `paco-hir`, `paco-resolve`,
`paco-types`, `paco-match`, `paco-borrow`, `paco-mir`, `paco-eval`,
`paco-codegen-cranelift`, `paco-codegen-llvm`, `paco-link`, `paco-driver`,
`paco-test-harness`.

### Tasks

#### devops (P0.ops.*)

**P0.ops.1** — Create `compiler/Cargo.toml` workspace manifest listing all 15 crates.
Pin Rust edition to 2024. Pin toolchain in `rust-toolchain.toml`.

**P0.ops.2** — Add GitHub Actions CI workflow (`.github/workflows/ci.yml`) that
runs on push and PR to `main`:
- `cargo check --workspace`
- `cargo clippy --workspace -- -D warnings`
- `cargo test --workspace`
- Runs on: `ubuntu-latest`, `macos-latest`, `windows-latest`.

**P0.ops.3** — Add a `Makefile` (or `justfile`) with convenience targets:
`build`, `test`, `check`, `clippy`, `clean`.

**P0.ops.4** — Configure `.gitignore` for Rust targets and editor artifacts.

#### development (P0.dev.*)

These tasks can run in parallel with P0.ops.*.

**P0.dev.1** — Create the 15 crate stubs (`cargo new --lib`) with empty `lib.rs`
and the dependency edges exactly as in `architecture.md §4`. No crate may
depend on a crate below it in the layering (layered-dependencies rule).

**P0.dev.2** — In `paco-span`: define `FileId`, `Span` (byte range), `SourceMap`
(maps FileId → file text, Span → (line, col)). These types are shared by every
other crate.

**P0.dev.3** — In `paco-diag`: define `Diagnostic`, `Severity`, `Label`,
`Suggestion`, `Reporter`. Wire to `ariadne` (or `codespan-reporting`) for
rendering. Reporter must collect diagnostics without printing; it prints only
when the caller calls `reporter.emit()`.

**P0.dev.4** — In `paco-syntax::ast`: define the full AST node hierarchy for
the Paco grammar (`grammar.ebnf`). Four top-level categories (from
`architecture.md §7`):
- **Items**: `FnDecl`, `StructDecl`, `EnumDecl`, `TraitDecl`, `MethodsBlock`, `UseDecl`.
- **Expressions** (`Expr`): one variant per expression form in the grammar.
- **Patterns** (`Pat`): one variant per pattern form.
- **Types** (`Ty`): one variant per type form.

Every node must carry a `Span`. No semantic information yet — strings for
identifiers, not `DefId`.

**P0.dev.5** — In `paco-syntax::ast`: add the immutable `Visit` and mutable
`MutVisit` visitor traits generated by a small macro (see `architecture.md §7`).

**P0.dev.6** — In `paco-driver`: create the `main.rs` entry point with CLI
argument parsing (`clap`). Support subcommands: `build`, `check`, `run`,
`test`, `fmt`, `doc`, `clean`. All subcommands may panic with "not implemented"
for now.

#### tests (P0.tst.*)

Parallel with P0.dev.*.

**P0.tst.1** — In `paco-test-harness`: implement the golden test infrastructure.
A golden test is a directory containing:
- `input.paco` — the source file.
- `expected.stderr` (for fail tests) or `expected.stdout` (for run-output tests).
- `flags.toml` — `kind = "fail"` or `kind = "run"`, optional `phase_min`.

The harness invokes the compiler binary and diffs actual output against expected.
Tests with `phase_min > current_phase` are skipped automatically.

**P0.tst.2** — Create `tests/conformance/` directory structure with one
subdirectory per phase (`core/`, `phase_02/`, ...).

**P0.tst.3** — Add a `cargo test` integration that discovers and runs all golden
tests in `tests/conformance/`.

### Exit criteria

- [ ] `cargo build --workspace` passes on Linux, macOS, and Windows.
- [ ] `cargo test --workspace` runs (0 failures, N tests skipped).
- [ ] `cargo clippy --workspace -- -D warnings` passes.
- [ ] All 15 crates exist with the correct dependency edges (no cycles).
- [ ] `paco-span`, `paco-diag` have implemented types (not stubs).
- [ ] `paco-syntax::ast` has a complete node hierarchy matching `grammar.ebnf`.
- [ ] Golden test harness can discover `.paco` tests and report `SKIP` (no tests
      in scope yet).
- [ ] CI runs on all three platforms.

---

## Phase 1 — Minimal executable core

**Goal:** `paco run hello.paco` prints "Hello, world!". Recursive factorial runs.
The execution path is a tree-walking interpreter (no codegen).

**Duration estimate:** 2–4 weeks.

### Language subset for this phase

```
Items:      fn declarations (no generics, no traits)
Types:      int, float, bool, string (primitives only)
Exprs:      integer/float/string/bool literals, identifiers,
            arithmetic (+, -, *, /, %), comparison (==, !=, <, <=, >, >=),
            logical (&&, ||, !), if/else, loop, while, block, return,
            function calls (direct, non-method), print (builtin)
Patterns:   none yet (no match)
```

### Crates touched

`paco-syntax` (lex + parse), `paco-hir` (minimal), `paco-resolve` (minimal),
`paco-types` (minimal — only primitives), `paco-mir` (minimal — only for eval),
`paco-eval`, `paco-driver` (interpreter mode), `paco-diag`.

### Tasks

#### development — lexer (P1.dev-lex.*)

**P1.dev-lex.1** — In `paco-syntax::lex`: implement a `logos`-derived lexer
covering all token categories in `docs/grammar/tokens.md`:
- All keywords (exact list from tokens.md).
- Identifiers: `[A-Za-z_][A-Za-z0-9_]*`.
- Integer literals: decimal, hex (`0x…`), binary (`0b…`), with `_` separators.
- Float literals: `3.14`, `1.0e9`, `2.5e-3`.
- String literals with escape sequences (`\n`, `\t`, `\r`, `\"`, `\\`).
- Char literals: `'a'`, `'\n'`.
- All operators and delimiters from tokens.md.
- Line comments (`//`) and block comments (`/* */`) — skipped as trivia but
  retained in a parallel trivia stream for `paco fmt` (future).
- Doc comments (`///`) — tokenized as their own kind for `paco doc`.
- Lifetime tokens: `'` immediately followed by identifier.
- `&mut` — lexed as a single compound token (tokens.md §Compound tokens).

**P1.dev-lex.2** — Each token must carry a `Span`. Lexical errors
(invalid character, unterminated string) emit a diagnostic via `Reporter` and
produce an `Error` token — lexing never aborts.

**P1.dev-lex.3** — Resolve the `'a` vs `'a'` ambiguity: peek past the
apostrophe; a single character followed by `'` is a char literal, otherwise
a lifetime token (see `architecture.md §5`).

#### development — parser (P1.dev-par.*)

Starts after P1.dev-lex.1 is done (needs the token type).

**P1.dev-par.1** — In `paco-syntax::parse`: implement a recursive-descent
parser for the Phase 1 language subset. Produce an AST rooted at `Module`.

Key grammar rules to implement now (from `grammar.ebnf`):
- `Program`, `Item`, `FnDecl` (no generics, no `where`).
- `Block`, `Stmt`, `LetStmt`.
- `Expr` down to literals and identifiers.
- `IfExpr`, `LoopExpr`, `WhileExpr`.
- `CallExpr` (direct function calls).
- All arithmetic and comparison binary operators with correct precedence.

**P1.dev-par.2** — Error recovery: at item boundaries (`fn`), synchronize by
skipping to the next `fn` keyword at the same brace depth. Inside a function,
resync at newlines following a complete expression or at `}`.

**P1.dev-par.3** — Every AST node produced by the parser must carry a `Span`
from the token stream.

#### development — interpreter (P1.dev-eval.*)

Can start in parallel with P1.dev-par.* using stub AST nodes as input contract.

**P1.dev-eval.1** — In `paco-resolve` (minimal): implement a simple symbol
table (a `HashMap<String, BindingId>`) for function-level name resolution.
Detect undeclared names and function-not-found errors.

**P1.dev-eval.2** — In `paco-types` (minimal): implement type inference for
primitive types only. Infer `int`, `float`, `bool`, `string` from literals.
Report mismatches (`int + string` → error).

**P1.dev-eval.3** — In `paco-eval`: implement the tree-walking interpreter.
Walk the AST directly (not MIR yet — MIR lowering comes later). Implement:
- Variable environments (stack of scopes).
- Arithmetic, comparison, logical evaluation.
- `if/else` branching.
- `loop` / `while` with `break`/`continue`.
- Function calls (push frame, evaluate body, return value).
- `print` as a builtin.

**P1.dev-eval.4** — In `paco-driver`: wire up `paco run <file>` to invoke
lexer → parser → resolver → type checker → interpreter. Print diagnostics at
the end of each phase if any errors were emitted.

#### tests (P1.tst.*)

Parallel with development tasks.

**P1.tst.1** — Write `tests/conformance/core/hello_world/`:
```paco
fn main() {
    print("Hello, world!")
}
```
Expected stdout: `Hello, world!\n`.

**P1.tst.2** — Write `tests/conformance/core/factorial/` with a recursive
factorial function. Expected: `factorial(10) = 3628800`.

**P1.tst.3** — Write `tests/conformance/core/arithmetic/` covering all
arithmetic and comparison operators.

**P1.tst.4** — Write `tests/conformance/core/type_error_add_string_int/`
(must-fail). Expected diagnostic: type mismatch error at `+` site with the
source span highlighted.

**P1.tst.5** — Write `tests/conformance/core/undeclared_name/` (must-fail).
Expected diagnostic: name not found.

#### review (P1.rev.*)

After development tasks complete.

**P1.rev.1** — Review the lexer against `docs/grammar/tokens.md`. Every token
category must be covered. Check edge cases: `0b` and `0x` prefixes, `_`
separators in numbers, `///` vs `//` vs `/*`.

**P1.rev.2** — Review the parser against `grammar.ebnf` for the Phase 1 subset.
Operator precedence must match the grammar's layered rules.

**P1.rev.3** — Review diagnostic messages for clarity. Each error must include
the span (line + column highlighted) and a human-readable message. Run the
must-fail golden tests and inspect the output.

### Parallelism map

```
P1.dev-lex.1-3  ──►  P1.dev-par.1-3  ──►  P1.dev-eval.3-4
P1.dev-eval.1-2 ────────────────────────────────────────────► (same target)
P1.tst.1-5      (written any time; run when P1.dev-eval.4 is done)
P1.rev.1-3      (after P1.dev.* complete)
```

### Exit criteria

- [x] `paco run hello.paco` prints `Hello, world!`.
- [x] Recursive `factorial(10)` returns `3628800`.
- [x] All Phase 1 golden tests pass.
- [x] Type mismatch between `int` and `string` at `+` produces a diagnostic
      with a span pointing at the operator.
- [x] Undeclared name produces a diagnostic with a span.
- [ ] Lexer handles all tokens in `tokens.md` without panicking on valid input.
- [ ] Parser never panics on any input (fuzz-test invariant).

### Risk

Low. Tree-walking interpreters for a subset this small are well-understood.
The only subtlety is operator precedence in the recursive-descent parser.

---

## Phase 2 — Type system and data

**Goal:** programs with custom, statically-checked types run through the interpreter.

**Duration estimate:** 2–3 months.

### New language features

- `struct T { field: Type, ... fn method(self&, ...) -> T { ... } }`
- `enum E { Variant, Variant(T), Variant { field: T } }`
- `methods T { ... }` blocks (external extension)
- Generics: `fn f<T>(v: T)`, `struct S<T> { ... }` (with bounds deferred to Phase 6)
- Type inference: full Hindley–Milner bidirectional (synthesis + checking)
- `Vec<T>::new()`, `Option<T>`, `Result<T, E>` as built-in generic types
- Method calls: `x.method(arg)`
- Associated functions: `T::new()`
- `impl`-free satisfaction tracking: methods on a type are collected from its block

### Crates touched

`paco-syntax` (parser extension), `paco-hir` (full lowering), `paco-resolve`
(full), `paco-types` (HM inference + generics).

### Tasks

#### development — HIR lowering (P2.dev-hir.*)

**P2.dev-hir.1** — Define the full HIR node set in `paco-hir`. Differences from
AST (see `architecture.md §8`):
- Names replaced by `DefId` (items) and `BindingId` (locals).
- Method receivers (`self&`, `self&mut`, `self`) become explicit parameters with
  known types.
- Associated functions distinguished from methods at the HIR level.

**P2.dev-hir.2** — Implement AST → HIR lowering for `struct`, `enum`, and
`methods` items: field collection, method collection, receiver normalization.

**P2.dev-hir.3** — Implement HIR lowering for method calls and associated
function calls. At this stage, resolve the callee to a `DefId` using the
type of the receiver (which comes from the type checker).

**P2.dev-hir.4** — Desugar `if let` and `while let` into `match` in HIR
(even though match exhaustiveness comes in Phase 3, the HIR form must exist).

#### development — name resolution (P2.dev-res.*)

Parallel with P2.dev-hir.*.

**P2.dev-res.1** — In `paco-resolve`: build the item table. A map
`DefId → Item` covering all `fn`, `struct`, `enum`, and their methods, across
all modules loaded in the compilation unit.

**P2.dev-res.2** — Implement per-scope symbol tables for locals. Shadowing
within a scope is allowed (spec §2): each `let` produces a fresh `BindingId`.

**P2.dev-res.3** — Implement path resolution for `T::method` and `T::new`.
At this stage, paths are module-local only (no external imports yet).

#### development — type checker (P2.dev-types.*)

Parallel with P2.dev-hir.*; depends on the HIR type definition.

**P2.dev-types.1** — In `paco-types`: define the `Ty` enum
(`architecture.md §10.1`): primitives, tuple, slice, borrow, `Rc<T>`, `Arc<T>`,
function type, `dyn` (stub), and generic application (`Vec<T>`, `Result<T, E>`).
Intern all types in a per-compilation arena.

**P2.dev-types.2** — Implement bidirectional Hindley–Milner inference
(`architecture.md §10.2`). Two modes:
- **Synthesis** (bottom-up): walk an expression and produce its type.
- **Checking** (top-down): walk an expression with an expected type and propagate
  it down. This is needed for enum constructors (`Ok(x)`) and closures.
Unresolved type variables at the end of a function body are an error.

**P2.dev-types.3** — Implement type checking for `struct` field access, method
dispatch (look up the method on the receiver's type in the item table), and
associated function calls.

**P2.dev-types.4** — Implement generic type substitution for the built-in generic
types (`Vec<T>`, `Option<T>`, `Result<T, E>`). User-defined generics may be
restricted to concrete types for now (full generics follow in Phase 6).

#### development — parser extension (P2.dev-par.*)

**P2.dev-par.1** — Extend `paco-syntax::parse` to handle `struct`, `enum`,
`methods`, and `use` items. Generics in angle brackets (may conflict with `<`
operator — see `grammar.ebnf §8` ambiguity table; resolve by greedy parse of
angle-bracket depth).

**P2.dev-par.2** — Parse method calls (postfix `.method(args)`) and associated
function calls (`T::fn(args)`).

#### tests (P2.tst.*)

**P2.tst.1** — Struct with methods: create, access fields, call methods.

**P2.tst.2** — Enum with data variants: create variants, use in if/match
(basic pattern not needed yet — match is Phase 3, but construction and moving must work).

**P2.tst.3** — Generic `Option<int>` and `Result<int, string>` basic use.

**P2.tst.4** — Type mismatch calling a method with wrong argument type (must-fail).

**P2.tst.5** — Field not found on struct (must-fail with span on the field name).

**P2.tst.6** — Method not found on type (must-fail with span).

#### review (P2.rev.*)

**P2.rev.1** — Verify HIR lowering against `spec.md §2` (methods inside struct)
and `ADR 0002`. In particular: methods defined inside `struct` vs `methods T {}`
blocks must both produce the same method table entry.

**P2.rev.2** — Verify type inference against `spec.md §3`. The inference must
handle the `let squares = (1..=5).map(|n| n * n).collect<Vec<int>>()` pattern
(closure type is inferred from context).

**P2.rev.3** — Open architectural question resolution (needed before end of
Phase 2): decide **HIR storage strategy** — arena (`&'hir` references) vs
`Vec`-indexed IDs (`architecture.md §22.1`). Record the decision in a new
internal ADR.

#### devops (P2.ops.*)

**P2.ops.1** — Add `paco fmt` skeleton (can format Phase 1 programs only) to
`paco-driver`. The formatter needs the lexer's trivia stream (whitespace +
comments). Start the formatter now so it grows with the language.
See `architecture.md §22.6` (formatter host decision — decide now).

### Parallelism map

```
P2.dev-hir.*    ─┐
P2.dev-res.*    ─┤─► P2.dev-types.*  ─► P2.rev.*
P2.dev-par.*    ─┘
P2.tst.*        (any time; run when P2.dev-types.* is done)
P2.ops.1        (parallel, independent)
```

### Exit criteria

- [ ] Programs with `struct`, `enum`, methods compile and run in the interpreter.
- [ ] Type inference infers field types without annotation.
- [ ] Method call dispatches to the correct function.
- [ ] `Option::Some(42)` and `Result::Ok(x)` / `Result::Err(e)` types are
      statically correct.
- [ ] All Phase 2 golden tests pass.
- [ ] HIR storage strategy is decided and documented.
- [ ] `paco fmt` formats Phase 1 programs without corruption (idempotent).

### Risk

**Medium.** Bidirectional HM inference is well-understood but requires careful
implementation. The interaction between inference and generic types (`Vec<T>`) is
where most bugs will appear. Restrict to concrete instantiations early if needed.

---

## Phase 3 — Pattern matching

**Goal:** `match` with exhaustiveness checking, guards, `@` bindings, `if let`.

**Duration estimate:** 3–6 weeks.

### New language features

- `match expr { Pat => expr, ... }` (all pattern forms from `grammar.ebnf §6`)
- Exhaustiveness and reachability checks (Maranget's algorithm)
- Guards: `Pat if cond => expr`
- `if let Pat = expr { ... }`
- `while let Pat = expr { ... }`
- `for x in xs { ... }` (desugared to iterator protocol)

### Crates touched

`paco-match` (new implementation), `paco-hir` (desugaring), `paco-syntax` (parser).

### Tasks

#### development — exhaustiveness (P3.dev-match.*)

**P3.dev-match.1** — In `paco-match`: implement the pattern matrix data structure
(rows of patterns per match arm, one column per scrutinee component).

**P3.dev-match.2** — Implement Maranget's usefulness algorithm:
- For each candidate row: is it *useful* (does any value reach it)?
  If no → unreachable arm diagnostic.
- After all rows: is the matrix *exhaustive*? If no → non-exhaustive diagnostic
  with a *witness* value that escapes (e.g., `Shape::Triangle` in a match that
  only covers `Circle` and `Rectangle`).

**P3.dev-match.3** — Support all pattern forms: wildcards (`_`), literals,
ranges (`1..=9`), paths (enum variants and unit structs), tuple patterns,
struct patterns (`Point { x, y }`), slice patterns (`[a, b, ..]`), `@` bindings,
and reference patterns (`&pat`). Guards are treated as opaque (a guarded arm never
proves exhaustiveness on its own).

#### development — desugaring (P3.dev-desugar.*)

Parallel with P3.dev-match.*.

**P3.dev-desugar.1** — In `paco-hir`: lower `if let Pat = expr { .. } else { .. }`
to a `match` expression with two arms (`Pat => ..., _ => ...`).

**P3.dev-desugar.2** — Lower `while let Pat = expr { body }` to a `loop` with an
inner `match` and a `break` on the non-matching arm.

**P3.dev-desugar.3** — Lower `for x in xs { body }` to:
```
let mut iter = xs.into_iter()
loop {
    match iter.next() {
        Some(x) => body,
        None    => break,
    }
}
```
This requires the `IntoIter` and `Iterator` trait shapes to be known to the
desugarer (even before full trait checking in Phase 6).

#### tests (P3.tst.*)

**P3.tst.1** — A `match` over a 3-variant enum covering all variants.

**P3.tst.2** — Non-exhaustive `match` missing one variant (must-fail with witness).

**P3.tst.3** — Unreachable arm after a wildcard (must-fail, warning or error).

**P3.tst.4** — `match` with guard (`if x > 0`).

**P3.tst.5** — `@` binding: `n @ 1..=9 => ...`.

**P3.tst.6** — `if let Some(x) = opt { ... }`.

**P3.tst.7** — `for x in vec { print(x) }` (iterator desugaring).

**P3.tst.8** — CSV parser example from the spec (parse a line into fields using
`match` and string iteration).

### Exit criteria

- [ ] All pattern forms from `grammar.ebnf §6` are parsed and type-checked.
- [ ] Non-exhaustive `match` is a compile error with a witness value.
- [ ] Unreachable arms produce diagnostics.
- [ ] `if let` / `while let` / `for` desugar correctly.
- [ ] All Phase 3 golden tests pass.

---

## Phase 4 — Ownership and move

**Goal:** move semantics enforced. Use-after-move is a compile error. RAII
(deterministic cleanup via `Drop`) runs at scope end. No borrowing yet.

**Duration estimate:** 1–2 months.

### New language features

- Move semantics: assigning or passing a value transfers ownership.
- Use-after-move: compile error when a moved value is used.
- RAII: the compiler inserts `Drop` calls at scope end via MIR.
- Copy types: primitive types (`int`, `float`, `bool`, `char`, `byte`) are
  implicitly copied rather than moved.

### Crates touched

`paco-borrow` (sub-phase 12.1 — ownership tracking),
`paco-mir` (Drop terminator and explicit destructor calls).

### Tasks

#### development — ownership tracking (P4.dev-borrow.*)

**P4.dev-borrow.1** — In `paco-borrow`: implement definite-assignment dataflow
over the HIR (`architecture.md §12.1`). Each binding is one of:
`Initialized | MovedOut | PartiallyMoved`.

**P4.dev-borrow.2** — Propagate move state across control flow. In a branch
(`if/else`, `match`), a binding must be consistently moved-out in all branches
or in none. If a binding is moved in some branches but not others, error.

**P4.dev-borrow.3** — Emit `use-after-move` diagnostics (lint name from
`AGENTS.md §5`) with a span pointing at the use site and a note pointing at the
move site.

**P4.dev-borrow.4** — Mark primitive types as `Copy`. Copy types do not emit
`use-after-move` errors.

#### development — MIR Drop (P4.dev-mir.*)

Parallel with P4.dev-borrow.*.

**P4.dev-mir.1** — In `paco-mir`: add a `Drop(local)` terminator (see
`architecture.md §13.2`, table row "End-of-scope cleanup").

**P4.dev-mir.2** — In the HIR → MIR lowering: insert `Drop` terminators at
scope exits. A scope exit is: reaching `}` of a block, a `return`, or a
`break`/`continue` that leaves the scope. Drop order is reverse declaration
order within a scope.

**P4.dev-mir.3** — In `paco-eval` (the tree-walking interpreter): execute `Drop`
by calling the type's `drop` method if it exists (or a no-op for types without
one). This validates RAII semantics before codegen.

#### tests (P4.tst.*)

**P4.tst.1** — Moving a value into a function: the original binding is
invalid after the call.

**P4.tst.2** — Double-move attempt (must-fail with `use-after-move` at second use).

**P4.tst.3** — Conditional move: moving in only one branch of `if/else`
(must-fail — inconsistent move state across branches).

**P4.tst.4** — Copy type (`int`) is still accessible after "move" (no error).

**P4.tst.5** — Drop order: a struct that tracks drops (via a print in its `drop`
method) must show reverse declaration order.

### Exit criteria

- [ ] Use-after-move is a compile error with correct spans.
- [ ] Double-move is a compile error.
- [ ] Conditional moves are caught.
- [ ] Copy types (`int`, `float`, `bool`, `char`, `byte`) are not subject to
      move errors.
- [ ] MIR contains explicit `Drop` terminators at scope exits.
- [ ] `paco-eval` executes drops in the correct reverse order.
- [ ] All Phase 4 golden tests pass.

---

## Phase 5 — Borrowing and lifetime inference

**Goal:** `&`/`&mut` borrows with the aliasing rule enforced. Lifetime inference
covers the common cases. Explicit `'a` annotations for the ambiguous cases.

**Duration estimate:** 3–6 months.

> **This is the highest-risk phase.** See `requirements.md §5` and
> `architecture.md §12.3`. Allocate more time than estimated.

### New language features

- `&T` shared borrows, `&mut T` mutable borrows.
- Aliasing rule: N shared `&` borrows **or** exactly one `&mut` borrow, never both.
- Lifetime inference for the common cases (no annotation needed).
- Explicit `'a` lifetime annotations for ambiguous cases.
- Diagnostics that suggest the exact annotation to paste.

### Crates touched

`paco-borrow` (sub-phases §12.2 and §12.3).

### Tasks

#### development — borrow check (P5.dev-loans.*)

**P5.dev-loans.1** — In `paco-borrow`: define the *loan* data structure:
`Loan { owner: BindingId, kind: Shared | Mutable, span: Span }`.

**P5.dev-loans.2** — Implement flow-sensitive loan analysis over HIR:
- At a `&x` expression: create a shared loan against `x`.
- At a `&mut x` expression: create a mutable loan against `x`.
- A loan is alive as long as the reference is live.
- At any program point, check the aliasing rule.

**P5.dev-loans.3** — Emit borrow conflict diagnostics: "cannot borrow `x` as
mutable because it is also borrowed as immutable", with spans for both the
existing loan and the conflicting loan.

**P5.dev-loans.4** — Enforce concurrency safety: data sent over a channel or
captured by `spawn` must be moved (owned), not borrowed
(`architecture.md §12.4`). Emit `shared-without-sync` for violations.

#### development — lifetime inference (P5.dev-lt.*)

Parallel with P5.dev-loans.*; depends on P4.dev-borrow.* being done.

**P5.dev-lt.1** — Represent lifetime variables as `LifetimeVar`s. Emit
constraints during borrow analysis: `'a outlives 'b`, `'a ≥ scope-of(x)`.

**P5.dev-lt.2** — Implement a constraint solver that finds the *smallest*
(shortest) lifetime assignment satisfying all constraints.
- If a satisfying assignment exists → inference succeeds, no annotation needed.
- If no assignment satisfies → the offending constraint is the error site.

**P5.dev-lt.3** — Implement the ambiguity detection: when multiple input
references could be the source of the output lifetime, the solver detects the
ambiguity and produces a diagnostic that includes *the exact `'a` annotation
to paste* (see `architecture.md §12.3` and `spec.md §3`).

**P5.dev-lt.4** — Start with inference equivalent to Rust NLL (non-lexical
lifetimes). Layer additional heuristics after the base is solid (ADR 0001).

#### tests (P5.tst.*)

**P5.tst.1** — Shared borrows in a read-only function; multiple simultaneous `&`
borrows of the same value.

**P5.tst.2** — `&mut` borrow used to mutate a field; no other borrow active.

**P5.tst.3** — Simultaneous `&` and `&mut` borrow (must-fail, aliasing violation).

**P5.tst.4** — Borrow outlives the owner (must-fail, lifetime error).

**P5.tst.5** — `first_word(s: &string) -> &string` — inference infers that the
return lives as long as `s`, no annotation needed.

**P5.tst.6** — Two-input reference ambiguity (must-fail with a diagnostic
suggesting the explicit `'a` annotation).

**P5.tst.7** — Sending a borrow over a channel (must-fail, `shared-without-sync`).

**P5.tst.8** — `Rc<T>` and `Arc<T>` as escape hatches: verify they compile and
that `Rc` cannot cross task boundaries.

#### review (P5.rev.*)

**P5.rev.1** — Formal safety review of the borrow checker against the aliasing
rule in `spec.md §3` and `ADR 0001`. Check that no program that causes undefined
behavior (double-free, use-after-free) can pass the check.

**P5.rev.2** — Re-evaluate the borrow-check-on-HIR-vs-MIR decision
(`architecture.md §22.3`). If flow-sensitive cases are being missed or diagnostics
are obscure, pivot to MIR-based borrow check.

### Exit criteria

- [ ] Aliasing-rule violations are compile errors with clear spans.
- [ ] Borrows that outlive their owners are compile errors.
- [ ] `first_word(s: &string) -> &string` compiles without any annotation.
- [ ] Ambiguous lifetime cases produce a diagnostic with the suggested annotation.
- [ ] All Phase 5 golden tests pass.
- [ ] No safe Paco program that passes borrow check can cause a use-after-free
      (validated by differential testing against the reference model in `review`).

### Risk

**High.** Aggressive lifetime inference approaching "NLL+" is research-grade.
Mitigation: start exactly at NLL, accept more annotation requests early, extend
heuristics conservatively after the base is stable.

---

## Phase 6 — Traits and dispatch

**Goal:** trait definitions, implicit satisfaction (statically checked), generics
with bounds, `dyn Trait` dynamic dispatch, operator overloading, `From<T>` + `?`.

**Duration estimate:** 2–3 months.

### New language features

- `trait T { fn method(self&) -> R; ... }`
- Implicit satisfaction: no `implements`; a type satisfies a trait if it has all
  required methods (ADR 0002).
- Generics with bounds: `fn f<T: Trait>(v: T)`.
- Monomorphization: one copy per concrete instantiation.
- `dyn Trait`: dynamic dispatch via vtable.
- Operator traits: `Add`, `Sub`, `Mul`, `Div`, `Neg`, `Index`, `Iter`, `Display`.
- `From<T>` trait in the prelude + `?` desugaring with implicit conversion (ADR 0007).

### Crates touched

`paco-types` (trait solver, vtable construction),
`paco-mir` (monomorphization, vtable dispatch, `From` desugaring).

### Tasks

#### development — trait solver (P6.dev-trait.*)

**P6.dev-trait.1** — In `paco-types`: implement the implicit trait satisfaction
check (`architecture.md §10.3`):
1. Look up the trait's required method signatures.
2. Look up the candidate type's methods (in the type's own block + any in-scope
   `methods T {}` blocks).
3. Check structural conformance: every required method exists with a compatible
   signature. `self` parameters unify with the candidate type.

**P6.dev-trait.2** — Implement the coherence rule: when two in-scope `methods T`
blocks both provide a method of the same name with overlapping signatures, emit
an error listing the conflicting definitions.

**P6.dev-trait.3** — Implement generic bounds: `T: Trait` in function signatures
and struct definitions. At a call site, verify that the concrete type satisfies
the bound.

**P6.dev-trait.4** — Wire `dyn Trait` to a vtable. A `dyn Trait` value is a fat
pointer: a data pointer plus a vtable pointer. The vtable is a struct of function
pointers, one per required method.

#### development — monomorphization (P6.dev-mono.*)

Parallel with P6.dev-trait.*.

**P6.dev-mono.1** — In `paco-mir`: implement the monomorphization collector.
Walk all MIR bodies; for each call to a generic function with concrete type
arguments, record the instantiation `(generic_fn, type_args)`.

**P6.dev-mono.2** — Materialize each instantiation as a fresh MIR body with type
variables substituted by the concrete types.

#### development — operators and From (P6.dev-ops.*)

Parallel with P6.dev-trait.*.

**P6.dev-ops.1** — Desugar binary operator expressions (`a + b`) to trait method
calls (`Add::add(a, b)`) in HIR lowering.

**P6.dev-ops.2** — Desugar `?` to check-and-return in HIR. If the inner error
type differs from the outer, insert a `From::from(e)` call. If no `From`
implementation satisfies the required conversion, the type checker reports the
error at the `?` site (`ADR 0007`).

**P6.dev-ops.3** — Add `From<T>`, `Into<T>`, `Add`, `Sub`, `Mul`, `Div`, `Neg`,
`Index`, `Iter`, `Display`, `Clone`, `Eq`, `Ord` to the compiler's known trait
registry (these will later be implemented in the stdlib in Paco; for now the
compiler knows their signatures so the trait solver can check against them).

#### tests (P6.tst.*)

**P6.tst.1** — A user-defined trait satisfied implicitly by a struct.

**P6.tst.2** — A generic function with a trait bound called with a conforming type.

**P6.tst.3** — A generic function called with a non-conforming type (must-fail
with a clear "type does not satisfy trait" diagnostic).

**P6.tst.4** — `dyn Trait` object: create and call through a trait object.

**P6.tst.5** — Operator overloading: a `Point` type with `+` via `Add`.

**P6.tst.6** — `?` with matching error types (no conversion).

**P6.tst.7** — `?` with different error types; `From` impl present (must convert).

**P6.tst.8** — `?` with different error types; no `From` impl (must-fail with
clear diagnostic at the `?` site).

**P6.tst.9** — Ambiguous coherence: two `methods T` blocks providing the same
method (must-fail).

### Exit criteria

- [ ] Implicit trait satisfaction is checked statically. No `implements` keyword.
- [ ] Generic functions monomorphize correctly.
- [ ] `dyn Trait` dispatches through a vtable.
- [ ] Operator overloading works for user-defined types.
- [ ] `?` with `From` conversion compiles. `?` without a `From` impl fails with
      a clear diagnostic.
- [ ] Coherence violation is an error.
- [ ] All Phase 6 golden tests pass.

---

## Phase 7 — Dev codegen (native binary)

**Goal:** `paco build` produces a real native binary via Cranelift. The
tree-walking interpreter is demoted to `comptime` use only.

**Duration estimate:** 3–4 months.

> **Second biggest milestone after Phase 1.** The language "really exists" now.

### New components

- Full HIR → MIR lowering (all constructs, not just the interpreter path).
- `paco-codegen-cranelift`: MIR → Cranelift IR → object file.
- `paco-link`: link object file(s) + runtime stub → single static binary.

### Tasks

#### development — MIR lowering (P7.dev-mir.*)

**P7.dev-mir.1** — Complete HIR → MIR lowering for all language constructs not
yet covered: closures, `for` loops, `match` arms, `?` desugaring, `dyn` dispatch
calls, destructor insertion. Reference `architecture.md §13.2` for the full
table of implicit → explicit mappings.

**P7.dev-mir.2** — Resolve the open architectural question: **MIR ownership of
types** — does MIR re-intern in its own arena or share `paco-types`'s arena?
(`architecture.md §22.2`). Record the decision. Implement accordingly.

**P7.dev-mir.3** — Implement monomorphization materialization: for each
instantiation collected in Phase 6 (P6.dev-mono.*), materialize the body by
substituting type variables.

#### development — Cranelift backend (P7.dev-cran.*)

Parallel with P7.dev-mir.* once the MIR shape is stable.

**P7.dev-cran.1** — In `paco-codegen-cranelift`: implement the `Backend` trait
(`architecture.md §15`). The `lower_body` method translates one MIR `Body` into
one Cranelift function.

**P7.dev-cran.2** — Lower MIR basic blocks to Cranelift basic blocks. Lower MIR
terminators: `Goto`, `Branch`, `SwitchInt`, `Call`, `Return`.

**P7.dev-cran.3** — Lower MIR statements: `assign`, `storage-live`,
`storage-dead`, `drop` (calls into the runtime or user `drop` fn).

**P7.dev-cran.4** — Lower MIR types to Cranelift types:
- `int` → `I64`, `float` → `F64`, `bool` → `I8`, `char` → `I32`.
- Structs: `cranelift_module::StructType` based on `@repr` layout.
- References: `I64` (pointer size, system-dependent).

**P7.dev-cran.5** — Emit function symbols; handle external symbols for the
runtime ABI (`paco_rt_*` functions from `architecture.md §16`).

#### development — linker (P7.dev-link.*)

**P7.dev-link.1** — In `paco-link`: collect object files from codegen. Link with
`cranelift-object` (for now; full `lld` integration is an option for later).
Embed `libpaco_runtime.a` (a stub for now — actual runtime is Phase 8).

**P7.dev-link.2** — Resolve the linker selection open question
(`architecture.md §22.5`): `lld` everywhere or system linker fallback. Document
the decision.

**P7.dev-link.3** — Wire `paco-driver build.rs` to call codegen + link and
produce the final binary.

#### devops (P7.ops.*)

**P7.ops.1** — Resolve the single-file vs multi-file parallelism open question
(`architecture.md §22.4`). For now, stay single-threaded; note where parallelism
would go.

**P7.ops.2** — Add a CI step that builds a test binary for each run-output
golden test and diffs its stdout against the expected output. Run on all
three platforms.

#### tests (P7.tst.*)

**P7.tst.1** — Re-run all Phase 1–6 run-output golden tests through the codegen
pipeline. Outputs must match the interpreter.

**P7.tst.2** — A program that allocates a `Vec<int>`, pushes values, iterates,
and prints. Tests heap allocation through codegen.

**P7.tst.3** — A program that uses `Result` with `?` and propagates an error.
Tests branch generation in codegen.

**P7.tst.4** — Differential test harness: for every run-output test, run via
(interpreter) and (Cranelift binary). Outputs must match.

### Exit criteria

- [ ] `paco build hello.paco` produces a static binary that runs on Linux and macOS.
- [ ] All Phase 1–6 run-output tests pass through the codegen path.
- [ ] Differential tests (interpreter vs Cranelift) pass for all conformance programs.
- [ ] MIR arena decision is documented.
- [ ] Linker decision is documented.
- [ ] CI produces and runs binaries on all three platforms.

### Risk

**Medium-high.** The full HIR → MIR lowering is the largest single code
change in the project. Strategy: keep the interpreter running in parallel;
use differential testing to catch regressions immediately.

---

## Phase 8 — Concurrency

**Goal:** `spawn`, channels, `select`, `iter` generators run via the M:N
scheduler embedded in the binary.

**Duration estimate:** 3–4 months.

### New components

- `runtime/`: M:N task scheduler, growable stacks, OS I/O poller.
- MIR instructions for `spawn`, channel send/recv/close.
- Runtime ABI exposed through `paco-link`.

### Tasks

#### development — runtime (P8.dev-rt.*)

**P8.dev-rt.1** — In `runtime/`: implement the M:N scheduler core:
- Task queue (work-stealing or simple FIFO to start).
- Stack allocation for tasks (growable stacks; start with fixed-size segments).
- Context switch (platform-specific assembly: `x86_64` first, then `aarch64`).

**P8.dev-rt.2** — Implement OS I/O poller integration:
- `epoll` on Linux.
- `kqueue` on macOS.
- `IOCP` on Windows.
When a task blocks on I/O, the scheduler suspends it and runs another task. No
`async`/`await` visible to user code (spec §6, ADR 0004).

**P8.dev-rt.3** — Implement the C ABI exported by the runtime
(`architecture.md §16`):
```
paco_rt_spawn(entry, args_ptr, args_size) -> TaskHandle
paco_rt_join(TaskHandle) -> JoinResult
paco_rt_channel_new(elem_size, capacity) -> Channel
paco_rt_channel_send(Channel, value_ptr) -> SendResult
paco_rt_channel_recv(Channel) -> RecvResult
paco_rt_channel_close(Channel)
paco_rt_panic(message_ptr, message_len) -> !
```

**P8.dev-rt.4** — Per-task panic isolation: a panic in a spawned task does not
bring down the process. `paco_rt_join` returns an `Err(panic_message)`.

#### development — spawn and channels (P8.dev-spawn.*)

Parallel with P8.dev-rt.*; depends on the runtime ABI being defined (P8.dev-rt.3).

**P8.dev-spawn.1** — In `paco-mir`: add `Spawn(entry, captures)` and
`ChannelNew(elem_ty, capacity)` as MIR constructs.

**P8.dev-spawn.2** — In `paco-codegen-cranelift`: lower `Spawn` to a call to
`paco_rt_spawn`, marshalling the captured variables into an argument struct.

**P8.dev-spawn.3** — Lower channel operations (`send`, `recv`, `close`) to calls
to the corresponding runtime ABI functions.

**P8.dev-spawn.4** — Enforce in `paco-borrow`: captured values in `spawn { ... }`
must be moved (cannot borrow across a task boundary).
`send(tx, x)` moves `x`; `x` is no longer usable after the call.

#### development — select (P8.dev-select.*)

**P8.dev-select.1** — Parse and lower `select { ... }` (`grammar.ebnf §3` select
expression) to a runtime poll loop over the listed channel operations plus an
optional `default =>` arm.

**P8.dev-select.2** — Implement the runtime-level select: try each channel in
round-robin order; if all block, suspend the task until one becomes ready. If a
`default` arm is present, execute it immediately instead of blocking.

#### development — iter generators (P8.dev-iter.*)

**P8.dev-iter.1** — Parse `iter fn name() -> T { ... yield expr ... }` and lower
it to a state-machine struct in HIR/MIR. Each `yield` becomes a suspension point.

**P8.dev-iter.2** — Implement the `Iterator` protocol on the generated struct:
`next() -> Option<T>` resumes the generator and returns the next yielded value
or `None` when it returns.

**P8.dev-iter.3** — Ensure `for x in iter_fn()` desugars correctly to calls to
the state-machine's `next()`.

#### devops (P8.ops.*)

**P8.ops.1** — CI must cross-compile and test the runtime on all three platforms.
The I/O poller is platform-specific — each must be tested independently.

#### tests (P8.tst.*)

**P8.tst.1** — Channel ping-pong: two tasks exchange values through a channel.
Verify outputs are in order.

**P8.tst.2** — Task isolation: a spawned task panics; the main task receives the
panic via `h.join()` and continues.

**P8.tst.3** — `select` with two ready channels: one of the two arms executes.

**P8.tst.4** — `select` with `default` when no channel is ready.

**P8.tst.5** — `iter` generator: an infinite counter generator; `take(5)` yields
the first 5.

**P8.tst.6** — HTTP server example from the spec: accepts connections and handles
each in a spawned task (uses the I/O poller). Must serve concurrent requests.

**P8.tst.7** — Sending a non-moved borrow over a channel (must-fail,
`shared-without-sync`).

### Exit criteria

- [ ] `spawn f()` runs `f` concurrently and `h.join()` recovers its return value.
- [ ] A panicking task's panic is isolated; the spawning task continues.
- [ ] Channel send/recv/close work correctly with the aliasing rules enforced.
- [ ] `select` picks a ready channel; falls to `default` when all block.
- [ ] `iter fn` generators work with `for x in gen()`.
- [ ] HTTP server example handles concurrent connections.
- [ ] All Phase 8 golden tests pass on Linux, macOS, and Windows.

### Risk

**Medium-high.** The M:N runtime is the largest Rust-level engineering effort.
The I/O poller is platform-specific. Growable stacks require platform assembly.
Strategy: Linux first, macOS second, Windows last. Use a minimal fixed-size
stack initially and grow later.

---

## Phase 9 — comptime

**Goal:** full comptime evaluator, `@derive` generates code, type introspection works.

**Duration estimate:** 2–3 months.

### New language features

- `comptime` blocks and functions.
- Type introspection: iterating a struct's fields at compile time.
- Code generation from `comptime` that re-enters the pipeline as HIR.
- `@derive(Display, Clone, Eq, ...)` generates the required methods.

### Crates touched

`paco-eval` (full MIR interpreter), `paco-hir` (attribute expansion and
re-ingestion of generated code).

### Tasks

**P9.dev.1** — Extend `paco-eval` (currently the Phase 1–6 interpreter) to fully
interpret MIR rather than AST. The evaluator now runs after MIR lowering.

**P9.dev.2** — Add the comptime sandboxing rules (`architecture.md §14`):
no I/O, no FFI, instruction budget to prevent runaway evaluation.

**P9.dev.3** — Expose the type table as ordinary Paco values inside `comptime`.
A field of a struct becomes a `FieldInfo { name: string, ty: Type }`. Implement
the `for field in T::fields()` introspection loop.

**P9.dev.4** — Implement `Code` values: a `comptime` function can return a
`Code` fragment that represents Paco source. When a `Code` value is produced,
it re-enters the pipeline at HIR lowering (not at the parser) — so the type
checker re-checks it (`architecture.md §14`).

**P9.dev.5** — In `paco-hir`: implement attribute expansion for `@derive`.
For each attribute argument (e.g., `Display`), invoke the `derive_Display`
comptime function (or a compiler-built-in equivalent) and inject the generated
HIR into the type's method table.

**P9.dev.6** — Implement the built-in derive targets: `Display`, `Clone`, `Eq`,
`Ord`. These generate method bodies in Paco code evaluated at compile time.

#### tests (P9.tst.*)

**P9.tst.1** — A `comptime fn` that takes a type and returns the count of its fields.

**P9.tst.2** — `@derive(Display)` on a struct; print it with `print(s)`.

**P9.tst.3** — `@derive(Clone)` on a struct; clone it and mutate the copy.

**P9.tst.4** — `DataFrame<LogRow>` from `ADR 0011`: struct with `@derive(Schema)`,
a comptime-typed data frame.

**P9.tst.5** — Comptime I/O attempt (must fail at compile time).

**P9.tst.6** — Infinite comptime loop (must fail with budget-exceeded diagnostic).

### Exit criteria

- [ ] `comptime` functions run during compilation.
- [ ] Type introspection iterates struct fields.
- [ ] `@derive(Display, Clone, Eq)` generates correct implementations.
- [ ] Generated code is re-type-checked (no escaping unchecked code).
- [ ] I/O and FFI in `comptime` are compile errors.
- [ ] All Phase 9 golden tests pass.

---

## Phase 10 — Optimizing backend

**Goal:** `paco build --release` produces optimized native binaries via LLVM.
Cross-compilation works.

**Duration estimate:** 2–3 months.

### Crates touched

`paco-codegen-llvm`, `paco-link` (target selection).

### Tasks

**P10.dev.1** — In `paco-codegen-llvm`: implement the same `Backend` trait as
Cranelift. Use `inkwell` (safe LLVM bindings). Map MIR → LLVM IR, paying
attention to target ABI (struct layout, pointer integer types).

**P10.dev.2** — Run LLVM's standard optimization pipeline at `-O2`/`-O3`.
Do not duplicate optimizations LLVM already performs. Paco-specific opts
(e.g., collapsing `Result<T, !>` into `T`) live in MIR.

**P10.dev.3** — In `paco-link`: add target triple selection for cross-compilation
(`--target=aarch64-unknown-linux-gnu`, etc.).

**P10.dev.4** — Verify semantic parity: every differential test (Phase 7+) must
pass with a third execution path: interpreter == Cranelift == LLVM.

#### devops (P10.ops.*)

**P10.ops.1** — Add LLVM toolchain to CI. Gate behind a feature flag
(`cargo build --features llvm`) to keep LLVM out of the default dev build.

**P10.ops.2** — Add cross-compilation matrix to CI: at minimum
`x86_64-unknown-linux-gnu` and `aarch64-unknown-linux-gnu`.

#### tests (P10.tst.*)

**P10.tst.1** — All run-output conformance tests through the LLVM path.

**P10.tst.2** — Performance comparison: a tight loop (e.g., summing 1 billion
integers) compiled with `--release` vs dev build. The release build must be
measurably faster (at least 2×).

**P10.tst.3** — Cross-compiled binary for `aarch64` runs correctly under QEMU.

### Exit criteria

- [ ] `paco build --release` produces an optimized binary.
- [ ] All conformance tests pass through LLVM.
- [ ] Differential tests (interpreter / Cranelift / LLVM) all agree.
- [ ] Cross-compilation to at least one non-native target works.
- [ ] CI runs the LLVM backend on all three platforms.

---

## Phase 11 — Tooling

**Runs in parallel** with other phases starting at Phase 2.

**Goal:** complete developer experience: `paco fmt`, `paco test`, `paco doc`,
and eventually the LSP and query-based incremental compilation.

### Sub-phases

#### 11a — `paco fmt` (start at Phase 2)

**P11a.dev.1** — Implement the canonical formatter in `paco-driver::fmt`. The
formatter reads the lexer's trivia stream (whitespace + comments retained in the
parallel stream from P1.dev-lex.1). It reconstructs source from the AST +
trivia, applying the canonical style rules.

**P11a.dev.2** — Style rules (non-negotiable, matching the spec):
- 4-space indentation.
- Space after `:` in type annotations.
- No space before `(` in function calls.
- Trailing comma in multi-line argument lists.
- One blank line between top-level items.

**P11a.dev.3** — `paco fmt --write` rewrites files in place.
`paco fmt` (without `--write`) diffs and exits with code 1 if the file
is not canonical.

**P11a.tst.1** — Idempotence test: `fmt(fmt(f)) == fmt(f)` for all test files.

**P11a.tst.2** — Round-trip test: `fmt(f)` compiles to the same program as `f`.

#### 11b — `paco test` (start at Phase 1)

**P11b.dev.1** — In `paco-driver::test`: collect all functions annotated with
`@test` in a module. Run each function; report pass/fail.

**P11b.dev.2** — Support `@should_panic`: a test function expected to panic
passes if (and only if) it panics.

**P11b.dev.3** — Support `@bench`: run the function N times, report median
execution time and variance.

**P11b.dev.4** — `paco test [path]` runs tests under the given path only.

#### 11c — `paco doc` (start at Phase 2)

**P11c.dev.1** — In `paco-driver::doc`: collect `///` doc comments from the AST.
Content is Markdown. Generate HTML (or a structured JSON for downstream
rendering).

**P11c.dev.2** — Link cross-references: `[Type::method]` in a doc comment
resolves to the appropriate anchor.

#### 11d — Query system and LSP (start at Phase 7)

**P11d.dev.1** — Wrap the compiler pipeline in a `salsa`-style query system in
`paco-driver`. Each phase becomes a memoized function keyed by file hash +
dependency versions. Only invalidated keys recompute on incremental builds.

**P11d.dev.2** — Implement a Language Server Protocol (LSP) server:
- `textDocument/hover` → type of identifier under cursor.
- `textDocument/definition` → jump to definition.
- `textDocument/diagnostics` → all errors/warnings.
- `textDocument/completion` → field/method completion.

### Exit criteria

- [ ] `paco fmt` formats all conformance test files idempotently.
- [ ] `paco test @test` functions run and report pass/fail.
- [ ] `paco doc` generates HTML from `///` comments.
- [ ] `paco build` is measurably faster on a second run with unchanged files
      (query system).
- [ ] LSP provides hover types and go-to-definition in VS Code.

---

## Phase 12 — Standard library

**Runs in parallel** starting at Phase 7. Written in Paco, not Rust.

**Goal:** a standard library sufficient for real projects.

### Modules (in `src/`)

#### 12a — `std::core` (earliest — can start at Phase 2)

Defines the foundation types and traits the compiler already knows about:
- `Option<T>`, `Result<T, E>`, `From<T>`, `Into<T>`.
- `panic(msg: string) -> !`.
- `Clone`, `Copy`, `Drop`.

#### 12b — `std::collections` (start at Phase 7)

- `Vec<T>`: growable array. `new()`, `push`, `pop`, `len`, `get`, `iter`.
- `Map<K, V>`: hash map. `new()`, `insert`, `get`, `remove`, `contains_key`.
- `Set<T>`: hash set.
- `String`: owned UTF-8 string with `push_str`, `split`, `trim`, etc.

#### 12c — `std::io` (start at Phase 8, needs concurrency)

- `read_file(path: string) -> Result<string, IoError>`.
- `write_file(path: string, content: string) -> Result<(), IoError>`.
- `stdin()`, `stdout()`, `stderr()`.
- TCP/UDP socket primitives (wrapping the runtime I/O poller).

#### 12d — `std::fmt` (start at Phase 9, needs comptime)

- `Display` trait + default `print(v)` for any `Display` implementor.
- `Debug` trait.
- Format strings via comptime: `fmt!("{name} = {value}")`.

#### 12e — `std::math` (start at Phase 9, needs comptime)

- `Matrix<T>`: dense matrix with operator overloading (`+`, `*` via `Mul`).
  Uses `comptime` for zero-overhead expression fusion (ADR 0011).
- `Vec<T>` (numeric sense, distinct from the collection): fixed-size vector
  for SIMD-friendly computation.
- `DataFrame<Schema>`: compile-time typed data frame using `@derive(Schema)`.
  `column(.field_name)` returns a typed column slice.
- Statistical functions: `mean`, `std_dev`, `min`, `max`, `median`.
- Linear algebra: `dot`, `transpose`, `inverse` (for square matrices).

### Exit criteria

- [ ] `std::core` types (`Option`, `Result`) compile and all conformance tests
      that use them pass without compiler built-ins.
- [ ] `Vec<T>` and `Map<K, V>` are idiomatic (constructed with `::new()`).
- [ ] File I/O example from the spec compiles and runs.
- [ ] `@derive(Display)` uses `std::fmt::Display` from the stdlib.
- [ ] `Matrix<T>` `A + B * C` fuses into a single loop via comptime.
- [ ] `DataFrame<LogRow>` column access is zero-overhead (verified by inspecting
      MIR — no runtime string lookup).

---

## Cross-cutting concerns

### Diagnostics

Every error and warning must have:
- A stable code (`PACO-Exxxx` for errors, kebab-case lint names for warnings).
- A primary span (the source location of the problem).
- At least one note or help message explaining the fix.
- A `Suggestion` (structured rewrite) whenever the fix is mechanical.

Review each phase's diagnostics against this checklist before marking the phase
done.

### Testing invariants (all phases)

1. **Parser never panics.** Any input — including random bytes — must produce an
   error diagnostic, not a crash.
2. **Interpreter == Cranelift == LLVM.** For every run-output test that all three
   paths can execute, their outputs must be byte-identical.
3. **Safe programs pass.** A correct Paco program must not be rejected by the
   borrow checker.
4. **Unsafe programs fail.** A program that violates ownership or aliasing must
   be rejected.

### Open architectural questions (must be resolved before their phase)

| Question | Resolve before | Reference |
|----------|---------------|-----------|
| HIR storage: arena vs Vec-indexed IDs | Phase 2 | `architecture.md §22.1` |
| MIR type arena sharing | Phase 4 | `architecture.md §22.2` |
| Borrow check on HIR vs MIR | Phase 5 | `architecture.md §22.3` |
| Single-file vs multi-file parallelism | Phase 7 | `architecture.md §22.4` |
| Linker selection | Phase 7 | `architecture.md §22.5` |
| `paco fmt` host crate | Phase 2 | `architecture.md §22.6` |

Each resolution must be recorded as a new internal ADR in `compiler/docs/`.

---

## Parallelism summary across phases

The table below shows which phases and sub-tasks can run simultaneously.

| Active agents | What they work on |
|---------------|-------------------|
| devops + development + tests | **Phase 0** (all parallel) |
| dev-lex + dev-eval-skeleton + tests | **Phase 1** (lex first, then parse) |
| dev-hir + dev-res + dev-types + dev-par + tests + devops-fmt | **Phase 2** |
| dev-match + dev-desugar + tests | **Phase 3** |
| dev-borrow + dev-mir-drop + tests | **Phase 4** |
| dev-loans + dev-lt + tests + review | **Phase 5** |
| dev-trait + dev-mono + dev-ops + tests | **Phase 6** |
| dev-mir + dev-cranelift + dev-link + devops + tests | **Phase 7** |
| dev-runtime + dev-spawn + dev-select + dev-iter + devops + tests | **Phase 8** |
| dev-eval-full + dev-derive + tests | **Phase 9** |
| dev-llvm + devops + tests | **Phase 10** |
| dev-fmt + dev-test-runner + dev-doc (from Phase 2) | **Phase 11** (parallel) |
| dev-stdlib-core (from Phase 2) + dev-stdlib-collections + dev-stdlib-io + dev-stdlib-math | **Phase 12** (parallel) |

---

## Honest timeline note

Reaching **Phase 7** (native binary) is a many-month effort for one dedicated
person. The complete Paco (Phase 12) is 2–3 years of work, or a parallel team.
The phased structure guarantees that at every milestone something demonstrable
runs. Celebrate each milestone — the language "growing" is the motivator that
keeps the project alive.

