# Paco

A general-purpose programming language focused on simplicity, safe concurrency,
ownership-based memory safety, strong enums, and pattern matching.

> **Status:** design / bootstrap. There is no working compiler yet — this repo
> holds the design, the implementation requirements, and the skeleton.

## Principles

1. **Opinionated, but with freedom** — one recommended way, with explicit escape hatches.
2. **Visible cost** — no hidden allocation, copying, or dynamic behavior.
3. **Low mental cost by default** — complexity only shows up when you need it.

## Where to start

- `AGENTS.md` — context for AI agents and a language quick-reference.
- `docs/design/spec.md` — the language specification (the "what").
- `docs/implementation/requirements.md` — requirements and roadmap (the "how to build").
- `docs/design/decisions/` — ADRs: the design decisions and their rationale.
- `examples/` — idiomatic programs (also serve as regression tests).

## Repository layout

```
paco/
├── AGENTS.md            # context for AI agents
├── README.md
├── docs/
│   ├── design/          # spec + ADRs (decisions)
│   ├── implementation/  # requirements + roadmap
│   └── grammar/         # tokens and EBNF grammar
├── examples/            # example programs (.paco)
├── tests/conformance/   # tests: should_compile, should_fail, run_output
├── compiler/            # RUST — the compiler
├── runtime/             # RUST (+asm) — scheduler, channels (embedded in binaries)
└── src/                 # PACO — the standard library
    ├── core/            #   Option, Result, fundamental traits
    ├── collections/     #   Vec, Map, Set
    ├── io/              #   files, stdin/stdout
    └── math/            #   numeric / data module
```

The Rust ↔ Paco boundary: what runs **below** the language (compiler, runtime)
is Rust; what lives **inside** the language (standard library) is Paco.

## Dependencies

Decentralized, version-control-based. A dependency is its source URL pinned to a
semantic-version tag; the manifest is `paco.mod`. There is no central registry.

## License

To be defined.
