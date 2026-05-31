//! Ownership and move analysis for the executable frontend phases.

use std::collections::HashMap;

use paco_diag::{Diagnostic, Reporter};
use paco_span::Span;
use paco_syntax::ast::{
    Block, Expr, FnDecl, Item, Literal, MatchArm, Module, Pat, Stmt, Ty, VariantFields,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BorrowError;

pub fn check_module(module: &Module, reporter: &mut Reporter) -> Result<(), BorrowError> {
    let program = Program::from_module(module);
    for item in &module.items {
        match item {
            Item::Fn(function) => check_function(function, &program, reporter),
            Item::Struct(decl) => {
                for method in &decl.methods {
                    check_function(method, &program, reporter);
                }
            }
            Item::Enum(decl) => {
                for method in &decl.methods {
                    check_function(method, &program, reporter);
                }
            }
            Item::Methods(block) => {
                for method in &block.methods {
                    check_function(method, &program, reporter);
                }
            }
            Item::Trait(_) | Item::Use(_) => {}
        }
    }
    if reporter.has_errors() {
        Err(BorrowError)
    } else {
        Ok(())
    }
}

#[derive(Clone, Debug, Default)]
struct Program {
    functions: HashMap<String, FunctionSignature>,
    methods: HashMap<(String, String), FunctionSignature>,
    fields: HashMap<(String, String), Ty>,
    variants: HashMap<(String, String), Vec<Ty>>,
    type_params: HashMap<String, Vec<String>>,
}

impl Program {
    fn from_module(module: &Module) -> Self {
        let mut program = Self::default();
        for item in &module.items {
            match item {
                Item::Fn(function) => {
                    program
                        .functions
                        .insert(function.name.clone(), FunctionSignature::from(function));
                }
                Item::Struct(decl) => {
                    program
                        .type_params
                        .insert(decl.name.clone(), decl.generics.clone());
                    for field in &decl.fields {
                        program
                            .fields
                            .insert((decl.name.clone(), field.name.clone()), field.ty.clone());
                    }
                    for method in &decl.methods {
                        program.methods.insert(
                            (decl.name.clone(), method.name.clone()),
                            FunctionSignature::from(method),
                        );
                    }
                }
                Item::Enum(decl) => {
                    program
                        .type_params
                        .insert(decl.name.clone(), decl.generics.clone());
                    for variant in &decl.variants {
                        let fields = match &variant.fields {
                            VariantFields::Unit => Vec::new(),
                            VariantFields::Tuple(fields) => fields.clone(),
                            VariantFields::Struct(fields) => {
                                fields.iter().map(|field| field.ty.clone()).collect()
                            }
                        };
                        program
                            .variants
                            .insert((decl.name.clone(), variant.name.clone()), fields);
                    }
                    for method in &decl.methods {
                        program.methods.insert(
                            (decl.name.clone(), method.name.clone()),
                            FunctionSignature::from(method),
                        );
                    }
                }
                Item::Methods(block) => {
                    if let Some(name) = type_name(&block.target) {
                        for method in &block.methods {
                            program.methods.insert(
                                (name.clone(), method.name.clone()),
                                FunctionSignature::from(method),
                            );
                        }
                    }
                }
                Item::Trait(_) | Item::Use(_) => {}
            }
        }
        program
    }
}

#[derive(Clone, Debug)]
struct FunctionSignature {
    generics: Vec<String>,
    params: Vec<Ty>,
    return_ty: Option<Ty>,
}

impl FunctionSignature {
    fn from(function: &FnDecl) -> Self {
        Self {
            generics: function.generics.clone(),
            params: function
                .params
                .iter()
                .map(|param| param.ty.clone())
                .collect(),
            return_ty: function.return_ty.clone(),
        }
    }
}

#[derive(Clone, Debug)]
struct BindingState {
    copy: bool,
    ty: Option<Ty>,
    moved_at: Option<Span>,
    move_count: usize,
}

#[derive(Clone, Debug)]
struct OwnershipState {
    scopes: Vec<HashMap<String, BindingState>>,
    reachable: bool,
}

impl Default for OwnershipState {
    fn default() -> Self {
        Self {
            scopes: Vec::new(),
            reachable: true,
        }
    }
}

impl OwnershipState {
    fn push_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }

    fn pop_scope(&mut self) {
        self.scopes.pop();
    }

    fn define(&mut self, name: String, copy: bool, ty: Option<Ty>) {
        if let Some(scope) = self.scopes.last_mut() {
            scope.insert(
                name,
                BindingState {
                    copy,
                    ty,
                    moved_at: None,
                    move_count: 0,
                },
            );
        }
    }

    fn get(&self, name: &str) -> Option<&BindingState> {
        self.scopes.iter().rev().find_map(|scope| scope.get(name))
    }

    fn get_mut(&mut self, name: &str) -> Option<&mut BindingState> {
        self.scopes
            .iter_mut()
            .rev()
            .find_map(|scope| scope.get_mut(name))
    }
}

