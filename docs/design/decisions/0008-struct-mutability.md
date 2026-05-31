# Struct Mutability: Binding-Level Control Only

* Status: accepted
* Deciders: Core Developers
* Date: 2026-05-31

## Context and Problem Statement

A struct definition has fields. The question is: what controls whether those
fields can be mutated — the *binding* that holds the struct value, or per-field
annotations inside the struct definition?

Languages handle this differently. Rust uses binding-level mutability exclusively
(`let mut`). Some functional languages allow per-field `mut` modifiers. The
choice has deep consequences for the borrow checker, the mental model, and the
consistency of the language surface.

## Decision Drivers

* **Low mental cost**: a single, learnable rule beats two overlapping rules.
* **Consistency with receivers**: method receivers already express mutability
  explicitly (`self&` vs `self&mut`). The binding model follows the same logic.
* **Borrow checker tractability**: per-field mutability creates hairy interactions
  with `&mut` borrows that are costly to specify and implement correctly.
* **"Visible cost" principle**: mutation is always requested explicitly, never
  inferred from field declarations the reader may not remember.

## Considered Options

* **Option A: Binding-level only (`let mut`)** — the whole struct is mutable or
  immutable based on how it is bound. No per-field `mut` modifiers.
* **Option B: Per-field `mut` modifiers** — individual fields declare `mut`
  independently of the binding, allowing fine-grained control.

## Decision Outcome

Chosen option: **Option A** — mutability is a property of the *binding*, not of
the type or its fields.

```paco
let cfg = Config { host: "localhost", port: 8080 }
cfg.port = 9090    // ERROR: `cfg` is an immutable binding

let mut cfg2 = Config { host: "localhost", port: 8080 }
cfg2.port = 9090   // OK
cfg2.host = "prod" // OK — the entire struct is mutable
```

Per-field `mut` modifiers do not exist. There is one rule: bind with `let mut`
to mutate. The same principle governs method receivers:

* `self&` — read-only access to `self`; the caller's binding need not be `mut`.
* `self&mut` — mutable access; the compiler requires the caller's binding is
  `let mut` (or the caller itself holds a `&mut`).

For the pattern of "one field that may change while the rest is constant," the
idiomatic solution is explicit interior mutability (`Rc<T>`, `Arc<T>`), which
makes the cost visible rather than hiding it inside a field modifier. Since `Rc`
and `Arc` enforce immutability of shared contents, developers wrap the target
data inside standard library interior mutability containers:
* `Cell<T>` — for copyable types, avoiding runtime checks.
* `RefCell<T>` — for general types under single-threaded `Rc` (monitored via borrow checking).
* `Mutex<T>` / `RwLock<T>` — for multi-threaded `Arc` access, ensuring synchronization.

### Consequences

* **Good (Single rule)**: "want to mutate? use `let mut`" is one sentence. There
  is no second rule about which fields are exempt.
* **Good (Borrow checker simplicity)**: `&mut Struct` unambiguously means "the
  caller may mutate the entire struct." No need to track per-field write
  permissions across call boundaries.
* **Good (Consistency)**: the receiver syntax (`self&` / `self&mut`) and the
  binding syntax (`let` / `let mut`) express mutability the same way.
* **Bad (Coarse granularity)**: there is no way to express "the caller can change
  `port` but not `host`" directly in the type. That invariant must be enforced
  through the method API (making the fields private and exposing only
  `set_port(&mut self, ...)`).
* **Mitigation**: encapsulation via private fields + public methods is the
  idiomatic mechanism for type-level invariants. This is consistent with Paco's
  "traits as capabilities, not inheritance" philosophy.

## Pros and Cons of the Options

### Option A: Binding-level only

* Good: One rule, easy to teach, consistent with receiver syntax.
* Good: `&mut T` has an unambiguous meaning for the whole type.
* Bad: Cannot express partial mutability without encapsulation.

### Option B: Per-field `mut` modifiers

* Good: Fine-grained control; useful for the "mostly read, one counter" pattern.
* Bad: Interaction with `&mut T` borrows requires specifying whether `&mut`
  grants access to immutable fields — a significant specification complexity.
* Bad: Two overlapping mutability rules (`let mut` and field `mut`) that
  developers must understand simultaneously.
