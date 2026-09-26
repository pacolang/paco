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
   `paco build --release` (RFC 0003). Frontend work cannot be duplicated. A
   single typed IR feeds both.
2. **Aggressive lifetime inference at the core** (RFC 0001). The borrow checker
   is the highest-risk component (requirements §5). The architecture isolates
   it so that progress on the rest of the language is not blocked by it.
3. **`comptime` reuses the compiler.** Compile-time evaluation must execute the
   same language users write, on the same IR the backends see (RFC 0005). The
   `comptime` evaluator is a consumer of the MIR, not a separate language.

Three secondary constraints shape the implementation:

- **Always something that runs** (requirements §4). A tree-walking interpreter
  was the only execution path until codegen existed. Since RFC 0027, programs
  always run compiled — `paco run` builds with Cranelift into a
  content-addressed cache and executes the binary (§18.1) — and the only
  interpreter is the `comptime` evaluator over MIR (§14).
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
through the pipeline, only privileged module paths (`stdlib::…`).

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
  [7b] Autodiff ─────────────► MIR (every `grad` expanded, §13.5)
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
├── paco-driver/               # binaries `paco` (launcher) and `paco-compile`: CLI, subcommands, orchestration
│   └── src/
│       ├── launcher.rs        # `paco`: cached `paco run` hits, else execs `paco-compile`
│       ├── main.rs            # `paco-compile`
│       ├── cache.rs           # `paco run`'s build cache (§18.1)
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
├── paco-comptime/             # `comptime` interpreter over MIR
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
- The surface syntax is still moving. `docs/grammar/grammar.ebnf` is the
  normative reference and is no longer marked "complete" — ADRs 0013–0017 added
  modules, visibility, constants, type aliases and FFI to it, and the open
  questions in spec §20 (const generics above all) will change it again. A
  generator that requires a frozen grammar file as input is an obstacle to that.

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
3. **Implicit nodes become explicit.** Method receivers (`&self`, `&mut self`,
   `self`) become regular parameters with explicit types. Receiverless
   functions inside a `struct` block are not methods; they are associated
   functions, resolved through the type's namespace.
4. **Attributes are interpreted.** `#[derive(...)]` expands into trait
   satisfactions to check; `#[test]`, `#[bench]`, `#[should_panic]` are recorded for
   the test harness; `#[allow(lint_code)]` annotates the surrounding scope.

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

**Path resolution.** A `Path` like `stdlib::io::read_file` resolves left-to-right
through the item table, with `stdlib` reserved for the standard library and other
URL-shaped roots (`example.com/team/json`) resolved via the dependency graph
from `paco.mod`. During Phases 0–6 the dependency tooling is deferred (RFC
0005); paths to external packages resolve through local relative directories
configured in `paco.mod`.

**Implicit trait satisfaction (RFC 0002, spec §6).** Name resolution does *not*
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

Traits are **implicitly satisfied, statically checked** (RFC 0002). For each
trait obligation `T: Trait`, the solver:

1. Looks up `Trait`'s required signatures.
2. Looks up `T`'s methods (in the type's own block plus in any in-scope
   `methods T { ... }` block).
3. Checks structural conformance: every required method exists with a
   compatible signature, where `self` parameters unify with `T`.

Compatibility rules and ambiguity policy follow RFC 0002. When two in-scope
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

### 10.4.1 Dimensions

A dimension argument is a `Type::Dim` holding a `ConstExpr` or `Dyn`, or, when
it is a single name, a `Type::Generic`. Three mechanisms keep dimension
equality cheap and decidable (RFC 0029):

- **Dimension atoms.** Every name a dimension can mention — a `const` or `dim`
  parameter, a witness, an opened `Dyn`, an opened existential, and each opaque
  `/` or `%` term — is interned as a `DimVarId(u32)` in a process-wide arena.
  Names that are only known at run time are *rigid atoms*: generic names of the
  form `display@id` (`x.dim0@17`), unique per program, equal only to
  themselves. Their origin span, how the program reads their value and their
  display name (a witness renames an anonymous `x.dim0` to `n`) are kept beside
  the typed module.
