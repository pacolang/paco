# Compilation Backend: Dual-Engine Schema with Cranelift and LLVM

* Status: accepted
* Deciders: Core Developers
* Date: 2026-05-25

## Context and Problem Statement

Compilers face a structural trade-off between compilation speed and execution speed. Optimization suites (like LLVM) perform hundreds of complex analysis passes to generate highly optimized, minimal machine code, but compile times are notoriously slow. This sluggish feedback loop degrades developer productivity, particularly in fields requiring rapid iteration such as game development and interactive data analysis. 

Conversely, fast codegen engines compile code almost instantaneously but perform minimal optimizations, resulting in slower, larger binaries that are unsuitable for production.

Paco requires a compiler architecture that supports both a sub-second compilation feedback loop during local development and state-of-the-art runtime performance in production.

## Decision Drivers

* **Development velocity**: Maintain a sub-second compile-run cycle during active debugging and coding.
* **Execution performance**: Generate production binaries that maximize hardware efficiency, vectorization, and execution speed.
* **Cross-compilation**: Support targeting multiple CPU architectures and operating systems out of the box.
* **Maintainability**: Prevent compiler backend differences from introducing semantic bugs or duplicating frontend code.

## Considered Options

* **Option 1: LLVM Only**: Use LLVM as the single compilation backend for both development and production.
* **Option 2: Cranelift Only**: Use Cranelift as the single backend, prioritizing compilation speed.
* **Option 3: Custom Assembly Code Generator**: Write a bespoke code generator targeting specific architectures (e.g., x86_64, AArch64).
* **Option 4: Dual Backend (Cranelift for Dev, LLVM for Release)**: Implement a dual-backend pipeline. Lower the typed AST to a backend-independent Intermediate Representation (IR). Feed this IR to Cranelift for development builds (`paco build`) and to LLVM for optimized production builds (`paco build --release`).

## Decision Outcome

Chosen option: **Option 4**, because it successfully decouples developer iteration speed from production execution performance. Cranelift provides near-instantaneous feedback during local development, while LLVM's mature optimization pipeline is leveraged for release-ready binaries.

To maintain compiler architecture sanity and prevent duplicating translation logic, the compiler frontend will lower the Abstract Syntax Tree (AST) to a common, custom **Paco Intermediate Representation (Paco IR)**. This IR makes implicit constructs (destructors, method dispatch, memory layout) explicit, and is subsequently lowered to either Cranelift IR or LLVM IR.

### Consequences

* **Good (Agile Dev Cycle)**: Near-instant compilation speeds for developers, keeping them in flow.
* **Good (Production Speed)**: Release binaries benefit from LLVM's advanced optimization passes (auto-vectorization, aggressive loop unrolling, etc.).
* **Good (Portability)**: Leverages LLVM's extensive target architecture support for production cross-compilation.
* **Bad (Parity Risk)**: Maintaining semantic parity between two separate backend codegen pipelines is difficult. A bug might appear in the LLVM-compiled release binary but not in the Cranelift-compiled development binary.
* **Mitigation**: Maintain a shared, exhaustive conformance test suite. Every test must yield identical runtime outputs when compiled under both the Cranelift and LLVM backends.

## Pros and Cons of the Options

### Option 1: LLVM Only

* Good: Single backend to maintain; optimizations are active during development, catching backend bugs early.
* Bad: Slow compile times frustrate developers, particularly in large codebases.

### Option 2: Cranelift Only

* Good: Simple, fast compiler codebase.
* Bad: Binaries lack advanced loop optimizations, vectorization, and micro-architectural tuning, making Paco uncompetitive for computation-heavy workloads (numeric analysis, games).

### Option 3: Custom Assembly Code Generator

* Good: Full control over codegen and zero dependencies.
* Bad: Massively increases development effort; supporting multiple platforms and complex optimizations becomes practically unfeasible for a small core team.

### Option 4: Dual Backend (Cranelift + LLVM)

* Good: Delivers the optimal development experience without compromising production execution speed.
* Bad: Doubles the integration effort, requiring developers to interface with both `cranelift-codegen` and LLVM bindings (e.g., `inkwell` in Rust).
