//! Appends MIR to a body block by block, splitting at every call.

use paco_span::Span;
use paco_types::{FloatWidth, IntWidth, Type};

use crate::body::{
    BasicBlock, BasicBlockId, BinOp, BlockSpans, Body, CallTarget, Constant, Local, LocalDecl, Operand, Place, Rvalue, Statement,
    Terminator,
};

pub struct Builder {
    pub body: Body,
    current: Option<(BasicBlockId, Vec<Statement>, Vec<Span>)>,
    pub span: Span,
}

pub fn float(value: f64, width: FloatWidth) -> Operand {
    Operand::Constant(Constant::Float(width.round(value).to_bits(), width))
}

pub fn int(value: i64) -> Operand {
    Operand::Constant(Constant::Int(value, IntWidth::I64))
}

pub fn copy(local: Local) -> Operand {
    Operand::Copy(Place::Local(local))
}

impl Builder {
    pub fn new(body: Body) -> Self {
        let span = body.span;
        Self { body, current: None, span }
    }

    pub fn local(&mut self, ty: Type) -> Local {
        self.body.locals.push(LocalDecl { name: None, ty, mutable: true });
        Local(self.body.locals.len() as u32 - 1)
    }

    pub fn ty(&self, local: Local) -> &Type {
        &self.body.locals[local.0 as usize].ty
    }

    /// A new, empty block to be filled later with `switch_to`.
    pub fn block(&mut self) -> BasicBlockId {
        self.body.blocks.push(BasicBlock { statements: Vec::new(), terminator: Terminator::Unreachable });
        self.body.spans.resize(self.body.blocks.len(), BlockSpans::default());
        BasicBlockId(self.body.blocks.len() as u32 - 1)
    }

    pub fn switch_to(&mut self, block: BasicBlockId) {
        assert!(self.current.is_none(), "the current block is not finished");
        self.current = Some((block, Vec::new(), Vec::new()));
    }

    /// Continues `block` after its statements, dropping its terminator.
    pub fn reopen(&mut self, block: BasicBlockId) {
        assert!(self.current.is_none(), "the current block is not finished");
        let index = block.0 as usize;
        let statements = std::mem::take(&mut self.body.blocks[index].statements);
        let spans = (0..statements.len()).map(|statement| self.body.statement_span(index, statement)).collect();
        self.current = Some((block, statements, spans));
    }

    pub fn is_open(&self) -> bool {
        self.current.is_some()
    }

    pub fn push(&mut self, statement: Statement) {
        let span = self.span;
        let current = self.current.as_mut().expect("no current block");
        current.1.push(statement);
        current.2.push(span);
    }

    pub fn assign(&mut self, place: Place, rvalue: Rvalue) {
        self.push(Statement::Assign(place, rvalue));
    }

    pub fn temp(&mut self, ty: Type, rvalue: Rvalue) -> Local {
        let local = self.local(ty);
        self.assign(Place::Local(local), rvalue);
        local
    }

    pub fn binary(&mut self, op: BinOp, left: Operand, right: Operand, ty: Type) -> Local {
        self.temp(ty, Rvalue::BinaryOp(op, left, right))
    }

    pub fn finish(&mut self, terminator: Terminator) {
        let (block, statements, spans) = self.current.take().expect("no current block");
        let index = block.0 as usize;
        if self.body.spans.len() < self.body.blocks.len() {
            self.body.spans.resize(self.body.blocks.len(), BlockSpans::default());
        }
        self.body.spans[index] = BlockSpans { statements: spans, terminator: Some(self.span) };
        self.body.blocks[index] = BasicBlock { statements, terminator };
    }

    pub fn goto(&mut self, target: BasicBlockId) {
        self.finish(Terminator::Goto(target));
    }

    /// Calls `target` and continues in a fresh block.
    pub fn call(&mut self, target: &str, args: Vec<Operand>, destination: Option<Place>) {
        let resume = self.block();
        self.finish(Terminator::Call { target: CallTarget(target.to_string()), args, destination, resume });
        self.switch_to(resume);
    }

    pub fn call_value(&mut self, target: &str, args: Vec<Operand>, ty: Type) -> Local {
        let local = self.local(ty);
        self.call(target, args, Some(Place::Local(local)));
        local
    }

    /// Runs `then` when `condition` holds and continues after it either way.
    pub fn when(&mut self, condition: Operand, then: impl FnOnce(&mut Self)) {
        let yes = self.block();
        let join = self.block();
        self.finish(Terminator::SwitchInt { discriminant: condition, targets: vec![(1, yes)], otherwise: join });
        self.switch_to(yes);
        then(self);
        self.goto(join);
        self.switch_to(join);
    }

    pub fn if_else(&mut self, condition: Operand, then: impl FnOnce(&mut Self), otherwise: impl FnOnce(&mut Self)) {
        let yes = self.block();
        let no = self.block();
        let join = self.block();
        self.finish(Terminator::SwitchInt { discriminant: condition, targets: vec![(1, yes)], otherwise: no });
        self.switch_to(yes);
        then(self);
        self.goto(join);
        self.switch_to(no);
        otherwise(self);
        self.goto(join);
        self.switch_to(join);
    }

    /// `for i in 0..len { each(i) }`.
    pub fn for_each(&mut self, len: Operand, each: impl FnOnce(&mut Self, Local)) {
        let index = self.local(Type::Int(IntWidth::I64));
        self.assign(Place::Local(index), Rvalue::Use(int(0)));
        let head = self.block();
        let body = self.block();
        let exit = self.block();
        self.goto(head);
        self.switch_to(head);
        let more = self.binary(BinOp::Lt, copy(index), len, Type::Bool);
        self.finish(Terminator::SwitchInt { discriminant: copy(more), targets: vec![(1, body)], otherwise: exit });
        self.switch_to(body);
        each(self, index);
        self.assign(Place::Local(index), Rvalue::BinaryOp(BinOp::Add, copy(index), int(1)));
        self.goto(head);
        self.switch_to(exit);
    }

    pub fn into_body(self) -> Body {
        assert!(self.current.is_none(), "the current block is not finished");
        self.body
    }
}