- **Polynomial arena.** A `ConstExpr` is a `PolyId` into a hash-consed arena
  of polynomials (`BTreeMap<Monomial, i64>`, monomials sorted by atom), so
  equal expressions share one id and equality is an integer compare. Checked
  arithmetic turns an overflowing coefficient into an opaque atom. The verdict
  of `e = a` is equal (`e - a == 0`), provably different (a non-zero constant,
  `PACO-E0336`) or cannot prove (`PACO-E0342`); nothing else is decided.
- **Opening.** Binding a value to an immutable name opens each `Dyn` position
  of its type into an anonymous atom (`x.dim0`), and each existential `?b` the
  producer returned into a fresh named atom (`x.b`); a destructured struct
  shares one atom per `?b` across its fields. A block whose result mentions a
  name bound inside it gives the result a fresh name, so no name outlives its
  scope; assigning a named value to a variable from an outer scope, or a
  conversion to `Dyn` anywhere but a `Dyn` parameter, is `PACO-E0343`. An
  anonymous atom may decay back to `Dyn`; a named one may not. Inside an item,
  each `dim` parameter is an atom too, so a callee's parameter of the same
  spelling never captures it.

The shape intrinsics (`dim`, `with_dims`, `as_dims`, `assume_dims`,
`erase_dims`) exist on every type with `fn extent(&self, axis: i64) -> i64`
(`stdlib::dims::Shaped`) that declares no method of the same name; their target
type comes from the expression's context (a `let` annotation, a parameter, the
return type).

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

The differentiator of Paco (RFC 0001, spec §16). The architecture commits to
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
elision rules, deeper inference through trait objects). Per RFC 0001, the
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
| Coercions (e.g. `&[T; N] → &[]T`) | Explicit `Cast`                             |
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

### 13.3.1 Hidden dimension arguments

After type checking, every dimension known only at run time is `Dyn` in an
instance's key: one instance serves every extent, and nothing symbolic counts
toward the instantiation limit. Its value travels as a leading `i64` argument:
one per `dim` parameter, and one per `const` position (scalar or pack element)
bound to `Dyn`, in type-argument order. The caller computes each from a
literal, a witness's local, a cached extent, a hidden argument of its own, or
checked arithmetic over them that panics in every profile instead of
overflowing; an anonymous `Dyn` with nothing to read passes `-1`, which is what
a `Dyn` position reads inside the item. The extent an opened name stands for
is read once, through the type's `extent`, when the value is bound. Closures
and task bodies receive the dimension values of their enclosing body as extra
captures. Compiler-generated calls cannot supply values, so a `drop` method and
an `iter fn` take none. `with_dims` compiles to one comparison per position
whose source and target names differ and a move; `assume_dims` compares only
in debug builds.

### 13.4 Why one MIR for two backends works