fn check_function(function: &FnDecl, program: &Program, reporter: &mut Reporter) {
    let mut state = OwnershipState::default();
    state.push_scope();
    for param in &function.params {
        if let Pat::Ident(name, _) = &param.pattern {
            state.define(name.clone(), ty_is_copy(&param.ty), Some(param.ty.clone()));
        }
    }
    check_block(&function.body, program, &mut state, reporter);
}

fn check_block(
    block: &Block,
    program: &Program,
    state: &mut OwnershipState,
    reporter: &mut Reporter,
) {
    state.push_scope();
    for statement in &block.stmts {
        if !state.reachable {
            break;
        }
        match statement {
            Stmt::Let(statement) => {
                let binding_ty = statement.ty.clone().or_else(|| {
                    statement
                        .value
                        .as_ref()
                        .and_then(|expr| expr_ty(expr, program, state))
                });
                let copy = binding_ty.as_ref().map_or_else(
                    || {
                        statement
                            .value
                            .as_ref()
                            .is_some_and(|expr| expr_is_copy(expr, program, state))
                    },
                    ty_is_copy,
                );
                if let Some(value) = &statement.value {
                    if copy {
                        check_expr(value, program, state, reporter);
                    } else {
                        consume_expr(value, program, state, reporter);
                    }
                }
                bind_pattern(&statement.pattern, copy, binding_ty, program, state);
            }
            Stmt::Expr(expr) => check_discarded_expr(expr, program, state, reporter),
            Stmt::Item(_) => {}
        }
    }
    if state.reachable
        && let Some(tail) = &block.tail
    {
        check_expr(tail, program, state, reporter);
    }
    state.pop_scope();
}

fn check_expr(expr: &Expr, program: &Program, state: &mut OwnershipState, reporter: &mut Reporter) {
    match expr {
        Expr::Literal(_, _) => {}
        Expr::Ident(name, span) => check_ident_use(name, *span, state, reporter),
        Expr::Block(block) => check_block(block, program, state, reporter),
        Expr::If {
            condition,
            then_branch,
            else_branch,
            span,
        } => {
            check_expr(condition, program, state, reporter);
            let before = state.clone();
            let mut then_state = before.clone();
            check_block(then_branch, program, &mut then_state, reporter);
            let mut else_state = before.clone();
            if let Some(else_branch) = else_branch {
                check_expr(else_branch, program, &mut else_state, reporter);
            }
            merge_branch_states(*span, &before, &then_state, &else_state, state, reporter);
        }
        Expr::Loop { body, span } => check_loop_body(*span, body, program, state, reporter),
        Expr::While {
            condition,
            body,
            span,
        } => {
            check_expr(condition, program, state, reporter);
            check_loop_body(*span, body, program, state, reporter);
        }
        Expr::Match {
            scrutinee,
            arms,
            span,
            ..
        } => check_match(scrutinee, arms, *span, program, state, reporter),
        Expr::Call { callee, args, .. } => check_call(callee, args, program, state, reporter),
        Expr::MethodCall {
            receiver,
            method,
            args,
            ..
        } => check_method_call(receiver, method, args, program, state, reporter),
        Expr::AssociatedCall { args, .. } => {
            for arg in args {
                consume_expr(arg, program, state, reporter);
            }
        }
        Expr::Binary { left, right, .. } => {
            check_expr(left, program, state, reporter);
            check_expr(right, program, state, reporter);
        }
        Expr::Unary { expr, .. } => check_expr(expr, program, state, reporter),
        Expr::Assign { target, value, .. } => {
            consume_expr(value, program, state, reporter);
            match target.as_ref() {
                Expr::Ident(name, _) => {
                    if let Some(binding) = state.get_mut(name) {
                        binding.moved_at = None;
                    }
                }
                _ => check_expr(target, program, state, reporter),
            }
        }
        Expr::Field { base, .. } => check_expr(base, program, state, reporter),
        Expr::Index { base, index, .. } => {
            check_expr(base, program, state, reporter);
            check_expr(index, program, state, reporter);
        }
        Expr::Return(value, _) | Expr::Break(value, _) => {
            if let Some(value) = value {
                consume_expr(value, program, state, reporter);
            }
            state.reachable = false;
        }
        Expr::Continue(_) => {
            state.reachable = false;
        }
        Expr::Spawn { expr, .. } | Expr::Comptime { expr, .. } | Expr::Borrow { expr, .. } => {
            check_expr(expr, program, state, reporter);
        }
        Expr::Select { arms, default, .. } => {
            for arm in arms {
                check_expr(&arm.operation, program, state, reporter);
                check_block(&arm.body, program, state, reporter);
            }
            if let Some(default) = default {
                check_block(default, program, state, reporter);
            }
        }
        Expr::Yield(expr, _) => consume_expr(expr, program, state, reporter),
        Expr::StructLiteral { fields, .. } => {
            for (_, value) in fields {
                consume_expr(value, program, state, reporter);
            }
        }
    }
}

