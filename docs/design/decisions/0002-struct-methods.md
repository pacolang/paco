# Behavior Association: Nested Struct Methods and External Extensions

* Status: accepted
* Deciders: Core Developers
* Date: 2026-05-25

## Context and Problem Statement

Programming languages bind data and behavior (methods) in various ways. Classical Object-Oriented Programming (OOP) languages combine them within a `class` structure, but often introduce complex inheritance hierarchies, hidden runtime dispatch (vtables), and implicit reference/pointer semantics. 

Systems languages like Rust separate data definitions (`struct`) from implementation blocks (`impl`), which decouples them completely but introduces verbosity and syntactic separation. Go uses standalone functions with receiver arguments, which can lead to methods being scattered across multiple files.

Paco requires a behavioral model that keeps code highly cohesive, readable, and consistent, while avoiding the pitfalls of OOP inheritance and keeping method dispatch statically resolved (zero-cost) by default.

## Decision Drivers

* **High cohesion and readability**: Grouping data structures and their primary operations together so code is easy to read and maintain.
* **Decoupling and extensibility**: Allowing developers to extend existing types (including types from other modules) without modifying their original source files.
* **Syntactic consistency**: Avoiding multiple, redundant syntaxes for the same capability, which keeps the language formatter (`paco fmt`) simple and predictable.
* **No hidden costs**: Ensuring that mutability and dispatch method overhead are explicitly visible in the code.

## Considered Options

* **Option 1: Go-style Receiver Functions**: Define methods as standalone functions with a special receiver parameter (e.g., `fn write(self&mut File, data: []byte)`) outside the struct.
* **Option 2: Rust-style Implementation Blocks (`impl`)**: Keep structs purely as data definitions and write all methods in separate `impl` blocks.
* **Option 3: Zig/Swift-style Nested Methods**: Define all methods directly within the `struct` or `enum` definition block.
* **Option 4: Nested Methods for Local Types + `methods T` Blocks for External Extensions**: Define methods inside the struct/enum body for the type's primary definition, and use a separate `methods T { ... }` block *only* when extending types imported from another module.

## Decision Outcome

Chosen option: **Option 4**, because it maximizes readability and structure consistency. Placing methods inside the `struct`/`enum` block provides a unified, cohesive definition for a type. The separate `methods T { ... }` block provides an explicit, clean syntax to extend external types without polluting the primary struct syntax with multiple ways to define local methods.

To support this, Paco methods declare their receivers explicitly to ensure visible cost and ownership guarantees:
* `self&`: Shared borrow (read-only). This is the most common case.
* `self&mut`: Mutable borrow (read-write).
* `self`: Consumes the instance (move semantics). Used when transforming the object into a different type (e.g., `into_bytes()`).

There is no implicit default receiver (e.g., writing just `self` always moves the value, never borrows it silently), ensuring alignment with Paco's "visible cost" principle. The compiler will issue a warning if a developer writes `self` when a read-only `self&` is sufficient.

### Consequences

* **Good (Organization)**: Data and its primary methods are grouped in a single block, reducing context switching when reading code.
* **Good (No OOP Bloat)**: No inheritance hierarchies exist. Polymorphism is achieved statically via traits or dynamically via explicit `dyn Trait` references.
* **Good (Extensibility)**: Third-party types can be extended cleanly using the `methods T { ... }` block.
* **Bad (File Length)**: Struct definitions can become long if a type implements many methods.
* **Mitigation**: Codebases are encouraged to delegate concerns to separate helper structs and modules, keeping individual type definitions cohesive.

## Pros and Cons of the Options

### Option 1: Go-style Receiver Functions

* Good: Allows methods to be added anywhere within the same package.
* Bad: Separates the methods from the data definition visually, making it harder to determine all methods available on a type at a glance.
* Bad: Allows multiple ways to write the same method structure, complicating tooling and formatting.

### Option 2: Rust-style Implementation Blocks (`impl`)

* Good: Complete separation of data and behavior; multiple `impl` blocks can be scattered across files.
* Bad: Introduces syntactic boilerplate for local types, requiring the developer to repeat type names in separate headers.

### Option 3: Zig/Swift-style Nested Methods

* Good: Zero syntactic boilerplate; extremely readable for local types.
* Bad: Lacks a mechanism to extend types defined in external modules without modifying their source code.

### Option 4: Nested Methods for Local Types + `methods T` Blocks for External Extensions

* Good: Provides the clean, nested syntax of Option 3 for the common case, and the extensibility of Option 2 for external types.
* Good: Enforces a single, clean way to define local methods, ensuring consistent formatting.
* Bad: The compiler must handle two syntax locations for method resolution (local struct block and external `methods` blocks).