The MIR encodes Paco's semantics, not any backend's quirks. Lowering MIR to
Cranelift IR is a near-1:1 translation; lowering MIR to LLVM IR involves more
type wrangling (LLVM's struct layout, pointer/integer distinction) but no
semantic decisions. Crucially, the MIR is the **last point** where the two
backends share a representation, which is exactly where the differential test
suite (§19) anchors.

### 13.5 Autodiff (`paco-mir::autodiff`)

Gradients are a MIR-to-MIR transformation (RFC 0026), run by the driver after
lowering and before code generation whenever a program calls
`stdlib::autodiff::grad`; `paco check` runs it too for such programs, so every
rejection is reported before code generation, on Paco source. Its output is
ordinary MIR, so `paco run` (a Cranelift debug build), `paco build` and
`paco build --release` all execute the same derivative code, and the
transform can differentiate its own output (a gradient of a gradient).

- **Sites.** Lowering turns `grad(f, inputs)` into a call to `$grad:` + `f`'s
  instance, with `f`'s own arguments (hidden `dim` arguments included). The
  transform replaces each one, innermost first, by: create a tape, call
  `f`'s *augmented primal*, call `f`'s *pullback* with seed `1.0` and
  pointers to zeroed adjoints of each input, build `(output, gradients)` and
  free the tape.
- **Activity.** Per function and per set of differentiated parameters, a
  flow-insensitive dataflow over locals marks values *varied* (reached from a
  differentiated parameter) and *useful* (reaching the result or a `&mut`
  parameter); only *active* values (both) get adjoints. Calls use per-callee
  summaries of which parameters reach which outputs, iterated bottom-up to a
  fixed point so recursion converges. Integers, `bool`, strings, FP8 and the
  hidden dimension arguments are never active. Tape handles are tracked as
  carriers, so a pullback being differentiated sees that what it pops
  depends on what the primal pushed.
- **Primal and pullback.** `f__primal_<mask>(params…, tape)` is `f` plus
  pushes onto the tape: the predecessor of every block with several (the
  branch trace, which also covers loops and early returns), the old value of
  every local the pullback reads before it is overwritten, and the final value
  of those locals when `f` returns. `f__pullback_<mask>(tape, seeds…,
  adjoint pointers…)` replays `f`'s blocks backwards, popping as it goes, so
  every derivative rule reads its operands exactly as the primal saw them.
  Adjoints of float leaves (fields of structs, tuples and enum payloads, at
  static paths) are pullback locals; adjoints of slice elements live in
  shadow buffers the tape keys by the primal buffer's address. Borrows are
  followed to the place they point at: a `&mut` parameter is in-out (its
  pullback seeds from the caller's adjoint of its final value and returns the
  adjoint of its initial one).
- **Rules.** `+ - * /`, unary `-`, float casts (adjoint cast back), casts to
  integers (zero), aggregates and projections (fieldwise), and every float
  math method (`sqrt`, `exp`, `ln`, `sin`, `cos`, `tanh`, `powf`, `abs`,
  `min`, `max`), in the value's own width. Rules with an infinite partial
  (`sqrt`, `ln`, `/`, `powf`) run only for a nonzero adjoint, so a value the
  primal computed but did not use contributes zero, not NaN.
- **Calls.** A call with active arguments calls the callee's primal (sharing
  the tape) and, in reverse, its pullback. A function with a registered
  `#[derivative(of = f)]` is not differentiated: the derivative runs in the
  primal, its `Pullback` value goes on the tape, and its `pullback` runs in
  reverse. The derivatives of the tape operations themselves (push, pop) are
  what makes second order work.
- **Tape.** `paco-runtime-ffi/src/autodiff.rs`: a byte stack of saved values
  and branch ids, an adjoint stack for second order, the shadow buffers, and
  the frees deferred while it records — `paco_free` hands every buffer the
  primal drops to the innermost recording tape, so the pullback can still read
  it; all of it is released when the gradient finishes, which the ASan leg
  checks for every gradient conformance case.
- **Diagnostics.** `PACO-E0810`–`PACO-E0815`, each on the offending
  statement with one note per call from the `grad` site.

Measured on `tests/bench/autodiff_scalar.paco` (a 10^6-iteration scalar
loop, `cargo test -p paco-driver --release --test main bench_autodiff --
--ignored --nocapture`, median of 5): the primal takes 11.7 ms (LLVM
release) and 12.6 ms (Cranelift), the gradient 43.0 ms and 46.3 ms; the
tape peaks at 32 000 048 bytes, 32 bytes per iteration (one branch id and
three saved values). `tests/conformance/autodiff/projectile_fit/input.paco` (13 gradients through 40
Euler steps each) runs in 2.7 ms; each tape peaks at 3 880 bytes. The
gradient is about 3.8 times the primal because each push and pop is a runtime
call; checkpointing (recomputing instead of storing) is not needed at these
sizes.

---

## 14. `comptime` evaluator (`paco-comptime`)

**Input:** a MIR `Body` plus an evaluation context.
**Output:** a value (which may be a `Code` value — a fragment of MIR — that
re-enters the pipeline).

The evaluator is an interpreter over the MIR. It supports the same
language users write, restricted to a deterministic, sandboxed subset:

- **No I/O.** Reading files, opening sockets, spawning tasks, or accessing the
  clock raises a `ComptimeError`.
