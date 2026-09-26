<p align="center">
  <a href="https://github.com/pacolang/paco"><img alt="License" src="https://img.shields.io/github/license/pacolang/paco"></a>
  <a href="https://github.com/pacolang/paco/stargazers"><img alt="Stars" src="https://img.shields.io/github/stars/pacolang/paco"></a>
  <a href="https://github.com/pacolang/paco/network/members"><img alt="Forks" src="https://img.shields.io/github/forks/pacolang/paco"></a>
  <a href="https://github.com/pacolang/paco/issues"><img alt="Issues" src="https://img.shields.io/github/issues/pacolang/paco"></a>
</p>

<h1 align="center">Paco</h1>

<p align="center">
  A general-purpose, compiled programming language for building AI systems end to end — services, CLIs, games, critical flows and compilers on one foundation, in one binary.
  <br />
  <a href="docs/design/spec.md"><strong>Explore the spec »</strong></a>
  <br />
  <br />
  <a href="https://github.com/pacolang/examples">View Examples</a>
  ·
  <a href="https://github.com/pacolang/paco/issues/new">Report a Bug</a>
  ·
  <a href="https://github.com/pacolang/rfcs">Propose an RFC</a>
</p>

**Read this in:** **English** · [Português](README.pt-BR.md) · [Español](README.es.md)

> **Status:** early implementation. The compiler checks, runs (`paco run`, a cached Cranelift build) and compiles to native binaries through Cranelift and LLVM (`paco build`); the standard library is minimal.

## Table of Contents

- [About](#about)
- [Principles](#principles)
- [Ecosystem](#ecosystem)
- [Getting Started](#getting-started)
- [Usage](#usage)
- [Repository Layout](#repository-layout)
- [Roadmap](#roadmap)
- [Contributing](#contributing)
- [License](#license)
- [Contact](#contact)

## About

Services, CLIs, games, critical flows, compilers and AI systems share one foundation; building AI systems end to end is the tie-breaker when two designs conflict ([RFC 0013](https://github.com/pacolang/rfcs/blob/main/text/0013-ai-systems-north-star.md), [RFC 0028](https://github.com/pacolang/rfcs/blob/main/text/0028-general-purpose-ai-tie-breaker.md)).

Paco is at the design/bootstrap stage. `AGENTS.md` is the language quick-reference; `docs/design/spec.md` is the full specification (the "what"); `docs/implementation/requirements.md` is the roadmap (the "how to build").

## Principles

1. **Opinionated, but with freedom** — one recommended way, with explicit escape hatches.
2. **Visible cost** — no hidden allocation, copying, or dynamic behavior.
3. **Low mental cost by default** — complexity only shows up when you need it.

## Ecosystem

One organization, `github.com/pacolang`, one repository per project. This repository, `pacolang/paco`, is the core: compiler, runtime and `stdlib`, one version. Each official domain library — `pacolang/tensor`, `pacolang/math`, `pacolang/blas`, and so on as they are proposed — gets its own repository, its own semantic version, and its own compiler-version-range declaration in its `paco.mod`; none of them ships with this repository or its distribution. A program fetches one with `paco get github.com/pacolang/<name>@<version>`, the same way it fetches any other dependency. `docs/ecosystem.md` has the admission criteria, the dependency rules and the full current list ([RFC 0030](https://github.com/pacolang/rfcs/blob/main/text/0030-repository-organization-and-stdlib-scope.md)).

`stdlib/numerics.paco`, `stdlib/math.paco` and `stdlib/blas.paco` (`Tensor`, `Matrix`, `DataFrame` and the BLAS binding) still live in this repository today — they are marked for extraction to `pacolang/tensor`, `pacolang/math` and `pacolang/blas` respectively, moving in a later, dependent change.

Example programs live in their own repository, [`pacolang/examples`](https://github.com/pacolang/examples) — not here.

## Getting Started

### Prerequisites

- The Rust toolchain pinned in `rust-toolchain.toml`.
- LLVM 18.1 for the optimizing backend (see `compiler/paco-codegen-llvm/README.md`). Without it, `cargo build --no-default-features -p paco-driver` builds a `paco` whose `run` and `build` use Cranelift only.
- macOS: the Xcode command line tools (`xcode-select --install`).
- Windows: the MSVC build tools and Windows SDK.

### Installation

Paco builds and runs on Linux (`x86_64`, `aarch64`), macOS (`aarch64`, `x86_64`) and Windows (`x86_64`). On Linux, `paco build` needs nothing beyond the prerequisites above (`scripts/dist.sh` packages a self-contained distribution).

```bash
cargo build --release
```

## Usage

```bash
paco new hello               # create a project
paco run                     # build (cached) + run
paco build --release         # optimizing backend, single binary
paco test                    # run #[test] functions
paco fmt --write             # canonical formatter
```

See [`pacolang/examples`](https://github.com/pacolang/examples) for complete, runnable programs.

## Repository Layout

```
paco/
├── AGENTS.md            # context for AI agents
├── README.md
├── docs/
│   ├── design/          # the language specification
│   ├── implementation/  # requirements + roadmap
│   └── grammar/         # tokens and EBNF grammar
├── tests/conformance/   # tests: input.paco + expected output, run on every backend
├── compiler/            # RUST — the compiler
├── runtime/             # RUST — scheduler, channels, I/O poller (linked into binaries)
└── stdlib/              # PACO — the standard library
    ├── core/            #   prelude: Option, Result, traits, collections, derive
    ├── io.paco, string.paco, sync.paco, collections.paco, dims.paco, autodiff.paco
    └── math.paco, numerics.paco, blas.paco   # marked for extraction, see docs/ecosystem.md
```

The Rust ↔ Paco boundary: what runs **below** the language (compiler, runtime) is Rust; what lives **inside** the language (standard library) is Paco.

## Roadmap

See `docs/implementation/requirements.md` for the build plan, and this repository's [issues](https://github.com/pacolang/paco/issues) and [milestones](https://github.com/pacolang/paco/milestones) for day-to-day tracking.

## Contributing

Contributions are what make the open-source community such an amazing place to learn and create. Any contribution you make is **greatly appreciated**.

1. Fork the repository.
2. Create your feature branch (`git checkout -b feat/my-feature`).
3. Read `AGENTS.md` before touching any `.paco` file or compiler code.
4. Commit your changes and open a pull request.

For a language or ecosystem design change, open an RFC in [`pacolang/rfcs`](https://github.com/pacolang/rfcs) first.

## License

Distributed under the Apache License, Version 2.0. See [`LICENSE`](LICENSE) for more information. Programs built with Paco may be released under any license.

## Contact

Project Link: [https://github.com/pacolang/paco](https://github.com/pacolang/paco)
