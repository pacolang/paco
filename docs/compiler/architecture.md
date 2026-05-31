# Paco — Compiler Architecture (v0.1)

> Companion to `docs/design/spec.md` (the **what**) and
> `docs/implementation/requirements.md` (the **what to build, in what order**).
> This document describes **how the compiler is structured**: the pipeline, the
> intermediate representations, the crate layout, the shared infrastructure, and
> the rules that keep two backends honest about the same semantics.
>
> Status: living draft. Decisions recorded in ADRs `0001`–`0005` are the source
> of truth; this document only refines *how* those decisions are realized inside
> the compiler. Do not reopen an ADR here.

---

## 0. How to read this document

- **§1–§2** frame the goals and the boundary between the compiler, the runtime,
  and the standard library.
- **§3** is the top-level pipeline. Every other section refines one of its boxes.
- **§4** is the crate layout — the physical realization of the pipeline in Rust.
- **§5–§16** walk the pipeline phase by phase: lexer, parser, AST, HIR, name
  resolution, type and trait check, pattern exhaustiveness, ownership and
  borrow check, the Paco IR, `comptime`, codegen, linking and the runtime ABI.
- **§17–§19** cover the cross-cutting infrastructure: diagnostics, the query
  model, and the test harness.
- **§20** maps each architectural piece to the phases of the roadmap.
- **§21** offers architectural recommendations on the open design frictions
  (spec §18). These are recommendations, not decisions — the human signs off.
- **§22** lists the architectural questions still open.
- **§23** is a one-paragraph summary.

Conventions: **MUST**, **SHOULD**, **MAY** follow RFC 2119, matching
`requirements.md`. The text uses *HIR* and *MIR* as short names for the two
typed intermediate representations introduced in §10.

---

## 1. Goals of the architecture

The compiler is shaped by three load-bearing constraints:

1. **Two backends, one frontend.** Cranelift drives `paco build`, LLVM drives
   `paco build --release` (ADR 0003). Frontend work cannot be duplicated. A
   single typed IR feeds both.
2. **Aggressive lifetime inference at the core** (ADR 0001). The borrow checker
   is the highest-risk component (requirements §5). The architecture isolates
   it so that progress on the rest of the language is not blocked by it.
3. **`comptime` reuses the compiler.** Compile-time evaluation must execute the
   same language users write, on the same IR the backends see (ADR 0005). The
   `comptime` evaluator is a consumer of the MIR, not a separate language.

Three secondary constraints shape the implementation:

- **Always something that runs** (requirements §4). A tree-walking interpreter
  exists from Phase 1 and stays alive — first as the only execution path, then
  as the `comptime` engine and as a differential oracle against codegen.
- **Visible cost extends to the compiler.** No silent re-parsing, no hidden
  global mutable state, no implicit cross-phase shortcuts. Each phase has a
  declared input and output.
- **Diagnostics are a first-class output** (spec §16, AGENTS.md §5). Error
  recovery and span fidelity are requirements, not polish.

---

## 2. The Rust ↔ Paco boundary

The codebase has three layers; the architecture only governs the first two.

| Layer    | Language  | Repository path | Responsibility                              |
|----------|-----------|-----------------|---------------------------------------------|
| Compiler | Rust      | `compiler/`     | Translate `.paco` to a native binary.       |
| Runtime  | Rust + asm| `runtime/`      | M:N scheduler, channels, growable stacks.   |
| Stdlib   | Paco      | `src/`          | `core`, `collections`, `io`, `math`.        |

The compiler links the runtime into every emitted binary (§17). The stdlib is
compiled by the compiler like any other Paco package; it has no privileged path
through the pipeline, only privileged module paths (`std::…`).

> This boundary is deliberate. The runtime is the only place where unsafe,
> platform-specific code lives (context switches, OS pollers). Keeping it out of
> the compiler means the compiler stays a pure data transformer.

---

## 3. Pipeline overview

Source flows through the compiler in nine numbered stages. Each stage has a
single responsibility, a typed input, and a typed output. No stage reaches
backward.

```
   .paco source files
        │
        ▼
  [1] Lexer ─────────────────► token stream  (with spans)
        │
        ▼
  [2] Parser ────────────────► AST           (concrete syntax, with spans)
        │
        ▼
  [3] AST lowering ──────────► HIR           (desugared, name-resolved)
        │
        ▼
  [4] Type & trait check ────► HIR + types   (every node carries a Ty)
        │
        ▼
  [5] Pattern exhaustiveness ► HIR + types   (annotated; rejects non-exhaustive)
        │
        ▼
  [6] Ownership & borrow ────► HIR + types + borrows
        │                       (lifetime inference; lints emitted)
        ▼
  [7] HIR → MIR lowering ────► MIR (Paco IR) (CFG, SSA, explicit destructors)
        │
        ├─────────────────────► comptime evaluator (§14)
        │
        ▼
  [8] Backend lowering ──────► Cranelift IR  ┐
                                LLVM IR      ┘  (one of two, per build mode)
        │
        ▼
  [9] Linking ───────────────► single static native binary
```