- **No FFI.** Calls to `extern` functions are rejected *inside `comptime`*. The
  language itself has an FFI (RFC 0017); this is a sandbox restriction, not an
  absence. A foreign call at build time would make compilation depend on the
  host's shared libraries and would not be reproducible.
- **Bounded loops by quota.** A configurable instruction budget aborts runaway
  evaluation (denial-of-service protection at build time).
- **Type introspection is a first-class operation.** The evaluator exposes the
  type table as ordinary Paco values, so `comptime fn for_each_field(T: type)`
  is just a loop.
- **Same arithmetic and formatting as compiled code.** Integer operations are
  checked exactly as a debug build checks them (overflow is a compile error at
  the MIR span); float conversions, `format_float` and string building call
  the same Rust functions in `paco-runtime` that the runtime's `extern "C"`
  entry points wrap. A differential test compares `comptime { f(x) }` with
  `f(x)` at run time over the comptime subset (§19).

It evaluates `comptime` blocks, `comptime fn` calls and `#[derive]` expansion
for every command (`check`, `run`, `build`), after borrow checking and before
code generation; results are substituted as constants and `Code` values
re-enter the pipeline. It is the only interpreter in the compiler: programs
never run on it (RFC 0027), which keeps it small — it needs only what the
sandbox allows.

> The evaluator does *not* re-typecheck generated code. `Code` values returned
> from `comptime` re-enter the pipeline at HIR-lowering, so the type checker
> sees them. This avoids the "evaluator and compiler disagree on what the code
> means" failure mode.

---

## 15. Codegen

Both codegen crates implement the same trait, `paco_mir::Backend`:

```rust
trait Backend {
    fn lower_body(&mut self, name: &str, body: &mir::Body) -> Result<(), String>;
    fn finish(self, target: &Target) -> Result<ObjectFile, String>;
}
```

`Target` carries the target triple (`None` for the host) and the build
profile. The driver picks `paco-codegen-cranelift` for `paco build` and
`paco-codegen-llvm` for `paco build --release` (RFC 0003); `--backend
cranelift|llvm` overrides that choice, and `--target <triple>` works with
either. Which types need drop/clone glue is decided once, in `paco_mir::glue`,
so both backends emit glue for exactly the same types.

### 15.1 Cranelift (dev)

Cranelift consumes one function at a time. The lowering is straightforward
because MIR is already CFG-shaped. The dev backend skips optimization passes by
design — compile speed is the goal. Single-binary output is produced via
`cranelift-object`.

### 15.2 LLVM (release)

Implemented in `paco-codegen-llvm` against LLVM 18 through `inkwell` (safe
Rust bindings). Lowering mirrors the Cranelift one block for block: every
local gets an entry-block `alloca` that `mem2reg` later promotes, addresses
are real `ptr` values (integers are converted with `inttoptr`/`ptrtoint` only
where MIR mixes them), aggregate copies use the `llvm.memcpy` intrinsic, and
debug-profile arithmetic uses the `*.with.overflow` intrinsics before
trapping.

The LLVM backend runs LLVM's standard `default<O3>` pipeline on release
builds and emits an object file for the host or for any triple LLVM was
built for (x86-64 and AArch64 are initialized). The Paco compiler does *not* duplicate optimizations that LLVM already
does. The MIR is the only place where Paco-specific optimizations (e.g.
collapsing `Result<T, !>` into `T`) may live.

### 15.3 Semantic parity

The two backends MUST produce programs with identical observable semantics. The
mitigation in RFC 0003 — a shared conformance suite — is operationalized in §19.

---

## 16. Linking and the runtime ABI (`paco-link`)

`paco build` needs no C compiler, linker driver or distribution package. The
code generators emit one object per backend; `paco-link` builds the whole
`ld.lld` command line itself and runs `rust-lld -flavor gnu`. The linker is
looked up in the distribution (`lib/paco/bin/rust-lld`), then in the rustup
toolchain that built `paco`, then as `ld.lld` on `PATH`.

