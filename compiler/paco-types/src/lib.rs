//! Type checking for executable frontend features.

use std::collections::{HashMap, HashSet, hash_map::Entry};

use paco_diag::{Diagnostic, Reporter};
use paco_match::{ConstructorSet, analyze_match};
use paco_span::Span;
use paco_syntax::ast::{
    BinaryOp, Block, EnumDecl, Expr, FnDecl, Item, LetStmt, Literal, MatchArm, MethodsBlock,
    Module, Param, Pat, Stmt, StructDecl, Ty, UnaryOp, VariantFields,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Type {
    Int,
    Float,
    Bool,
    String,
    Unit,
    Never,
    Struct(String, Vec<Type>),
    Enum(String, Vec<Type>),
    Borrow { mutable: bool, ty: Box<Type> },
    Generic(String),
    Unknown,
    Error,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TypeError;

pub fn check_module(module: &Module, reporter: &mut Reporter) -> Result<(), TypeError> {
    let program = Program::from_module(module, reporter);

    for item in &module.items {
        match item {
            Item::Fn(function) => {
                let Some(signature) = program.functions.get(&function.name) else {
                    continue;
                };
                check_function(function, signature, &program, reporter);
            }
            Item::Struct(decl) => {
                for method in &decl.methods {
                    check_attached_function(method, &decl.name, &program, reporter);
                }
            }
            Item::Enum(decl) => {
                for method in &decl.methods {
                    check_attached_function(method, &decl.name, &program, reporter);
                }
            }
            Item::Methods(block) => check_methods_block(block, &program, reporter),
            Item::Trait(_) | Item::Use(_) => {}
        }
    }

    if reporter.has_errors() {
        Err(TypeError)
    } else {
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Binding {
    ty: Type,
    mutable: bool,
}

#[derive(Clone, Debug)]
struct StructInfo {
    generics: Vec<String>,
    fields: Vec<(String, Ty, Span)>,
}

#[derive(Clone, Debug)]
struct EnumInfo {
    generics: Vec<String>,
    variants: Vec<VariantInfo>,
}

#[derive(Clone, Debug)]
struct VariantInfo {
    name: String,
    fields: VariantFields,
    span: Span,
}

#[derive(Clone, Debug)]
struct FunctionSig {
    generics: Vec<String>,
    params: Vec<Type>,
    body_params: Vec<Type>,
    return_ty: Type,
    receiver: Option<Receiver>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Receiver {
    mutable: bool,
}

#[derive(Default)]
struct Program {
    structs: HashMap<String, StructInfo>,
    enums: HashMap<String, EnumInfo>,
    functions: HashMap<String, FunctionSig>,
    methods: HashMap<(String, String), FunctionSig>,
    associated: HashMap<(String, String), FunctionSig>,
}

impl Program {
    fn from_module(module: &Module, reporter: &mut Reporter) -> Self {
        let mut program = Self::default();
        program.collect_types(module, reporter);
        program.validate_declared_types(module, reporter);
        program.collect_functions(module, reporter);
        program.check_recursive_value_layout(reporter);
        program
    }

    fn collect_types(&mut self, module: &Module, reporter: &mut Reporter) {
        for item in &module.items {
            match item {
                Item::Struct(decl) => {
                    if self.structs.contains_key(&decl.name) || self.enums.contains_key(&decl.name)
                    {
                        reporter.push(Diagnostic::error(
                            "PACO-E0310",
                            decl.span,
                            format!("duplicate type `{}`", decl.name),
                        ));
                        continue;
                    }
                    let mut seen = HashSet::new();
                    let mut fields = Vec::new();
                    for field in &decl.fields {
                        if !seen.insert(field.name.clone()) {
                            reporter.push(Diagnostic::error(
                                "PACO-E0311",
                                field.span,
                                format!("duplicate field `{}`", field.name),
                            ));
                        }
                        fields.push((field.name.clone(), field.ty.clone(), field.span));
                    }
                    self.structs.insert(
                        decl.name.clone(),
                        StructInfo {
                            generics: decl.generics.clone(),
                            fields,
                        },
                    );
                }
                Item::Enum(decl) => {
                    if self.structs.contains_key(&decl.name) || self.enums.contains_key(&decl.name)
                    {
                        reporter.push(Diagnostic::error(
                            "PACO-E0310",
                            decl.span,
                            format!("duplicate type `{}`", decl.name),
                        ));
                        continue;
                    }
                    let mut seen = HashSet::new();
                    let mut variants = Vec::new();
                    for variant in &decl.variants {
                        if !seen.insert(variant.name.clone()) {
                            reporter.push(Diagnostic::error(
                                "PACO-E0313",
                                variant.span,
                                format!("duplicate enum variant `{}`", variant.name),
                            ));
                        }
                        variants.push(VariantInfo {
                            name: variant.name.clone(),
                            fields: variant.fields.clone(),
                            span: variant.span,
                        });
                    }
                    self.enums.insert(
                        decl.name.clone(),
                        EnumInfo {
                            generics: decl.generics.clone(),
                            variants,
                        },
                    );
                }
                _ => {}
            }
        }
    }

    fn collect_functions(&mut self, module: &Module, reporter: &mut Reporter) {
        for item in &module.items {
            match item {
                Item::Fn(function) => {
                    let signature = self.function_sig(function, None, reporter);
                    self.functions.insert(function.name.clone(), signature);
                }
                Item::Struct(decl) => self.collect_attached_functions(decl, reporter),
                Item::Enum(decl) => self.collect_enum_functions(decl, reporter),
                Item::Methods(block) => self.collect_extension_methods(block, reporter),
                Item::Trait(_) | Item::Use(_) => {}
            }
        }
    }

    fn validate_declared_types(&self, module: &Module, reporter: &mut Reporter) {
        for item in &module.items {
            match item {
                Item::Struct(decl) => {
                    let env = generic_substitutions(&decl.generics);
                    for field in &decl.fields {
                        self.ty_from_ast(&field.ty, &env, reporter);
                    }
                }
                Item::Enum(decl) => {
                    let env = generic_substitutions(&decl.generics);
                    for variant in &decl.variants {
                        match &variant.fields {
                            VariantFields::Unit => {}
                            VariantFields::Tuple(types) => {
                                for ty in types {
                                    self.ty_from_ast(ty, &env, reporter);
                                }
                            }
                            VariantFields::Struct(fields) => {
                                for field in fields {
                                    self.ty_from_ast(&field.ty, &env, reporter);
                                }
                            }
                        }
                    }
                }
                Item::Fn(_) | Item::Methods(_) | Item::Trait(_) | Item::Use(_) => {}
            }
        }
    }

    fn collect_attached_functions(&mut self, decl: &StructDecl, reporter: &mut Reporter) {
        let self_ty = Type::Struct(
            decl.name.clone(),
            decl.generics
                .iter()
                .map(|name| Type::Generic(name.clone()))
                .collect(),
        );
        for method in &decl.methods {
            let signature = self.function_sig(method, Some(self_ty.clone()), reporter);
            self.insert_attached_signature(&decl.name, method, signature, reporter);
        }
    }

    fn collect_enum_functions(&mut self, decl: &EnumDecl, reporter: &mut Reporter) {
        let self_ty = Type::Enum(
            decl.name.clone(),
            decl.generics
                .iter()
                .map(|name| Type::Generic(name.clone()))
                .collect(),
        );
        for method in &decl.methods {
            let signature = self.function_sig(method, Some(self_ty.clone()), reporter);
            self.insert_attached_signature(&decl.name, method, signature, reporter);
        }
    }

    fn collect_extension_methods(&mut self, block: &MethodsBlock, reporter: &mut Reporter) {
        let target_ty = self.ty_from_ast(&block.target, &HashMap::new(), reporter);
        let Some(target_name) = target_type_name(&target_ty) else {
            reporter.push(Diagnostic::error(
                "PACO-E0310",
                ty_span(&block.target),
                "methods block target must be a known nominal type",
            ));
            return;
        };
        for method in &block.methods {
            let signature = self.function_sig(method, Some(target_ty.clone()), reporter);
            self.insert_attached_signature(&target_name, method, signature, reporter);
        }
    }

    fn insert_attached_signature(
        &mut self,
        type_name: &str,
        function: &FnDecl,
        signature: FunctionSig,
        reporter: &mut Reporter,
    ) {
        let key = (type_name.to_string(), function.name.clone());
        let target = if signature.receiver.is_some() {
            &mut self.methods
        } else {
            &mut self.associated
        };
        match target.entry(key) {
            Entry::Occupied(_) => {
                reporter.push(Diagnostic::error(
                    "PACO-E0314",
                    function.span,
                    format!("duplicate method `{}` for `{type_name}`", function.name),
                ));
            }
            Entry::Vacant(entry) => {
                entry.insert(signature);
            }
        }
    }

    fn function_sig(
        &self,
        function: &FnDecl,
        self_ty: Option<Type>,
        reporter: &mut Reporter,
    ) -> FunctionSig {
        let mut env = HashMap::new();
        for generic in &function.generics {
            env.insert(generic.clone(), Type::Generic(generic.clone()));
        }
        if let Some(self_ty) = &self_ty {
            env.insert("Self".to_string(), self_ty.clone());
        }

        let receiver = function.params.first().and_then(receiver_from_param);
        let body_params = function
            .params
            .iter()
            .enumerate()
            .map(|(index, param)| {
                if index == 0 && receiver.is_some() {
                    self_ty.clone().unwrap_or(Type::Error)
                } else {
                    self.ty_from_ast(&param.ty, &env, reporter)
                }
            })
            .collect::<Vec<_>>();
        let params = if receiver.is_some() {
            body_params.iter().skip(1).cloned().collect()
        } else {
            body_params.clone()
        };
        let return_ty = function
            .return_ty
            .as_ref()
            .map_or(Type::Unit, |ty| self.ty_from_ast(ty, &env, reporter));

        FunctionSig {
            params,
            body_params,
            return_ty,
            receiver,
            generics: function.generics.clone(),
        }
    }

    fn ty_from_ast(
        &self,
        ty: &Ty,
        generics: &HashMap<String, Type>,
        reporter: &mut Reporter,
    ) -> Type {
        match ty {
            Ty::Path(path, _) if path.as_slice() == ["int"] => Type::Int,
            Ty::Path(path, _) if path.as_slice() == ["float"] => Type::Float,
            Ty::Path(path, _) if path.as_slice() == ["bool"] => Type::Bool,
            Ty::Path(path, _) if path.as_slice() == ["string"] => Type::String,
            Ty::Borrow { mutable, ty, .. } => Type::Borrow {
                mutable: *mutable,
                ty: Box::new(self.ty_from_ast(ty, generics, reporter)),
            },
            Ty::Path(path, span) if path.len() == 1 => {
                if let Some(ty) = generics.get(&path[0]) {
                    ty.clone()
                } else if let Some(info) = self.structs.get(&path[0]) {
                    self.check_generic_arity(&path[0], info.generics.len(), 0, *span, reporter);
                    Type::Struct(path[0].clone(), Vec::new())
                } else if let Some(info) = self.enums.get(&path[0]) {
                    self.check_generic_arity(&path[0], info.generics.len(), 0, *span, reporter);
                    Type::Enum(path[0].clone(), Vec::new())
                } else {
                    reporter.push(Diagnostic::error(
                        "PACO-E0306",
                        ty_span(ty),
                        format!("type is not supported yet: {}", path.join("::")),
                    ));
                    Type::Error
                }
            }
            Ty::Generic { path, args, .. } if path.len() == 1 => {
                let args = args
                    .iter()
                    .map(|arg| self.ty_from_ast(arg, generics, reporter))
                    .collect::<Vec<_>>();
                if let Some(info) = self.structs.get(&path[0]) {
                    self.check_generic_arity(
                        &path[0],
                        info.generics.len(),
                        args.len(),
                        ty_span(ty),
                        reporter,
                    );
                    Type::Struct(path[0].clone(), args)
                } else if let Some(info) = self.enums.get(&path[0]) {
                    self.check_generic_arity(
                        &path[0],
                        info.generics.len(),
                        args.len(),
                        ty_span(ty),
                        reporter,
                    );
                    Type::Enum(path[0].clone(), args)
                } else {
                    reporter.push(Diagnostic::error(
                        "PACO-E0306",
                        ty_span(ty),
                        format!("type is not supported yet: {}", path.join("::")),
                    ));
                    Type::Error
                }
            }
            _ => {
                reporter.push(Diagnostic::error(
                    "PACO-E0306",
                    ty_span(ty),
                    "type is not supported yet",
                ));
                Type::Error
            }
        }
    }

    fn check_generic_arity(
        &self,
        name: &str,
        expected: usize,
        actual: usize,
        span: Span,
        reporter: &mut Reporter,
    ) {
        if expected != actual {
            reporter.push(Diagnostic::error(
                "PACO-E0316",
                span,
                format!("generic arity mismatch for `{name}`: expected {expected}, found {actual}"),
            ));
        }
    }

    fn check_recursive_value_layout(&self, reporter: &mut Reporter) {
        for name in self.structs.keys() {
            let mut visiting = HashSet::new();
            let current = Type::Struct(name.clone(), Vec::new());
            if let Some(span) = self.find_recursive_value_field(name, &current, &mut visiting) {
                reporter.push(Diagnostic::error(
                    "PACO-E0315",
                    span,
                    format!("recursive by-value field in `{name}`"),
                ));
            }
        }
    }

    fn find_recursive_value_field(
        &self,
        root: &str,
        current: &Type,
        visiting: &mut HashSet<String>,
    ) -> Option<Span> {
        let Type::Struct(current_name, _) = current else {
            return None;
        };
        let key = current.name();
        if !visiting.insert(key.clone()) {
            return None;
        }
        let info = self.structs.get(current_name)?;
        let generics = self.layout_env(current);
        for (_, field_ty, span) in &info.fields {
            let field_ty = self.ty_from_ast_for_layout(field_ty, &generics);
            let Type::Struct(field_name, _) = &field_ty else {
                continue;
            };
            if field_name == root {
                visiting.remove(&key);
                return Some(*span);
            }
            if self.structs.contains_key(field_name)
                && let Some(span) = self.find_recursive_value_field(root, &field_ty, visiting)
            {
                visiting.remove(&key);
                return Some(span);
            }
        }
        visiting.remove(&key);
        None
    }

    fn layout_env(&self, ty: &Type) -> HashMap<String, Type> {
        let Type::Struct(name, args) = ty else {
            return HashMap::new();
        };
        let Some(info) = self.structs.get(name) else {
            return HashMap::new();
        };
        info.generics
            .iter()
            .cloned()
            .zip(args.iter().cloned())
            .collect()
    }

    fn ty_from_ast_for_layout(&self, ty: &Ty, generics: &HashMap<String, Type>) -> Type {
        match ty {
            Ty::Path(path, _) if path.len() == 1 => {
                if let Some(ty) = generics.get(&path[0]) {
                    ty.clone()
                } else if self.structs.contains_key(&path[0]) {
                    Type::Struct(path[0].clone(), Vec::new())
                } else if self.enums.contains_key(&path[0]) {
                    Type::Enum(path[0].clone(), Vec::new())
                } else {
                    Type::Unknown
                }
            }
            Ty::Generic { path, args, .. } if path.len() == 1 => {
                let args = args
                    .iter()
                    .map(|arg| self.ty_from_ast_for_layout(arg, generics))
                    .collect::<Vec<_>>();
                if self.structs.contains_key(&path[0]) {
                    Type::Struct(path[0].clone(), args)
                } else if self.enums.contains_key(&path[0]) {
                    Type::Enum(path[0].clone(), args)
                } else {
                    Type::Unknown
                }
            }
            _ => Type::Unknown,
        }
    }
}

struct FunctionContext<'a> {
    scopes: Vec<HashMap<String, Binding>>,
    expected_return: &'a Type,
    loop_depth: usize,
}

fn check_attached_function(
    function: &FnDecl,
    type_name: &str,
    program: &Program,
    reporter: &mut Reporter,
) {
    let key = (type_name.to_string(), function.name.clone());
    let signature = program
        .methods
        .get(&key)
        .or_else(|| program.associated.get(&key));
    if let Some(signature) = signature {
        check_function(function, signature, program, reporter);
    }
}

fn check_methods_block(block: &MethodsBlock, program: &Program, reporter: &mut Reporter) {
    let target_ty = program.ty_from_ast(&block.target, &HashMap::new(), reporter);
    let Some(type_name) = target_type_name(&target_ty) else {
        return;
    };
    for method in &block.methods {
        check_attached_function(method, &type_name, program, reporter);
    }
}

fn check_function(
    function: &FnDecl,
    signature: &FunctionSig,
    program: &Program,
    reporter: &mut Reporter,
) {
    let mut context = FunctionContext {
        scopes: vec![HashMap::new()],
        expected_return: &signature.return_ty,
        loop_depth: 0,
    };
    for (param, param_ty) in function.params.iter().zip(&signature.body_params) {
        if let Pat::Ident(name, _) = &param.pattern {
            context.scopes.last_mut().unwrap().insert(
                name.clone(),
                Binding {
                    ty: param_ty.clone(),
                    mutable: receiver_from_param(param).is_some_and(|receiver| receiver.mutable),
                },
            );
        }
    }

    let actual = infer_block(&function.body, program, &mut context, reporter);
    if !compatible(&actual, &signature.return_ty) && actual != Type::Never {
        reporter.push(Diagnostic::error(
            "PACO-E0302",
            function.span,
            format!(
                "return type mismatch: expected {}, found {}",
                signature.return_ty.name(),
                actual.name()
            ),
        ));
    }
}

fn infer_block(
    block: &Block,
    program: &Program,
    context: &mut FunctionContext<'_>,
    reporter: &mut Reporter,
) -> Type {
    context.scopes.push(HashMap::new());
    for statement in &block.stmts {
        match statement {
            Stmt::Let(statement) => check_let(statement, program, context, reporter),
            Stmt::Expr(expr) => {
                if infer_expr(expr, program, context, reporter) == Type::Never {
                    context.scopes.pop();
                    return Type::Never;
                }
            }
            Stmt::Item(_) => {}
        }
    }
    let result = block.tail.as_ref().map_or(Type::Unit, |expr| {
        infer_expr(expr, program, context, reporter)
    });
    context.scopes.pop();
    result
}

fn check_let(
    statement: &LetStmt,
    program: &Program,
    context: &mut FunctionContext<'_>,
    reporter: &mut Reporter,
) {
    let value_ty = statement.value.as_ref().map_or(Type::Unknown, |value| {
        infer_expr(value, program, context, reporter)
    });
    let declared_ty = statement
        .ty
        .as_ref()
        .map(|ty| program.ty_from_ast(ty, &HashMap::new(), reporter));
    let binding_ty = declared_ty.clone().unwrap_or_else(|| value_ty.clone());
    if let Some(declared_ty) = declared_ty
        && !compatible(&value_ty, &declared_ty)
    {
        reporter.push(Diagnostic::error(
            "PACO-E0302",
            statement.span,
            format!(
                "type mismatch: expected {}, found {}",
                declared_ty.name(),
                value_ty.name()
            ),
        ));
    }
    if let Pat::Ident(name, _) = &statement.pattern {
        context.scopes.last_mut().unwrap().insert(
            name.clone(),
            Binding {
                ty: binding_ty,
                mutable: statement.mutable,
            },
        );
    }
}

fn infer_expr(
    expr: &Expr,
    program: &Program,
    context: &mut FunctionContext<'_>,
    reporter: &mut Reporter,
) -> Type {
    match expr {
        Expr::Literal(Literal::Int(_), _) => Type::Int,
        Expr::Literal(Literal::Float(_), _) => Type::Float,
        Expr::Literal(Literal::Bool(_), _) => Type::Bool,
        Expr::Literal(Literal::String(_), _) => Type::String,
        Expr::Literal(Literal::Char(_), span) => unsupported_expr("char literals", *span, reporter),
        Expr::Ident(name, _) => lookup(&context.scopes, name)
            .map(|binding| binding.ty)
            .unwrap_or(Type::Unknown),
        Expr::Block(block) => infer_block(block, program, context, reporter),
        Expr::If {
            condition,
            then_branch,
            else_branch,
            span,
        } => infer_if(
            condition,
            then_branch,
            else_branch.as_deref(),
            *span,
            program,
            context,
            reporter,
        ),
        Expr::Loop { body, .. } => {
            context.loop_depth += 1;
            infer_block(body, program, context, reporter);
            context.loop_depth -= 1;
            Type::Unit
        }
        Expr::While {
            condition,
            body,
            span,
            ..
        } => {
            let condition_ty = infer_expr(condition, program, context, reporter);
            if !compatible(&condition_ty, &Type::Bool) {
                reporter.push(Diagnostic::error(
                    "PACO-E0303",
                    *span,
                    format!(
                        "while condition must be bool, found {}",
                        condition_ty.name()
                    ),
                ));
            }
            context.loop_depth += 1;
            infer_block(body, program, context, reporter);
            context.loop_depth -= 1;
            Type::Unit
        }
        Expr::Call { callee, args, span } => {
            infer_call(callee, args, *span, program, context, reporter)
        }
        Expr::MethodCall {
            receiver,
            method,
            args,
            span,
        } => infer_method_call(receiver, method, args, *span, program, context, reporter),
        Expr::AssociatedCall {
            ty,
            function,
            args,
            span,
        } => infer_associated_call(ty, function, args, *span, program, context, reporter),
        Expr::StructLiteral { ty, fields, span } => {
            infer_struct_literal(ty, fields, *span, program, context, reporter)
        }
        Expr::Field { base, field, span } => {
            infer_field(base, field, *span, program, context, reporter)
        }
        Expr::Binary {
            op,
            left,
            right,
            span,
        } => infer_binary(*op, left, right, *span, program, context, reporter),
        Expr::Unary { op, expr, span } => infer_unary(*op, expr, *span, program, context, reporter),
        Expr::Assign {
            target,
            value,
            span,
        } => infer_assign(target, value, *span, program, context, reporter),
        Expr::Return(value, span) => {
            let actual = value.as_ref().map_or(Type::Unit, |value| {
                infer_expr(value, program, context, reporter)
            });
            if !compatible(&actual, context.expected_return) {
                reporter.push(Diagnostic::error(
                    "PACO-E0302",
                    *span,
                    format!(
                        "return type mismatch: expected {}, found {}",
                        context.expected_return.name(),
                        actual.name()
                    ),
                ));
            }
            Type::Never
        }
        Expr::Break(value, span) => {
            if context.loop_depth == 0 {
                reporter.push(Diagnostic::error(
                    "PACO-E0308",
                    *span,
                    "break cannot be used outside of a loop",
                ));
            }
            if let Some(value) = value {
                infer_expr(value, program, context, reporter);
            }
            Type::Never
        }
        Expr::Continue(span) => {
            if context.loop_depth == 0 {
                reporter.push(Diagnostic::error(
                    "PACO-E0308",
                    *span,
                    "continue cannot be used outside of a loop",
                ));
            }
            Type::Never
        }
        Expr::Match {
            scrutinee,
            arms,
            span,
        } => infer_match(scrutinee, arms, *span, program, context, reporter),
        Expr::Index { span, .. } => unsupported_expr("index expressions", *span, reporter),
        Expr::Spawn { span, .. } => unsupported_expr("spawn expressions", *span, reporter),
        Expr::Select { span, .. } => unsupported_expr("select expressions", *span, reporter),
        Expr::Comptime { span, .. } => unsupported_expr("comptime expressions", *span, reporter),
        Expr::Yield(_, span) => unsupported_expr("yield expressions", *span, reporter),
        Expr::Borrow {
            mutable,
            expr,
            span,
        } => {
            if *mutable && !is_mutable_place(expr, context) {
                reporter.push(Diagnostic::error(
                    "PACO-E0307",
                    *span,
                    format!(
                        "cannot take a mutable borrow of immutable `{}`",
                        place_name(expr).unwrap_or("place")
                    ),
                ));
            }
            Type::Borrow {
                mutable: *mutable,
                ty: Box::new(infer_expr(expr, program, context, reporter)),
            }
        }
    }
}

fn infer_if(
    condition: &Expr,
    then_branch: &Block,
    else_branch: Option<&Expr>,
    span: Span,
    program: &Program,
    context: &mut FunctionContext<'_>,
    reporter: &mut Reporter,
) -> Type {
    let condition_ty = infer_expr(condition, program, context, reporter);
    if !compatible(&condition_ty, &Type::Bool) {
        reporter.push(Diagnostic::error(
            "PACO-E0303",
            span,
            format!("if condition must be bool, found {}", condition_ty.name()),
        ));
    }
    let then_ty = infer_block(then_branch, program, context, reporter);
    let else_ty = else_branch.map_or(Type::Unit, |else_branch| {
        infer_expr(else_branch, program, context, reporter)
    });
    join_branch_types(&then_ty, &else_ty).unwrap_or_else(|| {
        reporter.push(Diagnostic::error(
            "PACO-E0304",
            span,
            format!(
                "if branches have incompatible types: {} and {}",
                then_ty.name(),
                else_ty.name()
            ),
        ));
        Type::Error
    })
}

fn infer_match(
    scrutinee: &Expr,
    arms: &[MatchArm],
    span: Span,
    program: &Program,
    context: &mut FunctionContext<'_>,
    reporter: &mut Reporter,
) -> Type {
    let scrutinee_ty = infer_expr(scrutinee, program, context, reporter);
    check_match_coverage(&scrutinee_ty, arms, span, program, reporter);

    let mut result_ty = None;
    for arm in arms {
        context.scopes.push(HashMap::new());
        bind_pattern_types(&arm.pattern, &scrutinee_ty, program, context, reporter);
        if let Some(guard) = &arm.guard {
            let guard_ty = infer_expr(guard, program, context, reporter);
            if !compatible(&guard_ty, &Type::Bool) {
                reporter.push(Diagnostic::error(
                    "PACO-E0303",
                    arm.span,
                    format!("match guard must be bool, found {}", guard_ty.name()),
                ));
            }
        }
        let arm_ty = infer_expr(&arm.body, program, context, reporter);
        context.scopes.pop();
        result_ty = Some(match result_ty {
            None => arm_ty,
            Some(previous) => join_branch_types(&previous, &arm_ty).unwrap_or_else(|| {
                reporter.push(Diagnostic::error(
                    "PACO-E0304",
                    arm.span,
                    format!(
                        "match arms have incompatible types: {} and {}",
                        previous.name(),
                        arm_ty.name()
                    ),
                ));
                Type::Error
            }),
        });
    }

    result_ty.unwrap_or_else(|| {
        reporter.push(Diagnostic::error(
            "PACO-E0401",
            span,
            "non-exhaustive match: no arms were provided",
        ));
        Type::Error
    })
}

fn bind_pattern_types(
    pattern: &Pat,
    expected: &Type,
    program: &Program,
    context: &mut FunctionContext<'_>,
    reporter: &mut Reporter,
) {
    match pattern {
        Pat::Ident(name, _) => {
            context.scopes.last_mut().unwrap().insert(
                name.clone(),
                Binding {
                    ty: expected.clone(),
                    mutable: false,
                },
            );
        }
        Pat::Binding { name, pattern, .. } => {
            context.scopes.last_mut().unwrap().insert(
                name.clone(),
                Binding {
                    ty: expected.clone(),
                    mutable: false,
                },
            );
            bind_pattern_types(pattern, expected, program, context, reporter);
        }
        Pat::Wildcard(_) => {}
        Pat::Literal(literal, span) => {
            let actual = literal_type(literal);
            if !compatible(&actual, expected) {
                reporter.push(Diagnostic::error(
                    "PACO-E0302",
                    *span,
                    format!(
                        "pattern type mismatch: expected {}, found {}",
                        expected.name(),
                        actual.name()
                    ),
                ));
            }
        }
        Pat::Range {
            start, end, span, ..
        } => {
            if !compatible(expected, &Type::Int) {
                reporter.push(Diagnostic::error(
                    "PACO-E0302",
                    *span,
                    format!("range pattern requires int, found {}", expected.name()),
                ));
            }
            if !is_int_literal_pattern(start) || !is_int_literal_pattern(end) {
                reporter.push(Diagnostic::error(
                    "PACO-E0306",
                    *span,
                    "range pattern bounds must be integer literals",
                ));
                return;
            }
            bind_pattern_types(start, &Type::Int, program, context, reporter);
            bind_pattern_types(end, &Type::Int, program, context, reporter);
        }
        Pat::Enum { path, fields, span } => {
            let Type::Enum(enum_name, _) = expected else {
                reporter.push(Diagnostic::error(
                    "PACO-E0302",
                    *span,
                    format!(
                        "enum pattern requires enum value, found {}",
                        expected.name()
                    ),
                ));
                return;
            };
            if path.len() < 2 || path.first() != Some(enum_name) {
                reporter.push(Diagnostic::error(
                    "PACO-E0302",
                    *span,
                    format!("enum pattern does not match `{enum_name}`"),
                ));
                return;
            }
            let variant_name = path.last().unwrap();
            let Some(variant) = program.enums.get(enum_name).and_then(|info| {
                info.variants
                    .iter()
                    .find(|variant| &variant.name == variant_name)
            }) else {
                reporter.push(Diagnostic::error(
                    "PACO-E0314",
                    *span,
                    format!("variant `{variant_name}` not found for `{enum_name}`"),
                ));
                return;
            };
            let expected_fields = variant_field_types(variant, expected, program, reporter);
            if fields.len() != expected_fields.len() {
                reporter.push(Diagnostic::error(
                    "PACO-E0305",
                    *span,
                    format!(
                        "variant `{variant_name}` expects {} fields, found {}",
                        expected_fields.len(),
                        fields.len()
                    ),
                ));
            }
            for (field, field_ty) in fields.iter().zip(expected_fields) {
                bind_pattern_types(field, &field_ty, program, context, reporter);
            }
        }
        Pat::Or(patterns, span) => {
            bind_or_pattern_types(patterns, *span, expected, program, context, reporter);
        }
        Pat::Tuple(_, span) | Pat::Struct { span, .. } => {
            reporter.push(Diagnostic::error(
                "PACO-E0306",
                *span,
                "pattern form is not supported by pattern type checking",
            ));
        }
    }
}

fn bind_or_pattern_types(
    patterns: &[Pat],
    span: Span,
    expected: &Type,
    program: &Program,
    context: &mut FunctionContext<'_>,
    reporter: &mut Reporter,
) {
    let mut alternatives = Vec::new();
    for pattern in patterns {
        context.scopes.push(HashMap::new());
        bind_pattern_types(pattern, expected, program, context, reporter);
        alternatives.push(context.scopes.pop().unwrap_or_default());
    }
    let Some(first) = alternatives.first() else {
        return;
    };
    let same_bindings = alternatives.iter().skip(1).all(|alternative| {
        alternative.len() == first.len()
            && first.iter().all(|(name, binding)| {
                alternative
                    .get(name)
                    .is_some_and(|other| compatible(&binding.ty, &other.ty))
            })
    });
    if !same_bindings {
        reporter.push(Diagnostic::error(
            "PACO-E0306",
            span,
            "or-pattern alternatives must bind the same names with compatible types",
        ));
        return;
    }
    context.scopes.last_mut().unwrap().extend(
        first
            .iter()
            .map(|(name, binding)| (name.clone(), binding.clone())),
    );
}

fn check_match_coverage(
    scrutinee_ty: &Type,
    arms: &[MatchArm],
    span: Span,
    program: &Program,
    reporter: &mut Reporter,
) {
    let Some(constructors) = constructors_for_type(scrutinee_ty, program) else {
        return;
    };
    let report = analyze_match(arms, constructors);
    for unreachable in report.unreachable_arms {
        let message = unreachable.witness.map_or_else(
            || "unreachable match arm".to_string(),
            |witness| format!("unreachable match arm: `{witness}` is already covered"),
        );
        reporter.push(Diagnostic::error(
            "PACO-E0402",
            pattern_span(&arms[unreachable.index].pattern),
            message,
        ));
    }
    if let Some(witness) = report.missing_witness {
        reporter.push(Diagnostic::error(
            "PACO-E0401",
            span,
            format!("non-exhaustive match: missing `{witness}`"),
        ));
    }
}

fn constructors_for_type(scrutinee_ty: &Type, program: &Program) -> Option<ConstructorSet> {
    match scrutinee_ty {
        Type::Bool => Some(ConstructorSet::closed(["true", "false"])),
        Type::Enum(name, _) => program.enums.get(name).map(|info| {
            ConstructorSet::closed(
                info.variants
                    .iter()
                    .map(|variant| format!("{name}::{}", variant.name)),
            )
        }),
        Type::Int | Type::Float | Type::String => Some(ConstructorSet::open("_")),
        Type::Unknown | Type::Error => None,
        _ => None,
    }
}

fn is_int_literal_pattern(pattern: &Pat) -> bool {
    matches!(pattern, Pat::Literal(Literal::Int(_), _))
}

fn variant_field_types(
    variant: &VariantInfo,
    enum_ty: &Type,
    program: &Program,
    reporter: &mut Reporter,
) -> Vec<Type> {
    match &variant.fields {
        VariantFields::Unit => Vec::new(),
        VariantFields::Tuple(types) => types
            .iter()
            .map(|ty| instantiate_ty(ty, enum_ty, program, reporter))
            .collect(),
        VariantFields::Struct(_) => {
            reporter.push(Diagnostic::error(
                "PACO-E0306",
                variant.span,
                "named enum variant patterns are not supported yet",
            ));
            Vec::new()
        }
    }
}

fn infer_call(
    callee: &Expr,
    args: &[Expr],
    span: Span,
    program: &Program,
    context: &mut FunctionContext<'_>,
    reporter: &mut Reporter,
) -> Type {
    let Expr::Ident(name, _) = callee else {
        return unsupported_expr("indirect calls", span, reporter);
    };

    if name == "print" {
        if args.len() != 1 {
            reporter.push(Diagnostic::error(
                "PACO-E0305",
                span,
                format!("print expects 1 argument, found {}", args.len()),
            ));
        }
        for arg in args {
            infer_expr(arg, program, context, reporter);
        }
        return Type::Unit;
    }

    let Some(signature) = program.functions.get(name).cloned() else {
        return Type::Unknown;
    };
    let mut substitutions = generic_substitutions(&signature.generics);
    check_args(
        args,
        &signature.params,
        span,
        program,
        context,
        reporter,
        &mut substitutions,
    );
    substitute_generics(&signature.return_ty, &substitutions)
}

fn infer_method_call(
    receiver: &Expr,
    method: &str,
    args: &[Expr],
    span: Span,
    program: &Program,
    context: &mut FunctionContext<'_>,
    reporter: &mut Reporter,
) -> Type {
    let receiver_ty = infer_expr(receiver, program, context, reporter);
    let Some(type_name) = target_type_name(&receiver_ty) else {
        reporter.push(Diagnostic::error(
            "PACO-E0314",
            span,
            format!("method `{method}` not found"),
        ));
        return Type::Error;
    };
    let Some(signature) = program
        .methods
        .get(&(type_name.clone(), method.to_string()))
        .cloned()
    else {
        reporter.push(Diagnostic::error(
            "PACO-E0314",
            span,
            format!("method `{method}` not found for `{type_name}`"),
        ));
        return Type::Error;
    };
    if signature
        .receiver
        .as_ref()
        .is_some_and(|receiver| receiver.mutable)
        && !is_mutable_place(receiver, context)
    {
        reporter.push(Diagnostic::error(
            "PACO-E0307",
            span,
            "mutable method receiver requires a mutable binding",
        ));
    }
    let mut substitutions = generic_substitutions(&signature.generics);
    substitutions.insert("Self".to_string(), receiver_ty.clone());
    if let Some(expected_receiver) = signature.body_params.first() {
        unify_type(expected_receiver, &receiver_ty, &mut substitutions);
    }
    check_args(
        args,
        &signature.params,
        span,
        program,
        context,
        reporter,
        &mut substitutions,
    );
    substitute_generics(&signature.return_ty, &substitutions)
}

fn infer_associated_call(
    ty: &Ty,
    function: &str,
    args: &[Expr],
    span: Span,
    program: &Program,
    context: &mut FunctionContext<'_>,
    reporter: &mut Reporter,
) -> Type {
    let target_ty = program.ty_from_ast(ty, &HashMap::new(), reporter);
    let Some(type_name) = target_type_name(&target_ty) else {
        return Type::Error;
    };

    if let Some(enum_info) = program.enums.get(&type_name)
        && let Some(variant) = enum_info
            .variants
            .iter()
            .find(|variant| variant.name == function)
    {
        check_variant_args(variant, args, &target_ty, program, context, reporter);
        return target_ty;
    }

    let Some(signature) = program
        .associated
        .get(&(type_name.clone(), function.to_string()))
        .cloned()
    else {
        reporter.push(Diagnostic::error(
            "PACO-E0314",
            span,
            format!("associated function `{function}` not found for `{type_name}`"),
        ));
        return Type::Error;
    };
    let mut substitutions = generic_substitutions(&signature.generics);
    substitutions.insert("Self".to_string(), target_ty.clone());
    check_args(
        args,
        &signature.params,
        span,
        program,
        context,
        reporter,
        &mut substitutions,
    );
    substitute_generics(&signature.return_ty, &substitutions)
}

fn infer_struct_literal(
    ty: &Ty,
    fields: &[(String, Expr)],
    span: Span,
    program: &Program,
    context: &mut FunctionContext<'_>,
    reporter: &mut Reporter,
) -> Type {
    let struct_ty = program.ty_from_ast(ty, &HashMap::new(), reporter);
    let Type::Struct(name, _) = &struct_ty else {
        reporter.push(Diagnostic::error(
            "PACO-E0311",
            span,
            "struct literal target must be a struct type",
        ));
        return Type::Error;
    };
    let Some(expected_fields) = instantiated_struct_fields(&struct_ty, program, reporter) else {
        return Type::Error;
    };
    let mut provided = HashSet::new();
    for (field_name, value) in fields {
        if !provided.insert(field_name.clone()) {
            reporter.push(Diagnostic::error(
                "PACO-E0311",
                span,
                format!("duplicate field `{field_name}`"),
            ));
        }
        let Some(expected_ty) = expected_fields.get(field_name) else {
            reporter.push(Diagnostic::error(
                "PACO-E0311",
                span,
                format!("unknown field `{field_name}` for `{name}`"),
            ));
            infer_expr(value, program, context, reporter);
            continue;
        };
        let actual_ty = infer_expr(value, program, context, reporter);
        if !compatible(&actual_ty, expected_ty) {
            reporter.push(Diagnostic::error(
                "PACO-E0302",
                span,
                format!(
                    "type mismatch: expected {}, found {}",
                    expected_ty.name(),
                    actual_ty.name()
                ),
            ));
        }
    }
    for field_name in expected_fields.keys() {
        if !provided.contains(field_name) {
            reporter.push(Diagnostic::error(
                "PACO-E0311",
                span,
                format!("missing field `{field_name}` for `{name}`"),
            ));
        }
    }
    struct_ty
}

fn infer_field(
    base: &Expr,
    field: &str,
    span: Span,
    program: &Program,
    context: &mut FunctionContext<'_>,
    reporter: &mut Reporter,
) -> Type {
    let base_ty = match infer_expr(base, program, context, reporter) {
        Type::Borrow { ty, .. } => *ty,
        ty => ty,
    };
    let Some(fields) = instantiated_struct_fields(&base_ty, program, reporter) else {
        reporter.push(Diagnostic::error(
            "PACO-E0311",
            span,
            format!("unknown field `{field}`"),
        ));
        return Type::Error;
    };
    fields.get(field).cloned().unwrap_or_else(|| {
        reporter.push(Diagnostic::error(
            "PACO-E0311",
            span,
            format!("unknown field `{field}`"),
        ));
        Type::Error
    })
}

fn infer_assign(
    target: &Expr,
    value: &Expr,
    span: Span,
    program: &Program,
    context: &mut FunctionContext<'_>,
    reporter: &mut Reporter,
) -> Type {
    let value_ty = infer_expr(value, program, context, reporter);
    let target_ty = match target {
        Expr::Ident(name, _) => {
            let binding = lookup(&context.scopes, name);
            if let Some(binding) = &binding
                && !binding.mutable
            {
                reporter.push(Diagnostic::error(
                    "PACO-E0307",
                    span,
                    format!("cannot assign to immutable binding `{name}`"),
                ));
            }
            binding.map(|binding| binding.ty).unwrap_or(Type::Unknown)
        }
        Expr::Field { base, field, .. } => {
            if !is_assignable_field_base(base, context) {
                reporter.push(Diagnostic::error(
                    "PACO-E0307",
                    span,
                    "cannot assign through immutable binding or shared borrow",
                ));
            }
            infer_field(base, field, span, program, context, reporter)
        }
        _ => unsupported_expr(
            "assignment targets other than identifiers or fields",
            span,
            reporter,
        ),
    };
    if !compatible(&value_ty, &target_ty) {
        reporter.push(Diagnostic::error(
            "PACO-E0302",
            span,
            format!(
                "type mismatch: expected {}, found {}",
                target_ty.name(),
                value_ty.name()
            ),
        ));
    }
    Type::Unit
}

fn infer_binary(
    op: BinaryOp,
    left: &Expr,
    right: &Expr,
    span: Span,
    program: &Program,
    context: &mut FunctionContext<'_>,
    reporter: &mut Reporter,
) -> Type {
    let left_ty = infer_expr(left, program, context, reporter);
    let right_ty = infer_expr(right, program, context, reporter);
    match op {
        BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul | BinaryOp::Div | BinaryOp::Rem => {
            if compatible(&left_ty, &Type::Int) && compatible(&right_ty, &Type::Int) {
                Type::Int
            } else if compatible(&left_ty, &Type::Float) && compatible(&right_ty, &Type::Float) {
                Type::Float
            } else {
                reporter.push(Diagnostic::error(
                    "PACO-E0301",
                    span,
                    format!(
                        "type mismatch: expected compatible numeric types, found {} and {}",
                        left_ty.name(),
                        right_ty.name()
                    ),
                ));
                Type::Error
            }
        }
        BinaryOp::Eq | BinaryOp::Ne => {
            if compatible(&left_ty, &right_ty) {
                Type::Bool
            } else {
                reporter.push(Diagnostic::error(
                    "PACO-E0301",
                    span,
                    format!(
                        "type mismatch: cannot compare {} and {}",
                        left_ty.name(),
                        right_ty.name()
                    ),
                ));
                Type::Error
            }
        }
        BinaryOp::Lt | BinaryOp::Le | BinaryOp::Gt | BinaryOp::Ge => {
            if (compatible(&left_ty, &Type::Int) && compatible(&right_ty, &Type::Int))
                || (compatible(&left_ty, &Type::Float) && compatible(&right_ty, &Type::Float))
            {
                Type::Bool
            } else {
                reporter.push(Diagnostic::error(
                    "PACO-E0301",
                    span,
                    format!(
                        "type mismatch: expected compatible numeric types, found {} and {}",
                        left_ty.name(),
                        right_ty.name()
                    ),
                ));
                Type::Error
            }
        }
        BinaryOp::And | BinaryOp::Or => {
            if compatible(&left_ty, &Type::Bool) && compatible(&right_ty, &Type::Bool) {
                Type::Bool
            } else {
                reporter.push(Diagnostic::error(
                    "PACO-E0301",
                    span,
                    format!(
                        "type mismatch: expected bool and bool, found {} and {}",
                        left_ty.name(),
                        right_ty.name()
                    ),
                ));
                Type::Error
            }
        }
    }
}

fn infer_unary(
    op: UnaryOp,
    expr: &Expr,
    span: Span,
    program: &Program,
    context: &mut FunctionContext<'_>,
    reporter: &mut Reporter,
) -> Type {
    let ty = infer_expr(expr, program, context, reporter);
    match op {
        UnaryOp::Not if compatible(&ty, &Type::Bool) => Type::Bool,
        UnaryOp::Neg if compatible(&ty, &Type::Int) => Type::Int,
        UnaryOp::Not => {
            reporter.push(Diagnostic::error(
                "PACO-E0301",
                span,
                format!("type mismatch: expected bool, found {}", ty.name()),
            ));
            Type::Error
        }
        UnaryOp::Neg => {
            reporter.push(Diagnostic::error(
                "PACO-E0301",
                span,
                format!("type mismatch: expected numeric, found {}", ty.name()),
            ));
            Type::Error
        }
    }
}

fn check_args(
    args: &[Expr],
    expected: &[Type],
    span: Span,
    program: &Program,
    context: &mut FunctionContext<'_>,
    reporter: &mut Reporter,
    substitutions: &mut HashMap<String, Type>,
) {
    if args.len() != expected.len() {
        reporter.push(Diagnostic::error(
            "PACO-E0305",
            span,
            format!(
                "expected {} arguments, found {}",
                expected.len(),
                args.len()
            ),
        ));
    }
    for (arg, expected) in args.iter().zip(expected) {
        let actual = infer_expr(arg, program, context, reporter);
        if !unify_type(expected, &actual, substitutions) {
            reporter.push(Diagnostic::error(
                "PACO-E0302",
                span,
                format!(
                    "type mismatch: expected {}, found {}",
                    expected.name(),
                    actual.name()
                ),
            ));
        }
    }
}

fn check_variant_args(
    variant: &VariantInfo,
    args: &[Expr],
    enum_ty: &Type,
    program: &Program,
    context: &mut FunctionContext<'_>,
    reporter: &mut Reporter,
) {
    let expected = match &variant.fields {
        VariantFields::Unit => Vec::new(),
        VariantFields::Tuple(tys) => tys
            .iter()
            .map(|ty| instantiate_ty(ty, enum_ty, program, reporter))
            .collect(),
        VariantFields::Struct(_) => {
            reporter.push(Diagnostic::error(
                "PACO-E0306",
                variant.span,
                "named enum variant construction is not supported yet",
            ));
            Vec::new()
        }
    };
    check_args(
        args,
        &expected,
        variant.span,
        program,
        context,
        reporter,
        &mut HashMap::new(),
    );
}

fn instantiated_struct_fields(
    ty: &Type,
    program: &Program,
    reporter: &mut Reporter,
) -> Option<HashMap<String, Type>> {
    let Type::Struct(name, args) = ty else {
        return None;
    };
    let info = program.structs.get(name)?;
    if info.generics.len() != args.len() {
        reporter.push(Diagnostic::error(
            "PACO-E0316",
            Span::new_root(0, 0),
            format!(
                "generic arity mismatch for `{name}`: expected {}, found {}",
                info.generics.len(),
                args.len()
            ),
        ));
        return Some(HashMap::new());
    }
    let env = info
        .generics
        .iter()
        .cloned()
        .zip(args.iter().cloned())
        .collect::<HashMap<_, _>>();
    Some(
        info.fields
            .iter()
            .map(|(name, ty, _)| (name.clone(), program.ty_from_ast(ty, &env, reporter)))
            .collect(),
    )
}

fn instantiate_ty(ty: &Ty, owner: &Type, program: &Program, reporter: &mut Reporter) -> Type {
    let env = match owner {
        Type::Struct(name, args) => program
            .structs
            .get(name)
            .map(|info| {
                info.generics
                    .iter()
                    .cloned()
                    .zip(args.iter().cloned())
                    .collect()
            })
            .unwrap_or_default(),
        Type::Enum(name, args) => program
            .enums
            .get(name)
            .map(|info| {
                info.generics
                    .iter()
                    .cloned()
                    .zip(args.iter().cloned())
                    .collect()
            })
            .unwrap_or_default(),
        _ => HashMap::new(),
    };
    program.ty_from_ast(ty, &env, reporter)
}

fn generic_substitutions(generics: &[String]) -> HashMap<String, Type> {
    generics
        .iter()
        .map(|name| (name.clone(), Type::Generic(name.clone())))
        .collect()
}

fn unify_type(expected: &Type, actual: &Type, substitutions: &mut HashMap<String, Type>) -> bool {
    match (expected, actual) {
        (Type::Generic(name), _) => {
            if let Some(existing) = substitutions.get(name).cloned() {
                if matches!(existing, Type::Generic(_)) {
                    substitutions.insert(name.clone(), actual.clone());
                    true
                } else {
                    compatible(actual, &existing)
                }
            } else {
                substitutions.insert(name.clone(), actual.clone());
                true
            }
        }
        (Type::Struct(expected_name, expected_args), Type::Struct(actual_name, actual_args))
        | (Type::Enum(expected_name, expected_args), Type::Enum(actual_name, actual_args)) => {
            expected_name == actual_name
                && expected_args.len() == actual_args.len()
                && expected_args
                    .iter()
                    .zip(actual_args)
                    .all(|(expected, actual)| unify_type(expected, actual, substitutions))
        }
        (
            Type::Borrow {
                mutable: expected_mutable,
                ty: expected,
            },
            Type::Borrow {
                mutable: actual_mutable,
                ty: actual,
            },
        ) => expected_mutable == actual_mutable && unify_type(expected, actual, substitutions),
        _ => compatible(actual, expected),
    }
}

fn substitute_generics(ty: &Type, substitutions: &HashMap<String, Type>) -> Type {
    match ty {
        Type::Generic(name) => substitutions
            .get(name)
            .cloned()
            .unwrap_or_else(|| Type::Generic(name.clone())),
        Type::Struct(name, args) => Type::Struct(
            name.clone(),
            args.iter()
                .map(|arg| substitute_generics(arg, substitutions))
                .collect(),
        ),
        Type::Enum(name, args) => Type::Enum(
            name.clone(),
            args.iter()
                .map(|arg| substitute_generics(arg, substitutions))
                .collect(),
        ),
        Type::Borrow { mutable, ty } => Type::Borrow {
            mutable: *mutable,
            ty: Box::new(substitute_generics(ty, substitutions)),
        },
        other => other.clone(),
    }
}

fn receiver_from_param(param: &Param) -> Option<Receiver> {
    let Pat::Ident(name, _) = &param.pattern else {
        return None;
    };
    if name != "self" {
        return None;
    }
    match &param.ty {
        Ty::Borrow { mutable, .. } => Some(Receiver { mutable: *mutable }),
        Ty::Path(_, _) => Some(Receiver { mutable: false }),
        _ => None,
    }
}

fn target_type_name(ty: &Type) -> Option<String> {
    match ty {
        Type::Struct(name, _) | Type::Enum(name, _) => Some(name.clone()),
        _ => None,
    }
}

fn is_mutable_place(expr: &Expr, context: &FunctionContext<'_>) -> bool {
    match expr {
        Expr::Ident(name, _) => {
            lookup(&context.scopes, name).is_some_and(|binding| binding.mutable)
        }
        Expr::Field { base, .. } => is_assignable_field_base(base, context),
        _ => false,
    }
}

fn is_assignable_field_base(expr: &Expr, context: &FunctionContext<'_>) -> bool {
    match expr {
        Expr::Ident(name, _) => lookup(&context.scopes, name).is_some_and(|binding| {
            matches!(binding.ty, Type::Borrow { mutable: true, .. })
                || (binding.mutable && !matches!(binding.ty, Type::Borrow { mutable: false, .. }))
        }),
        Expr::Field { base, .. } => is_assignable_field_base(base, context),
        _ => false,
    }
}

fn place_name(expr: &Expr) -> Option<&str> {
    match expr {
        Expr::Ident(name, _) => Some(name),
        Expr::Field { base, .. } => place_name(base),
        _ => None,
    }
}

fn ty_span(ty: &Ty) -> Span {
    match ty {
        Ty::Path(_, span)
        | Ty::Generic { span, .. }
        | Ty::Tuple(_, span)
        | Ty::Slice(_, span)
        | Ty::Dyn { span, .. }
        | Ty::Fn { span, .. }
        | Ty::Infer(span)
        | Ty::Never(span)
        | Ty::Borrow { span, .. } => *span,
    }
}

fn pattern_span(pattern: &Pat) -> Span {
    match pattern {
        Pat::Ident(_, span)
        | Pat::Wildcard(span)
        | Pat::Literal(_, span)
        | Pat::Tuple(_, span)
        | Pat::Struct { span, .. }
        | Pat::Enum { span, .. }
        | Pat::Range { span, .. }
        | Pat::Or(_, span)
        | Pat::Binding { span, .. } => *span,
    }
}

fn literal_type(literal: &Literal) -> Type {
    match literal {
        Literal::Int(_) => Type::Int,
        Literal::Float(_) => Type::Float,
        Literal::Bool(_) => Type::Bool,
        Literal::String(_) | Literal::Char(_) => Type::String,
    }
}

fn unsupported_expr(feature: &str, span: Span, reporter: &mut Reporter) -> Type {
    reporter.push(Diagnostic::error(
        "PACO-E0306",
        span,
        format!("expression is not supported yet: {feature}"),
    ));
    Type::Error
}

fn lookup(scopes: &[HashMap<String, Binding>], name: &str) -> Option<Binding> {
    scopes
        .iter()
        .rev()
        .find_map(|scope| scope.get(name).cloned())
}

fn compatible(actual: &Type, expected: &Type) -> bool {
    actual == expected
        || matches!(actual, Type::Unknown | Type::Error)
        || matches!(expected, Type::Unknown | Type::Error)
}

fn join_branch_types(left: &Type, right: &Type) -> Option<Type> {
    if compatible(left, right) {
        Some(left.clone())
    } else if *left == Type::Never {
        Some(right.clone())
    } else if *right == Type::Never {
        Some(left.clone())
    } else {
        None
    }
}

impl Type {
    fn name(&self) -> String {
        match self {
            Type::Int => "int".to_string(),
            Type::Float => "float".to_string(),
            Type::Bool => "bool".to_string(),
            Type::String => "string".to_string(),
            Type::Unit => "unit".to_string(),
            Type::Never => "never".to_string(),
            Type::Struct(name, args) | Type::Enum(name, args) if args.is_empty() => name.clone(),
            Type::Struct(name, args) | Type::Enum(name, args) => {
                let args = args.iter().map(Type::name).collect::<Vec<_>>().join(", ");
                format!("{name}<{args}>")
            }
            Type::Borrow { mutable, ty } if *mutable => format!("&mut {}", ty.name()),
            Type::Borrow { ty, .. } => format!("&{}", ty.name()),
            Type::Generic(name) => name.clone(),
            Type::Unknown => "unknown".to_string(),
            Type::Error => "error".to_string(),
        }
    }
}
