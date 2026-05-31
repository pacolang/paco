//! Ownership and move analysis for executable frontend features.

use std::collections::HashMap;

use paco_diag::{Diagnostic, Reporter};
use paco_span::Span;
use paco_syntax::ast::{
    Block, Expr, FnDecl, Item, LetStmt, Literal, MatchArm, Module, Pat, Stmt, Ty, VariantFields,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BorrowError;

const TEMPORARY_BORROW_OWNER: &str = "<temporary>";

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
    borrows: Vec<BorrowBinding>,
}

#[derive(Clone, Debug)]
struct BorrowBinding {
    owner: String,
    mutable: bool,
    last_use: usize,
    local_escape: bool,
    field_path: Option<Vec<String>>,
}

#[derive(Clone, Debug)]
struct OwnershipState {
    scopes: Vec<HashMap<String, BindingState>>,
    reachable: bool,
    current_statement: usize,
    last_uses: Vec<HashMap<String, usize>>,
    temporary_borrows: Vec<BorrowBinding>,
}

impl Default for OwnershipState {
    fn default() -> Self {
        Self {
            scopes: Vec::new(),
            reachable: true,
            current_statement: 0,
            last_uses: Vec::new(),
            temporary_borrows: Vec::new(),
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
        self.define_with_borrows(name, copy, ty, Vec::new());
    }

    fn define_with_borrows(
        &mut self,
        name: String,
        copy: bool,
        ty: Option<Ty>,
        borrows: Vec<BorrowBinding>,
    ) {
        if let Some(scope) = self.scopes.last_mut() {
            scope.insert(
                name,
                BindingState {
                    copy,
                    ty,
                    moved_at: None,
                    move_count: 0,
                    borrows,
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
    check_lifetime_signature(function, program, reporter);
    check_block(&function.body, program, &mut state, reporter);
}

fn check_lifetime_signature(function: &FnDecl, program: &Program, reporter: &mut Reporter) {
    let Some(Ty::Borrow { lifetime, span, .. }) = &function.return_ty else {
        return;
    };
    let local_owner_types = owned_param_types(function);
    let local_owners: std::collections::HashSet<String> =
        local_owner_types.keys().cloned().collect();
    if let Some(name) =
        returned_local_borrow_name(&function.body, program, &local_owners, &local_owner_types)
    {
        reporter.push(Diagnostic::error(
            "lifetime-error",
            *span,
            borrow_outlives_owner_message(&name),
        ));
        return;
    }
    if lifetime.is_some() {
        return;
    }
    let borrowed_params = function
        .params
        .iter()
        .filter(|param| matches!(param.ty, Ty::Borrow { .. }))
        .count();
    if borrowed_params > 1 {
        reporter.push(Diagnostic::error(
            "ambiguous-lifetime",
            *span,
            format!(
                "ambiguous returned reference lifetime in `{}`; add an explicit `'a` lifetime annotation",
                function.name
            ),
        ));
    }
}

fn owned_param_types(function: &FnDecl) -> HashMap<String, Ty> {
    function
        .params
        .iter()
        .filter(|param| !matches!(param.ty, Ty::Borrow { .. }))
        .filter_map(|param| match &param.pattern {
            Pat::Ident(name, _) => Some((name.clone(), param.ty.clone())),
            _ => None,
        })
        .collect()
}

fn returned_local_borrow_name(
    block: &Block,
    program: &Program,
    outer_local_names: &std::collections::HashSet<String>,
    outer_local_types: &HashMap<String, Ty>,
) -> Option<String> {
    let mut local_names = outer_local_names.clone();
    let mut local_types = outer_local_types.clone();
    for statement in &block.stmts {
        let Stmt::Let(statement) = statement else {
            continue;
        };
        let Pat::Ident(name, _) = &statement.pattern else {
            continue;
        };
        local_names.insert(name.clone());
        if let Some(ty) = local_binding_ty(statement, &local_types, program) {
            local_types.insert(name.clone(), ty);
        }
    }
    let borrow_origins = local_borrow_origins(block, &local_names, &local_types, program);
    block.tail.as_deref().and_then(|tail| {
        local_borrow_name_from_expr(tail, &local_names, &local_types, &borrow_origins, program)
    })
}

fn local_binding_ty(
    statement: &LetStmt,
    local_types: &HashMap<String, Ty>,
    program: &Program,
) -> Option<Ty> {
    statement
        .ty
        .clone()
        .or_else(|| escape_expr_ty(statement.value.as_ref()?, local_types, program))
}

fn escape_expr_ty(expr: &Expr, local_types: &HashMap<String, Ty>, program: &Program) -> Option<Ty> {
    match expr {
        Expr::Ident(name, _) => local_types.get(name).cloned(),
        Expr::StructLiteral { ty, .. } => Some(ty.clone()),
        Expr::Borrow {
            mutable,
            expr,
            span,
        } => Some(Ty::Borrow {
            mutable: *mutable,
            lifetime: None,
            ty: Box::new(escape_expr_ty(expr, local_types, program)?),
            span: *span,
        }),
        Expr::Call { callee, .. } => {
            let Expr::Ident(function_name, _) = callee.as_ref() else {
                return None;
            };
            program
                .functions
                .get(function_name)
                .and_then(|signature| signature.return_ty.clone())
        }
        Expr::AssociatedCall { ty, function, .. } => type_name(ty)
            .and_then(|name| program.methods.get(&(name, function.clone())))
            .and_then(|signature| signature.return_ty.clone())
            .or_else(|| Some(ty.clone())),
        Expr::MethodCall {
            receiver, method, ..
        } => {
            let receiver_ty = receiver_type_name_from_expr(receiver, local_types, program)?;
            program
                .methods
                .get(&(receiver_ty, method.clone()))
                .and_then(|signature| signature.return_ty.clone())
        }
        Expr::Block(block) => block
            .tail
            .as_deref()
            .and_then(|tail| escape_expr_ty(tail, local_types, program)),
        _ => None,
    }
}

fn receiver_type_name_from_expr(
    receiver: &Expr,
    local_types: &HashMap<String, Ty>,
    program: &Program,
) -> Option<String> {
    if let Expr::Ident(name, _) = receiver {
        return local_types.get(name).and_then(method_receiver_type_name);
    }
    let receiver_ty = escape_expr_ty(receiver, local_types, program)?;
    method_receiver_type_name(&receiver_ty)
}

fn local_borrow_origins(
    block: &Block,
    local_names: &std::collections::HashSet<String>,
    local_types: &HashMap<String, Ty>,
    program: &Program,
) -> HashMap<String, String> {
    let mut origins = HashMap::new();
    for statement in &block.stmts {
        let Stmt::Let(statement) = statement else {
            continue;
        };
        let Pat::Ident(binding_name, _) = &statement.pattern else {
            continue;
        };
        let Some(value) = &statement.value else {
            continue;
        };
        if let Some(owner) =
            local_borrow_name_from_expr(value, local_names, local_types, &origins, program)
        {
            origins.insert(binding_name.clone(), owner);
        }
    }
    origins
}

fn local_borrow_name_from_expr(
    expr: &Expr,
    local_names: &std::collections::HashSet<String>,
    local_types: &HashMap<String, Ty>,
    borrow_origins: &HashMap<String, String>,
    program: &Program,
) -> Option<String> {
    match expr {
        Expr::Borrow { expr, .. } => {
            local_borrow_name_from_place(expr, local_names, borrow_origins).or_else(|| {
                escape_expr_ty(expr, local_types, program)
                    .map(|_| TEMPORARY_BORROW_OWNER.to_string())
            })
        }
        Expr::Ident(name, _) => borrow_origins.get(name).cloned(),
        Expr::Call { callee, args, .. } => {
            let Expr::Ident(function_name, _) = callee.as_ref() else {
                return None;
            };
            let signature = program.functions.get(function_name)?;
            let Some(Ty::Borrow {
                lifetime: return_lifetime,
                ..
            }) = &signature.return_ty
            else {
                return None;
            };
            local_borrow_name_from_call_args(
                &signature.params,
                args,
                return_lifetime.as_ref(),
                local_names,
                local_types,
                borrow_origins,
                program,
            )
        }
        Expr::MethodCall {
            receiver,
            method,
            args,
            ..
        } => local_borrow_name_from_method_call(
            receiver,
            method,
            args,
            local_names,
            local_types,
            borrow_origins,
            program,
        ),
        Expr::Block(block) => returned_local_borrow_name(block, program, local_names, local_types),
        Expr::If {
            then_branch,
            else_branch,
            ..
        } => returned_local_borrow_name(then_branch, program, local_names, local_types).or_else(
            || {
                else_branch.as_deref().and_then(|expr| {
                    local_borrow_name_from_expr(
                        expr,
                        local_names,
                        local_types,
                        borrow_origins,
                        program,
                    )
                })
            },
        ),
        Expr::Match { arms, .. } => arms.iter().find_map(|arm| {
            local_borrow_name_from_expr(
                &arm.body,
                local_names,
                local_types,
                borrow_origins,
                program,
            )
        }),
        _ => None,
    }
}

fn local_borrow_name_from_call_args(
    params: &[Ty],
    args: &[Expr],
    return_lifetime: Option<&String>,
    local_names: &std::collections::HashSet<String>,
    local_types: &HashMap<String, Ty>,
    borrow_origins: &HashMap<String, String>,
    program: &Program,
) -> Option<String> {
    let mut origins = Vec::new();
    for (param, arg) in params.iter().zip(args) {
        let Ty::Borrow {
            lifetime: param_lifetime,
            ..
        } = param
        else {
            continue;
        };
        if let Some(return_lifetime) = return_lifetime
            && param_lifetime.as_ref() != Some(return_lifetime)
        {
            continue;
        }
        if let Some(origin) =
            local_borrow_name_from_expr(arg, local_names, local_types, borrow_origins, program)
        {
            origins.push(origin);
        }
    }
    if return_lifetime.is_some() || origins.len() == 1 {
        origins.into_iter().next()
    } else {
        None
    }
}

fn local_borrow_name_from_method_call(
    receiver: &Expr,
    method: &str,
    args: &[Expr],
    local_names: &std::collections::HashSet<String>,
    local_types: &HashMap<String, Ty>,
    borrow_origins: &HashMap<String, String>,
    program: &Program,
) -> Option<String> {
    let signature = local_method_signature(receiver, method, local_types, borrow_origins, program)?;
    let Some(Ty::Borrow {
        lifetime: return_lifetime,
        ..
    }) = &signature.return_ty
    else {
        return None;
    };
    let mut origins = Vec::new();
    for (index, param_ty) in signature.params.iter().enumerate() {
        let Ty::Borrow {
            lifetime: param_lifetime,
            ..
        } = param_ty
        else {
            continue;
        };
        if let Some(return_lifetime) = return_lifetime
            && param_lifetime.as_ref() != Some(return_lifetime)
        {
            continue;
        }
        let origin = if index == 0 {
            local_borrow_name_from_receiver(
                receiver,
                local_names,
                local_types,
                borrow_origins,
                program,
            )
        } else {
            args.get(index - 1).and_then(|arg| {
                local_borrow_name_from_expr(arg, local_names, local_types, borrow_origins, program)
            })
        };
        if let Some(origin) = origin {
            origins.push(origin);
        }
    }
    if return_lifetime.is_some() || origins.len() == 1 {
        origins.into_iter().next()
    } else {
        None
    }
}

fn local_borrow_name_from_receiver(
    receiver: &Expr,
    local_names: &std::collections::HashSet<String>,
    local_types: &HashMap<String, Ty>,
    borrow_origins: &HashMap<String, String>,
    program: &Program,
) -> Option<String> {
    local_borrow_name_from_place(receiver, local_names, borrow_origins).or_else(|| {
        receiver_type_name_for_escape(receiver, local_types, borrow_origins, program)
            .map(|_| TEMPORARY_BORROW_OWNER.to_string())
    })
}

fn local_method_signature<'a>(
    receiver: &Expr,
    method: &str,
    local_types: &HashMap<String, Ty>,
    borrow_origins: &HashMap<String, String>,
    program: &'a Program,
) -> Option<&'a FunctionSignature> {
    let receiver_ty =
        receiver_type_name_for_escape(receiver, local_types, borrow_origins, program)?;
    program.methods.get(&(receiver_ty, method.to_string()))
}

fn receiver_type_name_for_escape(
    receiver: &Expr,
    local_types: &HashMap<String, Ty>,
    borrow_origins: &HashMap<String, String>,
    program: &Program,
) -> Option<String> {
    if let Expr::Ident(name, _) = receiver
        && let Some(receiver_ty) = local_types
            .get(name)
            .or_else(|| {
                borrow_origins
                    .get(name)
                    .and_then(|origin| local_types.get(origin))
            })
            .and_then(method_receiver_type_name)
    {
        return Some(receiver_ty);
    }
    receiver_type_name_from_expr(receiver, local_types, program)
}

fn method_receiver_type_name(ty: &Ty) -> Option<String> {
    match ty {
        Ty::Borrow { ty, .. } => method_receiver_type_name(ty),
        _ => type_name(ty),
    }
}

fn local_borrow_name_from_place(
    expr: &Expr,
    local_names: &std::collections::HashSet<String>,
    borrow_origins: &HashMap<String, String>,
) -> Option<String> {
    match expr {
        Expr::Ident(name, _) if local_names.contains(name) => Some(name.clone()),
        Expr::Ident(name, _) => borrow_origins.get(name).cloned(),
        Expr::Field { base, .. } => local_borrow_name_from_place(base, local_names, borrow_origins),
        _ => None,
    }
}

fn check_block_borrow_escape(expr: &Expr, program: &Program, reporter: &mut Reporter) {
    let Expr::Block(block) = expr else {
        return;
    };
    if let Some(name) = returned_local_borrow_name(
        block,
        program,
        &std::collections::HashSet::new(),
        &HashMap::new(),
    ) {
        reporter.push(Diagnostic::error(
            "lifetime-error",
            block.span,
            borrow_outlives_owner_message(&name),
        ));
    }
}

fn borrow_outlives_owner_message(name: &str) -> String {
    if name == TEMPORARY_BORROW_OWNER {
        "borrow of temporary value cannot outlive its owner".to_string()
    } else {
        format!("borrow of local `{name}` cannot outlive its owner")
    }
}

fn check_return_borrow_escape(
    value: &Expr,
    program: &Program,
    state: &OwnershipState,
    reporter: &mut Reporter,
) {
    for origin in borrow_origins_from_expr(value, program, state) {
        if origin.local_escape
            || origin.owner == TEMPORARY_BORROW_OWNER
            || state.get(&origin.owner).is_some()
        {
            reporter.push(Diagnostic::error(
                "lifetime-error",
                expr_span(value),
                borrow_outlives_owner_message(&origin.owner),
            ));
            return;
        }
    }
}

fn check_block(
    block: &Block,
    program: &Program,
    state: &mut OwnershipState,
    reporter: &mut Reporter,
) {
    state.push_scope();
    let last_uses = block_last_uses(block);
    state.last_uses.push(last_uses.clone());
    for statement in &block.stmts {
        state.current_statement = statement_position(statement);
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
                    check_block_borrow_escape(value, program, reporter);
                }
                let borrows = borrow_bindings(
                    statement.value.as_ref(),
                    &statement.pattern,
                    &last_uses,
                    program,
                    state,
                );
                define_let_pattern(
                    &statement.pattern,
                    copy,
                    binding_ty,
                    borrows,
                    program,
                    state,
                    reporter,
                );
            }
            Stmt::Expr(expr) => check_discarded_expr(expr, program, state, reporter),
            Stmt::Item(_) => {}
        }
    }
    if state.reachable
        && let Some(tail) = &block.tail
    {
        state.current_statement = expr_span(tail).start();
        check_expr(tail, program, state, reporter);
    }
    state.last_uses.pop();
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
            let temporaries =
                check_argument_temporary_borrow_conflicts(args, program, state, reporter);
            for arg in args {
                consume_expr_with_temporary_borrows(arg, &temporaries, program, state, reporter);
            }
        }
        Expr::Binary { left, right, .. } => {
            check_expr(left, program, state, reporter);
            check_expr(right, program, state, reporter);
        }
        Expr::Unary { expr, .. } => check_expr(expr, program, state, reporter),
        Expr::Assign {
            target,
            value,
            span,
        } => {
            let assigned_borrows = target_identifier(target).map(|name| {
                borrow_bindings_for_name(value, current_last_use(name, state), program, state)
            });
            consume_expr(value, program, state, reporter);
            match target.as_ref() {
                Expr::Ident(name, span) => {
                    check_assignment_while_borrowed(name, *span, state, reporter);
                    if let Some(binding) = state.get_mut(name) {
                        binding.moved_at = None;
                        binding.borrows = assigned_borrows.unwrap_or_default();
                    }
                }
                Expr::Field { .. } => {
                    if let Some(name) = root_place_name(target) {
                        check_assignment_while_borrowed(name, *span, state, reporter);
                        if let Some(field_path) = place_field_path(target) {
                            let last_use = current_last_use(name, state);
                            let field_borrows =
                                borrow_bindings_for_name(value, last_use, program, state)
                                    .into_iter()
                                    .map(|borrow| prefix_borrow_field_path(borrow, &field_path))
                                    .collect::<Vec<_>>();
                            if let Some(binding) = state.get_mut(name) {
                                binding.borrows.retain(|borrow| {
                                    !borrow_field_path_starts_with(borrow, &field_path)
                                });
                                binding.borrows.extend(field_borrows);
                            }
                        }
                    }
                    check_expr(target, program, state, reporter);
                }
                _ => check_expr(target, program, state, reporter),
            }
        }
        Expr::Field { base, .. } => check_expr(base, program, state, reporter),
        Expr::Index { base, index, .. } => {
            check_expr(base, program, state, reporter);
            check_expr(index, program, state, reporter);
        }
        Expr::Return(value, _) => {
            if let Some(value) = value {
                check_return_borrow_escape(value, program, state, reporter);
                consume_expr(value, program, state, reporter);
            }
            state.reachable = false;
        }
        Expr::Break(value, _) => {
            if let Some(value) = value {
                consume_expr(value, program, state, reporter);
            }
            state.reachable = false;
        }
        Expr::Continue(_) => {
            state.reachable = false;
        }
        Expr::Spawn { expr, .. } | Expr::Comptime { expr, .. } => {
            check_expr(expr, program, state, reporter);
        }
        Expr::Borrow {
            mutable,
            expr,
            span,
        } => {
            for borrow in temporary_borrow_bindings(*mutable, expr, state.current_statement, state)
            {
                check_borrow_conflict("<temporary>", *span, &borrow, state, reporter);
            }
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
        let temporaries = check_argument_temporary_borrow_conflicts(args, program, state, reporter);
        for arg in args {
            consume_expr_with_temporary_borrows(arg, &temporaries, program, state, reporter);
        }
        return;
    };
    let temporaries = check_argument_temporary_borrow_conflicts(args, program, state, reporter);
    if name == "print" {
        for arg in args {
            check_expr_with_temporary_borrows(arg, &temporaries, program, state, reporter);
        }
        return;
    }
    let signature = program.functions.get(name);
    for (index, arg) in args.iter().enumerate() {
        if signature
            .and_then(|signature| signature.params.get(index))
            .is_some_and(ty_is_copy)
        {
            check_expr_with_temporary_borrows(arg, &temporaries, program, state, reporter);
        } else {
            consume_expr_with_temporary_borrows(arg, &temporaries, program, state, reporter);
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
    let receiver_borrows = signature
        .and_then(|signature| signature.params.first())
        .and_then(|ty| match ty {
            Ty::Borrow { mutable, .. } => Some(*mutable),
            _ => None,
        })
        .map(|mutable| {
            borrow_owners(receiver, state)
                .into_iter()
                .map(|owner| BorrowBinding {
                    owner,
                    mutable,
                    last_use: state.current_statement,
                    local_escape: false,
                    field_path: None,
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    for receiver_borrow in &receiver_borrows {
        check_borrow_conflict(
            "<receiver>",
            expr_span(receiver),
            receiver_borrow,
            state,
            reporter,
        );
    }
    let temporaries = check_argument_temporary_borrow_conflicts_with(
        args,
        receiver_borrows,
        program,
        state,
        reporter,
    );
    if receiver_is_copy || receiver_is_borrowed {
        check_expr_with_temporary_borrows(receiver, &temporaries, program, state, reporter);
    } else {
        consume_expr_with_temporary_borrows(receiver, &temporaries, program, state, reporter);
    }
    for (index, arg) in args.iter().enumerate() {
        if signature
            .and_then(|signature| signature.params.get(index + 1))
            .is_some_and(ty_is_copy)
        {
            check_expr_with_temporary_borrows(arg, &temporaries, program, state, reporter);
        } else {
            consume_expr_with_temporary_borrows(arg, &temporaries, program, state, reporter);
        }
    }
}

fn check_argument_temporary_borrow_conflicts(
    args: &[Expr],
    program: &Program,
    state: &OwnershipState,
    reporter: &mut Reporter,
) -> Vec<BorrowBinding> {
    check_argument_temporary_borrow_conflicts_with(args, Vec::new(), program, state, reporter)
}

fn check_argument_temporary_borrow_conflicts_with(
    args: &[Expr],
    initial: Vec<BorrowBinding>,
    program: &Program,
    state: &OwnershipState,
    reporter: &mut Reporter,
) -> Vec<BorrowBinding> {
    let mut temporaries = initial;
    for arg in args {
        for borrow in temporary_borrows_for_expr(arg, program, state) {
            if temporaries.iter().any(|existing| {
                existing.owner == borrow.owner && (existing.mutable || borrow.mutable)
            }) {
                reporter.push(Diagnostic::error(
                    "borrow-conflict",
                    expr_span(arg),
                    format!(
                        "cannot borrow `{}` because another call argument already borrows it",
                        borrow.owner
                    ),
                ));
                return temporaries;
            }
            temporaries.push(borrow);
        }
    }
    temporaries
}

fn check_expr_with_temporary_borrows(
    expr: &Expr,
    borrows: &[BorrowBinding],
    program: &Program,
    state: &mut OwnershipState,
    reporter: &mut Reporter,
) {
    let current = temporary_borrows_for_expr(expr, program, state);
    let original_len = push_temporary_borrows_except(state, borrows, &current);
    check_expr(expr, program, state, reporter);
    state.temporary_borrows.truncate(original_len);
}

fn consume_expr_with_temporary_borrows(
    expr: &Expr,
    borrows: &[BorrowBinding],
    program: &Program,
    state: &mut OwnershipState,
    reporter: &mut Reporter,
) {
    let original_len = push_temporary_borrows(state, borrows);
    consume_expr(expr, program, state, reporter);
    state.temporary_borrows.truncate(original_len);
}

fn push_temporary_borrows(state: &mut OwnershipState, borrows: &[BorrowBinding]) -> usize {
    push_temporary_borrows_except(state, borrows, &[])
}

fn push_temporary_borrows_except(
    state: &mut OwnershipState,
    borrows: &[BorrowBinding],
    excluded: &[BorrowBinding],
) -> usize {
    let original_len = state.temporary_borrows.len();
    state.temporary_borrows.extend(
        borrows
            .iter()
            .filter(|borrow| {
                !excluded
                    .iter()
                    .any(|excluded| same_borrow(borrow, excluded))
            })
            .cloned()
            .map(|mut borrow| {
                borrow.last_use = usize::MAX;
                borrow
            }),
    );
    original_len
}

fn temporary_borrows_for_expr(
    expr: &Expr,
    program: &Program,
    state: &OwnershipState,
) -> Vec<BorrowBinding> {
    borrow_origins_from_expr(expr, program, state)
        .into_iter()
        .map(|origin| BorrowBinding {
            owner: origin.owner,
            mutable: origin.mutable,
            last_use: state.current_statement,
            local_escape: origin.local_escape,
            field_path: origin.field_path,
        })
        .collect()
}

fn same_borrow(left: &BorrowBinding, right: &BorrowBinding) -> bool {
    left.owner == right.owner && left.mutable == right.mutable
}

fn expr_span(expr: &Expr) -> Span {
    match expr {
        Expr::Literal(_, span)
        | Expr::Ident(_, span)
        | Expr::Return(_, span)
        | Expr::Break(_, span)
        | Expr::Continue(span)
        | Expr::Yield(_, span) => *span,
        Expr::Block(block) => block.span,
        Expr::If { span, .. }
        | Expr::Loop { span, .. }
        | Expr::While { span, .. }
        | Expr::Match { span, .. }
        | Expr::Call { span, .. }
        | Expr::MethodCall { span, .. }
        | Expr::AssociatedCall { span, .. }
        | Expr::Binary { span, .. }
        | Expr::Unary { span, .. }
        | Expr::Assign { span, .. }
        | Expr::Field { span, .. }
        | Expr::Index { span, .. }
        | Expr::Spawn { span, .. }
        | Expr::Select { span, .. }
        | Expr::Comptime { span, .. }
        | Expr::StructLiteral { span, .. }
        | Expr::Borrow { span, .. } => *span,
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
            check_move_while_borrowed(name, *span, state, reporter);
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
                "field moves are not supported by ownership analysis",
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
    let last_uses = block_last_uses(block);
    state.last_uses.push(last_uses.clone());
    for statement in &block.stmts {
        state.current_statement = statement_position(statement);
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
                    check_block_borrow_escape(value, program, reporter);
                }
                let borrows = borrow_bindings(
                    statement.value.as_ref(),
                    &statement.pattern,
                    &last_uses,
                    program,
                    state,
                );
                define_let_pattern(
                    &statement.pattern,
                    copy,
                    binding_ty,
                    borrows,
                    program,
                    state,
                    reporter,
                );
            }
            Stmt::Expr(expr) => check_discarded_expr(expr, program, state, reporter),
            Stmt::Item(_) => {}
        }
    }
    if state.reachable
        && let Some(tail) = &block.tail
    {
        state.current_statement = expr_span(tail).start();
        consume_expr(tail, program, state, reporter);
    }
    state.last_uses.pop();
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

fn define_let_pattern(
    pattern: &Pat,
    copy: bool,
    ty: Option<Ty>,
    borrows: Vec<BorrowBinding>,
    program: &Program,
    state: &mut OwnershipState,
    reporter: &mut Reporter,
) {
    if let Pat::Ident(name, span) = pattern {
        for borrow in &borrows {
            if borrow.owner == TEMPORARY_BORROW_OWNER || borrow.local_escape {
                reporter.push(Diagnostic::error(
                    "lifetime-error",
                    *span,
                    borrow_outlives_owner_message(&borrow.owner),
                ));
            } else {
                check_borrow_conflict(name, *span, borrow, state, reporter);
            }
        }
        state.define_with_borrows(name.clone(), copy, ty, borrows);
    } else {
        bind_pattern(pattern, copy, ty, program, state);
    }
}

fn borrow_bindings(
    value: Option<&Expr>,
    pattern: &Pat,
    last_uses: &HashMap<String, usize>,
    program: &Program,
    state: &OwnershipState,
) -> Vec<BorrowBinding> {
    let Pat::Ident(binding_name, _) = pattern else {
        return Vec::new();
    };
    let Some(value) = value else {
        return Vec::new();
    };
    let last_use = last_uses.get(binding_name).copied().unwrap_or(0);
    borrow_bindings_for_name(value, last_use, program, state)
}

fn borrow_bindings_for_name(
    value: &Expr,
    last_use: usize,
    program: &Program,
    state: &OwnershipState,
) -> Vec<BorrowBinding> {
    borrow_origins_from_expr(value, program, state)
        .into_iter()
        .map(|origin| BorrowBinding {
            owner: origin.owner,
            mutable: origin.mutable,
            last_use,
            local_escape: origin.local_escape,
            field_path: origin.field_path,
        })
        .collect()
}

#[derive(Clone, Debug)]
struct BorrowOrigin {
    owner: String,
    mutable: bool,
    local_escape: bool,
    field_path: Option<Vec<String>>,
}

fn borrow_origins_from_expr(
    expr: &Expr,
    program: &Program,
    state: &OwnershipState,
) -> Vec<BorrowOrigin> {
    match expr {
        Expr::Borrow { mutable, expr, .. } => borrow_owners_or_temporary(expr, program, state)
            .into_iter()
            .map(|owner| BorrowOrigin {
                owner,
                mutable: *mutable,
                local_escape: false,
                field_path: None,
            })
            .collect(),
        Expr::Ident(name, _) => state
            .get(name)
            .map(|binding| {
                binding
                    .borrows
                    .iter()
                    .map(|borrow| BorrowOrigin {
                        owner: borrow.owner.clone(),
                        mutable: borrow.mutable,
                        local_escape: borrow.local_escape,
                        field_path: borrow.field_path.clone(),
                    })
                    .collect()
            })
            .unwrap_or_default(),
        Expr::Call { callee, args, .. } => {
            let Expr::Ident(function_name, _) = callee.as_ref() else {
                return Vec::new();
            };
            let Some(signature) = program.functions.get(function_name) else {
                return Vec::new();
            };
            let Some(Ty::Borrow {
                mutable,
                lifetime: return_lifetime,
                ..
            }) = &signature.return_ty
            else {
                return Vec::new();
            };
            borrow_origins_from_params(
                &signature.params,
                args,
                None,
                *mutable,
                return_lifetime.as_ref(),
                program,
                state,
            )
        }
        Expr::MethodCall {
            receiver,
            method,
            args,
            ..
        } => {
            let Some(signature) = method_signature(receiver, method, program, state) else {
                return Vec::new();
            };
            let Some(Ty::Borrow {
                mutable,
                lifetime: return_lifetime,
                ..
            }) = &signature.return_ty
            else {
                return Vec::new();
            };
            borrow_origins_from_params(
                &signature.params,
                args,
                Some(receiver),
                *mutable,
                return_lifetime.as_ref(),
                program,
                state,
            )
        }
        Expr::Block(block) => borrow_origins_from_block(block, program, state),
        Expr::Field { .. } => {
            if matches!(expr_ty(expr, program, state), Some(Ty::Borrow { .. })) {
                field_borrow_origins(expr, state)
            } else {
                Vec::new()
            }
        }
        Expr::StructLiteral { fields, .. } => fields
            .iter()
            .flat_map(|(field, value)| {
                borrow_origins_from_expr(value, program, state)
                    .into_iter()
                    .map(|origin| prefix_origin_field_path(origin, std::slice::from_ref(field)))
            })
            .collect(),
        Expr::If {
            then_branch,
            else_branch,
            ..
        } => {
            let mut origins = borrow_origins_from_block(then_branch, program, state);
            if let Some(else_branch) = else_branch.as_deref() {
                origins.extend(borrow_origins_from_expr(else_branch, program, state));
            }
            origins
        }
        Expr::Match { arms, .. } => arms
            .iter()
            .flat_map(|arm| borrow_origins_from_expr(&arm.body, program, state))
            .collect(),
        _ => Vec::new(),
    }
}

fn borrow_origins_from_block(
    block: &Block,
    program: &Program,
    state: &OwnershipState,
) -> Vec<BorrowOrigin> {
    let mut local_state = state.clone();
    local_state.push_scope();
    let last_uses = block_last_uses(block);
    for statement in &block.stmts {
        local_state.current_statement = statement_position(statement);
        let Stmt::Let(statement) = statement else {
            continue;
        };
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
        let borrows = borrow_bindings(
            statement.value.as_ref(),
            &statement.pattern,
            &last_uses,
            program,
            &local_state,
        );
        define_pattern_for_origin(
            &statement.pattern,
            copy,
            binding_ty,
            borrows,
            program,
            &mut local_state,
        );
    }
    let mut origins = block
        .tail
        .as_deref()
        .map(|tail| borrow_origins_from_expr(tail, program, &local_state))
        .unwrap_or_default();
    for origin in &mut origins {
        if origin.owner != TEMPORARY_BORROW_OWNER
            && state.get(&origin.owner).is_none()
            && local_state.get(&origin.owner).is_some()
        {
            origin.local_escape = true;
        }
    }
    origins
}

fn define_pattern_for_origin(
    pattern: &Pat,
    copy: bool,
    ty: Option<Ty>,
    borrows: Vec<BorrowBinding>,
    program: &Program,
    state: &mut OwnershipState,
) {
    if let Pat::Ident(name, _) = pattern {
        state.define_with_borrows(name.clone(), copy, ty, borrows);
    } else {
        bind_pattern(pattern, copy, ty, program, state);
    }
}

fn borrow_origins_from_params(
    params: &[Ty],
    args: &[Expr],
    receiver: Option<&Expr>,
    returned_mutable: bool,
    return_lifetime: Option<&String>,
    program: &Program,
    state: &OwnershipState,
) -> Vec<BorrowOrigin> {
    let mut origins = Vec::new();
    let mut contributing_params = 0;
    for (index, param_ty) in params.iter().enumerate() {
        let Ty::Borrow {
            lifetime: param_lifetime,
            ..
        } = param_ty
        else {
            continue;
        };
        if let Some(return_lifetime) = return_lifetime
            && param_lifetime.as_ref() != Some(return_lifetime)
        {
            continue;
        }
        let param_origins = if index == 0 && receiver.is_some() {
            receiver
                .map(|receiver| {
                    borrow_owners_or_temporary(receiver, program, state)
                        .into_iter()
                        .map(|owner| BorrowOrigin {
                            owner,
                            mutable: returned_mutable,
                            local_escape: false,
                            field_path: None,
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        } else {
            let arg_index = if receiver.is_some() { index - 1 } else { index };
            args.get(arg_index)
                .map(|arg| {
                    borrow_origins_from_expr(arg, program, state)
                        .into_iter()
                        .map(|origin| BorrowOrigin {
                            owner: origin.owner,
                            mutable: returned_mutable,
                            local_escape: origin.local_escape,
                            field_path: None,
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        };
        if !param_origins.is_empty() {
            contributing_params += 1;
            origins.extend(param_origins);
        }
    }
    if return_lifetime.is_some() || contributing_params == 1 {
        origins
    } else {
        Vec::new()
    }
}

fn temporary_borrow_bindings(
    mutable: bool,
    expr: &Expr,
    current_statement: usize,
    state: &OwnershipState,
) -> Vec<BorrowBinding> {
    borrow_owners(expr, state)
        .into_iter()
        .map(|owner| BorrowBinding {
            owner,
            mutable,
            last_use: current_statement,
            local_escape: false,
            field_path: None,
        })
        .collect()
}

fn field_borrow_origins(expr: &Expr, state: &OwnershipState) -> Vec<BorrowOrigin> {
    let Expr::Field { .. } = expr else {
        return Vec::new();
    };
    let Some(root) = root_place_name(expr) else {
        return Vec::new();
    };
    let Some(field_path) = place_field_path(expr) else {
        return Vec::new();
    };
    state
        .get(root)
        .map(|binding| {
            binding
                .borrows
                .iter()
                .filter(|borrow| borrow.field_path.as_ref() == Some(&field_path))
                .map(|borrow| BorrowOrigin {
                    owner: borrow.owner.clone(),
                    mutable: borrow.mutable,
                    local_escape: borrow.local_escape,
                    field_path: None,
                })
                .collect()
        })
        .unwrap_or_default()
}

fn borrow_owners_or_temporary(
    expr: &Expr,
    program: &Program,
    state: &OwnershipState,
) -> Vec<String> {
    let owners = borrow_owners(expr, state);
    if !owners.is_empty() {
        owners
    } else if expr_ty(expr, program, state).is_some() {
        vec![TEMPORARY_BORROW_OWNER.to_string()]
    } else {
        Vec::new()
    }
}

fn current_last_use(name: &str, state: &OwnershipState) -> usize {
    state
        .last_uses
        .iter()
        .rev()
        .find_map(|uses| uses.get(name).copied())
        .unwrap_or(0)
}

fn target_identifier(expr: &Expr) -> Option<&str> {
    match expr {
        Expr::Ident(name, _) => Some(name),
        _ => None,
    }
}

fn prefix_origin_field_path(mut origin: BorrowOrigin, prefix: &[String]) -> BorrowOrigin {
    let mut field_path = prefix.to_vec();
    if let Some(existing) = origin.field_path.take() {
        field_path.extend(existing);
    }
    origin.field_path = Some(field_path);
    origin
}

fn prefix_borrow_field_path(mut borrow: BorrowBinding, prefix: &[String]) -> BorrowBinding {
    let mut field_path = prefix.to_vec();
    if let Some(existing) = borrow.field_path.take() {
        field_path.extend(existing);
    }
    borrow.field_path = Some(field_path);
    borrow
}

fn borrow_field_path_starts_with(borrow: &BorrowBinding, prefix: &[String]) -> bool {
    borrow
        .field_path
        .as_ref()
        .is_some_and(|field_path| field_path.starts_with(prefix))
}

fn place_field_path(expr: &Expr) -> Option<Vec<String>> {
    match expr {
        Expr::Ident(_, _) => Some(Vec::new()),
        Expr::Field { base, field, .. } => {
            let mut path = place_field_path(base)?;
            path.push(field.clone());
            Some(path)
        }
        _ => None,
    }
}

fn borrow_owners(expr: &Expr, state: &OwnershipState) -> Vec<String> {
    let Some(root) = root_place_name(expr) else {
        return Vec::new();
    };
    let Some(binding) = state.get(root) else {
        return vec![root.to_string()];
    };
    if binding.borrows.is_empty() {
        vec![root.to_string()]
    } else {
        binding
            .borrows
            .iter()
            .map(|borrow| borrow.owner.clone())
            .collect()
    }
}

fn check_borrow_conflict(
    name: &str,
    span: Span,
    new_borrow: &BorrowBinding,
    state: &OwnershipState,
    reporter: &mut Reporter,
) {
    for existing in active_borrows_of(&new_borrow.owner, state) {
        if new_borrow.mutable || existing.mutable {
            reporter.push(Diagnostic::error(
                "borrow-conflict",
                span,
                format!(
                    "cannot borrow `{}` as {} because it is already borrowed by `{}`",
                    new_borrow.owner,
                    if new_borrow.mutable {
                        "mutable"
                    } else {
                        "shared"
                    },
                    name
                ),
            ));
            return;
        }
    }
}

fn check_move_while_borrowed(
    name: &str,
    span: Span,
    state: &OwnershipState,
    reporter: &mut Reporter,
) {
    if active_borrows_of(name, state).next().is_some() {
        reporter.push(Diagnostic::error(
            "borrow-conflict",
            span,
            format!("cannot move `{name}` while it is borrowed"),
        ));
    }
}

fn check_assignment_while_borrowed(
    name: &str,
    span: Span,
    state: &OwnershipState,
    reporter: &mut Reporter,
) {
    if active_borrows_of(name, state).next().is_some() {
        reporter.push(Diagnostic::error(
            "borrow-conflict",
            span,
            format!("cannot assign to `{name}` while it is borrowed"),
        ));
    }
}

fn root_place_name(expr: &Expr) -> Option<&str> {
    match expr {
        Expr::Ident(name, _) => Some(name),
        Expr::Field { base, .. } => root_place_name(base),
        _ => None,
    }
}

fn active_borrows_of<'a>(
    owner: &'a str,
    state: &'a OwnershipState,
) -> impl Iterator<Item = &'a BorrowBinding> {
    state
        .scopes
        .iter()
        .flat_map(|scope| scope.values())
        .flat_map(|binding| binding.borrows.iter())
        .chain(state.temporary_borrows.iter())
        .filter(move |borrow| borrow.owner == owner && borrow.last_use >= state.current_statement)
}

fn block_last_uses(block: &Block) -> HashMap<String, usize> {
    let mut uses = HashMap::new();
    for statement in &block.stmts {
        collect_statement_uses(statement, &mut uses);
    }
    if let Some(tail) = &block.tail {
        collect_expr_uses(tail, &mut uses);
    }
    uses
}

fn statement_position(statement: &Stmt) -> usize {
    match statement {
        Stmt::Let(statement) => statement.span.start(),
        Stmt::Expr(expr) => expr_span(expr).start(),
        Stmt::Item(_) => 0,
    }
}

fn collect_statement_uses(statement: &Stmt, uses: &mut HashMap<String, usize>) {
    match statement {
        Stmt::Let(statement) => {
            if let Some(value) = &statement.value {
                collect_expr_uses(value, uses);
            }
        }
        Stmt::Expr(expr) => collect_expr_uses(expr, uses),
        Stmt::Item(_) => {}
    }
}

fn collect_expr_uses(expr: &Expr, uses: &mut HashMap<String, usize>) {
    match expr {
        Expr::Ident(name, span) => {
            uses.insert(name.clone(), span.start());
        }
        Expr::Block(block) => {
            for statement in &block.stmts {
                collect_statement_uses(statement, uses);
            }
            if let Some(tail) = &block.tail {
                collect_expr_uses(tail, uses);
            }
        }
        Expr::Loop { body, .. } => {
            for statement in &body.stmts {
                collect_statement_uses(statement, uses);
            }
            if let Some(tail) = &body.tail {
                collect_expr_uses(tail, uses);
            }
        }
        Expr::If {
            condition,
            then_branch,
            else_branch,
            ..
        } => {
            collect_expr_uses(condition, uses);
            for statement in &then_branch.stmts {
                collect_statement_uses(statement, uses);
            }
            if let Some(tail) = &then_branch.tail {
                collect_expr_uses(tail, uses);
            }
            if let Some(else_branch) = else_branch {
                collect_expr_uses(else_branch, uses);
            }
        }
        Expr::While {
            condition, body, ..
        } => {
            collect_expr_uses(condition, uses);
            for statement in &body.stmts {
                collect_statement_uses(statement, uses);
            }
            if let Some(tail) = &body.tail {
                collect_expr_uses(tail, uses);
            }
        }
        Expr::Match {
            scrutinee, arms, ..
        } => {
            collect_expr_uses(scrutinee, uses);
            for arm in arms {
                if let Some(guard) = &arm.guard {
                    collect_expr_uses(guard, uses);
                }
                collect_expr_uses(&arm.body, uses);
            }
        }
        Expr::Call { callee, args, .. } => {
            collect_expr_uses(callee, uses);
            for arg in args {
                collect_expr_uses(arg, uses);
            }
        }
        Expr::MethodCall { receiver, args, .. } => {
            collect_expr_uses(receiver, uses);
            for arg in args {
                collect_expr_uses(arg, uses);
            }
        }
        Expr::AssociatedCall { args, .. } => {
            for arg in args {
                collect_expr_uses(arg, uses);
            }
        }
        Expr::StructLiteral { fields, .. } => {
            for (_, value) in fields {
                collect_expr_uses(value, uses);
            }
        }
        Expr::Binary { left, right, .. } => {
            collect_expr_uses(left, uses);
            collect_expr_uses(right, uses);
        }
        Expr::Unary { expr, .. }
        | Expr::Return(Some(expr), _)
        | Expr::Break(Some(expr), _)
        | Expr::Spawn { expr, .. }
        | Expr::Comptime { expr, .. }
        | Expr::Yield(expr, _)
        | Expr::Borrow { expr, .. } => collect_expr_uses(expr, uses),
        Expr::Assign { target, value, .. } => {
            collect_expr_uses(target, uses);
            collect_expr_uses(value, uses);
        }
        Expr::Field { base, .. } => collect_expr_uses(base, uses),
        Expr::Index {
            base,
            index: subscript,
            ..
        } => {
            collect_expr_uses(base, uses);
            collect_expr_uses(subscript, uses);
        }
        Expr::Select { arms, default, .. } => {
            for arm in arms {
                collect_expr_uses(&arm.operation, uses);
                for statement in &arm.body.stmts {
                    collect_statement_uses(statement, uses);
                }
            }
            if let Some(default) = default {
                for statement in &default.stmts {
                    collect_statement_uses(statement, uses);
                }
            }
        }
        Expr::Literal(_, _) | Expr::Return(None, _) | Expr::Break(None, _) | Expr::Continue(_) => {}
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
    fn primitive_copy_classification_matches_ownership_rules() {
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
