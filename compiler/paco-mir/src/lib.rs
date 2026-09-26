//! Paco's backend-independent mid-level IR (MIR).

pub mod autodiff;
pub mod backend;
pub mod body;
pub mod comptime;
pub mod dims;
pub mod glue;
pub mod layout;
pub mod lower;
pub mod type_layout;

pub use backend::{Backend, ENTRY_RETURNS_VALUE_SYMBOL, ENTRY_SYMBOL, ObjectFile, SourceLocator, Target};
pub use comptime::{ComptimeKey, ComptimeValue, splice_sites};
pub use body::{
    BasicBlock, BasicBlockId, BinOp, BlockSpans, Body, CallTarget, Constant, Local, LocalDecl, MathOp, Operand,
    Place, Profile, QuoteTemplate, Rvalue, Statement, Terminator, UnOp,
};
pub use layout::{FieldLayout, Layout, Repr, scalar_layout, struct_layout};
pub use lower::{
    ComptimeSite, InstantiationRegistry, TypeRegistry, is_comptime_only, lower_function, lower_function_with_substitutions, lower_instance, lower_iter_fn,
    is_primitive_type_name, lower_iter_fn_with_substitutions, mangled_name, GRAD_PREFIX, PANIC_SYMBOL, TRY_TEMP,
};
pub use type_layout::TypeLayouts;