fn check_loop_body(
    span: Span,
    body: &Block,
    program: &Program,
    state: &mut OwnershipState,
    reporter: &mut Reporter,
) {
    let before = state.clone();
    let mut body_state = before.clone();
    check_block(body, program, &mut body_state, reporter);
    for scope_index in 0..before.scopes.len() {
        for name in before.scopes[scope_index].keys() {
            let move_count_before = before.scopes[scope_index]
                .get(name)
                .map_or(0, |binding| binding.move_count);
            let move_count_after = body_state
                .scopes
                .get(scope_index)
                .and_then(|scope| scope.get(name))
                .map_or(move_count_before, |binding| binding.move_count);
            if move_count_after > move_count_before {
                reporter.push(Diagnostic::error(
                    "use-after-move",
                    span,
                    format!("loop body may move `{name}` on more than one iteration"),
                ));
            }
        }
    }
}

fn check_call(
    callee: &Expr,
    args: &[Expr],
    program: &Program,
    state: &mut OwnershipState,
    reporter: &mut Reporter,
) {
    let Expr::Ident(name, _) = callee else {
        check_expr(callee, program, state, reporter);
        for arg in args {
            consume_expr(arg, program, state, reporter);
        }
        return;
    };
    if name == "print" {
        for arg in args {
            check_expr(arg, program, state, reporter);
        }
        return;
    }
    let signature = program.functions.get(name);
    for (index, arg) in args.iter().enumerate() {
        if signature
            .and_then(|signature| signature.params.get(index))
            .is_some_and(ty_is_copy)
        {
            check_expr(arg, program, state, reporter);
        } else {
            consume_expr(arg, program, state, reporter);
        }
    }
}

fn check_method_call(
    receiver: &Expr,
    method: &str,
    args: &[Expr],
    program: &Program,
    state: &mut OwnershipState,
    reporter: &mut Reporter,
) {
    let receiver_ty = expr_ty(receiver, program, state);
    let signature = method_signature(receiver, method, program, state);
    let receiver_is_copy = receiver_ty.as_ref().is_some_and(ty_is_copy);
    let receiver_is_borrowed = signature
        .and_then(|signature| signature.params.first())
        .is_some_and(|ty| matches!(ty, Ty::Borrow { .. }));
    if receiver_is_copy || receiver_is_borrowed {
        check_expr(receiver, program, state, reporter);
    } else {
        consume_expr(receiver, program, state, reporter);
    }
    for (index, arg) in args.iter().enumerate() {
        if signature
            .and_then(|signature| signature.params.get(index + 1))
            .is_some_and(ty_is_copy)
        {
            check_expr(arg, program, state, reporter);
        } else {
            consume_expr(arg, program, state, reporter);
        }
    }
}

fn check_match(
    scrutinee: &Expr,
    arms: &[MatchArm],
    span: Span,
    program: &Program,
    state: &mut OwnershipState,
    reporter: &mut Reporter,
) {
    let scrutinee_copy = expr_is_copy(scrutinee, program, state);
    if scrutinee_copy {
        check_expr(scrutinee, program, state, reporter);
    } else {
        consume_expr(scrutinee, program, state, reporter);
    }
    let before = state.clone();
    let mut branch_states = Vec::new();
    for arm in arms {
        let mut arm_state = before.clone();
        arm_state.push_scope();
        bind_pattern(
            &arm.pattern,
            scrutinee_copy,
            expr_ty(scrutinee, program, state),
            program,
            &mut arm_state,
        );
        if let Some(guard) = &arm.guard {
            check_expr(guard, program, &mut arm_state, reporter);
        }
        check_expr(&arm.body, program, &mut arm_state, reporter);
        arm_state.pop_scope();
        branch_states.push(arm_state);
    }
    if !branch_states.is_empty() {
        merge_many_branch_states(span, &before, &branch_states, state, reporter);
    }
}

