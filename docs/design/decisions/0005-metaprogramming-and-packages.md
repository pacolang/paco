# Metaprogramming via Traits and Comptime, and Package System Deferral

* Status: accepted
* Deciders: Core Developers
* Date: 2026-05-25

## Context and Problem Statement

Modern applications require code extensibility, reflection, and code generation (such as serialization, deserialization, and structural formatting). 

Dynamic scripting languages like Lua use runtime metatables to overload operators and alter behavior on the fly. However, runtime reflection and dynamic dispatch introduce execution overhead, hinder static type checking, and obscure optimization opportunities. 

Additionally, programming languages require a robust dependency management model. Designing and implementing a centralized registry (similar to Rust's Crates.io or Node's NPM) introduces significant scope creep, infrastructure hosting costs, and governance issues early in a language project.

Paco needs a metaprogramming model that is fully resolved at compile time with zero runtime cost, and a dependency system that is decentralized and can be deferred during the initial phases of compiler development.

## Decision Drivers

* **Zero runtime overhead**: Reflection, code generation, and operator overloading must not incur performance penalties at runtime.
* **Static safety**: Ensure all operator overloads and derived behaviors are verified by the type checker at compile time.
* **Reduced scope**: Keep early milestones focused entirely on core language design, deferring complex package manager tools.
* **Decentralization**: Avoid reliance on a centralized package registry, ensuring dependencies are fetched directly from source control repositories.

## Considered Options

### Metaprogramming Options

* **Option A: Runtime Metatables / Reflection**: Standardize dynamic reflection at runtime (e.g. Lua metatables or Java reflection).
* **Option B: Syntactic Macros**: Implement macro expansion systems (e.g., C preprocessor or Rust macros).
* **Option C: Special Traits and Compile-Time Execution (`comptime`)**: Overload operators via compiler-recognized static traits (e.g., `Add`, `Index`, `Display`, `Iter`). Provide a `comptime` execution block to run a subset of Paco code at compile-time to inspect types and generate code (such as `@derive(Display, Serialize)`).

### Package System Options

* **Option X: Centralized Registry and Resolution Tooling from Phase 1**: Host a package registry and implement dependency resolution commands immediately.
* **Option Y: Decentralized URL Imports with Deferred Tooling**: Design a decentralized module system where dependencies are identified by VCS URLs and version tags inside a `paco.mod` manifest. Code imports use URL-like paths (e.g., `use github.com/user/json`). Defer building the package fetching and tidying tools (`paco get`, `paco mod tidy`) to a later milestone, managing dependencies manually or via local path overrides initially.

## Decision Outcome

Chosen options: **Option C** (Special Traits + `comptime`) for metaprogramming, and **Option Y** (Decentralized URL Imports with Deferred Tooling) for package management.

### Metaprogramming
Paco rejects dynamic runtime metatables. Instead, it overloads operators statically using special traits. For code generation and reflection, the compiler features a `comptime` evaluator. Code block execution marked as `comptime` runs during compilation, allowing programmers to inspect type metadata and generate AST nodes. This allows safe, zero-cost boilerplate generation (e.g., automatically deriving serializability).

### Package System
Paco will use a decentralized, git-based dependency model. Dependencies are listed in `paco.mod` pointing directly to VCS repositories (e.g. GitHub URLs) and pinned to semantic tags. Tooling implementation is deferred: during Phase 0 through Phase 6, dependencies will be loaded from local relative directories. The dependency resolution tools (`paco get`, `paco mod tidy`) will be built in a later phase, preventing early scope bloat.

### Consequences

* **Good (Zero-Cost)**: Extensibility and code generation introduce zero runtime execution overhead.
* **Good (Static Safety)**: Operator overloading is fully type-checked, preventing runtime errors.
* **Good (Decentralization)**: Paco packages require no central hosting infrastructure, fetching directly from Git/VCS.
* **Good (Bootstrap Velocity)**: Postponing the package downloader lets the team focus on the parser, borrow checker, and codegen.
* **Bad (Compiler Complexity)**: The compiler must implement an interpreter to evaluate standard Paco code during compile time (`comptime`).
* **Bad (Early Friction)**: Early adopters must configure local paths manually in `paco.mod` before the automated downloading tools are built.

## Pros and Cons of the Options

### Metaprogramming

#### Option A: Runtime Metatables / Reflection

* Good: Flexible, allows features like hot-reloading and dynamic behavioral updates.
* Bad: Prevents optimization passes, slows down execution, and delays type errors to runtime.

#### Option B: Syntactic Macros

* Good: Very powerful code transformation capabilities.
* Bad: Writing macros requires learning a separate macro syntax, and debugging generated code is difficult for developers.

#### Option C: Special Traits and `comptime`

* Good: Code generators are written in Paco itself (no macro syntax to learn); errors are caught during compilation.
* Bad: Complicates compiler development by requiring a nested interpreter.

---

### Package System

#### Option X: Centralized Registry

* Good: Simple import syntax; single authority for packages.
* Bad: Requires hosting infrastructure, domain registration, registry maintenance, and raises namespace squatting issues.

#### Option Y: Decentralized URL Imports

* Good: Scales infinitely without central servers; integrates naturally with Git and existing VCS platforms.
* Bad: If a repository is deleted, the dependency breaks unless local mirroring or a lockfile cache is utilized.
* Bad: Deferring resolution tooling requires manual path mapping during initial development.