Each arrow is a function on data. The diagnostics stream is a side channel: any
stage may emit a `Diagnostic` (§17) without aborting the pipeline. The driver
decides when accumulated errors force a halt (typically: after each stage, if
any *error*-severity diagnostic was emitted).

The architecture keeps **two** typed IRs (HIR and MIR) rather than one. The HIR
preserves source structure for diagnostics and for the borrow checker (which
benefits from named bindings and lexical scopes). The MIR is a linear CFG
suitable for codegen and for the `comptime` interpreter. Lowering HIR → MIR is
the point where implicit constructs — destructors, autoref, method dispatch —
become explicit instructions.

---

## 4. Crate layout

The compiler is a Rust workspace. The split is by responsibility, not by phase:
keeping `paco-parse` and `paco-lex` separate from `paco-hir` would couple them
to the AST in a way that makes incremental work harder. The chosen granularity
is "one crate per stable interface".

```
compiler/
├── Cargo.toml                 # workspace root
├── paco-driver/               # binary `paco`: CLI, subcommands, orchestration
│   └── src/
│       ├── main.rs
│       ├── build.rs           # `paco build` / `--release`
│       ├── check.rs           # `paco check`
│       ├── test.rs            # `paco test`
│       └── fmt.rs             # `paco fmt`
├── paco-span/                 # SourceMap, Span, FileId; shared by every crate
├── paco-diag/                 # Diagnostic, Severity, Reporter; ariadne adapter
├── paco-syntax/               # tokens + AST + parser (kept together; §5–§7)
│   └── src/
│       ├── lex.rs             # logos-driven lexer
│       ├── ast.rs             # AST node types + visitors
│       └── parse.rs           # parser with error recovery
├── paco-hir/                  # HIR types + AST→HIR lowering + name resolution
├── paco-resolve/              # module graph, scope tables, path resolution
├── paco-types/                # Ty, type checker, trait solver, inference engine
├── paco-match/                # exhaustiveness and reachability for `match`
├── paco-borrow/               # ownership, borrow check, lifetime inference
├── paco-mir/                  # Paco IR definition + HIR→MIR lowering
├── paco-eval/                 # `comptime` interpreter over MIR
├── paco-codegen-cranelift/    # MIR → Cranelift IR; dev builds
├── paco-codegen-llvm/         # MIR → LLVM IR via inkwell; release builds
├── paco-link/                 # linking, runtime embedding, target selection
└── paco-test-harness/         # shared utilities for compiler tests
```

Three rules govern this layout:

- **Layered dependencies only.** Each crate may depend on crates listed above it
  in the diagram and on `paco-span` / `paco-diag` (which everyone uses), but
  never on a crate below. `paco-hir` does not know about MIR; `paco-types` does
  not know about codegen.
- **Codegen crates are siblings, not subtypes.** They consume the same MIR and
  emit the artifacts their respective backends require. The driver picks one.
- **`paco-driver` is the only crate that links a binary.** Everything else is a
  library, so every phase is independently testable.

> The codegen split into two crates costs a small amount of duplication for a
> large amount of clarity: LLVM is a heavy dependency and gating it behind a
> feature flag is straightforward when it lives in its own crate.

---

## 5. Lexer (`paco-syntax::lex`)

**Input:** UTF-8 source text (a `&str` plus a `FileId`).
**Output:** a flat `Vec<Token>` carrying spans.

Implementation: `logos`-derived enum. The lexer is decoupled from the parser so
that tooling (formatter, future LSP) can tokenize without parsing.

Tokens cover the categories listed in `docs/grammar/tokens.md`. Two lexical
subtleties matter to the architecture:

- **Lifetimes vs char literals.** `'a` is a lifetime; `'a'` is a char. The lexer
  resolves this by peeking past the apostrophe: a single character followed by
  `'` is a char literal, otherwise it is a lifetime token.
- **Significant newlines? No.** Paco has no statement terminators (spec §2). The
  parser uses precedence and grammar shape, not newline tokens, to disambiguate.
  Newlines are skipped trivia; comments are skipped trivia attached to the
  following token (for `paco doc` and `paco fmt`).

**Error policy.** Lexical errors (invalid character, unterminated string) emit a
diagnostic and produce an `Error` token. Lexing never aborts. This is what
makes the "multiple errors per compilation" requirement achievable.

**Trivia retention.** For the canonical formatter (`paco fmt`) the lexer
optionally retains whitespace and comments in a parallel stream keyed by span.
The compiler proper discards trivia.

---

## 6. Parser (`paco-syntax::parse`)

**Input:** token stream.
**Output:** an AST rooted at `Module` (one per file), plus diagnostics.