fn consume_expr(
    expr: &Expr,
    program: &Program,
    state: &mut OwnershipState,
    reporter: &mut Reporter,
) {
    match expr {
        Expr::Ident(name, span) => {
            check_ident_use(name, *span, state, reporter);
            if let Some(binding) = state.get_mut(name)
                && !binding.copy
            {
                binding.moved_at = Some(*span);
                binding.move_count += 1;
            }
        }
        Expr::Field { base, span, .. } => {
            check_expr(base, program, state, reporter);
            reporter.push(Diagnostic::error(
                "use-after-move",
                *span,
                "field moves are not supported in Phase 4",
            ));
        }
        Expr::Block(block) => consume_block(block, program, state, reporter),
        Expr::If {
            condition,
            then_branch,
            else_branch,
            span,
        } => {
            check_expr(condition, program, state, reporter);
            let before = state.clone();
            let mut then_state = before.clone();
            consume_block(then_branch, program, &mut then_state, reporter);
            let mut else_state = before.clone();
            if let Some(else_branch) = else_branch {
                consume_expr(else_branch, program, &mut else_state, reporter);
            }
            merge_branch_states(*span, &before, &then_state, &else_state, state, reporter);
        }
        Expr::Match {
            scrutinee,
            arms,
            span,
            ..
        } => consume_match(scrutinee, arms, *span, program, state, reporter),
        _ => check_expr(expr, program, state, reporter),
    }
}

fn consume_block(
    block: &Block,
    program: &Program,
    state: &mut OwnershipState,
    reporter: &mut Reporter,
) {
    state.push_scope();
    for statement in &block.stmts {
        if !state.reachable {
            break;
        }
        match statement {
            Stmt::Let(statement) => {
                let binding_ty = statement.ty.clone().or_else(|| {
                    statement
                        .value
                        .as_ref()
                        .and_then(|expr| expr_ty(expr, program, state))
                });
                let copy = binding_ty.as_ref().map_or_else(
                    || {
                        statement
                            .value
                            .as_ref()
                            .is_some_and(|expr| expr_is_copy(expr, program, state))
                    },
                    ty_is_copy,
                );
                if let Some(value) = &statement.value {
                    if copy {
                        check_expr(value, program, state, reporter);
                    } else {
                        consume_expr(value, program, state, reporter);
                    }
                }
                bind_pattern(&statement.pattern, copy, binding_ty, program, state);
            }
            Stmt::Expr(expr) => check_discarded_expr(expr, program, state, reporter),
            Stmt::Item(_) => {}
        }
    }
    if state.reachable
        && let Some(tail) = &block.tail
    {
        consume_expr(tail, program, state, reporter);
    }
    state.pop_scope();
}

fn consume_match(
    scrutinee: &Expr,
    arms: &[MatchArm],
    span: Span,
    program: &Program,
    state: &mut OwnershipState,
    reporter: &mut Reporter,
) {
    let scrutinee_copy = expr_is_copy(scrutinee, program, state);
    if scrutinee_copy {
        check_expr(scrutinee, program, state, reporter);
    } else {
        consume_expr(scrutinee, program, state, reporter);
    }
    let before = state.clone();
    let mut branch_states = Vec::new();
    for arm in arms {
        let mut arm_state = before.clone();
        arm_state.push_scope();
        bind_pattern(
            &arm.pattern,
            scrutinee_copy,
            expr_ty(scrutinee, program, state),
            program,
            &mut arm_state,
        );
        if let Some(guard) = &arm.guard {
            check_expr(guard, program, &mut arm_state, reporter);
        }
        consume_expr(&arm.body, program, &mut arm_state, reporter);
        arm_state.pop_scope();
        branch_states.push(arm_state);
    }
    if !branch_states.is_empty() {
        merge_many_branch_states(span, &before, &branch_states, state, reporter);
    }
}

fn check_discarded_expr(
    expr: &Expr,
    program: &Program,
    state: &mut OwnershipState,
    reporter: &mut Reporter,
) {
    if expr_is_copy(expr, program, state) {
        check_expr(expr, program, state, reporter);
    } else {
        consume_expr(expr, program, state, reporter);
    }
}

fn check_ident_use(name: &str, span: Span, state: &OwnershipState, reporter: &mut Reporter) {
    if let Some(binding) = state.get(name)
        && binding.moved_at.is_some()
    {
        reporter.push(Diagnostic::error(
            "use-after-move",
            span,
            format!("use of moved value `{name}`"),
        ));
    }
}

