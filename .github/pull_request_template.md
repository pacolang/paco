## What this does

<!-- One sentence. If it resolves an issue: "Closes #123". -->

## Why

<!-- The root cause if this is a fix, or the spec section / RFC if this is a feature. -->

## Quality gate

- [ ] **G0 — Root cause, not symptom.** If this is a bug fix, the change fixes the cause; the test only proves it.
- [ ] **G1 — Proof in the same PR.** A test exists that failed before this change and passes now.
- [ ] **G2 — Clean locally.** `cargo check --workspace`, `cargo clippy --workspace -- -D warnings` and `cargo test --workspace` all pass in `compiler/`.
- [ ] **G3 — Docs do not drift.** If this changes syntax or observable semantics, `docs/design/spec.md`, `docs/grammar/`, `AGENTS.md` and `context.md` are updated **in this PR** — not in a follow-up docs PR.
- [ ] **G4 — Diagnostics are registered.** Any new or changed `PACO-E/W/Lxxxx` has an entry in `docs/diagnostics/registry.toml` with explanation, cause, fix and `spec_ref`.
- [ ] **G5 — No silent stub.** No incomplete function without an explicit `todo!()`/`unimplemented!()`. "Works in the happy path" does not close an issue whose acceptance criteria asked for the error case.

## Tests

<!-- The exact command run, and its result. -->

## Deliberately not covered

<!-- What was left out on purpose, so it does not read as a regression later. -->
