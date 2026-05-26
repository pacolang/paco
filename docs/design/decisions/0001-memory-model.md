# Memory Management: Ownership, Lifetime Inference, and Reference Counting

* Status: accepted
* Deciders: Core Developers
* Date: 2026-05-25

## Context and Problem Statement

A modern programming language must address memory safety without sacrificing performance or imposing excessive cognitive overhead on the developer. 
Systems programming languages traditionally use manual memory management (e.g., C, C++), which is highly performant but prone to safety issues (use-after-free, double-free, data races). Modern application development languages typically use a Garbage Collector (GC) (e.g., Go, Java, Lua), which eliminates safety bugs but introduces non-deterministic latency spikes, GC pauses, and increased memory usage, making them less suitable for low-latency systems, game engines, or highly optimized numeric computing.

Paco needs a memory management model that guarantees compile-time memory safety, maintains zero-cost abstractions and predictable latency, and keeps the developer experience ergonomic and low-cost by default.

## Decision Drivers

* **Zero-cost memory safety**: Detect and prevent memory bugs at compile time without runtime checks or garbage collection.
* **Low cognitive load**: Reduce verbosity and complexity for common use cases so developers do not have to write verbose annotations for simple structures.
* **Predictable performance**: Enable deterministic resource deallocation (RAII) and prevent unexpected pause times.

## Considered Options

* **Option 1: Garbage Collection (GC)**: Manage memory automatically via a runtime garbage collector.
* **Option 2: Explicit/Manual Memory Management**: Trust the developer to allocate and free memory manually (e.g., `malloc`/`free`).
* **Option 3: Strict Ownership and Manual Lifetime Annotations**: Implement compile-time ownership semantics like Rust, requiring explicit annotations (`'a`) when borrowing references across scopes or structures.
* **Option 4: Ownership + Move by default with Aggressive Lifetime Inference and Reference Counting Escape Hatch**: Use ownership and borrow checking as the safety foundation, run a compiler pass to infer lifetimes in all common cases (almost entirely hiding `'a` annotations), and provide explicit Reference Counting (`Rc` / `Arc`) for complex, cyclical data structures.

## Decision Outcome

Chosen option: **Option 4**, because it strikes the ideal balance between safety, execution speed, and mental ergonomics. It provides the same safety and performance guarantees as Option 3, but significantly reduces verbosity through advanced compile-time lifetime inference. For cases where static borrow checking is too rigid, explicit reference counting serves as an easy-to-use escape hatch.

### Consequences

* **Good (Safety & Performance)**: Complete compile-time memory safety without the performance and latency penalties of a Garbage Collector.
* **Good (Ergonomics)**: The syntax remains clean and readable; developers rarely have to reason about or write explicit lifetime parameters (`'a`).
* **Good (Determinism)**: Memory and resources (such as file descriptors or socket connections) are reclaimed predictably at scope exit using RAII.
* **Bad (Compiler Complexity)**: Building a reliable lifetime inference engine that deduces lifetimes in complex structures is highly challenging and borders on academic research.
* **Mitigation**: The compiler implementation will start with explicit lifetime checking similar to Rust's rules, and gradually incorporate inference heuristics to systematically eliminate the need for manual annotations.

## Pros and Cons of the Options

### Option 1: Garbage Collection (GC)

* Good: Easiest developer experience; memory leaks are minimized and cyclical graphs are handled automatically.
* Bad: Non-deterministic pauses (stop-the-world phases) degrade performance in game loops, real-time audio, and low-latency servers.
* Bad: Larger binary sizes and higher runtime memory footprint due to metadata and runtime GC threads.

### Option 2: Explicit/Manual Memory Management

* Good: Ultimate control over hardware and memory layout; no compiler-imposed borrow checking friction.
* Bad: Highly vulnerable to security exploits and crash bugs (dangling pointers, buffer overflows).
* Bad: Increases developer overhead, requiring painstaking debugging of leaks and memory corruption.

### Option 3: Strict Ownership and Manual Lifetime Annotations

* Good: Eliminates data races and memory bugs at compile time with zero runtime overhead.
* Bad: High learning curve ("fighting the borrow checker") and syntax noise due to pervasive lifetime parameter annotations (`<'a>`).

### Option 4: Ownership + Move with Aggressive Lifetime Inference and RC/Arc Escape Hatch

* Good: Combines compile-time safety and zero-overhead performance with clean, low-overhead syntax.
* Good: Offers escape hatches (`Rc`/`Arc`) that are explicit, making their performance cost transparent to the developer.
* Bad: Requires a highly sophisticated compiler analysis engine, increasing compile times and implementation difficulty.