The parser implements the grammar in `docs/grammar/grammar.ebnf`. It is
**hand-written** rather than generated. Two reasons:

- Paco's error messages are part of the user-facing product. A hand-written
  recursive-descent parser with `chumsky`-style recovery primitives gives the
  finest control over the wording and the recovery points.
- The grammar still has open frictions (spec §18.6, AGENTS context §5.2). A
  generator that requires a stable grammar file as input is an obstacle to
  iterating on the surface syntax.

`chumsky` is a permitted alternative for early phases (requirements §7); if
chosen, the migration to a hand-written parser SHOULD happen before Phase 6,
where the demands on error wording rise.

**Error recovery.** Standard recursive-descent recovery: at each item boundary
(`fn`, `struct`, `enum`, `trait`, `methods`, `use`) the parser resyncs by
skipping tokens until it finds the next item keyword at the same brace depth.
Inside an item, statement boundaries (newlines that follow a complete
expression, or `}`) are the resync points.

**Span fidelity.** Every AST node MUST carry a `Span`. Spans are byte ranges in
the source; the `SourceMap` in `paco-span` resolves them to (file, line,
column) only at diagnostic-rendering time. This keeps the hot path cheap.

---

## 7. AST (`paco-syntax::ast`)

The AST mirrors the source closely. It distinguishes four top-level node kinds
from the start (requirements §3.2):

- **Items**: `FnDecl`, `StructDecl`, `EnumDecl`, `TraitDecl`, `MethodsBlock`,
  `UseDecl`. Items live at module scope.
- **Expressions** (`Expr`): the operand of any computation.
- **Patterns** (`Pat`): used in `match`, `let`, function parameters.
- **Types** (`Ty`): syntactic types as they appear in source — not yet resolved.

The AST is **untyped** and **unresolved**: identifiers are still strings, paths
are still token sequences. This is intentional — it keeps the parser one job
(produce concrete syntax), and it keeps the AST cheap to construct in error
paths where the rest of the pipeline will be skipped.

**Visitor infrastructure.** `paco-syntax` exposes both an immutable visitor
(`Visit`) and a folding visitor (`MutVisit`) generated by a small macro. Phases
that only want to read (the formatter, doc generation) use `Visit`; phases that
rewrite (desugaring, attribute expansion) use `MutVisit`.

---

## 8. HIR — High-level Intermediate Representation (`paco-hir`)

**Input:** AST + module graph from `paco-resolve`.
**Output:** HIR — a typed-shape, name-resolved, desugared tree.

The HIR is structurally similar to the AST but with four differences:

1. **Names are resolved.** Every identifier reference carries a `DefId` (for
   items) or a `BindingId` (for locals). The string is gone.
2. **Desugaring is done.** `if let`, `while let`, `for x in xs`, the `?`
   operator, range expressions, and method-call syntax all reduce to a smaller
   core. Method calls become trait-resolved function calls at type-check time.
3. **Implicit nodes become explicit.** Method receivers (`self&`, `self&mut`,
   `self`) become regular parameters with explicit types. Receiverless
   functions inside a `struct` block are not methods; they are associated
   functions, resolved through the type's namespace.
4. **Attributes are interpreted.** `@derive(...)` expands into trait
   satisfactions to check; `@test`, `@bench`, `@should_panic` are recorded for
   the test harness; `@allow("lint-code")` annotates the surrounding scope.

The HIR is where the borrow checker (§12) does its work. Keeping lexical
structure intact in HIR — rather than lowering straight to MIR — matters because
borrow check is easier to express and to explain when scopes are visible.

---

## 9. Name resolution and the module graph (`paco-resolve`)

Name resolution runs after parsing and before HIR construction. It owns three
data structures:

- **The module graph.** A directed graph of modules built by walking `use`
  declarations. Cycles are an error. The root is the compilation root (a
  binary or library crate in the `paco.mod` sense).
- **Per-scope symbol tables.** A scope is a function body, a block, a `match`
  arm, an `iter` body, or an item-level scope. Shadowing within a scope is
  allowed (`let x = ...; let x = ...;`, spec §2). Each `let` produces a fresh
  `BindingId`.
- **The item table.** A map `DefId → Item` covering every fn, struct, enum,
  trait, and associated function across all loaded modules. The table is the
  authoritative source for cross-module lookups.

**Path resolution.** A `Path` like `std::io::read_file` resolves left-to-right
through the item table, with `std` reserved for the standard library and other
URL-shaped roots (`example.com/team/json`) resolved via the dependency graph
from `paco.mod`. During Phases 0–6 the dependency tooling is deferred (ADR
0005); paths to external packages resolve through local relative directories
configured in `paco.mod`.

**Implicit trait satisfaction (ADR 0002, spec §6).** Name resolution does *not*
record which traits a type satisfies. That is a typing question, deferred to
`paco-types`. Resolution only records which methods exist on which types.