fn merge_many_branch_states(
    span: Span,
    before: &OwnershipState,
    branches: &[OwnershipState],
    target: &mut OwnershipState,
    reporter: &mut Reporter,
) {
    if branches.is_empty() {
        return;
    }
    for scope_index in 0..before.scopes.len() {
        for name in before.scopes[scope_index].keys() {
            let max_move_count = branches
                .iter()
                .filter_map(|branch| {
                    branch
                        .scopes
                        .get(scope_index)
                        .and_then(|scope| scope.get(name))
                        .map(|binding| binding.move_count)
                })
                .max();
            if let Some(max_move_count) = max_move_count
                && let Some(binding) = target
                    .scopes
                    .get_mut(scope_index)
                    .and_then(|scope| scope.get_mut(name))
            {
                binding.move_count = max_move_count;
            }
        }
    }

    let reachable_branches: Vec<_> = branches.iter().filter(|branch| branch.reachable).collect();
    if reachable_branches.is_empty() {
        target.reachable = false;
        return;
    }
    target.reachable = true;
    let first = reachable_branches[0];
    for scope_index in 0..before.scopes.len() {
        for name in before.scopes[scope_index].keys() {
            let moved_count = reachable_branches
                .iter()
                .filter(|branch| {
                    branch
                        .scopes
                        .get(scope_index)
                        .and_then(|scope| scope.get(name))
                        .is_some_and(|binding| binding.moved_at.is_some())
                })
                .count();
            if moved_count != 0 && moved_count != reachable_branches.len() {
                reporter.push(Diagnostic::error(
                    "use-after-move",
                    span,
                    format!("inconsistent move state for `{name}` across branches"),
                ));
            }
            if moved_count == reachable_branches.len() {
                let moved_at = first.scopes[scope_index].get(name).and_then(|b| b.moved_at);
                if let Some(binding) = target
                    .scopes
                    .get_mut(scope_index)
                    .and_then(|scope| scope.get_mut(name))
                {
                    binding.moved_at = moved_at;
                }
            } else if moved_count == 0
                && let Some(binding) = target
                    .scopes
                    .get_mut(scope_index)
                    .and_then(|scope| scope.get_mut(name))
            {
                binding.moved_at = None;
            }
        }
    }
}

fn merge_branch_states(
    span: Span,
    before: &OwnershipState,
    left: &OwnershipState,
    right: &OwnershipState,
    target: &mut OwnershipState,
    reporter: &mut Reporter,
) {
    merge_many_branch_states(
        span,
        before,
        &[left.clone(), right.clone()],
        target,
        reporter,
    );
}

fn bind_pattern(
    pattern: &Pat,
    copy: bool,
    ty: Option<Ty>,
    program: &Program,
    state: &mut OwnershipState,
) {
    match pattern {
        Pat::Ident(name, _) => state.define(name.clone(), copy, ty),
        Pat::Binding { name, pattern, .. } => {
            state.define(name.clone(), copy, ty.clone());
            bind_pattern(pattern, copy, ty, program, state);
        }
        Pat::Tuple(fields, _) => {
            let field_tys = match ty.as_ref() {
                Some(Ty::Tuple(items, _)) => items.clone(),
                _ => Vec::new(),
            };
            for (index, field) in fields.iter().enumerate() {
                let field_ty = field_tys.get(index).cloned().or_else(|| ty.clone());
                bind_pattern(
                    field,
                    field_ty.as_ref().map_or(copy, ty_is_copy),
                    field_ty,
                    program,
                    state,
                );
            }
        }
        Pat::Or(fields, _) => {
            for field in fields {
                bind_pattern(field, copy, ty.clone(), program, state);
            }
        }
        Pat::Struct { fields, .. } => {
            for (name, field) in fields {
                let field_ty = ty
                    .as_ref()
                    .and_then(|ty| field_ty_from_type(ty, name, program))
                    .or_else(|| ty.clone());
                bind_pattern(
                    field,
                    field_ty.as_ref().map_or(copy, ty_is_copy),
                    field_ty,
                    program,
                    state,
                );
            }
        }
        Pat::Enum { path, fields, .. } => {
            let field_tys = variant_tys_from_pattern(path, ty.as_ref(), program);
            for (index, field) in fields.iter().enumerate() {
                let field_ty = field_tys.get(index).cloned().or_else(|| ty.clone());
                bind_pattern(
                    field,
                    field_ty.as_ref().map_or(copy, ty_is_copy),
                    field_ty,
                    program,
                    state,
                );
            }
        }
        Pat::Range { .. } | Pat::Wildcard(_) | Pat::Literal(_, _) => {}
    }
}