**Link modes.** A program without `extern` blocks links **statically
against musl**: `crt1.o crti.o crtbegin.o <objects> libpaco_runtime.a
libunwind.a libc.a crtend.o crtn.o -static`, with `main` routed to the
runtime's `paco_rt_main`. The binary has no `PT_INTERP` and no `DT_NEEDED`
and runs on any Linux kernel of its architecture. A program with `extern`
blocks links **dynamically against the target's glibc**: the runtime
archive built for `<arch>-unknown-linux-gnu` provides the entry point
(`paco_rt_start`, calling `__libc_start_main`, so `crt1.o` and `libc6-dev`
are not needed), and the line names `libc.so.6`, `libm.so.6`,
`libgcc_s.so.1` and every `extern` library by path, with
`--dynamic-linker /lib64/ld-linux-x86-64.so.2` or
`/lib/ld-linux-aarch64.so.1`. Libraries are searched in `<root>/lib/<multiarch>`,
`<root>/usr/lib/<multiarch>`, `lib64`, `usr/lib64`, `usr/local/lib`, `lib`,
`usr/lib` (plus `LIBRARY_PATH` for host builds), where `<root>` is `/` or
`--sysroot`/`PACO_SYSROOT`. `--link static|dynamic` overrides the choice;
static mode with an `extern` block is `PACO-E0804`, a missing glibc
`PACO-E0802`, a missing `extern` library `PACO-E0803`, a missing
distribution file `PACO-E0805`. The native libraries Rust's standard
library needs on each target are checked against these lines by
`paco-link/tests/native_static_libs.rs`.

**Targets.** `--target <arch>-unknown-linux` (and the default, the host's
architecture) is completed to `-musl` or `-gnu` by the link mode; an
explicit `-musl`/`-gnu` suffix selects the mode. Both code generators
receive the completed triple. `x86_64` and `aarch64` are supported from
either host; cross-compiled programs are tested under `qemu-<arch>` when it
is installed (`paco_test_harness::run_for_target`). Every compiled
conformance program printed the same output on `aarch64` (static, both
backends, debug and release, under qemu 10.0) as on `x86_64` when checked
on 2026-09-24, so no case has a per-target expectation.

**Distribution layout** (`scripts/dist.sh`):

```
bin/paco
lib/paco/bin/rust-lld
lib/paco/lib/libLLVM.so.*        (rust-lld's shared library)
lib/paco/<arch>-unknown-linux-musl/{crt1.o,crti.o,crtn.o,crtbegin.o,crtend.o,libc.a,libunwind.a,libpaco_runtime.a}
lib/paco/<arch>-unknown-linux-gnu/libpaco_runtime.a
```

`paco` finds `lib/paco` next to its own executable. A development checkout
uses the rustup toolchain's `self-contained/` musl files and
`runtime/target/<target>/release/libpaco_runtime_ffi.a`; `paco-link/build.rs`
builds the host's musl and gnu runtimes, and `scripts/dist.sh` builds the
other architecture's (its allocator is C, compiled with clang against
Debian headers fetched by `scripts/fetch-sysroot.sh`).

**Runtime.** The runtime is Rust only (`runtime/paco-runtime-ffi`); no C
source is compiled when a program is built. Besides the concurrency
entry points (`paco_rt_spawn`, `paco_rt_join`, `paco_rt_channel`,
`paco_rt_send`, `paco_rt_recv`, generators and handle retain/release), it
exports the print, string, float-conversion and file helpers and the
process entry: `paco_rt_main` starts the scheduler, calls `__paco_entry`
and returns its result as the exit code when the `__paco_entry_returns_value`
byte the code generators emit is 1. stdout goes through one buffered writer
flushed at exit and before every stderr write. Float printing and
`float_to_string` take the value widened to `f64` plus a format code (0–3 the
small formats, 4 `f32`, 5 `f64`) and call `paco_runtime::format_float`, the
same function the `comptime` evaluator uses, so a float computed at compile
time prints the same text as one computed at run time.

