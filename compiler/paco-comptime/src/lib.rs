//! Compile-time evaluation: runs MIR bodies for `comptime` blocks, `comptime
//! fn`s and `#[derive]` expansion, sandboxed and bounded, with the same
//! memory layout, arithmetic and formatting compiled code uses.

mod machine;
mod quote;

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use paco_mir::{Body, ComptimeValue, TypeLayouts};
use paco_span::Span;
use paco_syntax::ast::Ty;
use paco_types::Type;

pub use machine::Machine;

/// Supplies the MIR body of each function evaluation calls, lowering it on
/// first request.
pub trait Bodies {
    fn body(&mut self, name: &str) -> Option<Rc<Body>>;
}

impl Bodies for HashMap<String, Rc<Body>> {
    fn body(&mut self, name: &str) -> Option<Rc<Body>> {
        self.get(name).cloned()
    }
}

/// Each struct's declared fields, by struct name.
pub type StructFields = HashMap<String, Vec<(String, Ty)>>;

/// What evaluation may see besides MIR bodies.
pub struct Program<'p> {
    pub layouts: &'p TypeLayouts<'p>,
    /// Names of `extern` functions; calling one is rejected.
    pub externs: HashSet<String>,
    /// Declared fields of every struct, for `fields_of`.
    pub structs: StructFields,
    pub enums: HashSet<String>,
}

#[derive(Clone, Copy, Debug)]
pub struct Limits {
    /// MIR statements and terminators one evaluation may execute.
    pub instructions: usize,
    pub call_depth: usize,
}

pub const DEFAULT_INSTRUCTION_BUDGET: usize = 200_000;
pub const DEFAULT_MAX_CALL_DEPTH: usize = 40;

impl Default for Limits {
    fn default() -> Self {
        Self { instructions: DEFAULT_INSTRUCTION_BUDGET, call_depth: DEFAULT_MAX_CALL_DEPTH }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Error {
    pub message: String,
    /// The MIR statement or terminator that failed, when known.
    pub span: Option<Span>,
}

impl Error {
    pub(crate) fn new(message: impl Into<String>) -> Self {
        Self { message: message.into(), span: None }
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

/// Runs `entry` with `args` and returns its result as a value lowering can
/// embed (or a `type`/`Code`), plus everything it printed.
pub fn evaluate(
    program: &Program<'_>,
    bodies: &mut dyn Bodies,
    entry: &str,
    args: &[(Type, ComptimeValue)],
    limits: Limits,
) -> Result<Evaluation, Error> {
    let mut machine = Machine::new(program, bodies, limits);
    let value = machine.run(entry, args)?;
    Ok(Evaluation { value, output: machine.output, stderr: machine.stderr })
}

pub struct Evaluation {
    pub value: ComptimeValue,
    pub output: String,
    pub stderr: String,
}