fn expr_is_copy(expr: &Expr, program: &Program, state: &OwnershipState) -> bool {
    match expr {
        Expr::Literal(literal, _) => !matches!(literal, Literal::String(_)),
        Expr::Ident(name, _) => state.get(name).is_some_and(|binding| binding.copy),
        Expr::Call { callee, .. } => {
            let Expr::Ident(name, _) = callee.as_ref() else {
                return false;
            };
            program
                .functions
                .get(name)
                .and_then(|signature| signature.return_ty.as_ref())
                .is_some_and(ty_is_copy)
        }
        Expr::MethodCall { .. } | Expr::AssociatedCall { .. } | Expr::Field { .. } => {
            expr_ty(expr, program, state)
                .as_ref()
                .is_some_and(ty_is_copy)
        }
        Expr::Binary { .. } | Expr::Unary { .. } => true,
        _ => false,
    }
}

fn expr_ty(expr: &Expr, program: &Program, state: &OwnershipState) -> Option<Ty> {
    match expr {
        Expr::Literal(literal, span) => Some(literal_ty(literal, *span)),
        Expr::Ident(name, _) => state.get(name).and_then(|binding| binding.ty.clone()),
        Expr::Block(block) => block_ty(block, program, state),
        Expr::If {
            then_branch,
            else_branch,
            ..
        } => {
            let then_ty = block_ty(then_branch, program, state)?;
            let else_ty = else_branch
                .as_ref()
                .and_then(|branch| expr_ty(branch, program, state))?;
            if same_ty_shape(&then_ty, &else_ty) {
                Some(then_ty)
            } else {
                None
            }
        }
        Expr::Match { arms, .. } => {
            let first_ty = arms
                .first()
                .and_then(|arm| match_arm_body_ty(expr, arm, program, state))?;
            if arms.iter().all(|arm| {
                match_arm_body_ty(expr, arm, program, state)
                    .is_some_and(|ty| same_ty_shape(&ty, &first_ty))
            }) {
                Some(first_ty)
            } else {
                None
            }
        }
        Expr::StructLiteral { ty, .. } => Some(ty.clone()),
        Expr::AssociatedCall {
            ty, function, args, ..
        } => type_name(ty)
            .and_then(|name| {
                program
                    .methods
                    .get(&(name, function.clone()))
                    .and_then(|signature| {
                        signature.return_ty.clone().map(|ty| {
                            substitute_ty(ty, &call_substitutions(signature, args, program, state))
                        })
                    })
            })
            .or_else(|| Some(ty.clone())),
        Expr::MethodCall {
            receiver,
            method,
            args,
            ..
        } => method_signature(receiver, method, program, state).and_then(|signature| {
            signature.return_ty.clone().map(|ty| {
                substitute_ty(
                    ty,
                    &method_call_substitutions(signature, args, program, state),
                )
            })
        }),
        Expr::Field { base, field, .. } => field_ty(base, field, program, state),
        Expr::Call { callee, args, .. } => {
            let Expr::Ident(name, _) = callee.as_ref() else {
                return None;
            };
            program.functions.get(name).and_then(|signature| {
                signature.return_ty.clone().map(|ty| {
                    substitute_ty(ty, &call_substitutions(signature, args, program, state))
                })
            })
        }
        _ => None,
    }
}

fn block_ty(block: &Block, program: &Program, state: &OwnershipState) -> Option<Ty> {
    let mut local_state = state.clone();
    local_state.push_scope();
    for statement in &block.stmts {
        if let Stmt::Let(statement) = statement {
            let binding_ty = statement.ty.clone().or_else(|| {
                statement
                    .value
                    .as_ref()
                    .and_then(|expr| expr_ty(expr, program, &local_state))
            });
            let copy = binding_ty.as_ref().map_or_else(
                || {
                    statement
                        .value
                        .as_ref()
                        .is_some_and(|expr| expr_is_copy(expr, program, &local_state))
                },
                ty_is_copy,
            );
            bind_pattern(
                &statement.pattern,
                copy,
                binding_ty,
                program,
                &mut local_state,
            );
        }
    }
    block
        .tail
        .as_ref()
        .and_then(|tail| expr_ty(tail, program, &local_state))
}

fn match_arm_body_ty(
    match_expr: &Expr,
    arm: &MatchArm,
    program: &Program,
    state: &OwnershipState,
) -> Option<Ty> {
    let Expr::Match { scrutinee, .. } = match_expr else {
        return None;
    };
    let scrutinee_ty = expr_ty(scrutinee, program, state);
    let scrutinee_copy = scrutinee_ty.as_ref().is_some_and(ty_is_copy);
    let mut arm_state = state.clone();
    arm_state.push_scope();
    bind_pattern(
        &arm.pattern,
        scrutinee_copy,
        scrutinee_ty,
        program,
        &mut arm_state,
    );
    expr_ty(&arm.body, program, &arm_state)
}