**Panics.** `panic(..)` and every runtime check (bounds, division by zero,
debug overflow, `unwrap`) call `paco_rt_panic` with the message and the Paco
`file:line:column` from the MIR span. It prints `panic at file:line:col:
message`; inside a spawned task the panic ends the task and `join` returns
`Err`; in `main` the process exits with status 101. Debug builds carry line
tables and frame pointers, and the runtime prints a trace of Paco functions
after the panic line.

**Allocator.** Compiled code and the runtime allocate with `paco_alloc`,
`paco_calloc`, `paco_realloc` and `paco_free`, backed by mimalloc (also
Rust's global allocator). C libraries called through `extern` keep their own
allocator: Paco frees Paco memory, C frees C memory. The `alloc_churn`
benchmark (`tests/bench/alloc_churn.paco`, `cargo test -p paco-driver --test
bench_alloc -- --ignored`, median of 5 runs on x86_64) measured static
mimalloc 0.12 s, dynamic mimalloc 0.09 s, dynamic glibc `malloc` 0.12 s;
the tolerance is 10 % over glibc `malloc`. Short string copies are inlined
because musl's `memcpy` start-up cost otherwise dominated the static build.

**libm.** Static programs cannot call `libm`: math functions are reached
only through `extern` blocks, which link dynamically against glibc, and
compiled code emits no libm calls (float `%` is not compiled yet; when it is,
it lowers to `fmod`, which is exact in musl and glibc alike). musl's
rounding of transcendental functions therefore never shows;
`tests/conformance/numerics/libm_boundaries` pins glibc's results.

**Sanitizers.** `PACO_SANITIZE=address` forces dynamic mode and links
through the system `cc -fsanitize=address` with the runtime built with the
`system-alloc` feature (libc `malloc`), so LeakSanitizer sees every
allocation. It is the one path that needs a C compiler; only the test
suite uses it. `PACO_SYSTEM_ALLOC=1` links that runtime without the
sanitizer, for allocator comparisons.

**macOS.** `aarch64` and `x86_64` macOS are native hosts and targets on
both backends. Programs link with `rust-lld -flavor darwin` against the
macOS SDK (`SDKROOT`, else the SDK of the developer directory
`xcode-select` records, else `xcrun --show-sdk-path`): `-platform_version
macos 11.0`, `-lSystem -liconv -dead_strip_dylibs`, `_main` aliased to
`_paco_rt_main`, so a binary loads only `libSystem` and no C compiler is
run (through `cc` a hello-world link took 323 ms on the CI runner, through
`rust-lld` 82 ms). `extern` libraries are found as `.dylib`, `.tbd` or `.a`
in `LIBRARY_PATH`, the SDK's `usr/lib`, `/opt/homebrew/lib` and
`/usr/local/lib`; a missing one is `PACO-E0803` naming those directories.
Mach-O links leave DWARF in the
object files, so a debug link runs `dsymutil --flat` into `<output>.dwarf`,
which panic traces read (the `paco run` cache keeps it beside the binary);
the runtime archive is built with its DWARF stripped so `dsymutil` only
copies the program's. The
Cranelift line tables use section-relative relocations with object
addresses in place, as LLVM writes Mach-O DWARF, and a named compile unit,
so the linker records the object in the debug map. Linux targets build from macOS
through the `rust-lld` path above; the Linux runtime's C allocator compiles
with the host clang against Debian headers (`scripts/runtime-env.sh`, with
`llvm-ar` from `rustup component add llvm-tools`). LeakSanitizer does not
exist on macOS, so the AddressSanitizer check there covers double frees and
use-after-free only.

**Windows.** `x86_64` Windows links PE/COFF with `rust-lld -flavor link`:
entry `mainCRTStartup`, `main` aliased to `paco_rt_main`, the CRT and SDK
import libraries (`kernel32`, `advapi32`, `ntdll`, `userenv`, `ws2_32`,
`dbghelp`, `bcrypt`, `msvcrt`) from the MSVC environment `cc` locates
(`LIB`), `extern` libraries as `<name>.lib` (`PACO-E0803` names the searched
directories), and `/debug:dwarf` for debug builds. Executables end in
`.exe`, also in the `paco run` cache.

Self-contained Mach-O and COFF linking, without the platform SDK, stays out
of scope: `libSystem`, the Windows CRT and the import libraries cannot be
redistributed.

---

## 17. Diagnostics (`paco-diag`)

A diagnostic is:

```rust
struct Diagnostic {
    code: DiagCode,       // e.g. "PACO-E0042" or "use-after-move"
    severity: Severity,   // Error | Warning | Note | Help
    primary: Label,       // (Span, message)
    secondary: Vec<Label>, // notes with a location: where each dimension was bound
    notes: Vec<String>,
    suggestion: Option<Suggestion>, // structured rewrite, when available
    fixes: Vec<Fix>,      // at most two, ranked; each a list of byte edits
}
```

`paco check`, `paco build` and `paco run` take `--format=json`, which writes one
object per diagnostic and line (`code`, `severity`, `message`, `file`, `line`,
`column`, `notes`, `fixes`); applying a fix's edits to the unchanged source
yields the repaired program. `paco explain <code>` prints a code's registry
entry (embedded at build time), and `paco shapes <file>` prints the type of
every `let` with dimensions and where each name in it was bound.

Rendering is delegated to `ariadne` or `codespan-reporting` (requirements §7).
The compiler never prints diagnostics from inside a phase; it collects them in
a `Reporter` and emits them in source order at the end. This guarantees
deterministic output across runs — important for golden tests (§19).

### 17.1 Codes and the lint catalogue

Every diagnostic carries a stable code. Errors use `PACO-Exxxx`; warnings use
the kebab-case lint names from AGENTS.md §5 (`use-after-move`,
`unhandled-result`, `non-exhaustive-match`, ...). Lint codes are silenceable
with `#[allow(...)]`; error codes are not.

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

### 18.1 Build cache and `paco run`

`paco run` is `paco build` (Cranelift, debug) plus execution, with a
content-addressed cache in front (RFC 0027), modelled on Go's `GOCACHE`. The
cache directory is `PACO_CACHE`, else `$XDG_CACHE_HOME/paco`, else
`~/.cache/paco`. An *index key* hashes the non-file inputs (compiler version,
entry path, profile, backend, target, link mode, sysroot, sanitizer, runtime
archive); it names a manifest listing every source file the last build read
(entry, imported modules, `stdlib`) with its content hash. If every listed file
still hashes the same, the manifest's output key names the cached binary,
which is executed without lexing, compiling or linking. Otherwise the program
is built, and binary and manifest are published by write-to-temp and rename,
so concurrent runs never see partial files. Entries unused for 30 days, then
the least recently used beyond 1 GiB, are pruned; `paco clean --cache` empties
the cache. Hashes are XXH3-128.

The `paco` executable is a small launcher that does not load LLVM: it parses
`paco run [FILE] [-- ARGS]`, looks the program up in the cache and, on a hit,
`exec`s the cached binary. Anything else, including a miss, is handed to
`paco-compile` next to it, which holds the whole compiler. Cache keys name
`paco-compile`'s path, size and modification time. `rust-lld` links with
`--threads=1`: for programs this size its thread pool costs more than it saves.

Latency targets for hello world: at most 10 ms of overhead on a cache hit and
at most 150 ms end to end on a miss, and `paco run` at least as fast as the
equivalent Rust workflow. Measured on the reference machine (release `paco`,
medians; `compiler/paco-driver/tests/run_latency.rs` and
`scripts/bench_run_latency.sh`):

| Scenario | `paco run` | Rust |
|---|---|---|
| hello world, cache hit (binary alone 1.7 ms) | 5.7 ms | `cargo run` 36.8 ms |
| hello world, miss | 53.2 ms | `rustc` + run 270.1 ms, fresh `cargo run` 356.2 ms |
| medium program, cache hit | 6.4 ms | `cargo run` 37.4 ms |
| medium program, miss | 71.9 ms | `rustc` + run 326.2 ms, fresh `cargo run` 446.7 ms |

The whole-program cache is the first level of incrementality. The query system
below is the second.

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
   These live in `tests/conformance/run_output/` and are meant to grow into a
   canonical, realistic corpus over time.
4. **Differential tests.** Every conformance program runs through Cranelift
   and LLVM, debug and release, statically and dynamically linked, and every
   output must equal the others and the case's hand-reviewed expected output
   (RFC 0003's parity mitigation). There is no interpreter leg (RFC 0027):
   since a MIR lowering bug gives the same wrong answer on every backend, the
   review of expected output is what catches it. A second differential
   compares the `comptime` evaluator (§14) with compiled code over the
   comptime subset (`tests/conformance/comptime/`).

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
| 1     | "Hello world" and factorial via interpreter.              | `paco-syntax`, `paco-hir`, `paco-resolve`, `paco-types` (minimal), `paco-mir` (minimal), `paco-eval` (removed by RFC 0027), `paco-driver`, `paco-diag`. |
| 2     | Structs, enums, methods, static typing.                   | `paco-types`, `paco-hir` (full).                           |
| 2b    | Modules, `pub`, `const`, reduced-precision floats, `extern` declarations. | `paco-syntax` (lexer/parser), `paco-resolve` (module graph, visibility), `paco-types` (float types). |
| 3     | `match` with exhaustiveness, `if let`, `while let`.       | `paco-match`.                                              |
| 4     | Ownership + move + RAII (no borrowing yet).               | `paco-borrow` (sub-phase §12.1), `paco-mir` (`Drop`).      |
| 5     | Borrows + lifetime inference. **Highest risk.**           | `paco-borrow` (sub-phases §12.2, §12.3).                   |
| 6     | Traits, implicit satisfaction, generics, `dyn`, operators.| `paco-types` (trait solver), `paco-mir` (monomorphization).|
| 7     | Cranelift codegen; `paco run` compiles and executes (RFC 0027).| `paco-codegen-cranelift`, `paco-link`.                     |
| 8     | Runtime: scheduler, channels, `select`, `iter`.           | `runtime/`, plus MIR support for `spawn`/channels.         |
| 9     | `comptime` (full), `#[derive]`.                             | `paco-comptime` (MIR interpreter, RFC 0027), `paco-hir` (attribute expansion). |
| 10    | LLVM backend for `--release`, cross-compilation.          | `paco-codegen-llvm`, `paco-link` (target selection).       |
| 11    | `paco fmt`, `paco test`, `paco doc`, LSP, query system.   | `paco-driver` (subcommands), `paco-syntax` (trivia).       |
| 12    | Standard library (`src/`), including `stdlib::blas`.         | (in Paco, not Rust.)                                       |
| 13    | Shapes; native reverse-mode autodiff in every build mode (tensors live in `github.com/pacolang/tensor`). | `paco-types` (const generic unification, `Differentiable`, `#[derivative]`), `paco-mir` (autodiff transform, RFC 0026), `runtime/` (tape). |
| 14    | GPU: PTX/SPIR-V codegen. **Largest and riskiest.**        | a third `paco-codegen-*` crate; `paco-mir` (address spaces). |

> The crate skeletons SHOULD be created up-front (Phase 0) even though most are
> empty, so that the dependency edges are fixed and visible. Adding a crate
> later forces an audit of who depends on whom.

---

## 21. Architectural notes on decided frictions

The frictions this section once tracked as open — automatic error conversion,
collection construction, `comptime` scope, struct mutability, string slicing and
data-analysis depth — are all decided. They are recorded in ADRs 0006–0011, and
their architectural consequences are folded into the relevant sections above
(§10 for the trait solver, §14 for `comptime`).

Two decisions taken later have architectural weight and no equivalent section
here yet:

- **RFC 0018 (blocking foreign calls).** The runtime needs a second thread pool,
  distinct from the M:N worker pool, plus a `spawn_blocking` entry point in the
  runtime ABI (§16). The `blocking-call-on-worker` lint is a `paco-types` check,
  not a runtime concern.
- **RFC 0019 (GPU strategy).** MIR must stay lowerable to a device target. It is
  already the shared input to two backends (§13); a third must remain possible.
  No MIR construct should assume a host-only execution model without that being
  a recorded decision.

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
