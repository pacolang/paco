# Concurrency Model: M:N Lightweight Tasks, Channels, and Synchronous Iterators

* Status: accepted
* Deciders: Core Developers
* Date: 2026-05-25

## Context and Problem Statement

High-concurrency systems (such as web servers handling millions of connections) are difficult to implement efficiently and safely. Traditional OS thread-per-connection models scale poorly due to fixed stack sizes (typically 1–8MB) and high kernel context-switching overhead. 

Explicit `async`/`await` models (as seen in Rust, JavaScript, and C#) scale well but introduce the "function color" problem: asynchronous functions must be colored differently from synchronous ones, splitting the standard library and APIs, and forcing developers to propagate `async` keywords throughout their call stacks.

Paco needs a concurrency model that scales to millions of concurrent tasks, maintains sequential syntax without the "function color" split, guarantees compile-time memory safety, and isolates failures so that a single crashed request does not bring down the entire process.

## Decision Drivers

* **Low cognitive load**: Prevent API bifurcation; sequential syntax should automatically support asynchronous operations.
* **Massive scalability**: Allow spawning millions of concurrent execution units with minimal memory and context-switching overhead.
* **Compile-time data race prevention**: Guarantee that concurrent tasks cannot access shared mutable memory concurrently.
* **Fault isolation**: Ensure that panics inside a concurrent task are isolated and do not crash the host process.
* **Zero-allocation iteration**: Support local streaming of values without scheduler or channel synchronization overhead.

## Considered Options

* **Option 1: OS Threads**: Use standard operating system threads for concurrency.
* **Option 2: Explicit Async/Await**: Build an asynchronous system using explicit `async` functions returning futures/promises, requiring developers to write `await` at yield points.
* **Option 3: Go-Style Implicit M:N Tasks with Channels, Task Panic Isolation, and Synchronous Generators (`iter`)**: Spawns green threads (tasks) scheduled on a thread pool. Blocking I/O and channel actions automatically and transparently suspend tasks. Compile-time ownership moves prevent data races. Spawner task handles allow catching panics as `Result` types. Introduce a separate `iter` keyword for purely synchronous generators.

## Decision Outcome

Chosen option: **Option 3**, because it offers the cleanest syntax for concurrent execution while ensuring robust memory safety and fault tolerance. 

* **Lightweight Tasks (`spawn`)**: Tasks are scheduled M:N on a pool of worker OS threads. Stacks grow dynamically on demand, starting at a few kilobytes.
* **Automatic Suspension**: There is no `async` or `await` keyword for concurrency. When a task performs a blocking operation (I/O, channel read/write, sleep), the Paco runtime suspends it and runs another task.
* **Race Safety via Ownership**: The ownership model guarantees that sending a non-`Arc` value over a channel moves its ownership, ensuring no shared mutable state exists between tasks.
* **Task Panic Isolation**: A panic in a spawned task does not crash the application. The panic is captured at the task boundary. Joining the task's handle (`handle.join()`) returns a `Result<T, TaskPanic>`, allowing the parent task to handle the failure gracefully.
* **Synchronous Iterators (`iter`)**: To avoid task and channel synchronization overhead in performance-critical local loop iterations (common in games and data analysis), Paco provides synchronous generators defined with `iter fn` and using `yield`. These are purely synchronous, zero-allocation state machines pulled by the caller.

### Consequences

* **Good (Ergonomics)**: High-performance asynchronous code reads like simple, synchronous, sequential code. No function coloring.
* **Good (Resilience)**: A bug causing a panic in a web request handler will only terminate that specific task, preserving the rest of the application.
* **Good (Race-Free)**: The compiler prevents data races at compile time.
* **Good (Zero-Cost Streams)**: `iter fn` allows fast local loops without triggering green thread allocations or channel operations.
* **Bad (Runtime Size & Complexity)**: Requires implementing a custom runtime including an M:N scheduler, a cooperative yielding scheme, growable stack support, and an OS-specific event poller (epoll, kqueue, IOCP).

## Pros and Cons of the Options

### Option 1: OS Threads

* Good: Simple compiler implementation; direct mapping to OS capabilities.
* Bad: Does not scale beyond a few thousand concurrent connections due to high memory footprint per thread and scheduler overhead.

### Option 2: Explicit Async/Await

* Good: Minimal runtime scheduler required; compiler compiles functions down to state machines.
* Bad: Splits the ecosystem into sync and async variants. A sync function cannot easily call an async function without blocking the thread, leading to verbose boilerplate and refactoring pain.

### Option 3: Go-Style Implicit Tasks with Channels, Task Isolation, and Synchronous Iterators

* Good: Blends Go's concurrent ergonomics with Rust's compile-time memory safety.
* Good: Restricts `yield` exclusively to synchronous `iter fn` blocks, making the distinction between "local pulled sequence" and "concurrent scheduled work" syntactically explicit.
* Bad: Massive runtime implementation effort, making it the most complex component of the Paco runtime.