fn literal_ty(literal: &Literal, span: Span) -> Ty {
    let name = match literal {
        Literal::Int(_) => "int",
        Literal::Float(_) => "float",
        Literal::Bool(_) => "bool",
        Literal::String(_) => "string",
        Literal::Char(_) => "char",
    };
    Ty::Path(vec![name.to_string()], span)
}

fn field_ty(base: &Expr, field: &str, program: &Program, state: &OwnershipState) -> Option<Ty> {
    let base_ty = expr_ty(base, program, state)?;
    field_ty_from_type(&base_ty, field, program)
}

fn field_ty_from_type(base_ty: &Ty, field: &str, program: &Program) -> Option<Ty> {
    let name = type_name(base_ty)?;
    let ty = program.fields.get(&(name, field.to_string()))?.clone();
    Some(substitute_ty(ty, &type_substitutions(base_ty, program)))
}

fn variant_tys_from_pattern(
    path: &[String],
    scrutinee_ty: Option<&Ty>,
    program: &Program,
) -> Vec<Ty> {
    let Some(variant_name) = path.last() else {
        return Vec::new();
    };
    let enum_name = if path.len() >= 2 {
        path.first().cloned()
    } else {
        scrutinee_ty.and_then(type_name)
    };
    let Some(enum_name) = enum_name else {
        return Vec::new();
    };
    let substitutions = scrutinee_ty
        .map(|ty| type_substitutions(ty, program))
        .unwrap_or_default();
    program
        .variants
        .get(&(enum_name, variant_name.clone()))
        .map(|fields| {
            fields
                .iter()
                .cloned()
                .map(|field| substitute_ty(field, &substitutions))
                .collect()
        })
        .unwrap_or_default()
}

fn call_substitutions(
    signature: &FunctionSignature,
    args: &[Expr],
    program: &Program,
    state: &OwnershipState,
) -> HashMap<String, Ty> {
    let mut substitutions = HashMap::new();
    for (param, arg) in signature.params.iter().zip(args) {
        collect_substitutions(
            param,
            &expr_ty(arg, program, state),
            &signature.generics,
            &mut substitutions,
        );
    }
    for generic in &signature.generics {
        substitutions
            .entry(generic.clone())
            .or_insert_with(|| Ty::Path(vec![generic.clone()], Span::new_root(0, 0)));
    }
    substitutions
}

fn method_call_substitutions(
    signature: &FunctionSignature,
    args: &[Expr],
    program: &Program,
    state: &OwnershipState,
) -> HashMap<String, Ty> {
    let mut substitutions = HashMap::new();
    for (param, arg) in signature.params.iter().skip(1).zip(args) {
        collect_substitutions(
            param,
            &expr_ty(arg, program, state),
            &signature.generics,
            &mut substitutions,
        );
    }
    for generic in &signature.generics {
        substitutions
            .entry(generic.clone())
            .or_insert_with(|| Ty::Path(vec![generic.clone()], Span::new_root(0, 0)));
    }
    substitutions
}

fn type_substitutions(ty: &Ty, program: &Program) -> HashMap<String, Ty> {
    let mut substitutions = HashMap::new();
    if let Ty::Generic { path, args, .. } = ty
        && let Some(name) = path.first()
        && let Some(params) = program.type_params.get(name)
    {
        for (param, arg) in params.iter().zip(args) {
            substitutions.insert(param.clone(), arg.clone());
        }
    }
    substitutions
}

fn collect_substitutions(
    param: &Ty,
    arg: &Option<Ty>,
    generics: &[String],
    substitutions: &mut HashMap<String, Ty>,
) {
    let Some(arg) = arg else {
        return;
    };
    match (param, arg) {
        (Ty::Path(path, _), arg) if path.len() == 1 && generics.contains(&path[0]) => {
            substitutions
                .entry(path[0].clone())
                .or_insert_with(|| arg.clone());
        }
        (
            Ty::Generic {
                path: param_path,
                args: param_args,
                ..
            },
            Ty::Generic {
                path: arg_path,
                args: arg_args,
                ..
            },
        ) if param_path == arg_path => {
            for (param_arg, arg_arg) in param_args.iter().zip(arg_args) {
                collect_substitutions(param_arg, &Some(arg_arg.clone()), generics, substitutions);
            }
        }
        (Ty::Tuple(param_items, _), Ty::Tuple(arg_items, _)) => {
            for (param_item, arg_item) in param_items.iter().zip(arg_items) {
                collect_substitutions(param_item, &Some(arg_item.clone()), generics, substitutions);
            }
        }
        _ => {}
    }
}