---

## 10. Type and trait checking (`paco-types`)

**Input:** HIR.
**Output:** HIR annotated with `Ty` on every expression and pattern, plus a
trait obligation table.

### 10.1 Type representation

A `Ty` is a small enum: primitive (`i32`, `f64`, `bool`, `char`, `string`,
`byte`), tuple, slice (`[]T`), borrow (`&T`, `&mut T`, with an inferred
lifetime variable), reference-counted (`Rc<T>`, `Arc<T>`), function, dyn-trait,
or a generic application (`Vec<T>`, `Result<T, E>`). Type variables are
represented by interned `TyVid`s.

Types are **interned** in a per-compilation arena. Equality is pointer equality
on the interned handle. This is what lets the type checker hash and compare
types in tight loops cheaply.

### 10.2 Inference

The algorithm is **bidirectional Hindley–Milner with constraints**
(requirements §3.4). Two modes:

- **Synthesis.** Given an expression, walk down and bubble its type up.
- **Checking.** Given an expression and an expected type, propagate the
  expectation down. This is what makes enum constructors and closures infer
  cleanly: the expected `Result<int, Error>` flows into `Ok(...)` and tells the
  inner expression what `int` it must produce.

The two modes meet at function-call boundaries and at `match` arms. Unresolved
type variables at the end of a function body are an error.

### 10.3 Trait resolution

Traits are **implicitly satisfied, statically checked** (ADR 0002). For each
trait obligation `T: Trait`, the solver:

1. Looks up `Trait`'s required signatures.
2. Looks up `T`'s methods (in the type's own block plus in any in-scope
   `methods T { ... }` block).
3. Checks structural conformance: every required method exists with a
   compatible signature, where `self` parameters unify with `T`.

Compatibility rules and ambiguity policy follow ADR 0002. When two in-scope
`methods T` blocks both provide a method of the same name with overlapping
signatures, the compiler errors (no silent winner) and lists the candidates.

> Coherence — preventing two libraries from quietly satisfying the same trait
> with conflicting methods — is the hardest sub-problem here. Restrict
> ambiguous cases early (requirements §5); generalise once the rules have
> survived real codebases.

### 10.4 Generics and monomorphization

Generics are **monomorphized** (spec §9). Each distinct instantiation of a
generic function or type produces a fresh, separately typed copy at MIR-lowering
time (§13). `paco-types` records the set of instantiations encountered; the
mono-collector in `paco-mir` materializes them.

### 10.5 Static vs dynamic dispatch

A call where the receiver's type is statically known compiles to a direct call
(zero cost). A call through `&dyn Trait` or `Box<dyn Trait>` compiles to a
vtable lookup (visible cost). The type checker produces both call shapes; the
MIR carries the distinction (§12.3).

---

## 11. Pattern checking (`paco-match`)

**Input:** HIR + types.
**Output:** annotated `match` expressions; diagnostics for non-exhaustive or
unreachable arms.

The algorithm is the standard usefulness-based check (Maranget). It treats a
`match` as a matrix of patterns and asks two questions for each candidate row:

- Is this row *useful* (does any value reach this arm)? If not, it is
  unreachable.
- After consuming all rows, is the matrix *exhaustive* (does every value of the
  scrutinee type match somewhere)? If not, the match is non-exhaustive and the
  compiler reports a *witness* — an example value that escapes.

The check supports the pattern forms enumerated in `grammar.ebnf` §6:
wildcards, literals, ranges, paths (enum variants), structs, tuples, slices,
`@` bindings, and guards. Guards are treated as opaque to exhaustiveness: a
guarded arm never proves exhaustiveness on its own. This matches the spec
(§5) and AGENTS.md §5 (`non-exhaustive-match`).

---

## 12. Ownership and borrow check (`paco-borrow`) — the critical phase

This is the highest-risk component in the project (requirements §5). The
architecture treats it as three sub-phases that can ship independently. Phase 4
of the roadmap delivers (a); Phase 5 delivers (b) and (c).

### 12.1 Ownership tracking

Every value has exactly one owner. Moves transfer the owner and invalidate the
source. Use-after-move is an error.

Concretely, `paco-borrow` runs a definite-assignment-style dataflow over the
HIR: each binding is *initialized*, *moved-out*, or *partially-moved*. Reading
a moved binding is the canonical use-after-move error.

This sub-phase requires no lifetime reasoning and is therefore implementable
before borrowing exists in the language at all.

### 12.2 Borrow check

`&` and `&mut` introduce *loans* against an owner. The aliasing rule is:

- At any program point, an owner has either zero loans, or many shared loans,
  or exactly one mutable loan.
- A loan is invalidated when its lifetime ends.

The check is implemented as a flow-sensitive analysis on the HIR's lexical
structure. The borrow checker emits the lints listed in AGENTS.md §5:
`use-after-move`, `needless-move-self`, `shared-without-sync`.

