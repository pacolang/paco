//! MIR control-flow-graph types.

use paco_span::Span;
use paco_types::{FloatWidth, IntWidth, Type};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Profile {
    Debug,
    Release,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Body {
    pub locals: Vec<LocalDecl>,
    pub blocks: Vec<BasicBlock>,
    pub profile: Profile,
    pub param_count: usize,
    pub return_ty: Type,
    pub span: Span,
    /// Parallel to `blocks`; empty for hand-built bodies.
    pub spans: Vec<BlockSpans>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct BlockSpans {
    pub statements: Vec<Span>,
    pub terminator: Option<Span>,
}

impl Body {
    pub fn statement_span(&self, block: usize, statement: usize) -> Span {
        self.spans.get(block).and_then(|spans| spans.statements.get(statement).copied()).unwrap_or(self.span)
    }

    pub fn terminator_span(&self, block: usize) -> Span {
        self.spans.get(block).and_then(|spans| spans.terminator).unwrap_or(self.span)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct Local(pub u32);

#[derive(Clone, Debug, PartialEq)]
pub struct LocalDecl {
    pub name: Option<String>,
    pub ty: Type,
    pub mutable: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Place {
    Local(Local),
    Field { base: Box<Place>, field: String },
    VariantField {
        base: Box<Place>,
        variant: String,
        index: usize,
    },
    /// `base[index]` on a `[]T`/`&[]T`/`&mut []T` receiver — the built-in
    /// slice case, which needs a runtime bounds check against `base`'s own
    /// stored length (codegen's job; `base`'s slice-ness is what tells
    /// codegen a check is needed here, unlike `Deref` below).
    Index { base: Box<Place>, index: Box<Operand> },
    /// An already-computed pointer value, treated as a place at that
    /// address — the structurally-dispatched `Index<Idx>` case (a
    /// user-defined `fn index(&self, i: Idx) -> &Output` method call's
    /// returned pointer), which needs no bounds check of its own: whatever
    /// the method implementation already checked (or didn't) is between it
    /// and its own caller, not this place's concern. `ty` is the pointee
    /// type (`Output`) — unlike `Field`/`VariantField`/`Index`, there is no
    /// base place/layout to derive it from structurally, so (matching
    /// `Rvalue::Load`'s identical need) it is carried explicitly.
    Deref { address: Box<Operand>, ty: Type },
}

impl From<Local> for Place {
    fn from(local: Local) -> Self {
        Place::Local(local)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Constant {
    Int(i64, IntWidth),
    /// The `f64` bits of a value already rounded to `FloatWidth`.
    Float(u64, FloatWidth),
    Bool(bool),
    Char(char),
    Str(String),
    Unit,
    /// A `type` value; only compile-time code handles it.
    Type(Type),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Operand {
    Copy(Place),
    Move(Place),
    Constant(Constant),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
    BitAnd,
    BitOr,
    BitXor,
    /// Panics in debug when the amount is negative or not below the bit
    /// width; masks the amount to the width in release.
    Shl,
    Shr,
    /// Two's-complement result, never checked.
    WrappingAdd,
    WrappingSub,
    WrappingMul,
    /// Whether the operation overflows the operands' integer type.
    AddOverflows,
    SubOverflows,
    MulOverflows,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UnOp {
    Not,
    Neg,
    BitNot,
}

/// Float math methods (`x.sqrt()`, `x.powf(y)`, ...), computed in `f64`
/// for `f16`/`bf16` and rounded back.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum MathOp {
    Sqrt,
    Exp,
    Ln,
    Sin,
    Cos,
    Tanh,
    Abs,
    Powf,
    Min,
    Max,
}

impl MathOp {
    pub fn from_method(name: &str) -> Option<MathOp> {
        Some(match name {
            "sqrt" => MathOp::Sqrt,
            "exp" => MathOp::Exp,
            "ln" => MathOp::Ln,
            "sin" => MathOp::Sin,
            "cos" => MathOp::Cos,
            "tanh" => MathOp::Tanh,
            "abs" => MathOp::Abs,
            "powf" => MathOp::Powf,
            "min" => MathOp::Min,
            "max" => MathOp::Max,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            MathOp::Sqrt => "sqrt",
            MathOp::Exp => "exp",
            MathOp::Ln => "ln",
            MathOp::Sin => "sin",
            MathOp::Cos => "cos",
            MathOp::Tanh => "tanh",
            MathOp::Abs => "abs",
            MathOp::Powf => "powf",
            MathOp::Min => "min",
            MathOp::Max => "max",
        }
    }

    /// The runtime function computing it in `f64`, when no instruction does.
    pub fn runtime_symbol(self) -> Option<&'static str> {
        match self {
            MathOp::Exp => Some("paco_math_exp"),
            MathOp::Ln => Some("paco_math_ln"),
            MathOp::Sin => Some("paco_math_sin"),
            MathOp::Cos => Some("paco_math_cos"),
            MathOp::Tanh => Some("paco_math_tanh"),
            MathOp::Powf => Some("paco_math_powf"),
            MathOp::Sqrt | MathOp::Abs | MathOp::Min | MathOp::Max => None,
        }
    }

    /// IEEE 754-2019 `minimum`/`maximum` for `min`/`max`: a NaN operand
    /// gives NaN and `-0.0 < 0.0`.
    pub fn eval(self, x: f64, y: f64) -> f64 {
        match self {
            MathOp::Sqrt => x.sqrt(),
            MathOp::Abs => x.abs(),
            MathOp::Exp => libm::exp(x),
            MathOp::Ln => libm::log(x),
            MathOp::Sin => libm::sin(x),
            MathOp::Cos => libm::cos(x),
            MathOp::Tanh => libm::tanh(x),
            MathOp::Powf => libm::pow(x, y),
            MathOp::Min | MathOp::Max if x.is_nan() || y.is_nan() => f64::NAN,
            MathOp::Min if x == y => if x.is_sign_negative() { x } else { y },
            MathOp::Max if x == y => if x.is_sign_positive() { x } else { y },
            MathOp::Min => x.min(y),
            MathOp::Max => x.max(y),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Rvalue {
    Use(Operand),
    /// `args[0]` is the receiver; `powf`/`min`/`max` take one more.
    Math(MathOp, Vec<Operand>),
    BinaryOp(BinOp, Operand, Operand),
    UnaryOp(UnOp, Operand),
    Ref { mutable: bool, place: Place },
    Aggregate {
        ty: Type,
        variant: Option<String>,
        fields: Vec<Operand>,
    },
    Discriminant(Place),
    SliceLen(Place),
    Cast { operand: Operand, target: Type },
    /// Loads a `ty`-typed value from a computed address — the inverse of
    /// `Ref`. Unlike `Place::Field`, `address` is not a name-keyed
    /// structured place; it is an arbitrary pointer value (e.g. a spawn
    /// thunk's captures-buffer pointer plus a byte offset already baked
    /// into how `address` was computed). Scoped, for now, to compiler-
    /// generated thunk lowering (capture unpacking), not Paco's `*p`.
    Load { address: Operand, ty: Type },
    /// Allocates `size` bytes of scratch stack storage and produces its
    /// address — a raw, type-less buffer for compiler-internal marshaling
    /// (a spawn thunk's packed captures buffer, one 8-byte slot per
    /// captured scalar — see `scalar_byte_len`'s own scoping note), not a
    /// real Paco value or a place any Paco-level pattern ever names.
    RawAlloc { size: u64 },
    /// The address of the named top-level function — resolved the same
    /// way `Terminator::Call`'s `CallTarget` already is (by-name, against
    /// codegen's `func_ids` table), just producing the address as an
    /// ordinary value instead of calling it. Needed to pass a spawn
    /// thunk's address to `paco_rt_spawn` as a function-pointer argument;
    /// MIR has no first-class function-pointer/closure *type* — this is
    /// only ever an opaque pointer-sized value.
    FuncAddr(String),
    /// A `quote { .. }` template and the value of each splice, keyed by the
    /// splice's span; only compile-time code evaluates it.
    Quote { template: QuoteTemplate, splices: Vec<(Span, Operand)> },
}

#[derive(Clone, Debug, PartialEq)]
pub struct QuoteTemplate(pub Box<paco_syntax::ast::QuoteBody>);

impl Eq for QuoteTemplate {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Statement {
    Assign(Place, Rvalue),
    /// Stores a `ty`-typed value to a computed address — the inverse of
    /// `Load`, for the same reason (writing a spawn thunk's result through
    /// its `result_out` pointer parameter, per the FFI's fixed C ABI,
    /// which takes no return value).
    /// An aggregate `ty` (struct, enum, tuple, string, slice) is moved into
    /// a fresh heap box and the box's address is stored instead.
    Store { address: Operand, value: Operand, ty: Type },
    /// Frees the heap box a `Store` of an aggregate `ty` left at `address`;
    /// a no-op for any other `ty`.
    FreeBox { address: Operand, ty: Type },
    Drop(Place),
    StorageDead(Local),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct BasicBlockId(pub u32);

#[derive(Clone, Debug, PartialEq)]
pub struct BasicBlock {
    pub statements: Vec<Statement>,
    pub terminator: Terminator,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CallTarget(pub String);

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Terminator {
    Goto(BasicBlockId),
    SwitchInt {
        discriminant: Operand,
        targets: Vec<(i128, BasicBlockId)>,
        otherwise: BasicBlockId,
    },
    Call {
        target: CallTarget,
        args: Vec<Operand>,
        destination: Option<Place>,
        resume: BasicBlockId,
    },
    CallIndirect {
        callee: Operand,
        args: Vec<Operand>,
        destination: Option<Place>,
        resume: BasicBlockId,
    },
    Return(Operand),
    Unreachable,
}
