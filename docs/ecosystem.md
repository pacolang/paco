# The Paco ecosystem

What may live in `stdlib`, what an official library is, and the rules that keep
the compiler from depending on either. The decision and its rationale are RFC
0030; this page is the checklist contributors and CI actually use. See
`https://github.com/pacolang/rfcs/blob/main/text/0030-repository-organization-and-stdlib-scope.md` for
why.

## Repositories

One organization, `github.com/pacolang`. One repository per project:

- `pacolang/paco` — the core: compiler, runtime and `stdlib`. One version, one
  release, one test suite.
- `pacolang/<name>` — one repository per official library, its own version,
  changelog and maintainers. Repository name defaults to the module name a
  program imports (`github.com/pacolang/string` for `stdlib::string` moved out,
  for example); a repository grouping several closely-related, separately-
  importable modules (so depending on one does not pull in another's link
  requirements) is the exception, not the default.

Nothing else. No `pacolang/.github` shared workflow repository and no
`pacolang/template` repository until a second official library actually exists
to justify factoring shared setup out of the first one's own CI.

## Standard-library admission reasons

A module or type stays in `stdlib` only for a reason checkable from its own
source:

| # | Reason | Checkable from |
|---|--------|-----------------|
| 1 | `#[builtin]` | A literal `#[builtin(...)]` attribute, or the compiler hardcoding recognition of the item's exact name/path in `paco-types` or `paco-mir` |
| 2 | Prelude entry | The item is in the spec §13 prelude table (in scope without import), or the module exists only to make already-prelude names explicitly importable |
| 3 | `paco_rt_*`/`extern` symbol | The module wraps a runtime-intrinsic function name the compiler resolves directly (`paco-resolve`'s builtin name list), or an `extern "C"` block |
| 4 | No compiler feature needs it, but every official library's public signature does | Ordinary Paco code kept in `stdlib` as the common vocabulary official libraries are written in terms of — the one reason that needs a human judgment call, recorded here |

Reasons 1–3 are structural and mechanically checked (`scripts/check_std_scope.py`,
the static-link CI job). Reason 4 is a documented judgment, not re-litigated per
pull request. **No type is admitted because a compiler feature is merely useful
with it** — const-generic shape checking, `Differentiable`, `#[derivative]`,
SIMD, the float types and the numeric traits all work on any type, and none of
them is a reason to keep a specific type in `stdlib`.

## Every `stdlib` module today

| Module | Path | Reasons | Notes |
|--------|------|---------|-------|
| `core` | `stdlib/core/` | 2 | The prelude itself (spec §13): `Option`/`Result`, the operator and conversion traits, `Vec`/`Map`/`Set`/`StringBuf`, and the comptime-reflection types (`FieldInfo`/`FieldIter`) behind the compiler-resolved `fields_of`/`type_name`/`splice_field`/`code_to_string` builtins. `paco-driver` also loads this one module implicitly, before any `use`. |
| `string` | `stdlib/string.paco` | 3 | `methods string { ... }` wraps the runtime-intrinsic string functions (`string_char_at`, `string_slice_utf8`, `string_to_bytes`, `string_len_bytes`, `string_byte_at`, `string_next_char_boundary`) that `paco-resolve` resolves by name. Not in the prelude — `use stdlib::string;` is required (spec §13). |
| `io` | `stdlib/io.paco` | 3 | `read_file`/`print_err` wrap the runtime intrinsics `fs_read_to_string`/`stderr_write`. |
| `env` | `stdlib/env.paco` | 3 | `args()` wraps the runtime intrinsics `arg_count`/`arg_at`. Has no `module env;` declaration — legal (the declaration is optional in the grammar, `paco-syntax`'s `module_decl` returns `Option`; a file is a module by its path, not by a declared name) but inconsistent with every other `stdlib` file and with spec §13's "every file opens with its module declaration" — a pre-existing spec/implementation mismatch outside this change's scope, flagged here rather than silently worked around. `process-args-and-exit-status` (open) owns extending this module (`args_bytes`, `var`) and its own docs. |
| `sync` | `stdlib/sync.paco` | 2 | No content of its own: every type it would hold (`Rc`/`Arc`/`Cell`/`RefCell`/`Mutex`/`RwLock`, `channel`/`Sender`/`Receiver`/`spawn_blocking`/`TaskPanic`) is already a compiler-registered prelude type (`paco-types`), not `.paco` source. The module exists so `use stdlib::sync;` resolves for code that wants an explicit, qualified import of prelude-level synchronization primitives. |
| `collections` | `stdlib/collections.paco` | 2 | Same shape as `sync`: `Vec`/`Map`/`Set`/`StringBuf` are prelude entries (defined in `stdlib/core/collections.paco`); this module reserves `stdlib::collections` as their explicit-import path. |
| `dims` | `stdlib/dims.paco` | 1 | `Shaped` and `DimError` are hardcoded by name in `paco-types` (the named-dimension diagnostics, e.g. `PACO-E0345`, name `Shaped` directly); `dim`/`with_dims`/`as_dims`/`assume_dims`/`erase_dims` are the fixed intrinsic set `paco-mir`'s lowering recognizes (RFC 0029). Only `witness`'s implementation is ordinary Paco code — the trait and intrinsics it supports are not. |
| `autodiff` | `stdlib/autodiff.paco` | 1 | `grad` carries the literal `#[builtin(grad)]` attribute (RFC 0026). |
| `test` | `stdlib/test.paco` | 1 | Every assertion (`assert`, `assert_eq`, `assert_ne`, `assert_true`, `assert_false`, `assert_some`, `assert_none`, `assert_ok`, `assert_err`) carries its own `#[builtin(name)]` attribute; the compiler replaces the call, not the written body. |
| `numerics` | `stdlib/numerics.paco` | — | **Marked for extraction.** `Tensor` and `ShapeError` satisfy none of reasons 1–4; the compiler's const-generic shape checking works on any type. Moves to `pacolang/tensor` in `extract-domain-libraries`. |
| `math` | `stdlib/math.paco` | — | **Marked for extraction.** `Matrix`, `DataFrame` and `Schema` satisfy none of reasons 1–4 (amends RFC 0011). Moves to `pacolang/math`. |
| `blas` | `stdlib/blas.paco` | — | **Marked for extraction.** An `extern "C"` binding to the system `libblas` — reason 3's structural form is present, but the binding exists only to serve `Matrix`, which is itself leaving; forcing dynamic linking on every program that merely imports `stdlib::math` is exactly what RFC 0030 stops. Moves to `pacolang/blas`. |

`scripts/check_std_scope.py` verifies this table's module set matches the real
`stdlib/` tree (every `.paco` file directly under `stdlib/` by its file stem, plus
`core` for the whole `stdlib/core/` directory) in both directions, and that no
`.paco` file under `stdlib/` contains a `use` of anything other than `stdlib::...`.

The static-link CI job builds a program importing every module in this table
**except** the ones marked "marked for extraction" above, with `--link static`,
proving none of the modules that are staying introduces a dynamic-linking
requirement (`extern` blocks make `--link static` a compile error, PACO-E0804).
When `extract-domain-libraries` removes `numerics`, `math` and `blas` from
`stdlib/`, drop their rows from this table and the job's exclusion list shrinks to
nothing on its own.

## Official library admission

Proposed the same way any language change is: an RFC, reviewed
before a repository is created. A library is official when:

- It satisfies none of the `stdlib` admission reasons above (if it did, it would be
  in `stdlib`, not a library) — it is domain code, not language-core code.
- It depends only on `stdlib` and other official libraries: never on community
  code, and never in a cycle with another official library.
- Its `paco.mod` declares the range of core compiler versions it supports (RFC
  0005's compiler-range field).
- It has an owner who commits to the compatibility and versioning rules below.

A library is retired by archiving its repository and removing its row from this
page and from `README.md`; nothing about retirement is compiler- or
CI-enforced — a retired library simply stops being listed as official, and
existing imports keep resolving to the tag they were pinned to (RFC 0005:
decentralized, no registry to remove an entry from).

## Dependency rules

- The compiler and runtime never depend on or name a library type. This is the
  structural half of reasons 1–3 above and the reason `check_compiler_names.py`
  exists as defense-in-depth on top of it.
- `stdlib` imports only `stdlib`.
- An official library imports only `stdlib` and other official libraries.
- No cycles between official libraries.

## Versioning

- The core (compiler, runtime, `stdlib`) has one version. A release of one is a
  release of all three.
- Each official library follows its own semantic version, independent of the
  core's.
- Each official library's `paco.mod` states the range of core versions it
  supports; `paco` checks a dependency's declared range against the core's own
  version before compiling (already implemented, `git-module-fetch` task 6b.2).

## Distribution

`scripts/dist.sh` copies `stdlib/` into `lib/paco/stdlib/` alongside the existing
`lib/paco/<target>/` directories. `stdlib_root()` (`paco-driver`) resolves, in
order: `PACO_STD`, then `<exe>/../../lib/paco/stdlib` (via `paco-link`'s
`Toolchain::locate()`), then the development-checkout path
(`CARGO_MANIFEST_DIR/../../stdlib`). Official libraries are never bundled with the
distribution — they are fetched with `paco get` like any other dependency.