### 12.3 Lifetime inference

The differentiator of Paco (ADR 0001, spec §16). The architecture commits to
two design rules:

- **Inference is a *constraint-solving* pass, not a *guessing* pass.** The
  borrow checker emits constraints (`'a outlives 'b`, `'a >= scope-of-x`)
  during the analysis; a separate solver finds the smallest assignment that
  satisfies all constraints. If no assignment exists, the offending constraint
  pinpoints the error location.
- **Failure suggests a fix.** When inference is genuinely ambiguous (multiple
  input references, return type can refer to any of them), the solver does not
  pick one. Instead, it surfaces the ambiguity with a diagnostic that contains
  *the exact `'a` annotation to paste*. The user confirms; the user does not
  reason about lifetimes from scratch.

The first viable implementation should match Rust's NLL (non-lexical lifetimes)
in expressive power, then layer additional heuristics on top (struct lifetime
elision rules, deeper inference through trait objects). Per ADR 0001, the
escalation order is: explicit checking first, inference heuristics second.

> Rationale for keeping borrow check on HIR rather than MIR. MIR is linearized
> and renamed; its diagnostics, when wrong, are unfixable by the user without a
> mental decompile. HIR keeps source names and lexical structure visible, which
> matters because lifetime errors are the errors users see most painfully.

### 12.4 Concurrency safety

The borrow checker enforces that data sent over a channel is moved
(`spawn`-captured values likewise). `Arc<T>` is the only sanctioned shared-
across-task wrapper; sending a bare `&T` across a channel is a `shared-without-
sync` error (AGENTS.md §5).

---

## 13. Paco IR (MIR) (`paco-mir`)

**Input:** HIR + types + borrow results.
**Output:** MIR — a linear, typed control-flow graph.

The MIR is the centerpiece of the architecture. Both backends consume it; the
`comptime` evaluator interprets it. Its design constrains everything that
follows.

### 13.1 Shape

A MIR `Body` represents one function (or one monomorphized instantiation of
one). A body is:

- A **list of locals**, each with a `Ty`. Locals include parameters, named
  bindings, and compiler-introduced temporaries.
- A **CFG**: basic blocks connected by terminators (`Goto`, `Branch`,
  `SwitchInt`, `Call`, `Return`, `Unreachable`).
- Each block is a sequence of **statements** ending in one **terminator**.

Statements are simple: `assign`, `storage-live`, `storage-dead`, `drop`.
Terminators are the only points where control flow can move.

The MIR is **not strict SSA**. Locals can be reassigned. This matches both
backends' tolerance (LLVM SSA-ifies in its own passes; Cranelift IR is SSA but
trivially built from this shape) and keeps lowering simple.

### 13.2 What the MIR makes explicit

The whole purpose of having a MIR is to make implicit things visible. Each of
the following implicit-in-HIR constructs becomes an explicit MIR instruction:

| Implicit in HIR              | Explicit in MIR                                  |
|------------------------------|--------------------------------------------------|
| End-of-scope cleanup         | `Drop(local)` terminator                         |
| Method call `x.f(a)`         | `Call(f_resolved, [&x, a])`                      |
| Static trait method call     | `Call(direct_fn, args)`                          |
| `dyn Trait` method call      | `Call(vtable_lookup(receiver, slot), args)`      |
| `?` propagation              | A `SwitchInt` on the discriminant + `Return`     |
| `for x in xs`                | A `loop` block calling `xs.next()` and matching  |
| Autoref / autoderef          | Explicit `Ref` / `Deref` rvalues                 |
| Coercions (e.g. `&[T;N] → &[T]`) | Explicit `Cast`                              |
| Monomorphization             | One body per `(generic_fn, type_args)` pair      |

A reviewer reading MIR can answer: *where does this destructor run?* and
*what does this dispatch cost?* — directly from the IR, without consulting the
source.

### 13.3 Layout decisions

- **Sized only.** All MIR locals have statically known sizes. Unsized values
  (`dyn Trait`, `[]T` values) only exist behind borrows or boxes; the MIR holds
  the borrow or box, never the unsized payload.
- **Calling convention is uniform.** Paco functions, trait methods, and
  closures use the same MIR `Call` form. The Rust-level calling convention
  (System V on Unix, Microsoft x64 on Windows) is a backend-lowering concern.
- **No exceptions.** There is no unwinding control flow in MIR. Panics call
  into the runtime, which either unwinds the task (in `spawn`) or aborts the
  process (in `main`). The MIR sees panic as an ordinary `Call` to a runtime
  intrinsic with `Unreachable` after it.

### 13.4 Why one MIR for two backends works

