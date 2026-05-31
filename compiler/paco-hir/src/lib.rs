//! Minimal HIR surface for data type name and type-shape lowering.

use std::collections::HashMap;

use paco_syntax::ast::{Block, Expr, FnDecl, Item, Module, Ty, VariantFields};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct DefId(pub usize);

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DefKind {
    Function,
    Struct,
    Enum,
    Field,
    Variant,
    Method,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Def {
    pub id: DefId,
    pub parent: Option<DefId>,
    pub name: String,
    pub kind: DefKind,
    pub references: Vec<DefId>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HirModule {
    pub defs: Vec<Def>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HirError;

pub fn lower_module(module: &Module) -> Result<HirModule, HirError> {
    let mut lowerer = Lowerer::default();
    lowerer.collect_items(module);
    lowerer.collect_references(module);
    Ok(HirModule { defs: lowerer.defs })
}

#[derive(Default)]
struct Lowerer {
    defs: Vec<Def>,
    top_level: HashMap<String, DefId>,
}

impl Lowerer {
    fn collect_items(&mut self, module: &Module) {
        for item in &module.items {
            match item {
                Item::Fn(function) => {
                    self.push_top_level(function.name.clone(), DefKind::Function);
                }
                Item::Struct(decl) => {
                    let parent = self.push_top_level(decl.name.clone(), DefKind::Struct);
                    for field in &decl.fields {
                        self.push_child(parent, field.name.clone(), DefKind::Field);
                    }
                    for method in &decl.methods {
                        self.push_child(parent, method.name.clone(), DefKind::Method);
                    }
                }
                Item::Enum(decl) => {
                    let parent = self.push_top_level(decl.name.clone(), DefKind::Enum);
                    for variant in &decl.variants {
                        self.push_child(parent, variant.name.clone(), DefKind::Variant);
                    }
                    for method in &decl.methods {
                        self.push_child(parent, method.name.clone(), DefKind::Method);
                    }
                }
                Item::Methods(decl) => {
                    if let Some(parent) = self.resolve_ty(&decl.target) {
                        for method in &decl.methods {
                            self.push_child(parent, method.name.clone(), DefKind::Method);
                        }
                    }
                }
                Item::Trait(_) | Item::Use(_) => {}
            }
        }
    }

    fn collect_references(&mut self, module: &Module) {
        for item in &module.items {
            match item {
                Item::Fn(function) => {
                    let Some(id) = self.top_level.get(&function.name).copied() else {
                        continue;
                    };
                    self.record_function_signature_refs(id, function);
                    self.record_block_refs(id, &function.body);
                }
                Item::Struct(decl) => {
                    if let Some(id) = self.top_level.get(&decl.name).copied() {
                        for field in &decl.fields {
                            self.record_ty_ref(id, &field.ty);
                        }
                    }
                    for method in &decl.methods {
                        if let Some(id) = self.find_child(&decl.name, &method.name) {
                            self.record_function_signature_refs(id, method);
                            self.record_block_refs(id, &method.body);
                        }
                    }
                }
                Item::Enum(decl) => {
                    if let Some(id) = self.top_level.get(&decl.name).copied() {
                        for variant in &decl.variants {
                            self.record_variant_refs(id, &variant.fields);
                        }
                    }
                    for method in &decl.methods {
                        if let Some(id) = self.find_child(&decl.name, &method.name) {
                            self.record_function_signature_refs(id, method);
                            self.record_block_refs(id, &method.body);
                        }
                    }
                }
                Item::Methods(decl) => {
                    let Some(parent) = self.resolve_ty(&decl.target) else {
                        continue;
                    };
                    for method in &decl.methods {
                        let Some(id) = self
                            .defs
                            .iter()
                            .find(|def| {
                                def.parent == Some(parent)
                                    && def.name == method.name
                                    && def.kind == DefKind::Method
                            })
                            .map(|def| def.id)
                        else {
                            continue;
                        };
                        self.record_function_signature_refs(id, method);
                        self.record_block_refs(id, &method.body);
                    }
                }
                Item::Trait(_) | Item::Use(_) => {}
            }
        }
    }

    fn record_function_signature_refs(&mut self, owner: DefId, function: &FnDecl) {
        for param in &function.params {
            self.record_ty_ref(owner, &param.ty);
        }
        if let Some(ty) = &function.return_ty {
            self.record_ty_ref(owner, ty);
        }
    }

    fn record_variant_refs(&mut self, owner: DefId, fields: &VariantFields) {
        match fields {
            VariantFields::Unit => {}
            VariantFields::Tuple(types) => {
                for ty in types {
                    self.record_ty_ref(owner, ty);
                }
            }
            VariantFields::Struct(fields) => {
                for field in fields {
                    self.record_ty_ref(owner, &field.ty);
                }
            }
        }
    }

    fn push_top_level(&mut self, name: String, kind: DefKind) -> DefId {
        let id = self.push_def(None, name.clone(), kind);
        self.top_level.insert(name, id);
        id
    }

    fn push_child(&mut self, parent: DefId, name: String, kind: DefKind) -> DefId {
        self.push_def(Some(parent), name, kind)
    }

    fn push_def(&mut self, parent: Option<DefId>, name: String, kind: DefKind) -> DefId {
        let id = DefId(self.defs.len());
        self.defs.push(Def {
            id,
            parent,
            name,
            kind,
            references: Vec::new(),
        });
        id
    }

    fn record_block_refs(&mut self, owner: DefId, block: &Block) {
        for statement in &block.stmts {
            match statement {
                paco_syntax::ast::Stmt::Let(statement) => {
                    if let Some(ty) = &statement.ty {
                        self.record_ty_ref(owner, ty);
                    }
                    if let Some(value) = &statement.value {
                        self.record_expr_ref(owner, value);
                    }
                }
                paco_syntax::ast::Stmt::Expr(expr) => self.record_expr_ref(owner, expr),
                paco_syntax::ast::Stmt::Item(_) => {}
            }
        }
        if let Some(tail) = &block.tail {
            self.record_expr_ref(owner, tail);
        }
    }

    fn record_expr_ref(&mut self, owner: DefId, expr: &Expr) {
        match expr {
            Expr::StructLiteral { ty, fields, .. } => {
                self.record_ty_ref(owner, ty);
                for (_, value) in fields {
                    self.record_expr_ref(owner, value);
                }
            }
            Expr::AssociatedCall { ty, args, .. } => {
                self.record_ty_ref(owner, ty);
                for arg in args {
                    self.record_expr_ref(owner, arg);
                }
            }
            Expr::Field { base, .. } => self.record_expr_ref(owner, base),
            Expr::MethodCall { receiver, args, .. } => {
                self.record_expr_ref(owner, receiver);
                for arg in args {
                    self.record_expr_ref(owner, arg);
                }
            }
            Expr::Block(block) => self.record_block_refs(owner, block),
            Expr::If {
                condition,
                then_branch,
                else_branch,
                ..
            } => {
                self.record_expr_ref(owner, condition);
                self.record_block_refs(owner, then_branch);
                if let Some(else_branch) = else_branch {
                    self.record_expr_ref(owner, else_branch);
                }
            }
            Expr::Loop { body, .. } => self.record_block_refs(owner, body),
            Expr::While {
                condition, body, ..
            } => {
                self.record_expr_ref(owner, condition);
                self.record_block_refs(owner, body);
            }
            Expr::Call { callee, args, .. } => {
                self.record_expr_ref(owner, callee);
                for arg in args {
                    self.record_expr_ref(owner, arg);
                }
            }
            Expr::Binary { left, right, .. } => {
                self.record_expr_ref(owner, left);
                self.record_expr_ref(owner, right);
            }
            Expr::Unary { expr, .. }
            | Expr::Spawn { expr, .. }
            | Expr::Comptime { expr, .. }
            | Expr::Yield(expr, _)
            | Expr::Borrow { expr, .. } => self.record_expr_ref(owner, expr),
            Expr::Assign { target, value, .. } => {
                self.record_expr_ref(owner, target);
                self.record_expr_ref(owner, value);
            }
            Expr::Index { base, index, .. } => {
                self.record_expr_ref(owner, base);
                self.record_expr_ref(owner, index);
            }
            Expr::Return(value, _) | Expr::Break(value, _) => {
                if let Some(value) = value {
                    self.record_expr_ref(owner, value);
                }
            }
            Expr::Match { .. } | Expr::Select { .. } => {}
            Expr::Literal(_, _) | Expr::Ident(_, _) | Expr::Continue(_) => {}
        }
    }

    fn record_ty_ref(&mut self, owner: DefId, ty: &Ty) {
        if let Some(id) = self.resolve_ty(ty)
            && let Some(def) = self.defs.get_mut(owner.0)
            && !def.references.contains(&id)
        {
            def.references.push(id);
        }
        if let Ty::Generic { args, .. } = ty {
            for arg in args {
                self.record_ty_ref(owner, arg);
            }
        }
    }

    fn resolve_ty(&self, ty: &Ty) -> Option<DefId> {
        match ty {
            Ty::Path(path, _) | Ty::Generic { path, .. } if path.len() == 1 => {
                self.top_level.get(&path[0]).copied()
            }
            _ => None,
        }
    }

    fn find_child(&self, parent_name: &str, child_name: &str) -> Option<DefId> {
        let parent = self.top_level.get(parent_name).copied()?;
        self.defs
            .iter()
            .find(|def| def.parent == Some(parent) && def.name == child_name)
            .map(|def| def.id)
    }
}
