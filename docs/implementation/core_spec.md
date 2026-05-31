# Phase 1 Conformance Specification

This document specifies the conformance testing requirements and compiler invariants for Phase 1 of the Paco compiler implementation. The goal of Phase 1 is a minimal executable core: parsing, resolving, type checking, and tree-walking interpretation of a simple language subset.

---

## 1. Expected Behavior

### 1.1 Conformance Test Runner
A new integration test runner must be created at [compiler/paco-driver/tests/conformance.rs](file:///c:/Users/Clebson/Documents/workspace/projects/pocolang/compiler/paco-driver/tests/conformance.rs).

#### Execution Mechanics
- **Discovery**: The runner must use `paco-test-harness` to dynamically discover all golden tests under the [tests/conformance/core/](file:///c:/Users/Clebson/Documents/workspace/projects/pocolang/tests/conformance) directory.
- **Filtering**: The harness will discover tests by looking for `flags.toml` configurations. Active tests matching `feature_min <= 1` must be executed.
- **In-process Driver Invocation**: Instead of spawning a subprocess, the runner must directly call the compiler driver's entry point function `paco_driver::run(cli)` with a simulated `Cli` structure.
  - For `run` tests, it translates to `paco run <input_file>`.
  - For `fail` tests, it translates to `paco run <input_file>` or `paco check <input_file>`, expecting a compilation failure result.
- **Output Validation**:
  - **Run Tests (`kind = "run"`)**:
    - The runner must assert that the compiler driver successfully executes and returns a `DriverOutput`.
    - The runner must compare `DriverOutput::stdout` against the contents of the test's `expected.stdout` file.
    - Line endings must be normalized (e.g., converting Windows `\r\n` to Unix `\n`), and trailing whitespace must be trimmed before comparison.
  - **Fail Tests (`kind = "fail"`)**:
    - The runner must assert that the compiler driver returns an `Err(diagnostics)` representing a compilation/evaluation error.
    - The diagnostic message must match the expectations defined in `expected.stderr` (or contain the expected diagnostic error codes and substrings).

---

### 1.2 Tests to Implement in `tests/conformance/core/`

Five conformance tests must be added to validate the minimal language subset features:

#### 1. `hello_world` (Run Test)
- **Directory**: `tests/conformance/core/hello_world/`
- **Goal**: Verifies basic execution of a minimal program, function call to a builtin, and string literal output.
- **Files**:
  - `flags.toml`:
    ```toml
    kind = "run"
    feature_min = 1
    ```
  - `input.paco`:
    ```paco
    fn main() {
        print("Hello, world!")
    }
    ```
  - `expected.stdout`:
    ```
    Hello, world!
    ```

#### 2. `factorial` (Run Test)
- **Directory**: `tests/conformance/core/factorial/`
- **Goal**: Verifies recursive function calls, function parameters, local variable bindings, conditional statements (`if/else` expressions), comparisons, multiplication, subtraction, and nested expressions.
- **Files**:
  - `flags.toml`:
    ```toml
    kind = "run"
    feature_min = 1
    ```
  - `input.paco`:
    ```paco
    fn fact(n: int) -> int {
        if n == 0 {
            1
        } else {
            n * fact(n - 1)
        }
    }

    fn main() {
        print(fact(10))
    }
    ```
  - `expected.stdout`:
    ```
    3628800
    ```

#### 3. `arithmetic` (Run Test)
- **Directory**: `tests/conformance/core/arithmetic/`
- **Goal**: Verifies correctness and precedence of all binary arithmetic and comparison operators.
- **Files**:
  - `flags.toml`:
    ```toml
    kind = "run"
    feature_min = 1
    ```
  - `input.paco`:
    ```paco
    fn main() {
        // Arithmetic Operators
        print(10 + 5)
        print(10 - 5)
        print(10 * 5)
        print(10 / 3)
        print(10 % 3)

        // Float Arithmetic
        print(5.5 + 2.5)
        print(5.5 - 2.5)
        print(5.5 * 2.0)
        print(5.5 / 2.0)

        // Comparison Operators (Integers)
        print(10 == 10)
        print(10 != 5)
        print(10 < 20)
        print(10 <= 10)
        print(10 > 5)
        print(10 >= 10)

        // Comparison Operators (Booleans)
        print(true == true)
        print(true != false)

        // Precedence & Associativity
        print(1 + 2 * 3) // multiplicative binds tighter: 7
        print((1 + 2) * 3) // parenthesis overrides precedence: 9
        print(10 - 4 - 2) // left-associative: 4
    }
    ```
  - `expected.stdout`:
    ```
    15
    5
    50
    3
    1
    8
    3
    11
    2.75
    true
    true
    true
    true
    true
    true
    true
    true
    7
    9
    4
    ```

#### 4. `type_error_add_string_int` (Fail Test)
- **Directory**: `tests/conformance/core/type_error_add_string_int/`
- **Goal**: Validates that type-checking prevents adding incompatible types (specifically `string` and `int`), highlighting the exact operator site in the diagnostics.
- **Files**:
  - `flags.toml`:
    ```toml
    kind = "fail"
    feature_min = 1
    ```
  - `input.paco`:
    ```paco
    fn main() {
        let x = "hello" + 5
    }
    ```
  - `expected.stderr`:
    ```
    error: type mismatch
      --> input.paco:2:21
       |
     2 |     let x = "hello" + 5
       |                     ^ expected string, found int
    ```
    *(Note: exact error code `PACO-E0301` or equivalent must be matched).*

#### 5. `undeclared_name` (Fail Test)
- **Directory**: `tests/conformance/core/undeclared_name/`
- **Goal**: Validates that name resolution catches references to undefined identifiers, highlighting the invalid identifier.
- **Files**:
  - `flags.toml`:
    ```toml
    kind = "fail"
    feature_min = 1
    ```
  - `input.paco`:
    ```paco
    fn main() {
        print(undeclared_variable)
    }
    ```
  - `expected.stderr`:
    ```
    error: name not found
      --> input.paco:2:11
       |
     2 |     print(undeclared_variable)
       |           ^^^^^^^^^^^^^^^^^^^
    ```
    *(Note: exact error code `PACO-E0201` or equivalent must be matched).*

---

## 2. Business Rules & Compiler Invariants

To pass Phase 1, the compiler must conform to the following invariants:

- **Single Entry Point**: Every program must define a `main` function taking no parameters and returning nothing. The absence of `main` is a compiler error (`PACO-E0001`).
- **Phase Boundary Halting**: The compiler executes in stages: Lexer -> Parser -> Resolver -> Type Checker -> Borrow Checker -> Interpreter. If any error diagnostic (severity `Error`) is produced during a stage, the pipeline must halt before entering the next stages, and execution/interpretation must not start.
- **Operator Rules**:
  - Binary arithmetic operators (`+`, `-`, `*`, `/`, `%`) require operand type unification. Operands must either be both `int` or both `float`.
  - Binary comparisons (`==`, `!=`, `<`, `<=`, `>`, `>=`) require matching types.
  - Logical operators (`&&`, `||`, `!`) require both operands to be of type `bool`.
- **Precedence Rules**:
  - Standard precedence hierarchy: `Parentheses` > `Unary` > `Multiplicative` (`*`, `/`, `%`) > `Additive` (`+`, `-`) > `Relational` (`==`, `!=`, `<`, `<=`, `>`, `>=`) > `Logical AND` > `Logical OR` > `Assignment`.
  - Binary operators are left-associative.
- **Scope and Shadowing**:
  - Declarations are bound to their lexical scope. Variable resolution looks up the nearest enclosing scope.
  - Re-declaration of a name in the same scope (shadowing via `let`) is permitted, producing a fresh binding ID.
- **Stdout Protocol**: The `print` builtin accepts a single parameter of any primitive type (`int`, `float`, `bool`, `string`, `char`) and writes its textual representation to standard output, followed by a trailing newline.

---

## 3. Edge Cases

The following edge cases must be handled robustly by the compiler and interpreter:

- **Division and Modulo by Zero**:
  - Evaluating `x / 0` or `x % 0` (for `int`) or `x / 0.0` (for `float`) must not crash the compiler or cause Rust panic.
  - Modulo/Division by zero in integer arithmetic must raise a clean Paco runtime error diagnostic.
- **Arithmetic Overflow/Underflow**:
  - In interpreter mode, standard integer boundaries (`i64::MIN` to `i64::MAX`) must be validated.
  - Operations exceeding these bounds (e.g. `9223372036854775807 + 1`) must emit a runtime error diagnostic instead of panicking the host process.
- **Scope Boundary Escaping**:
  - Variables declared inside an `if/else` block, `while` block, or function body must not be accessible outside their parent scope.
- **Trivia Handling**:
  - Multiple consecutive line comments (`//`), block comments (`/* ... */`), or nested block comments must be ignored as trivia by the parser without affecting token stream boundaries.
  - Unicode/UTF-8 contents inside comments and string literals must be parsed correctly without causing encoding crashes.
- **String Literals Escape Sequences**:
  - Backslashes, tabs, and newlines (`\n`, `\t`, `\r`, `\"`, `\\`) inside string literals must be unescaped correctly by the lexer before outputting to stdout.

---

## 4. Expected Impact

Implementing this specification provides the following impacts:
- **CI/CD Guardrails**: Automated testing of basic features on each commit prevents regressions.
- **Validation of the Compiler Pipeline**: Ensures that the lexing, parsing, name-resolution, and type-checking phases function cohesively.
- **Interpreter Parity Baseline**: Establishes the source of truth for the tree-walking interpreter, which serves as the reference for later native codegen backends (LLVM and Cranelift).

---

## 5. Files to Be Modified/Created

| File Path | Action | Description |
|-----------|--------|-------------|
| [docs/implementation/core_spec.md](file:///c:/Users/Clebson/Documents/workspace/projects/pocolang/docs/implementation/core_spec.md) | **Create** | This specification document. |
| [compiler/paco-driver/tests/conformance.rs](file:///c:/Users/Clebson/Documents/workspace/projects/pocolang/compiler/paco-driver/tests/conformance.rs) | **Create** | Conformance integration test runner using `paco-test-harness`. |
| [tests/conformance/core/hello_world/input.paco](file:///c:/Users/Clebson/Documents/workspace/projects/pocolang/tests/conformance/core/hello_world/input.paco) | **Create** | Input file for `hello_world` run test. |
| [tests/conformance/core/hello_world/flags.toml](file:///c:/Users/Clebson/Documents/workspace/projects/pocolang/tests/conformance/core/hello_world/flags.toml) | **Create** | Metadata flags for `hello_world` run test. |
| [tests/conformance/core/hello_world/expected.stdout](file:///c:/Users/Clebson/Documents/workspace/projects/pocolang/tests/conformance/core/hello_world/expected.stdout) | **Create** | Expected stdout for `hello_world` run test. |
| [tests/conformance/core/factorial/input.paco](file:///c:/Users/Clebson/Documents/workspace/projects/pocolang/tests/conformance/core/factorial/input.paco) | **Create** | Input file for `factorial` run test. |
| [tests/conformance/core/factorial/flags.toml](file:///c:/Users/Clebson/Documents/workspace/projects/pocolang/tests/conformance/core/factorial/flags.toml) | **Create** | Metadata flags for `factorial` run test. |
| [tests/conformance/core/factorial/expected.stdout](file:///c:/Users/Clebson/Documents/workspace/projects/pocolang/tests/conformance/core/factorial/expected.stdout) | **Create** | Expected stdout for `factorial` run test. |
| [tests/conformance/core/arithmetic/input.paco](file:///c:/Users/Clebson/Documents/workspace/projects/pocolang/tests/conformance/core/arithmetic/input.paco) | **Create** | Input file for `arithmetic` run test. |
| [tests/conformance/core/arithmetic/flags.toml](file:///c:/Users/Clebson/Documents/workspace/projects/pocolang/tests/conformance/core/arithmetic/flags.toml) | **Create** | Metadata flags for `arithmetic` run test. |
| [tests/conformance/core/arithmetic/expected.stdout](file:///c:/Users/Clebson/Documents/workspace/projects/pocolang/tests/conformance/core/arithmetic/expected.stdout) | **Create** | Expected stdout for `arithmetic` run test. |
| [tests/conformance/core/type_error_add_string_int/input.paco](file:///c:/Users/Clebson/Documents/workspace/projects/pocolang/tests/conformance/core/type_error_add_string_int/input.paco) | **Create** | Input file for `type_error_add_string_int` fail test. |
| [tests/conformance/core/type_error_add_string_int/flags.toml](file:///c:/Users/Clebson/Documents/workspace/projects/pocolang/tests/conformance/core/type_error_add_string_int/flags.toml) | **Create** | Metadata flags for `type_error_add_string_int` fail test. |
| [tests/conformance/core/type_error_add_string_int/expected.stderr](file:///c:/Users/Clebson/Documents/workspace/projects/pocolang/tests/conformance/core/type_error_add_string_int/expected.stderr) | **Create** | Expected compiler diagnostics for `type_error_add_string_int` fail test. |
| [tests/conformance/core/undeclared_name/input.paco](file:///c:/Users/Clebson/Documents/workspace/projects/pocolang/tests/conformance/core/undeclared_name/input.paco) | **Create** | Input file for `undeclared_name` fail test. |
| [tests/conformance/core/undeclared_name/flags.toml](file:///c:/Users/Clebson/Documents/workspace/projects/pocolang/tests/conformance/core/undeclared_name/flags.toml) | **Create** | Metadata flags for `undeclared_name` fail test. |
| [tests/conformance/core/undeclared_name/expected.stderr](file:///c:/Users/Clebson/Documents/workspace/projects/pocolang/tests/conformance/core/undeclared_name/expected.stderr) | **Create** | Expected compiler diagnostics for `undeclared_name` fail test. |