The MIR encodes Paco's semantics, not any backend's quirks. Lowering MIR to
Cranelift IR is a near-1:1 translation; lowering MIR to LLVM IR involves more
type wrangling (LLVM's struct layout, pointer/integer distinction) but no
semantic decisions. Crucially, the MIR is the **last point** where the two
backends share a representation, which is exactly where the differential test
suite (§19) anchors.

---

## 14. `comptime` evaluator (`paco-eval`)

**Input:** a MIR `Body` plus an evaluation context.
**Output:** a value (which may be a `Code` value — a fragment of MIR — that
re-enters the pipeline).

The evaluator is a tree-walking interpreter over the MIR. It supports the same
language users write, restricted to a deterministic, sandboxed subset:

- **No I/O.** Reading files, opening sockets, spawning tasks, or accessing the
  clock raises a `ComptimeError`.
- **No FFI.** Calls to extern functions are rejected.
- **Bounded loops by quota.** A configurable instruction budget aborts runaway
  evaluation (denial-of-service protection at build time).
- **Type introspection is a first-class operation.** The evaluator exposes the
  type table as ordinary Paco values, so `comptime fn for_each_field(T: type)`
  is just a loop.

The same evaluator is used (a) as the only execution path during Phases 1–6 of
the roadmap, before codegen exists, and (b) as the `comptime` engine forever
after. This double-duty is the whole reason MIR is designed to be interpretable
as well as compilable.

> The evaluator does *not* re-typecheck generated code. `Code` values returned
> from `comptime` re-enter the pipeline at HIR-lowering, so the type checker
> sees them. This avoids the "evaluator and compiler disagree on what the code
> means" failure mode.

---

## 15. Codegen

Both codegen crates implement the same trait:

```rust
trait Backend {
    fn lower_body(&mut self, body: &mir::Body) -> Result<()>;
    fn finish(self, target: &Target) -> Result<ObjectFile>;
}
```

The driver picks `paco-codegen-cranelift` for `paco build` and
`paco-codegen-llvm` for `paco build --release` (ADR 0003).

### 15.1 Cranelift (dev)

Cranelift consumes one function at a time. The lowering is straightforward
because MIR is already CFG-shaped. The dev backend skips optimization passes by
design — compile speed is the goal. Single-binary output is produced via
`cranelift-object`.

### 15.2 LLVM (release)

LLVM is reached through `inkwell` (safe Rust bindings). Lowering is similar in
shape but spends extra effort on struct layout (matching the target ABI) and
on emitting LLVM intrinsics where they help (`memcpy`, vector ops for §9 of
the spec).

The LLVM backend runs LLVM's standard optimization pipeline at `-O2` /`-O3`
levels. The Paco compiler does *not* duplicate optimizations that LLVM already
does. The MIR is the only place where Paco-specific optimizations (e.g.
collapsing `Result<T, !>` into `T`) may live.

### 15.3 Semantic parity

The two backends MUST produce programs with identical observable semantics. The
mitigation in ADR 0003 — a shared conformance suite — is operationalized in §19.

---

## 16. Linking and the runtime ABI (`paco-link`)

The compiler emits one or more object files (one per compilation unit) plus a
prelinked archive of the runtime (`libpaco_runtime.a`). The linker (`lld` by
default, the system linker as a fallback) merges them into the final static
binary. There is no dynamic linking against a Paco standard library.

The runtime exposes a small **C ABI** for the compiler to call into:

- `paco_rt_spawn(entry, args_ptr, args_size) -> TaskHandle`
- `paco_rt_join(TaskHandle) -> JoinResult`
- `paco_rt_channel_new(elem_size, capacity) -> Channel`
- `paco_rt_channel_send(Channel, value_ptr) -> SendResult`
- `paco_rt_channel_recv(Channel) -> RecvResult`
- `paco_rt_channel_close(Channel)`
- `paco_rt_panic(message_ptr, message_len) -> !`

These are the only symbols the compiler emits external references to. The
runtime is otherwise invisible at the language level (ADR 0004): users never
see futures, executors, or pollers.

> The ABI is intentionally minimal. Anything that can be expressed as
> ordinary Paco code (channel iteration, `select`) lives in the stdlib above
> these primitives, not in the runtime.

---

## 17. Diagnostics (`paco-diag`)

A diagnostic is:

```rust
struct Diagnostic {
    code: DiagCode,       // e.g. "PACO-E0042" or "use-after-move"
    severity: Severity,   // Error | Warning | Note | Help
    primary: Label,       // (Span, message)
    secondary: Vec<Label>,
    notes: Vec<String>,
    suggestion: Option<Suggestion>, // structured rewrite, when available
}
```

Rendering is delegated to `ariadne` or `codespan-reporting` (requirements §7).
The compiler never prints diagnostics from inside a phase; it collects them in
a `Reporter` and emits them in source order at the end. This guarantees
deterministic output across runs — important for golden tests (§19).

### 17.1 Codes and the lint catalogue

Every diagnostic carries a stable code. Errors use `PACO-Exxxx`; warnings use
the kebab-case lint names from AGENTS.md §5 (`use-after-move`,
`unhandled-result`, `non-exhaustive-match`, ...). Lint codes are silenceable
with `@allow("...")`; error codes are not.

### 17.2 Suggestions

Whenever the compiler can name the exact fix — the missing `'a` annotation, the
unhandled `Result` arm, the moved binding to clone — the diagnostic carries a
structured `Suggestion`. `paco fmt --fix` (future) consumes suggestions to
apply them automatically. This is the operational form of spec §16's promise:
"error messages suggest the exact annotation to paste".

---

## 18. Query model and incrementality

For the first phases of the roadmap, the driver is a **straight-line
pipeline**: it reads source, runs every phase in order, writes the binary, and
exits. This is simple to reason about and easy to test.

Once Phase 7 (codegen) lands, recomputing the entire pipeline for every change
becomes painful. The architecture earmarks `paco-driver` as the place where a
**query system** (à la `salsa`) replaces straight-line orchestration. Each
phase becomes a memoized function keyed by its inputs (file hash, dependency
revisions); only invalidated keys are recomputed.

The query system is **not** in Phase 0–7. It is added in Phase 11 (tooling),
alongside the LSP. Adding it early would couple the compiler to a complex
abstraction before its shape is known. Adding it late, against an already
modular set of phase crates, is straightforward.

> Phase crates are written today as pure functions over their inputs even when
> the driver does not yet memoize them. That discipline costs nothing now and
> makes the query migration nearly mechanical later.

---

## 19. Testing infrastructure (`paco-test-harness`)

The compiler ships four test bucket kinds, all run by `cargo test`:

1. **Unit tests** inside each crate. Standard Rust.
2. **Golden tests** of diagnostics. A `should_fail` `.paco` file plus an
   expected stderr (a `.stderr` file). The harness invokes the compiler and
   diffs the actual stderr against the expected one. Tests are pinned to
   diagnostic *codes*, not exact wording, where possible, so that error
   prose can be improved without churn.
3. **Run-output tests.** A `.paco` program plus a `.out` file with expected
   stdout. The harness builds and runs the binary; the output must match.
   These live in `tests/conformance/run_output/` and are the canonical examples
   from `examples/` over time.
4. **Differential tests.** During the phases where both the interpreter (§14)
   and the codegen pipeline can execute a program, the harness runs both and
   diffs their outputs. This is the operationalization of ADR 0003's parity
   mitigation: every conformance program runs through (interpreter,
   cranelift, llvm) and all three outputs must agree.

Property and fuzz testing (`proptest`, `cargo-fuzz`) target the parser and the
borrow checker. The parser invariant is "no input crashes the compiler"; the
borrow checker invariant is "if the program compiles, it is memory-safe under
the formal model" — checked against a hand-written reference checker on small
programs.

---

## 20. Phase-aligned implementation order

The phases of `requirements.md` §4 map onto the crates above as follows.

| Phase | What ships                                                | Crates touched                                             |
|-------|-----------------------------------------------------------|------------------------------------------------------------|
| 0     | Workspace, CI, empty stubs.                               | All — but only skeletons.                                  |
| 1     | "Hello world" and factorial via interpreter.              | `paco-syntax`, `paco-hir`, `paco-resolve`, `paco-types` (minimal), `paco-mir` (minimal), `paco-eval`, `paco-driver`, `paco-diag`. |
| 2     | Structs, enums, methods, static typing.                   | `paco-types`, `paco-hir` (full).                           |
| 3     | `match` with exhaustiveness, `if let`, `while let`.       | `paco-match`.                                              |
| 4     | Ownership + move + RAII (no borrowing yet).               | `paco-borrow` (sub-phase §12.1), `paco-mir` (`Drop`).      |
| 5     | Borrows + lifetime inference. **Highest risk.**           | `paco-borrow` (sub-phases §12.2, §12.3).                   |
| 6     | Traits, implicit satisfaction, generics, `dyn`, operators.| `paco-types` (trait solver), `paco-mir` (monomorphization).|
| 7     | Cranelift codegen; interpreter demoted to `comptime` only.| `paco-codegen-cranelift`, `paco-link`.                     |
| 8     | Runtime: scheduler, channels, `select`, `iter`.           | `runtime/`, plus MIR support for `spawn`/channels.         |
| 9     | `comptime` (full), `@derive`.                             | `paco-eval` (full), `paco-hir` (attribute expansion).      |
| 10    | LLVM backend for `--release`, cross-compilation.          | `paco-codegen-llvm`, `paco-link` (target selection).       |
| 11    | `paco fmt`, `paco test`, `paco doc`, LSP, query system.   | `paco-driver` (subcommands), `paco-syntax` (trivia).       |
| 12    | Standard library (`src/`).                                | (in Paco, not Rust.)                                       |

> The crate skeletons SHOULD be created up-front (Phase 0) even though most are
> empty, so that the dependency edges are fixed and visible. Adding a crate
> later forces an audit of who depends on whom.

---

## 21. Architectural recommendations on open design frictions

`spec.md` §18 lists six open design questions. Three of them have direct
consequences for the architecture; the recommendations below are *technical
preferences from the compiler's point of view*, not language decisions. The
human decides; the ADRs record.

### 21.1 Automatic error conversion (spec §18.5)

**Recommendation:** adopt a `From`-style trait, evaluated by `paco-types` and
desugared by `paco-hir`, that the `?` operator implicitly inserts.

**How it fits the pipeline.** `paco-hir` already desugars `?` (§8). Today the
desugaring assumes the inner and outer error types match. With a `From<E1>`
obligation generated at the same site, `paco-types` adds the conversion
obligation to the trait solver, which either finds the implementation or
reports a clear "no conversion from `E1` to `E2`" error. No new IR construct
is needed; the cost is one extra obligation per `?`.

**Why this and not macros.** Conversion is a typing question, not a syntactic
one; `paco-types` is the right home.

### 21.2 Collection construction syntax (spec §18.6)

**Recommendation:** pick **one** value-construction form. The compiler is
agnostic between `Vec::new()` and a literal form like `[]`, but it MUST reject
the ambiguous `[]T::new()` shape so the parser stays unambiguous.

**Why it matters for the architecture.** Parser ambiguity here would force the
grammar to be context-sensitive (looking past `]` to decide between a slice
type and a slice literal). That breaks the rule that the parser only needs
local lookahead. A single chosen form keeps the parser hand-written and small.

### 21.3 `comptime` scope (spec §18.4)

**Recommendation:** start with **`comptime` only**; no separate macro system.

**Why.** A second metaprogramming mechanism would require a second evaluator
or a token-tree macro engine. The MIR-interpreter approach scales: anything a
macro could do (code generation, attribute expansion, `@derive`) is already
expressible in `comptime`. Adding syntactic macros later is a non-breaking
extension if it ever becomes necessary; removing them is not.

### 21.4 Struct mutability, string slicing, data-analysis depth (spec §18.1–§18.3)

These three have no architectural consequence for the compiler at the level of
this document. Resolve them in spec / stdlib design; the compiler will follow.

---

## 22. Open architectural questions

These are unresolved at the time of writing. Each is *internal* to the
compiler — they do not change what users see, but they shape how the compiler
is built. They should be settled before the corresponding phase begins.

1. **HIR storage strategy.** Arena-allocated nodes with `&'hir` references
   (zero overhead, lifetime gymnastics in Rust), or `Vec`-indexed IDs
   (slightly less ergonomic, friendlier to incremental). *Decision needed
   before Phase 2 lands.*
2. **MIR ownership of types.** Does the MIR re-intern types in its own arena
   or share `paco-types`'s arena? *Decision needed before Phase 4.*
3. **Borrow check on HIR vs MIR.** This document recommends HIR (§12); Rust
   moved to MIR for NLL. The recommendation hinges on diagnostic quality. If
   the HIR analysis turns out to obscure flow-sensitive cases, the
   architecture pivots. *Re-evaluated during Phase 5.*
4. **Single-file vs multi-file parallelism.** The driver could compile files
   in parallel from Phase 0, or stay single-threaded until the query system
   lands. *Decision needed before Phase 7.*
5. **Linker selection.** `lld` everywhere vs. the system linker on macOS /
   Windows. Affects bootstrap friction. *Decision needed before Phase 7.*
6. **`paco fmt` host.** Does the formatter live in `paco-syntax` (sharing the
   lexer with trivia retained) or in a separate crate? *Decision needed
   before Phase 2 — `paco fmt` is the earliest tool to ship per requirements
   §4.*

---

## 23. Summary

The Paco compiler is a nine-stage pipeline (§3) implemented as a Rust workspace
of single-responsibility crates (§4). Two intermediate representations carry
typed information across the pipeline: an **HIR** that preserves source
structure and hosts the borrow checker (§8, §12), and an **MIR** that linearizes
control flow and serves as the contract between the frontend, the `comptime`
evaluator, and both codegen backends (§13). Diagnostics, span tracking, and
testing are first-class infrastructure rather than afterthoughts (§17, §19).

The hardest single component is the borrow checker with aggressive lifetime
inference (§12); the architecture's job there is to isolate it so that the rest
of the compiler can progress around it. The second hardest is the dual-backend
parity (§15); the architecture's job there is to keep the MIR thin and to test
behavioural equivalence ruthlessly.

Everything else — the lexer, the parser, the type checker, the IRs, the
codegen lowerings — is well-trodden ground. Following the phased plan in §20,
each piece can ship as a working, demonstrable milestone. That is the whole
point: the architecture serves the roadmap, and the roadmap guarantees that
something always runs.
