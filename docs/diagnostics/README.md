# Diagnostics

A Paco diagnostic is written for two readers: the person who will fix the code,
and the program that wrote it. Both need the same five things — a stable code, a
plain explanation, the cause, a suggested fix, and a pointer to the rule being
enforced.

`registry.toml` is the single source of truth for every code. A code, once
emitted in a release, **never changes meaning**. If a check is refined past the
point where its old wording applies, the old code is marked `status = "retired"`
and points at its successor. Codes are never recycled.

## Bands

Allocated by compiler stage. The first three bands describe what the compiler
already emits; the rest are reserved so that numbering does not have to be
rearranged later.

| Band | Stage | Crate | State |
|------|-------|-------|-------|
| `E01xx` | lexical and syntactic | `paco-syntax` | in use |
| `E02xx` | name resolution | `paco-resolve` | in use |
| `E03xx` | type checking | `paco-types` | in use |
| `E04xx` | pattern matching | `paco-match` | in use (emitted from `paco-types`) |
| `E05xx` | ownership and moves | `paco-borrow` | **reserved, nothing emitted** |
| `E06xx` | borrowing and lifetimes | `paco-borrow` | **reserved, nothing emitted** |
| `E07xx` | comptime | `paco-comptime` | reserved |
| `E08xx` | codegen, toolchain and autodiff | `paco-codegen-*`, `paco-link`, `paco-mir` | in use |
| `E09xx` | FFI and unsafe | `paco-types` | reserved |
| `E10xx` | modules and visibility | `paco-driver` | in use |
| `E11xx` | concurrency | `paco-types` | reserved |
| `E12xx` | tooling | `paco-driver` | reserved |
| `Wxxxx` | warnings | — | same banding |
| `Lxxxx` | named lints from `AGENTS.md` §5 | — | one code per lint |

> `paco-borrow` is the largest crate in the compiler and the component the risk
> register rates highest, and it currently emits no coded diagnostic at all.
> Closing that is the first task in this area.

## Required fields

Every entry carries `message_template`, `severity`, `explanation`, `cause`,
`fix`, `spec_ref`, `introduced_in` and `status`. `adr_ref` and `lint` are
optional. A retired entry (`status = "retired"`) names its successor in
`superseded_by`, which `paco explain` prints instead of the explanation; an
entry retired because the check itself disappeared has no successor, and
`paco explain` prints why. A code with no `spec_ref` is a bug: if no written rule is being
enforced, the diagnostic should not exist.