fn substitute_ty(ty: Ty, substitutions: &HashMap<String, Ty>) -> Ty {
    match ty {
        Ty::Path(path, span) if path.len() == 1 => substitutions
            .get(&path[0])
            .cloned()
            .unwrap_or(Ty::Path(path, span)),
        Ty::Generic { path, args, span } => Ty::Generic {
            path,
            args: args
                .into_iter()
                .map(|arg| substitute_ty(arg, substitutions))
                .collect(),
            span,
        },
        Ty::Tuple(items, span) => Ty::Tuple(
            items
                .into_iter()
                .map(|item| substitute_ty(item, substitutions))
                .collect(),
            span,
        ),
        Ty::Slice(item, span) => Ty::Slice(Box::new(substitute_ty(*item, substitutions)), span),
        Ty::Borrow {
            mutable,
            lifetime,
            ty,
            span,
        } => Ty::Borrow {
            mutable,
            lifetime,
            ty: Box::new(substitute_ty(*ty, substitutions)),
            span,
        },
        Ty::Fn {
            params,
            return_ty,
            span,
        } => Ty::Fn {
            params: params
                .into_iter()
                .map(|param| substitute_ty(param, substitutions))
                .collect(),
            return_ty: return_ty.map(|ty| Box::new(substitute_ty(*ty, substitutions))),
            span,
        },
        other => other,
    }
}

fn same_ty_shape(left: &Ty, right: &Ty) -> bool {
    match (left, right) {
        (Ty::Path(left, _), Ty::Path(right, _)) => left == right,
        (
            Ty::Generic {
                path: left_path,
                args: left_args,
                ..
            },
            Ty::Generic {
                path: right_path,
                args: right_args,
                ..
            },
        ) => {
            left_path == right_path
                && left_args.len() == right_args.len()
                && left_args
                    .iter()
                    .zip(right_args)
                    .all(|(left, right)| same_ty_shape(left, right))
        }
        (Ty::Tuple(left, _), Ty::Tuple(right, _)) => {
            left.len() == right.len()
                && left
                    .iter()
                    .zip(right)
                    .all(|(left, right)| same_ty_shape(left, right))
        }
        (Ty::Slice(left, _), Ty::Slice(right, _)) => same_ty_shape(left, right),
        (
            Ty::Borrow {
                mutable: left_mutable,
                ty: left,
                ..
            },
            Ty::Borrow {
                mutable: right_mutable,
                ty: right,
                ..
            },
        ) => left_mutable == right_mutable && same_ty_shape(left, right),
        (
            Ty::Dyn {
                trait_path: left, ..
            },
            Ty::Dyn {
                trait_path: right, ..
            },
        ) => left == right,
        (
            Ty::Fn {
                params: left_params,
                return_ty: left_return,
                ..
            },
            Ty::Fn {
                params: right_params,
                return_ty: right_return,
                ..
            },
        ) => {
            left_params.len() == right_params.len()
                && left_params
                    .iter()
                    .zip(right_params)
                    .all(|(left, right)| same_ty_shape(left, right))
                && match (left_return, right_return) {
                    (Some(left), Some(right)) => same_ty_shape(left, right),
                    (None, None) => true,
                    _ => false,
                }
        }
        (Ty::Infer(_), Ty::Infer(_)) | (Ty::Never(_), Ty::Never(_)) => true,
        _ => false,
    }
}

fn method_signature<'a>(
    receiver: &Expr,
    method: &str,
    program: &'a Program,
    state: &OwnershipState,
) -> Option<&'a FunctionSignature> {
    expr_ty(receiver, program, state)
        .as_ref()
        .and_then(type_name)
        .and_then(|name| program.methods.get(&(name, method.to_string())))
}

fn type_name(ty: &Ty) -> Option<String> {
    match ty {
        Ty::Path(path, _) | Ty::Generic { path, .. } => path.first().cloned(),
        _ => None,
    }
}

fn ty_is_copy(ty: &Ty) -> bool {
    match ty {
        Ty::Path(path, _) => path.first().is_some_and(|name| {
            matches!(name.as_str(), "int" | "float" | "bool" | "char" | "byte")
        }),
        Ty::Borrow { .. } => true,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn primitive_copy_classification_matches_phase_four_rules() {
        for name in ["int", "float", "bool", "char", "byte"] {
            assert!(ty_is_copy(&path_ty(name)), "{name} should be Copy");
        }
        assert!(!ty_is_copy(&path_ty("string")));
    }

    #[test]
    fn borrow_types_are_copy_for_move_analysis() {
        let borrowed = Ty::Borrow {
            mutable: false,
            lifetime: None,
            ty: Box::new(path_ty("Box")),
            span: Span::new_root(0, 0),
        };

        assert!(ty_is_copy(&borrowed));
    }

    fn path_ty(name: &str) -> Ty {
        Ty::Path(vec![name.to_string()], Span::new_root(0, 0))
    }
}
