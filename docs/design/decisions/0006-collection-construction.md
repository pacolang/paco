# Collection Construction: Associated Functions as the Canonical Constructor Form

* Status: accepted
* Deciders: Core Developers
* Date: 2026-05-26

## Context and Problem Statement

Many languages provide dedicated shorthand syntax for constructing collections
(e.g., `[1, 2, 3]` for lists in Python, `vec![...]` macros in Rust, `{}` for
maps in JavaScript). While ergonomic for small literals, these shorthands
proliferate special grammar rules and create an uneven surface where common
standard-library types get privileged syntax unavailable to user-defined types.

Paco's standard library includes collection types such as `Vec<T>`, `Map<K, V>`,
and `Set<T>`. The language needs a single, consistent way to construct instances
of all types — including these — that fits the existing design principles.

## Decision Drivers

* **Consistency**: A uniform construction pattern across all types removes special
  cases from both the grammar and the programmer's mental model.
* **Visible cost**: Explicit function calls make heap allocation obvious, in line
  with Paco's "no hidden cost" principle.
* **Grammar simplicity**: Avoiding shorthand literal syntax for collections keeps
  the lexical and syntactic grammars smaller and easier to implement.
* **Precedent**: The language already constructs `Rc<T>` and `String` via the same
  associated-function pattern (`Rc::new(value)`, `String::new("hi")`).

## Considered Options

* **Option 1: Literal syntax** — provide `[1, 2, 3]` for `Vec`, `{k: v}` for
  `Map`, etc.
* **Option 2: Macro-based construction** — provide `vec![...]`, `map!{...}` style
  macros.
* **Option 3: Associated `new` function** — use `Vec::new()` (and then `push`) as
  the sole construction path; no special grammar rule.

## Decision Outcome

Chosen option: **Option 3** — `Vec::new()` is the canonical form. The same
pattern applies to every type: `Map::new()`, `Set::new()`, `Rc::new(value)`, etc.

These are *associated functions* (no receiver), defined inside the type's block,
following the method-placement convention established in ADR 0002.

There is no shorthand literal syntax for collection construction.

```paco
let v = Vec::new()
v.push(1)
v.push(2)

let m = Map::new()
m.insert("host", "localhost")
m.insert("port", "8080")

// Idiomatic alternative for building from a known sequence:
let squares = (1..=5).map(|n| n * n).collect<Vec<int>>()
```

### Consequences

* **Good (Grammar simplicity)**: No special collection-literal production rule is
  needed in the lexical or syntactic grammar.
* **Good (Visible cost)**: Every allocation is a recognizable function call.
* **Good (Uniformity)**: Programmers learn one pattern (`Type::new(...)`) and apply
  it to all types, including user-defined ones.
* **Bad (Verbosity for small inline literals)**: Writing `Vec::new()` followed by
  several `push` calls is more verbose than `[1, 2, 3]` when the elements are
  known up front.
* **Mitigation**: Idiomatic Paco builds collections from iterators and `collect()`,
  which is concise. Small fully-known literals are uncommon in the target use cases
  (concurrent services, data pipelines, games).

## Pros and Cons of the Options

### Option 1: Literal syntax

* Good: Very concise for small, fully-known collections.
* Bad: Creates privileged syntax for a subset of types, inconsistent with
  user-defined collection types.
* Bad: Complicates the grammar and the formatter.

### Option 2: Macro-based construction

* Good: Slightly more consistent than built-in literals (any type could define a
  macro).
* Bad: Requires a macro system beyond `comptime`, adding language surface area.

### Option 3: Associated `new` function

* Good: Perfectly uniform — no type gets special grammar treatment.
* Good: Allocation is always a visible, named function call.
* Bad: More verbose for constructing a small, fully-known collection inline.
