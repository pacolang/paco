# HIR Storage Strategy: Vec-indexed IDs vs Arena Reference Graph

* Status: accepted
* Deciders: Core Developers
* Date: 2026-05-31

## Context and Problem Statement

During Phase 2, the Paco compiler implements the High-level Intermediate Representation (HIR). Unlike the Abstract Syntax Tree (AST), which is a hierarchical tree, the HIR is a rich graph where variables, functions, and types reference each other.

In Rust, representing graph structures can be done in two main ways:
1. **Arena Allocation with Borrowed References (`&'hir Node`)**: Allocating all nodes in an arena and using raw Rust references for links.
2. **Vec-indexed IDs (`DefId` and `BindingId` indices)**: Storing all definitions in a flat `Vec` and referencing them via integer-wrapper IDs.

We need to choose a strategy that guarantees safety, high compilation performance, and ease of implementation within Rust's borrow checker.

## Decision Drivers

* **Borrow Checker Compatibility**: Avoiding lifetime propagation (`'hir`) across all compiler passes (name resolver, type checker, borrow checker, and evaluator).
* **Performance**: Low memory overhead and fast lookups.
* **Serialization/Debugging**: Ease of printing and dumping the HIR representation for compiler diagnostics and test validation.

## Considered Options

* **Option 1: Arena Allocation (`&'hir Node`)**
* **Option 2: Vec-indexed IDs (`DefId`/`BindingId`)**

## Decision Outcome

Chosen option: **Option 2: Vec-indexed IDs (`DefId`/`BindingId`)**.

Storing HIR items in a global, flat vector where relations are expressed via indices (`DefId(usize)`) completely avoids lifetime propagation. This keeps the compiler passes clean, allows easy serialization, and avoids borrow-checker bottlenecks when passing references across multiple modules.

### Consequences

* **Good**: Pass APIs remain clean without lifetime parameters (no `Checker<'hir>`).
* **Good**: Safe, simple, and standard approach in modern Rust compilers.
* **Bad**: Lookups require passing a reference to the global definition table (e.g. `program.get(def_id)`).
* **Mitigation**: A context object (`Program` or `Context`) is already passed through all checker routines.
