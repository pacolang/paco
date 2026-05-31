# String Slicing: Explicit API with `Option`, No Direct Range Indexing

* Status: accepted
* Deciders: Core Developers
* Date: 2026-05-31

## Context and Problem Statement

Paco strings are always valid UTF-8. A Unicode character may occupy 1–4 bytes.
If a byte-range slice `s[n..m]` is permitted and the bounds fall in the middle
of a multi-byte code point, the result would be an invalid UTF-8 sequence.

Three responses to this are possible:
1. Allow the syntax and panic at runtime when boundaries are invalid.
2. Allow the syntax but return `Option` so the failure is a value, not a crash.
3. Disallow direct range indexing on strings entirely; require an explicit API.

Paco's design principles — "no implicit panics" and "visible cost" — must guide
the choice.

## Decision Drivers

* **"No implicit panics"**: `panic` is reserved for unrecoverable invariant
  violations, not for programmer errors that can be handled. A bad byte boundary
  is a handleable error, not an unrecoverable crash.
* **"Visible cost"**: the cost and intent of an operation must be visible at the
  call site. Accessing raw bytes vs. a validated UTF-8 sub-string are different
  operations with different costs; they should look different in code.
* **Consistency with `Option`/`Result`**: fallible operations return values, not
  exceptions. String boundary failures follow the same convention.
* **Lint support**: the `string-byte-boundary` lint (already specified) catches
  statically provable boundary violations. The API handles the dynamic case.

## Considered Options

* **Option A: `s[n..m]` exists and panics on invalid boundary** — familiar
  syntax, runtime panic on misuse.
* **Option B: `s[n..m]` exists and returns `Option<&string>`** — keeps the
  indexing syntax but makes the failure a value.
* **Option C: No `s[n..m]` on strings; explicit API only** — `s.get(0..n)`
  returns `Option<&string>`; `s.as_bytes()[0..n]` returns `[]byte`.

## Decision Outcome

Chosen option: **Option C** — there is no direct range-indexing syntax on strings.
The following API is the canonical interface:

```paco
let s = "café"

// UTF-8-safe slicing — returns Option, never panics
match s.get(0..3) {
    Some(sub) => print(sub),    // "caf" (3 bytes, valid boundary)
    None      => handle_error(),
}

// Attempting a bad boundary returns None — no panic
let bad = s.get(0..4)   // None: byte 4 cuts inside 'é' (2-byte codepoint)

// Raw byte access — no UTF-8 concern, explicit intent
let raw: []byte = s.as_bytes()
let slice = raw[0..3]           // []byte, always valid (bytes don't validate encoding)

// Character-level iteration — the idiomatic default
for c in s.chars() { ... }
for (i, c) in s.chars().enumerate() { ... }
```

The integer-indexed `s[n]` (single-byte access on a string) is also absent;
use `s.bytes().nth(n)` (returning `Option<byte>`) or iterate with `s.chars()`.

### Why not Option B

Option B preserves the familiar `s[n..m]` syntax but gives it non-obvious
semantics: `s[0..3]` returns an `Option`, while `arr[0..3]` on a slice returns
a direct value. This asymmetry would confuse readers and break the principle that
the same syntax should have consistent semantics across types.

### Consequences

* **Good (No implicit panics)**: boundary failures are always `None`, never a
  crash. Consistent with the `Result`/`Option` contract throughout the language.
* **Good (Visible intent)**: `s.get(0..n)` and `s.as_bytes()[0..n]` signal
  different operations at a glance. There is no ambiguous `s[0..n]` that might
  mean either.
* **Good (Grammar simplicity)**: strings do not need to participate in the
  general slice-indexing syntax with special-case runtime checks.
* **Bad (Unfamiliar for newcomers)**: programmers coming from Python, Go, or Rust
  expect `s[0:3]` or `&s[0..3]` to work on strings. The adjustment period is
  real.
* **Mitigation**: the `string-byte-boundary` lint and clear compiler error
  messages ("strings do not support direct range indexing; use `s.get(0..n)`")
  guide developers quickly to the correct API.

## Pros and Cons of the Options

### Option A: `s[n..m]` with runtime panic

* Good: Familiar syntax from Python, Go, and Rust.
* Bad: Violates "no implicit panics"; a bad boundary is a handleable mistake,
  not an unrecoverable invariant violation.

### Option B: `s[n..m]` returns `Option`

* Good: Familiar syntax; failure is a value.
* Bad: Asymmetry with `arr[n..m]` on slices (which returns a direct value).
  Same syntax, different return types — confusing and hard to document cleanly.

### Option C: Explicit API only

* Good: No asymmetry; clear, intent-revealing names; no implicit panics.
* Bad: More unfamiliar to programmers used to direct string indexing.
