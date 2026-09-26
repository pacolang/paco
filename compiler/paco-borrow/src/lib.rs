//! Ownership and move analysis for executable frontend features.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::marker::PhantomData;
use std::rc::Rc;

use paco_resolve::{LocalId, Locals};

use paco_diag::{Diagnostic, Reporter};
use paco_span::Span;
use paco_syntax::ast::{
    ClosureParam,
    self, Block, Expr, FnDecl, Item, LetStmt, Literal, MatchArm, Module, Pat, QuoteBody, Stmt, Ty,
    VariantFields, Visit, walk_expr,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BorrowError;

const TEMPORARY_BORROW_OWNER: &str = "<temporary>";

/// A point in a function body where control leaves a scope: either falling
/// off the end of a `Block`, or an explicit `return`/`break`/`continue`.
/// Identified by address into the `Module` that produced the owning
/// [`DropPlan`], matching [`paco_types::TypedModule`]'s node-identity scheme.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
enum ExitPoint {
    BlockEnd(*const Block),
    Control(*const Expr),
}

/// The result of [`analyze_module`]: for every scope-exit point in the
/// program, the owned, droppable locals live at that point, in reverse
/// declaration order (per design.md's "MIR lowering does not re-derive
/// analysis from scratch" decision).
pub struct DropPlan<'a> {
    points: HashMap<ExitPoint, Vec<LocalId>>,
    _module: PhantomData<&'a Module>,
}

impl<'a> DropPlan<'a> {
    /// Locals to drop when control falls off the end of `block` normally.
    pub fn drops_at_block_end(&self, block: &Block) -> &[LocalId] {
        self.points
            .get(&ExitPoint::BlockEnd(block as *const Block))
            .map_or(&[], Vec::as_slice)
    }

    /// Locals to drop at a `return`/`break`/`continue` expression, covering
    /// every scope the control transfer exits through.
    pub fn drops_at_control_transfer(&self, expr: &Expr) -> &[LocalId] {
        self.points
            .get(&ExitPoint::Control(expr as *const Expr))
            .map_or(&[], Vec::as_slice)
    }
}

/// Borrow-checks `module`, returning the same diagnostics as [`check_module`]
/// but also the computed [`DropPlan`].
pub fn analyze_module<'a>(
    module: &'a Module,
    reporter: &mut Reporter,
) -> Result<DropPlan<'a>, BorrowError> {
    analyze_module_with_imports(module, &[], reporter)
}

pub fn analyze_module_with_imports<'a>(
    module: &'a Module,
    imports: &[(String, &Module)],
    reporter: &mut Reporter,
) -> Result<DropPlan<'a>, BorrowError> {
    let typed = paco_types::infer_module_with_imports(module, imports, &mut Reporter::new()).ok();
    analyze_typed_module(module, imports, typed.as_ref(), reporter)
}

/// Borrow-checks `module` with the type checker's resolved types, which
/// decide whether an unannotated binding copies or moves.
pub fn analyze_typed_module<'a>(
    module: &'a Module,
    imports: &[(String, &Module)],
    typed: Option<&paco_types::TypedModule<'_>>,
    reporter: &mut Reporter,
) -> Result<DropPlan<'a>, BorrowError> {
    let program = run_checks(module, imports, typed, reporter);
    if reporter.has_errors() {
        Err(BorrowError)
    } else {
        Ok(DropPlan {
            points: program.drops.into_inner(),
            _module: PhantomData,
        })
    }
}

pub fn check_module(module: &Module, reporter: &mut Reporter) -> Result<(), BorrowError> {
    analyze_module(module, reporter).map(|_| ())
}

pub fn check_module_with_imports(
    module: &Module,
    imports: &[(String, &Module)],
    reporter: &mut Reporter,
) -> Result<(), BorrowError> {
    analyze_module_with_imports(module, imports, reporter).map(|_| ())
}

pub fn check_typed_module(
    module: &Module,
    imports: &[(String, &Module)],
    typed: &paco_types::TypedModule<'_>,
    reporter: &mut Reporter,
) -> Result<(), BorrowError> {
    analyze_typed_module(module, imports, Some(typed), reporter).map(|_| ())
}

fn run_checks(
    module: &Module,
    imports: &[(String, &Module)],
    typed: Option<&paco_types::TypedModule<'_>>,
    reporter: &mut Reporter,
) -> Program {
    let mut program = Program::from_module_with_imports(module, imports);
    if let Some(typed) = typed {
        program.expr_types = Rc::new(typed.expr_types().clone());
    }
    for item in &module.items {
        match item {
            Item::Fn(function) => check_function(function, None, &program, reporter),
            Item::Struct(decl) => {
                for method in &decl.methods {
                    check_function(method, Some(decl.name.as_str()), &program, reporter);
                }
            }
            Item::Enum(decl) => {
                for method in &decl.methods {
                    check_function(method, Some(decl.name.as_str()), &program, reporter);
                }
            }
            Item::Methods(block) => {
                for method in &block.methods {
                    check_function(method, type_name(&block.target).as_deref(), &program, reporter);
                }
            }
            Item::Trait(_) | Item::Use(_) | Item::Const(_) | Item::Extern(_) => {}
        }
    }
    program
}

#[derive(Clone, Debug, Default)]
struct Program {
    functions: HashMap<String, FunctionSignature>,
    iter_functions: HashSet<String>,
    methods: HashMap<(String, String), FunctionSignature>,
    fields: HashMap<(String, String), Ty>,
    variants: HashMap<(String, String), Vec<Ty>>,
    type_params: HashMap<String, Vec<String>>,
    drops: RefCell<HashMap<ExitPoint, Vec<LocalId>>>,
    locals: Rc<Locals>,
    /// The shape intrinsics of `Shaped` types (`x.dim(0)`, `x.as_dims()`),
    /// which borrow their receiver; the others consume it.
    intrinsics: HashMap<String, FunctionSignature>,
    /// Each struct's declared `type Tangent`.
    tangents: HashMap<String, Ty>,
    /// Functions marked `#[builtin(grad)]`.
    grads: HashSet<String>,
    /// `stdlib::test`'s nine `#[builtin(assert)]`/`#[builtin(assert_eq)]`/etc.
    /// functions: their arguments are read, never consumed (like `print`'s),
    /// since the compiler's own replacement only compares and formats them.
    test_asserts: HashSet<String>,
    /// User types marked `#[derive(Copy)]`, by plain and qualified name.
    copy_types: HashSet<String>,
    /// Each owner's generic parameters bounded by `Copy`.
    copy_params: HashMap<String, Vec<String>>,
    expr_types: Rc<HashMap<*const Expr, paco_types::Type>>,
}

impl Program {
    fn from_module_with_imports(module: &Module, imports: &[(String, &Module)]) -> Self {
        let mut program = Self { locals: Rc::new(paco_resolve::resolve_locals([module])), ..Self::default() };
        program.register_prelude();
        program.collect_items(module, "");
        for (qualifier, imported_module) in imports {
            program.collect_pub_items(imported_module, qualifier);
        }
        program
    }

    fn note_grad(&mut self, name: &str, function: &FnDecl) {
        if function.attrs.iter().any(|attr| {
            attr.name == "builtin" && matches!(attr.args.first(), Some(paco_syntax::ast::AttributeArg::Path(path, _)) if path == &["grad"])
        }) {
            self.grads.insert(name.to_string());
        }
    }

    fn note_test_assert(&mut self, name: &str, function: &FnDecl) {
        const TEST_ASSERT_BUILTINS: &[&str] = &[
            "assert", "assert_eq", "assert_ne", "assert_true", "assert_false", "assert_some", "assert_none", "assert_ok", "assert_err",
        ];
        if function.attrs.iter().any(|attr| {
            attr.name == "builtin"
                && matches!(attr.args.first(), Some(paco_syntax::ast::AttributeArg::Path(path, _)) if path.len() == 1 && TEST_ASSERT_BUILTINS.contains(&path[0].as_str()))
        }) {
            self.test_asserts.insert(name.to_string());
        }
    }

    fn note_copy(&mut self, name: &str, plain: &str, attrs: &[ast::Attribute], generics: &[ast::GenericParam]) {
        if ast::has_derive(attrs, "Copy") {
            self.copy_types.insert(name.to_string());
            self.copy_types.insert(plain.to_string());
        }
        self.copy_params.entry(name.to_string()).or_default().extend(ast::bounded_by(generics, "Copy"));
    }

    fn note_tangent(&mut self, name: &str, decl: &paco_syntax::ast::StructDecl) {
        if let Some(tangent) = decl.assoc_types.iter().find(|assoc| assoc.name == "Tangent").and_then(|assoc| assoc.default.clone()) {
            self.tangents.insert(name.to_string(), tangent);
        }
    }

    /// `grad(f, inputs)`'s type, `(f's result, the tangents of its
    /// parameters)`: a type whose `Tangent` is itself keeps its arguments.
    fn grad_result_ty(&self, args: &[Expr], span: Span) -> Option<Ty> {
        let [Expr::Ident(name, _), _] = args else { return None };
        let signature = self.functions.get(name)?;
        let strip = |ty: &Ty| match ty {
            Ty::Borrow { ty, .. } => (**ty).clone(),
            other => other.clone(),
        };
        let output = match &signature.return_ty {
            Some(ty) => ty.clone(),
            None => signature.params.iter().find_map(|param| matches!(param, Ty::Borrow { mutable: true, .. }).then(|| strip(param)))?,
        };
        let tangent = |param: &Ty| {
            let param = strip(param);
            let head = match &param {
                Ty::Path(path, _) | Ty::Generic { path, .. } => path.join("::"),
                _ => return param,
            };
            match self.tangents.get(&head) {
                Some(tangent @ (Ty::Path(path, _) | Ty::Generic { path, .. })) if path.join("::") != head => tangent.clone(),
                _ => param,
            }
        };
        let gradients: Vec<Ty> = signature.params.iter().map(tangent).collect();
        let gradients = match gradients.as_slice() {
            [single] => single.clone(),
            _ => Ty::Tuple(gradients, span),
        };
        Some(Ty::Tuple(vec![output, gradients], span))
    }

    fn collect_items(&mut self, module: &Module, qualifier: &str) {
        let key = |name: &str| if qualifier.is_empty() { name.to_string() } else { format!("{qualifier}::{name}") };
        for item in &module.items {
            match item {
                Item::Fn(function) => {
                    let name = key(&function.name);
                    if function.is_iter {
                        self.iter_functions.insert(name.clone());
                    }
                    self.note_grad(&name, function);
                    self.note_test_assert(&name, function);
                    self.functions.insert(name, FunctionSignature::from(function));
                }
                Item::Struct(decl) => {
                    let name = key(&decl.name);
                    self.note_copy(&name, &decl.name, &decl.attrs, &decl.generics);
                    self.note_tangent(&name, decl);
                    self.type_params.insert(name.clone(), paco_syntax::ast::generic_names(&decl.generics));
                    for field in &decl.fields {
                        self.fields
                            .insert((name.clone(), field.name.clone()), field.ty.clone());
                    }
                    for method in &decl.methods {
                        self.methods.insert(
                            (name.clone(), method.name.clone()),
                            FunctionSignature::from(method),
                        );
                    }
                }
                Item::Enum(decl) => {
                    let name = key(&decl.name);
                    self.note_copy(&name, &decl.name, &decl.attrs, &decl.generics);
                    self.type_params.insert(name.clone(), paco_syntax::ast::generic_names(&decl.generics));
                    for variant in &decl.variants {
                        let fields = match &variant.fields {
                            VariantFields::Unit => Vec::new(),
                            VariantFields::Tuple(fields) => fields.clone(),
                            VariantFields::Struct(fields) => {
                                fields.iter().map(|field| field.ty.clone()).collect()
                            }
                        };
                        self.variants
                            .insert((name.clone(), variant.name.clone()), fields);
                    }
                    for method in &decl.methods {
                        self.methods.insert(
                            (name.clone(), method.name.clone()),
                            FunctionSignature::from(method),
                        );
                    }
                }
                Item::Methods(block) => {
                    if let Some(name) = type_name(&block.target) {
                        let name = key(&name);
                        self.copy_params.entry(name.clone()).or_default().extend(ast::bounded_by(&block.generics, "Copy"));
                        for method in &block.methods {
                            self.methods.insert(
                                (name.clone(), method.name.clone()),
                                FunctionSignature::from(method),
                            );
                        }
                    }
                }
                Item::Trait(_) | Item::Use(_) | Item::Const(_) | Item::Extern(_) => {}
            }
        }
    }

    fn collect_pub_items(&mut self, module: &Module, qualifier: &str) {
        let key = |name: &str| if qualifier.is_empty() { name.to_string() } else { format!("{qualifier}::{name}") };
        for item in &module.items {
            match item {
                Item::Fn(function) if function.is_pub => {
                    let name = key(&function.name);
                    if self.functions.contains_key(&name) {
                        continue;
                    }
                    if function.is_iter {
                        self.iter_functions.insert(name.clone());
                    }
                    self.note_grad(&name, function);
                    self.note_test_assert(&name, function);
                    self.functions.insert(name, FunctionSignature::from(function));
                }
                Item::Struct(decl) if decl.is_pub => {
                    let name = key(&decl.name);
                    if self.type_params.contains_key(&name) {
                        continue;
                    }
                    self.note_copy(&name, &decl.name, &decl.attrs, &decl.generics);
                    self.note_tangent(&name, decl);
                    self.note_tangent(&decl.name, decl);
                    self.type_params.insert(name.clone(), paco_syntax::ast::generic_names(&decl.generics));
                    for field in &decl.fields {
                        self.fields
                            .insert((name.clone(), field.name.clone()), field.ty.clone());
                    }
                    for method in &decl.methods {
                        self.methods
                            .entry((name.clone(), method.name.clone()))
                            .or_insert_with(|| FunctionSignature::from(method));
                        if name != decl.name {
                            self.methods
                                .entry((decl.name.clone(), method.name.clone()))
                                .or_insert_with(|| FunctionSignature::from(method));
                        }
                    }
                }
                Item::Enum(decl) if decl.is_pub => {
                    let name = key(&decl.name);
                    if self.type_params.contains_key(&name) {
                        continue;
                    }
                    self.note_copy(&name, &decl.name, &decl.attrs, &decl.generics);
                    self.type_params.insert(name.clone(), paco_syntax::ast::generic_names(&decl.generics));
                    for variant in &decl.variants {
                        let fields = match &variant.fields {
                            VariantFields::Unit => Vec::new(),
                            VariantFields::Tuple(fields) => fields.clone(),
                            VariantFields::Struct(fields) => {
                                fields.iter().map(|field| field.ty.clone()).collect()
                            }
                        };
                        self.variants
                            .insert((name.clone(), variant.name.clone()), fields);
                    }
                    for method in &decl.methods {
                        self.methods
                            .entry((name.clone(), method.name.clone()))
                            .or_insert_with(|| FunctionSignature::from(method));
                        if name != decl.name {
                            self.methods
                                .entry((decl.name.clone(), method.name.clone()))
                                .or_insert_with(|| FunctionSignature::from(method));
                        }
                    }
                }
                Item::Methods(block) => {
                    if let Some(name) = type_name(&block.target) {
                        let qualified = key(&name);
                        for method in &block.methods {
                            self.methods
                                .entry((name.clone(), method.name.clone()))
                                .or_insert_with(|| FunctionSignature::from(method));
                            self.methods
                                .entry((qualified.clone(), method.name.clone()))
                                .or_insert_with(|| FunctionSignature::from(method));
                        }
                    }
                }
                _ => {}
            }
        }
    }

    /// ADR 0022 prelude, mirroring `paco_types::Program::register_prelude` —
    /// paco-borrow keeps its own parallel signature tables (syntax `Ty`, not
    /// the type checker's `Type`), so these builtins need registering here
    /// too for `tx.send(value)`-style calls to resolve to a known,
    /// receiver-borrowing method instead of the conservative "unknown
    /// method, consume the receiver" default.
    fn register_prelude(&mut self) {
        let root = Span::new_root(0, 0);
        let self_ref = Ty::Borrow {
            mutable: false,
            lifetime: None,
            ty: Box::new(Ty::Path(vec!["Self".to_string()], root)),
            span: root,
        };
        let i64_ty = || Ty::Path(vec!["i64".to_string()], root);
        self.intrinsics.insert(
            "dim".to_string(),
            FunctionSignature { generics: Vec::new(), params: vec![self_ref.clone(), i64_ty()], return_ty: Some(i64_ty()) },
        );
        self.intrinsics.insert(
            "as_dims".to_string(),
            FunctionSignature {
                generics: Vec::new(),
                params: vec![self_ref.clone()],
                return_ty: Some(Ty::Generic {
                    path: vec!["Result".to_string()],
                    args: vec![self_ref.clone(), Ty::Path(vec!["DimError".to_string()], root)],
                    span: root,
                }),
            },
        );
        let t = || Ty::Path(vec!["T".to_string()], root);
        self.functions.insert(
            "channel".to_string(),
            FunctionSignature {
                generics: vec!["T".to_string()],
                params: vec![Ty::Path(vec!["i64".to_string()], root)],
                return_ty: Some(Ty::Tuple(
                    vec![
                        Ty::Generic { path: vec!["Sender".to_string()], args: vec![t()], span: root },
                        Ty::Generic { path: vec!["Receiver".to_string()], args: vec![t()], span: root },
                    ],
                    root,
                )),
            },
        );
        self.functions.insert(
            "tcp_listen".to_string(),
            FunctionSignature {
                generics: Vec::new(),
                params: vec![Ty::Path(vec!["i64".to_string()], root)],
                return_ty: Some(Ty::Path(vec!["TcpListener".to_string()], root)),
            },
        );
        self.methods.insert(
            ("TcpListener".to_string(), "accept".to_string()),
            FunctionSignature {
                generics: Vec::new(),
                params: vec![self_ref.clone()],
                return_ty: Some(Ty::Path(vec!["TcpStream".to_string()], root)),
            },
        );
        self.methods.insert(
            ("TcpStream".to_string(), "read".to_string()),
            FunctionSignature {
                generics: Vec::new(),
                params: vec![self_ref.clone(), Ty::Path(vec!["i64".to_string()], root)],
                return_ty: Some(Ty::Path(vec!["string".to_string()], root)),
            },
        );
        self.methods.insert(
            ("TcpStream".to_string(), "write".to_string()),
            FunctionSignature {
                generics: Vec::new(),
                params: vec![self_ref.clone(), Ty::Path(vec!["string".to_string()], root)],
                return_ty: None,
            },
        );
        let result_of = |ok: Ty, err_name: &str| {
            Ty::Generic {
                path: vec!["Result".to_string()],
                args: vec![ok, Ty::Path(vec![err_name.to_string()], root)],
                span: root,
            }
        };
        self.methods.insert(
            ("Sender".to_string(), "send".to_string()),
            FunctionSignature {
                generics: Vec::new(),
                params: vec![self_ref.clone(), t()],
                return_ty: Some(result_of(Ty::Tuple(Vec::new(), root), "SendError")),
            },
        );
        self.methods.insert(
            ("Sender".to_string(), "close".to_string()),
            FunctionSignature {
                generics: Vec::new(),
                params: vec![self_ref.clone()],
                return_ty: None,
            },
        );
        self.methods.insert(
            ("Receiver".to_string(), "recv".to_string()),
            FunctionSignature {
                generics: Vec::new(),
                params: vec![self_ref.clone()],
                return_ty: Some(result_of(t(), "RecvError")),
            },
        );
        self.methods.insert(
            ("JoinHandle".to_string(), "join".to_string()),
            FunctionSignature {
                generics: Vec::new(),
                params: vec![self_ref.clone()],
                return_ty: Some(result_of(t(), "TaskPanic")),
            },
        );
        self.methods.insert(
            ("Generator".to_string(), "next".to_string()),
            FunctionSignature {
                generics: Vec::new(),
                params: vec![self_ref.clone()],
                return_ty: Some(Ty::Generic {
                    path: vec!["Option".to_string()],
                    args: vec![t()],
                    span: root,
                }),
            },
        );
        // `phase-9-comptime` Decisions 6-7: without these, `expr_ty`
        // can't determine a call to any of them's return type (they are
        // builtins, never a declared `Item::Fn`) — silently defaulting
        // every method call on the result (e.g. `for field in
        // fields_of(t) { .. }`'s desugared `.next()` call) to a *move*,
        // since `method_signature` then has no receiver type to look up
        // and `receiver_is_borrowed` falls back to `false`. Found via
        // task 7.1/7.2's own `for field in fields_of(t) { .. }` loop
        // spuriously failing borrow-check with "loop body may move
        // `$paco_for_iter_N` on more than one iteration".
        self.functions.insert(
            "fields_of".to_string(),
            FunctionSignature {
                generics: Vec::new(),
                params: vec![Ty::Path(vec!["type".to_string()], root)],
                return_ty: Some(Ty::Path(vec!["FieldIter".to_string()], root)),
            },
        );
        self.functions.insert(
            "type_name".to_string(),
            FunctionSignature {
                generics: Vec::new(),
                params: vec![Ty::Path(vec!["type".to_string()], root)],
                return_ty: Some(Ty::Path(vec!["string".to_string()], root)),
            },
        );
        self.functions.insert(
            "code_to_string".to_string(),
            FunctionSignature {
                generics: Vec::new(),
                params: vec![Ty::Path(vec!["Code".to_string()], root)],
                return_ty: Some(Ty::Path(vec!["string".to_string()], root)),
            },
        );
        for (name, mutable) in [("slice_as_ptr", false), ("slice_as_mut_ptr", true)] {
            self.functions.insert(
                name.to_string(),
                FunctionSignature {
                    generics: vec!["T".to_string()],
                    params: vec![Ty::Borrow {
                        mutable,
                        lifetime: None,
                        ty: Box::new(Ty::Slice(Box::new(t()), root)),
                        span: root,
                    }],
                    return_ty: Some(Ty::RawPointer { mutable, ty: Box::new(t()), span: root }),
                },
            );
        }
        self.register_shared_cell("Rc", "get", None, true, root, &self_ref);
        self.register_shared_cell("Arc", "get", None, true, root, &self_ref);
        self.register_shared_cell("Cell", "get", Some("set"), false, root, &self_ref);
        self.register_shared_cell("RefCell", "get", Some("set"), false, root, &self_ref);
        self.register_shared_cell("Mutex", "lock", Some("set"), false, root, &self_ref);
        self.register_shared_cell("RwLock", "read", Some("write"), false, root, &self_ref);
    }

    fn register_shared_cell(
        &mut self,
        name: &str,
        read_method: &str,
        write_method: Option<&str>,
        has_clone_and_count: bool,
        root: Span,
        self_ref: &Ty,
    ) {
        let t = Ty::Path(vec!["T".to_string()], root);
        let self_generic = || Ty::Generic { path: vec![name.to_string()], args: vec![t.clone()], span: root };
        self.methods.insert(
            (name.to_string(), "new".to_string()),
            FunctionSignature {
                generics: vec!["T".to_string()],
                params: vec![t.clone()],
                return_ty: Some(self_generic()),
            },
        );
        self.methods.insert(
            (name.to_string(), read_method.to_string()),
            FunctionSignature {
                generics: Vec::new(),
                params: vec![self_ref.clone()],
                return_ty: Some(t.clone()),
            },
        );
        if let Some(write_method) = write_method {
            self.methods.insert(
                (name.to_string(), write_method.to_string()),
                FunctionSignature {
                    generics: Vec::new(),
                    params: vec![self_ref.clone(), t.clone()],
                    return_ty: None,
                },
            );
        }
        if has_clone_and_count {
            self.methods.insert(
                (name.to_string(), "clone".to_string()),
                FunctionSignature {
                    generics: Vec::new(),
                    params: vec![self_ref.clone()],
                    return_ty: Some(self_generic()),
                },
            );
            self.methods.insert(
                (name.to_string(), "strong_count".to_string()),
                FunctionSignature {
                    generics: Vec::new(),
                    params: vec![self_ref.clone()],
                    return_ty: Some(Ty::Path(vec!["i64".to_string()], root)),
                },
            );
        }
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
            generics: paco_syntax::ast::generic_names(&function.generics),
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
    id: LocalId,
    name: String,
    copy: bool,
    ty: Option<Ty>,
    moved_at: Option<Span>,
    move_count: usize,
    borrows: Vec<BorrowBinding>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct Owner {
    id: Option<LocalId>,
    name: String,
}

impl Owner {
    fn temporary() -> Self {
        Self { id: None, name: TEMPORARY_BORROW_OWNER.to_string() }
    }

    fn is_temporary(&self) -> bool {
        self.id.is_none() && self.name == TEMPORARY_BORROW_OWNER
    }
}

#[derive(Clone, Debug)]
struct BorrowBinding {
    owner: Owner,
    mutable: bool,
    last_use: usize,
    local_escape: bool,
    field_path: Option<Vec<String>>,
}

#[derive(Clone, Debug)]
struct OwnershipState {
    locals: Rc<Locals>,
    scopes: Vec<HashMap<LocalId, BindingState>>,
    /// Bindings per scope, in declaration order — parallel to `scopes`,
    /// which is unordered. Needed to emit destructor calls in reverse
    /// declaration order (spec.md's RAII drop-order requirement).
    scope_order: Vec<Vec<LocalId>>,
    reachable: bool,
    current_statement: usize,
    last_uses: Vec<HashMap<LocalId, usize>>,
    temporary_borrows: Vec<BorrowBinding>,
    /// Scope-stack depth (`scopes.len()`) at each enclosing loop's entry, so
    /// `break`/`continue` only drop locals declared inside that loop body.
    loop_boundaries: Vec<usize>,
    return_body: Option<*const Block>,
    /// Type names that are `Copy` here beyond the built-in ones: derived
    /// user types and the function's `Copy`-bounded generic parameters.
    copy_names: Rc<HashSet<String>>,
}

impl OwnershipState {
    fn new(locals: Rc<Locals>, copy_names: Rc<HashSet<String>>) -> Self {
        Self {
            copy_names,
            locals,
            scopes: Vec::new(),
            scope_order: Vec::new(),
            reachable: true,
            current_statement: 0,
            last_uses: Vec::new(),
            temporary_borrows: Vec::new(),
            loop_boundaries: Vec::new(),
            return_body: None,
        }
    }
}

impl OwnershipState {
    fn push_scope(&mut self) {
        self.scopes.push(HashMap::new());
        self.scope_order.push(Vec::new());
    }

    fn pop_scope(&mut self) {
        self.scopes.pop();
        self.scope_order.pop();
    }

    fn define(&mut self, pattern: &Pat, copy: bool, ty: Option<Ty>) {
        self.define_with_borrows(pattern, copy, ty, Vec::new());
    }

    fn define_with_borrows(&mut self, pattern: &Pat, copy: bool, ty: Option<Ty>, borrows: Vec<BorrowBinding>) {
        let (Some(id), Pat::Ident(name, _) | Pat::Binding { name, .. }) = (self.locals.pat(pattern), pattern) else {
            return;
        };
        if let Some(scope) = self.scopes.last_mut() {
            let binding = BindingState { id, name: name.clone(), copy, ty, moved_at: None, move_count: 0, borrows };
            scope.insert(id, binding);
            if let Some(order) = self.scope_order.last_mut() {
                order.push(id);
            }
        }
    }

    fn id(&self, expr: &Expr) -> Option<LocalId> {
        self.locals.expr(expr)
    }

    fn owner(&self, expr: &Expr) -> Owner {
        let name = match expr {
            Expr::Ident(name, _) => name.clone(),
            _ => String::new(),
        };
        Owner { id: self.get(expr).map(|binding| binding.id), name }
    }

    fn binding_by_id(&self, id: Option<LocalId>) -> Option<&BindingState> {
        let id = id?;
        self.scopes.iter().rev().find_map(|scope| scope.get(&id))
    }

    fn binding_by_id_mut(&mut self, id: Option<LocalId>) -> Option<&mut BindingState> {
        let id = id?;
        self.scopes.iter_mut().rev().find_map(|scope| scope.get_mut(&id))
    }

    fn get(&self, expr: &Expr) -> Option<&BindingState> {
        self.binding_by_id(self.id(expr))
    }

    fn get_mut(&mut self, expr: &Expr) -> Option<&mut BindingState> {
        self.binding_by_id_mut(self.id(expr))
    }
}

fn check_function(function: &FnDecl, self_ty: Option<&str>, program: &Program, reporter: &mut Reporter) {
    let mut copy_names = program.copy_types.clone();
    copy_names.extend(ast::bounded_by(&function.generics, "Copy"));
    if let Some(owner) = self_ty {
        copy_names.extend(program.copy_params.get(owner).into_iter().flatten().cloned());
    }
    let mut state = OwnershipState::new(program.locals.clone(), Rc::new(copy_names));
    state.push_scope();
    for param in &function.params {
        if let Pat::Ident(..) = &param.pattern {
            let ty = resolve_self_ty(&param.ty, self_ty);
            state.define(&param.pattern, ty_is_copy(&ty, &state.copy_names), Some(ty));
        }
    }
    let reported = check_lifetime_signature(function, program, reporter);
    if function.return_ty.is_some() && !reported {
        state.return_body = Some(&function.body as *const Block);
    }
    check_block(&function.body, program, &mut state, reporter);
}

fn check_lifetime_signature(function: &FnDecl, program: &Program, reporter: &mut Reporter) -> bool {
    let Some(Ty::Borrow { lifetime, span, .. }) = &function.return_ty else {
        return false;
    };
    let local_owner_types = owned_param_types(function, program);
    let local_owners: HashMap<LocalId, String> = function
        .params
        .iter()
        .filter_map(|param| match &param.pattern {
            Pat::Ident(name, _) => Some((program.locals.pat(&param.pattern)?, name.clone())),
            _ => None,
        })
        .filter(|(id, _)| local_owner_types.contains_key(id))
        .collect();
    let escape = EscapeScope { names: local_owners, types: local_owner_types, origins: HashMap::new() };
    if let Some(owner) = returned_local_borrow_owner(&function.body, program, &escape) {
        reporter.push(Diagnostic::error(
            "lifetime-error",
            *span,
            borrow_outlives_owner_message(&owner.name),
        ));
        return true;
    }
    if lifetime.is_some() {
        return false;
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
    false
}

fn owned_param_types(function: &FnDecl, program: &Program) -> HashMap<LocalId, Ty> {
    function
        .params
        .iter()
        .filter(|param| !matches!(param.ty, Ty::Borrow { .. }))
        .filter_map(|param| match &param.pattern {
            Pat::Ident(..) => Some((program.locals.pat(&param.pattern)?, param.ty.clone())),
            _ => None,
        })
        .collect()
}

/// What the syntactic returned-borrow analysis knows about the locals in
/// scope: which ones own their value, their types, and which owner a
/// reference-holding local borrows from.
#[derive(Clone, Default)]
struct EscapeScope {
    names: HashMap<LocalId, String>,
    types: HashMap<LocalId, Ty>,
    origins: HashMap<LocalId, Owner>,
}

fn returned_local_borrow_owner(block: &Block, program: &Program, outer: &EscapeScope) -> Option<Owner> {
    let mut scope = outer.clone();
    for statement in &block.stmts {
        let Stmt::Let(statement) = statement else {
            continue;
        };
        let owner = statement
            .value
            .as_ref()
            .and_then(|value| local_borrow_owner_from_expr(value, &scope, program));
        let ty = local_binding_ty(statement, &scope, program);
        let mut ids = HashSet::new();
        collect_pattern_ids(&statement.pattern, &program.locals, &mut ids);
        for id in &ids {
            scope.types.remove(id);
            scope.origins.remove(id);
        }
        let (Pat::Ident(name, _), Some(id)) = (&statement.pattern, program.locals.pat(&statement.pattern)) else {
            continue;
        };
        scope.names.insert(id, name.clone());
        if let Some(ty) = ty {
            scope.types.insert(id, ty);
        }
        if let Some(owner) = owner {
            scope.origins.insert(id, owner);
        }
    }
    block.tail.as_deref().and_then(|tail| local_borrow_owner_from_expr(tail, &scope, program))
}

fn local_binding_ty(statement: &LetStmt, scope: &EscapeScope, program: &Program) -> Option<Ty> {
    statement
        .ty
        .clone()
        .or_else(|| escape_expr_ty(statement.value.as_ref()?, scope, program))
}

fn escape_expr_ty(expr: &Expr, scope: &EscapeScope, program: &Program) -> Option<Ty> {
    match expr {
        Expr::Ident(..) => scope.types.get(&program.locals.expr(expr)?).cloned(),
        Expr::StructLiteral { ty, .. } => Some(ty.clone()),
        Expr::Borrow {
            mutable,
            expr,
            span,
        } => Some(Ty::Borrow {
            mutable: *mutable,
            lifetime: None,
            ty: Box::new(escape_expr_ty(expr, scope, program)?),
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
            let receiver_ty = receiver_type_name_from_expr(receiver, scope, program)?;
            program
                .methods
                .get(&(receiver_ty, method.clone()))
                .and_then(|signature| signature.return_ty.clone())
        }
        Expr::Block(block) => block
            .tail
            .as_deref()
            .and_then(|tail| escape_expr_ty(tail, scope, program)),
        _ => None,
    }
}

fn receiver_type_name_from_expr(receiver: &Expr, scope: &EscapeScope, program: &Program) -> Option<String> {
    if let Expr::Ident(..) = receiver {
        return scope.types.get(&program.locals.expr(receiver)?).and_then(method_receiver_type_name);
    }
    let receiver_ty = escape_expr_ty(receiver, scope, program)?;
    method_receiver_type_name(&receiver_ty)
}

fn local_borrow_owner_from_expr(expr: &Expr, scope: &EscapeScope, program: &Program) -> Option<Owner> {
    match expr {
        Expr::Borrow { expr, .. } => local_borrow_owner_from_place(expr, scope, program)
            .or_else(|| escape_expr_ty(expr, scope, program).map(|_| Owner::temporary())),
        Expr::Ident(..) => scope.origins.get(&program.locals.expr(expr)?).cloned(),
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
            local_borrow_owner_from_call_args(&signature.params, args, return_lifetime.as_ref(), scope, program)
        }
        Expr::MethodCall {
            receiver,
            method,
            args,
            ..
        } => local_borrow_owner_from_method_call(receiver, method, args, scope, program),
        Expr::Block(block) => returned_local_borrow_owner(block, program, scope),
        Expr::If {
            then_branch,
            else_branch,
            ..
        } => returned_local_borrow_owner(then_branch, program, scope).or_else(|| {
            else_branch
                .as_deref()
                .and_then(|expr| local_borrow_owner_from_expr(expr, scope, program))
        }),
        Expr::Match { arms, .. } => arms
            .iter()
            .find_map(|arm| local_borrow_owner_from_expr(&arm.body, scope, program)),
        _ => None,
    }
}

fn local_borrow_owner_from_call_args(
    params: &[Ty],
    args: &[Expr],
    return_lifetime: Option<&String>,
    scope: &EscapeScope,
    program: &Program,
) -> Option<Owner> {
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
        if let Some(origin) = local_borrow_owner_from_expr(arg, scope, program) {
            origins.push(origin);
        }
    }
    if return_lifetime.is_some() || origins.len() == 1 {
        origins.into_iter().next()
    } else {
        None
    }
}

fn local_borrow_owner_from_method_call(
    receiver: &Expr,
    method: &str,
    args: &[Expr],
    scope: &EscapeScope,
    program: &Program,
) -> Option<Owner> {
    let signature = local_method_signature(receiver, method, scope, program)?;
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
            local_borrow_owner_from_receiver(receiver, scope, program)
        } else {
            args.get(index - 1)
                .and_then(|arg| local_borrow_owner_from_expr(arg, scope, program))
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

fn local_borrow_owner_from_receiver(receiver: &Expr, scope: &EscapeScope, program: &Program) -> Option<Owner> {
    local_borrow_owner_from_place(receiver, scope, program).or_else(|| {
        receiver_type_name_for_escape(receiver, scope, program).map(|_| Owner::temporary())
    })
}

fn local_method_signature<'a>(
    receiver: &Expr,
    method: &str,
    scope: &EscapeScope,
    program: &'a Program,
) -> Option<&'a FunctionSignature> {
    let receiver_ty = receiver_type_name_for_escape(receiver, scope, program)?;
    program.methods.get(&(receiver_ty, method.to_string()))
}

fn receiver_type_name_for_escape(receiver: &Expr, scope: &EscapeScope, program: &Program) -> Option<String> {
    if let Some(id) = program.locals.expr(receiver)
        && let Some(receiver_ty) = scope
            .types
            .get(&id)
            .or_else(|| {
                scope
                    .origins
                    .get(&id)
                    .and_then(|origin| scope.types.get(&origin.id?))
            })
            .and_then(method_receiver_type_name)
    {
        return Some(receiver_ty);
    }
    receiver_type_name_from_expr(receiver, scope, program)
}

fn method_receiver_type_name(ty: &Ty) -> Option<String> {
    match ty {
        Ty::Borrow { ty, .. } => method_receiver_type_name(ty),
        _ => type_name(ty),
    }
}

fn local_borrow_owner_from_place(expr: &Expr, scope: &EscapeScope, program: &Program) -> Option<Owner> {
    match expr {
        Expr::Ident(..) => {
            let id = program.locals.expr(expr)?;
            match scope.names.get(&id) {
                Some(name) => Some(Owner { id: Some(id), name: name.clone() }),
                None => scope.origins.get(&id).cloned(),
            }
        }
        Expr::Field { base, .. } => local_borrow_owner_from_place(base, scope, program),
        _ => None,
    }
}

fn check_block_borrow_escape(expr: &Expr, program: &Program, reporter: &mut Reporter) -> bool {
    let Expr::Block(block) = expr else {
        return false;
    };
    if let Some(owner) = returned_local_borrow_owner(block, program, &EscapeScope::default()) {
        reporter.push(Diagnostic::error(
            "lifetime-error",
            block.span,
            borrow_outlives_owner_message(&owner.name),
        ));
        return true;
    }
    false
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
        if origin_is_local(&origin, state) {
            reporter.push(Diagnostic::error(
                "lifetime-error",
                expr_span(value),
                borrow_outlives_owner_message(&origin.owner.name),
            ));
            return;
        }
    }
}

fn binding_is_reference(binding: &BindingState) -> bool {
    binding.borrows.is_empty() && matches!(binding.ty, Some(Ty::Borrow { .. }))
}

fn origin_is_local(origin: &BorrowOrigin, state: &OwnershipState) -> bool {
    origin.local_escape
        || origin.owner.is_temporary()
        || state.binding_by_id(origin.owner.id).is_some_and(|binding| !binding_is_reference(binding))
}

fn check_scope_end_escapes(scope_span: Span, state: &OwnershipState, reporter: &mut Reporter) {
    let Some((dying_scope, outer_scopes)) = state.scopes.split_last() else {
        return;
    };
    let dying: HashSet<LocalId> = dying_scope.keys().copied().collect();
    if dying.is_empty() {
        return;
    }
    let depth = outer_scopes.len();
    let loop_boundary = state.loop_boundaries.last().copied().filter(|boundary| *boundary <= depth);
    let enclosing_uses = &state.last_uses[..state.last_uses.len().saturating_sub(1)];
    for (index, scope) in outer_scopes.iter().enumerate() {
        let mut holders: Vec<_> = scope.values().collect();
        holders.sort_by(|left, right| left.name.cmp(&right.name));
        for binding in holders {
            let holder = &binding.name;
            let Some(borrow) = binding.borrows.iter().find(|borrow| borrow.owner.id.is_some_and(|id| dying.contains(&id))) else {
                continue;
            };
            let used_after = enclosing_uses
                .iter()
                .filter_map(|uses| uses.get(&binding.id))
                .any(|position| *position > scope_span.end());
            let outlives_iteration = loop_boundary.is_some_and(|boundary| index < boundary);
            if used_after || outlives_iteration {
                reporter.push(Diagnostic::error(
                    "lifetime-error",
                    scope_span,
                    format!(
                        "borrow of local `{}` is stored in `{holder}`, which outlives it; `{}` is dropped at the end of this scope",
                        borrow.owner.name, borrow.owner.name
                    ),
                ));
            }
        }
    }
}

fn place_root(expr: &Expr) -> Option<&Expr> {
    match expr {
        Expr::Ident(..) => Some(expr),
        Expr::Field { base, .. } | Expr::Index { base, .. } => place_root(base),
        Expr::Unary { op: paco_syntax::ast::UnaryOp::Deref, expr, .. } => place_root(expr),
        _ => None,
    }
}

fn store_borrows(
    place: &Expr,
    origins: &[BorrowOrigin],
    attach_to_root: bool,
    span: Span,
    state: &mut OwnershipState,
    reporter: &mut Reporter,
) {
    if origins.is_empty() {
        return;
    }
    let Some(root) = place_root(place) else {
        return;
    };
    let Some(root_binding) = state.get(root) else {
        return;
    };
    let root_id = root_binding.id;
    let holders: Vec<Owner> = if root_binding.borrows.is_empty() {
        vec![state.owner(root)]
    } else {
        let mut owners: Vec<Owner> = root_binding.borrows.iter().map(|borrow| borrow.owner.clone()).collect();
        owners.dedup();
        owners
    };
    for holder in holders {
        if holder.is_temporary() {
            continue;
        }
        let external = state.binding_by_id(holder.id).is_none_or(binding_is_reference);
        for origin in origins {
            let escapes = if external {
                origin_is_local(origin, state)
            } else {
                origin.local_escape || origin.owner.is_temporary()
            };
            if escapes {
                let holder = &holder.name;
                reporter.push(Diagnostic::error(
                    "lifetime-error",
                    span,
                    if origin.owner.is_temporary() {
                        format!("borrow of temporary value is stored in `{holder}`, which outlives it")
                    } else {
                        format!("borrow of local `{}` is stored in `{holder}`, which outlives it", origin.owner.name)
                    },
                ));
                return;
            }
        }
        if external || (holder.id == Some(root_id) && !attach_to_root) {
            continue;
        }
        let last_use = state
            .last_uses
            .iter()
            .filter_map(|uses| uses.get(&holder.id?).copied())
            .max()
            .unwrap_or(0);
        if let Some(binding) = state.binding_by_id_mut(holder.id) {
            binding.borrows.extend(origins.iter().map(|origin| BorrowBinding {
                owner: origin.owner.clone(),
                mutable: origin.mutable,
                last_use,
                local_escape: false,
                field_path: None,
            }));
        }
    }
}

fn ty_may_hold_borrow(ty: &Ty, program: &Program) -> bool {
    fn walk(ty: &Ty, own_params: &[String], program: &Program, seen: &mut HashSet<String>) -> bool {
        match ty {
            Ty::Borrow { .. } | Ty::Fn { .. } => true,
            Ty::Tuple(items, _) => items.iter().any(|item| walk(item, own_params, program, seen)),
            Ty::Slice(item, _) => walk(item, own_params, program, seen),
            Ty::Generic { path, args, .. } => {
                args.iter().any(|arg| walk(arg, own_params, program, seen)) || named(path, program, seen)
            }
            Ty::Path(path, _) => {
                !(path.len() == 1 && own_params.contains(&path[0])) && named(path, program, seen)
            }
            _ => false,
        }
    }
    fn named(path: &[String], program: &Program, seen: &mut HashSet<String>) -> bool {
        let name = path.join("::");
        let Some(params) = program.type_params.get(&name) else {
            return false;
        };
        if !seen.insert(name.clone()) {
            return false;
        }
        program
            .fields
            .iter()
            .filter(|((owner, _), _)| *owner == name)
            .map(|(_, ty)| ty)
            .chain(program.variants.iter().filter(|((owner, _), _)| *owner == name).flat_map(|(_, tys)| tys))
            .any(|ty| walk(ty, params, program, seen))
    }
    walk(ty, &[], program, &mut HashSet::new())
}

fn ty_may_hold_borrow_or_generic(ty: &Ty, program: &Program) -> bool {
    let generic = matches!(ty, Ty::Path(path, _) if path.len() == 1
        && !program.type_params.contains_key(&path[0])
        && path[0].starts_with(char::is_uppercase));
    generic || ty_may_hold_borrow(ty, program)
}

fn is_variant_constructor(name: &str, program: &Program) -> bool {
    !program.functions.contains_key(name) && program.variants.keys().any(|(_, variant)| variant == name)
}

fn bind_pattern_origins(pattern: &Pat, origins: &[BorrowOrigin], program: &Program, state: &mut OwnershipState) {
    if origins.is_empty() {
        return;
    }
    let mut ids = HashSet::new();
    collect_pattern_ids(pattern, &program.locals, &mut ids);
    for id in ids {
        if let Some(binding) = state.binding_by_id_mut(Some(id))
            && binding.ty.as_ref().is_none_or(|ty| ty_may_hold_borrow_or_generic(ty, program))
        {
            binding.borrows = origins
                .iter()
                .map(|origin| BorrowBinding {
                    owner: origin.owner.clone(),
                    mutable: origin.mutable,
                    last_use: 0,
                    local_escape: origin.local_escape,
                    field_path: None,
                })
                .collect();
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
    let last_uses = block_last_uses(block, &program.locals);
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
                let typed = statement.ty.is_none().then(|| statement.value.as_ref().and_then(|value| typed_copy(value, program, state))).flatten();
                let copy = typed.unwrap_or_else(|| {
                    binding_ty.as_ref().map_or_else(
                        || statement.value.as_ref().is_some_and(|expr| expr_is_copy(expr, program, state)),
                        |ty| ty_is_copy(ty, &state.copy_names),
                    )
                });
                let mut escape_reported = false;
                if let Some(value) = &statement.value {
                    if copy {
                        check_expr(value, program, state, reporter);
                    } else {
                        consume_expr(value, program, state, reporter);
                    }
                    escape_reported = check_block_borrow_escape(value, program, reporter);
                }
                let mut borrows = borrow_bindings(
                    statement.value.as_ref(),
                    &statement.pattern,
                    &last_uses,
                    program,
                    state,
                );
                if escape_reported {
                    borrows.retain(|borrow| !borrow.local_escape && !borrow.owner.is_temporary());
                }
                define_let_pattern(
                    &statement.pattern,
                    matches!(statement.value, Some(Expr::Borrow { .. })),
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
        if state.return_body == Some(block as *const Block) {
            check_return_borrow_escape(tail, program, state, reporter);
        }
        check_expr(tail, program, state, reporter);
    }
    if state.reachable {
        check_scope_end_escapes(block.span, state, reporter);
        let drops = droppable_locals_from_scope(state.scopes.last().unwrap(), state.scope_order.last().unwrap());
        if !drops.is_empty() {
            program
                .drops
                .borrow_mut()
                .insert(ExitPoint::BlockEnd(block as *const Block), drops);
        }
    }
    state.last_uses.pop();
    state.pop_scope();
}

fn check_expr(expr: &Expr, program: &Program, state: &mut OwnershipState, reporter: &mut Reporter) {
    match expr {
        Expr::Literal(_, _) => {}
        Expr::Ident(..) => check_ident_use(expr, state, reporter),
        Expr::Block(block) => check_block(block, program, state, reporter),
        Expr::Unsafe(block, _) => check_block(block, program, state, reporter),
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
        Expr::Call { callee, args, .. } => {
            check_call(callee, args, program, state, reporter);
            if matches!(callee.as_ref(), Expr::Ident(name, _) if name == "panic" && !program.functions.contains_key(name)) {
                state.reachable = false;
            }
        }
        Expr::MethodCall {
            receiver,
            method,
            args,
            ..
        } => check_method_call(receiver, method, args, program, state, reporter),
        Expr::AssociatedCall { ty, function, args, .. } => {
            let temporaries =
                check_argument_temporary_borrow_conflicts(args, program, state, reporter);
            if type_name(ty).is_some_and(|name| program.test_asserts.contains(&format!("{name}::{function}"))) {
                for arg in args {
                    check_expr_with_temporary_borrows(arg, &temporaries, program, state, reporter);
                }
                return;
            }
            let params = associated_function_params(ty, function, program);
            for (index, arg) in args.iter().enumerate() {
                if params.and_then(|params| params.get(index)).is_some_and(|ty| ty_is_copy(ty, &state.copy_names)) {
                    check_expr_with_temporary_borrows(arg, &temporaries, program, state, reporter);
                } else {
                    consume_expr_with_temporary_borrows(arg, &temporaries, program, state, reporter);
                }
            }
        }
        Expr::Binary { op, left, right, .. }
            if let Some(method) = operator_method(*op).or_else(|| ordering_method(*op))
                && method_signature(left, method, program, state).is_some() =>
        {
            check_method_call(left, method, std::slice::from_ref(right.as_ref()), program, state, reporter);
        }
        Expr::Binary { left, right, .. } => {
            check_expr(left, program, state, reporter);
            check_expr(right, program, state, reporter);
        }
        Expr::Unary { expr, .. } => check_expr(expr, program, state, reporter),
        Expr::Cast { expr, .. } => check_expr(expr, program, state, reporter),
        Expr::Assign {
            target,
            value,
            span,
        } => {
            let assigned_borrows = target_identifier(target).map(|target| {
                borrow_bindings_for_name(value, current_last_use(target, state), program, state)
            });
            if !matches!(target.as_ref(), Expr::Ident(..)) {
                let origins = borrow_origins_from_expr(value, program, state);
                let attach_to_root = !matches!(target.as_ref(), Expr::Field { .. });
                store_borrows(target, &origins, attach_to_root, *span, state, reporter);
            }
            consume_expr(value, program, state, reporter);
            match target.as_ref() {
                Expr::Ident(_, span) => {
                    check_assignment_while_borrowed(target, *span, state, reporter);
                    if let Some(binding) = state.get_mut(target) {
                        binding.moved_at = None;
                        binding.borrows = assigned_borrows.unwrap_or_default();
                    }
                }
                Expr::Field { .. } => {
                    if let Some(root) = root_place_name(target) {
                        check_assignment_while_borrowed(root, *span, state, reporter);
                        if let Some(field_path) = place_field_path(target) {
                            let last_use = current_last_use(root, state);
                            let field_borrows =
                                borrow_bindings_for_name(value, last_use, program, state)
                                    .into_iter()
                                    .map(|borrow| prefix_borrow_field_path(borrow, &field_path))
                                    .collect::<Vec<_>>();
                            if let Some(binding) = state.get_mut(root) {
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
            for index_expr in index {
                check_expr(index_expr, program, state, reporter);
            }
        }
        Expr::Return(value, _) => {
            if let Some(value) = value {
                check_return_borrow_escape(value, program, state, reporter);
                consume_expr(value, program, state, reporter);
            }
            record_drops_at_control_transfer(expr, program, state, 0);
            state.reachable = false;
        }
        Expr::Break(value, _) => {
            if let Some(value) = value {
                consume_expr(value, program, state, reporter);
            }
            let boundary = state.loop_boundaries.last().copied().unwrap_or(0);
            record_drops_at_control_transfer(expr, program, state, boundary);
            state.reachable = false;
        }
        Expr::Continue(_) => {
            let boundary = state.loop_boundaries.last().copied().unwrap_or(0);
            record_drops_at_control_transfer(expr, program, state, boundary);
            state.reachable = false;
        }
        Expr::Spawn { expr, span } => {
            check_shared_without_sync_capture(expr, *span, state, reporter);
            consume_expr(expr, program, state, reporter);
            let mut uses = HashMap::new();
            collect_expr_uses(expr, &program.locals, &mut uses);
            let mut captures: Vec<LocalId> = uses
                .into_keys()
                .filter(|id| state.binding_by_id(Some(*id)).is_some_and(|binding| !binding.copy && binding.moved_at.is_none()))
                .collect();
            captures.sort_by_key(|id| state.binding_by_id(Some(*id)).map(|binding| binding.name.clone()));
            for id in captures {
                consume_local(id, *span, state, reporter);
            }
        }
        Expr::Closure { params, body, span } => check_closure(params, body, *span, program, state, reporter),
        Expr::Comptime { expr, .. } => {
            check_expr(expr, program, state, reporter);
        }
        Expr::Splice(expr, _) => {
            check_expr(expr, program, state, reporter);
        }
        // `quote { .. }`'s own template (`phase-9-comptime` Decision 7) is
        // not ordinary code — only each splice point's own inner
        // expression is a real reference to the enclosing scope, same
        // reasoning as `paco-resolve`'s/`paco-types`' own quote handling.
        Expr::Quote(body, _) => check_quote_splices(body, program, state, reporter),
        Expr::Borrow {
            mutable,
            expr,
            span,
        } => {
            for borrow in temporary_borrow_bindings(*mutable, expr, state.current_statement, state)
            {
                check_borrow_conflict(*span, &borrow, state, reporter);
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
        Expr::Try {
            expr: inner_expr, ..
        } => {
            consume_expr(inner_expr, program, state, reporter);
            record_drops_at_control_transfer(expr, program, state, 0);
        }
        Expr::StructLiteral { fields, .. } => {
            for (_, value) in fields {
                consume_expr(value, program, state, reporter);
            }
        }
        Expr::Tuple(items, _) => {
            for item in items {
                consume_expr(item, program, state, reporter);
            }
        }
    }
}

/// The owned, droppable (non-`Copy`, not-moved) locals of `scope`, in
/// reverse declaration order per `order`.
fn droppable_locals_from_scope(scope: &HashMap<LocalId, BindingState>, order: &[LocalId]) -> Vec<LocalId> {
    order
        .iter()
        .rev()
        .filter(|id| scope.get(id).is_some_and(|binding| !binding.copy && binding.moved_at.is_none()))
        .copied()
        .collect()
}

/// The owned, droppable locals live across every scope from `boundary`
/// (inclusive) to the top of the stack, innermost scope first, each scope's
/// own locals in reverse declaration order. Used for `return` (`boundary =
/// 0`, the whole function) and `break`/`continue` (`boundary` = the
/// enclosing loop's entry depth).
fn droppable_locals_up_to(state: &OwnershipState, boundary: usize) -> Vec<LocalId> {
    let mut result = Vec::new();
    for index in (boundary..state.scopes.len()).rev() {
        result.extend(droppable_locals_from_scope(
            &state.scopes[index],
            &state.scope_order[index],
        ));
    }
    result
}

fn record_drops_at_control_transfer(
    expr: &Expr,
    program: &Program,
    state: &OwnershipState,
    boundary: usize,
) {
    let drops = droppable_locals_up_to(state, boundary);
    if !drops.is_empty() {
        program
            .drops
            .borrow_mut()
            .insert(ExitPoint::Control(expr as *const Expr), drops);
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
    body_state.loop_boundaries.push(body_state.scopes.len());
    check_block(body, program, &mut body_state, reporter);
    body_state.loop_boundaries.pop();
    for scope_index in 0..before.scopes.len() {
        for (id, binding) in &before.scopes[scope_index] {
            let move_count_before = binding.move_count;
            let after = body_state.scopes.get(scope_index).and_then(|scope| scope.get(id));
            let move_count_after = after.map_or(move_count_before, |binding| binding.move_count);
            let moved_at_end = after.is_none_or(|binding| binding.moved_at.is_some());
            if move_count_after > move_count_before && moved_at_end {
                reporter.push(Diagnostic::error(
                    "use-after-move",
                    span,
                    format!("loop body may move `{}` on more than one iteration", binding.name),
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
    if state.get(callee).is_some() {
        check_ident_use(callee, state, reporter);
    } else if name == "spawn_blocking"
        && let [Expr::Closure { body, span, .. }] = args
    {
        check_shared_without_sync_capture(body, *span, state, reporter);
    }
    let temporaries = check_argument_temporary_borrow_conflicts(args, program, state, reporter);
    if name == "print" || program.test_asserts.contains(name) {
        for arg in args {
            check_expr_with_temporary_borrows(arg, &temporaries, program, state, reporter);
        }
        return;
    }
    let signature = program.functions.get(name);
    for (index, arg) in args.iter().enumerate() {
        if signature
            .and_then(|signature| signature.params.get(index))
            .is_some_and(|ty| ty_is_copy(ty, &state.copy_names))
        {
            check_expr_with_temporary_borrows(arg, &temporaries, program, state, reporter);
        } else {
            consume_expr_with_temporary_borrows(arg, &temporaries, program, state, reporter);
        }
    }
}

fn collect_pattern_ids(pattern: &Pat, locals: &Locals, ids: &mut HashSet<LocalId>) {
    match pattern {
        Pat::Ident(..) => ids.extend(locals.pat(pattern)),
        Pat::Binding { pattern: inner, .. } => {
            ids.extend(locals.pat(pattern));
            collect_pattern_ids(inner, locals, ids);
        }
        Pat::Tuple(items, _) | Pat::Enum { fields: items, .. } | Pat::Or(items, _) => {
            items.iter().for_each(|item| collect_pattern_ids(item, locals, ids));
        }
        Pat::Struct { fields, .. } => fields.iter().for_each(|(_, item)| collect_pattern_ids(item, locals, ids)),
        Pat::Range { .. } | Pat::Wildcard(_) | Pat::Literal(..) => {}
    }
}

/// The outer locals a closure body refers to, ordered by name.
fn closure_captures(body: &Expr, state: &OwnershipState) -> Vec<LocalId> {
    let mut uses = HashMap::new();
    collect_expr_uses(body, &state.locals, &mut uses);
    let mut captures: Vec<&BindingState> = uses.into_keys().filter_map(|id| state.binding_by_id(Some(id))).collect();
    captures.sort_by(|left, right| left.name.cmp(&right.name));
    captures.into_iter().map(|binding| binding.id).collect()
}

fn check_closure(
    params: &[ClosureParam],
    body: &Expr,
    span: Span,
    program: &Program,
    state: &mut OwnershipState,
    reporter: &mut Reporter,
) {
    let mut inner = state.clone();
    inner.push_scope();
    for param in params {
        bind_pattern(&param.pattern, param.ty.as_ref().is_none_or(|ty| ty_is_copy(ty, &state.copy_names)), param.ty.clone(), program, &mut inner);
    }
    inner.reachable = true;
    check_expr(body, program, &mut inner, reporter);
    for id in closure_captures(body, state) {
        consume_local(id, span, state, reporter);
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
    if method == "send"
        && receiver_ty.as_ref().and_then(type_name).as_deref() == Some("Sender")
        && let Some(value_arg) = args.first()
        && matches!(expr_ty(value_arg, program, state), Some(Ty::Borrow { .. }))
    {
        reporter.push(Diagnostic::error(
            "shared-without-sync",
            expr_span(value_arg),
            "cannot send a bare reference over a channel; wrap it in `Arc` to share it with another task",
        ));
    }
    let signature = method_signature(receiver, method, program, state);
    let receiver_is_copy = typed_copy(receiver, program, state)
        .unwrap_or_else(|| receiver_ty.as_ref().is_some_and(|ty| ty_is_copy(ty, &state.copy_names)));
    let slice_len = method == "len"
        && args.is_empty()
        && matches!(
            receiver_ty.as_ref().map(|ty| match ty {
                Ty::Borrow { ty, .. } => ty.as_ref(),
                other => other,
            }),
            Some(Ty::Slice(..))
        );
    let receiver_is_borrowed = slice_len
        || signature
            .and_then(|signature| signature.params.first())
            .is_some_and(|ty| matches!(ty, Ty::Borrow { .. }));
    let receiver_borrows = signature
        .and_then(|signature| signature.params.first())
        .and_then(|ty| match ty {
            Ty::Borrow { mutable, .. } => Some(*mutable),
            _ => None,
        })
        .map(|mutable| {
            borrow_owners(receiver, mutable, state)
                .into_iter()
                .map(|(owner, mutable)| BorrowBinding {
                    owner,
                    mutable,
                    last_use: state.current_statement,
                    local_escape: false,
                    field_path: None,
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let reborrows_through_binding = root_place_name(receiver)
        .and_then(|root| state.get(root))
        .is_some_and(|binding| !binding.borrows.is_empty());
    for receiver_borrow in receiver_borrows.iter().filter(|_| !reborrows_through_binding) {
        check_borrow_conflict(
            expr_span(receiver),
            receiver_borrow,
            state,
            reporter,
        );
    }
    let stored_origins: Vec<BorrowOrigin> = signature
        .filter(|signature| matches!(signature.params.first(), Some(Ty::Borrow { mutable: true, .. })))
        .map(|signature| {
            signature
                .params
                .iter()
                .skip(1)
                .zip(args)
                .filter(|(param, _)| {
                    !matches!(param, Ty::Borrow { .. })
                        && ty_may_hold_borrow_or_generic(param, program)
                })
                .flat_map(|(_, arg)| borrow_origins_from_expr(arg, program, state))
                .collect()
        })
        .unwrap_or_default();
    store_borrows(receiver, &stored_origins, true, expr_span(receiver), state, reporter);
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
        if typed_copy(arg, program, state) == Some(true)
            || signature
                .and_then(|signature| signature.params.get(index + 1))
                .is_some_and(|ty| ty_is_copy(ty, &state.copy_names))
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
                        borrow.owner.name
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
    let current = if borrows.is_empty() { Vec::new() } else { temporary_borrows_for_expr(expr, program, state) };
    let original_len = push_temporary_borrows_except(state, borrows, &current);
    consume_expr(expr, program, state, reporter);
    state.temporary_borrows.truncate(original_len);
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
        | Expr::Quote(_, span)
        | Expr::Splice(_, span)
        | Expr::Yield(_, span)
        | Expr::Tuple(_, span) => *span,
        Expr::Block(block) => block.span,
        Expr::Unsafe(_, span) => *span,
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
        | Expr::Closure { span, .. }
        | Expr::Select { span, .. }
        | Expr::Comptime { span, .. }
        | Expr::StructLiteral { span, .. }
        | Expr::Borrow { span, .. }
        | Expr::Try { span, .. }
        | Expr::Cast { span, .. } => *span,
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
    let scrutinee_origins = borrow_origins_from_expr(scrutinee, program, state);
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
        bind_pattern_origins(&arm.pattern, &scrutinee_origins, program, &mut arm_state);
        if let Some(guard) = &arm.guard {
            check_expr(guard, program, &mut arm_state, reporter);
        }
        check_expr(&arm.body, program, &mut arm_state, reporter);
        if arm_state.reachable {
            check_scope_end_escapes(expr_span(&arm.body), &arm_state, reporter);
        }
        arm_state.pop_scope();
        branch_states.push(arm_state);
    }
    if !branch_states.is_empty() {
        merge_many_branch_states(span, &before, &branch_states, state, reporter);
    }
}

/// Borrow-checks a `quote { .. }` template's splice points only
/// (`phase-9-comptime` Decision 7) — mirrors `paco-resolve`'s/
/// `paco-types`' own quote handling: the template's own literal
/// structure is not real code yet, only each splice's own inner
/// expression is.
fn check_quote_splices(body: &QuoteBody, program: &Program, state: &mut OwnershipState, reporter: &mut Reporter) {
    struct SpliceChecker<'p, 's, 'r> {
        program: &'p Program,
        state: &'s mut OwnershipState,
        reporter: &'r mut Reporter,
    }
    impl Visit for SpliceChecker<'_, '_, '_> {
        fn visit_expr(&mut self, expr: &Expr) {
            if let Expr::Splice(inner, _) = expr {
                check_expr(inner, self.program, self.state, self.reporter);
                return;
            }
            walk_expr(self, expr);
        }

        fn visit_item(&mut self, item: &Item) {
            if let Item::Methods(block) = item
                && let Ty::Splice(inner, _) = &block.target
            {
                check_expr(inner, self.program, self.state, self.reporter);
            }
            ast::walk_item(self, item);
        }
    }
    let mut checker = SpliceChecker { program, state, reporter };
    match body {
        QuoteBody::Item(item) => checker.visit_item(item),
        QuoteBody::Expr(expr) => checker.visit_expr(expr),
    }
    paco_syntax::ast::visit_template_ty_splices(body, &mut |splice| check_expr(splice, program, state, reporter));
}

fn consume_expr(
    expr: &Expr,
    program: &Program,
    state: &mut OwnershipState,
    reporter: &mut Reporter,
) {
    match expr {
        Expr::Ident(_, span) => {
            if let Some(id) = state.id(expr) {
                consume_local(id, *span, state, reporter);
            }
        }
        Expr::Field { base, span, .. } => {
            if expr_ty(expr, program, state).as_ref().is_some_and(|ty| ty_is_copy(ty, &state.copy_names)) {
                check_expr(base, program, state, reporter);
                return;
            }
            // Moving a non-`Copy` field out of a plain local variable (not
            // behind a reference) is an ordinary partial move: this checker
            // has no per-field state, so it conservatively treats it as
            // moving the whole base binding — sound, since nothing later
            // can use `base` (a real remaining field) without this
            // checker's flat, whole-binding move tracking flagging it.
            if matches!(base.as_ref(), Expr::Ident(..))
                && !expr_ty(base, program, state)
                    .as_ref()
                    .is_some_and(|ty| matches!(ty, Ty::Borrow { .. } | Ty::RawPointer { .. }))
            {
                consume_expr(base, program, state, reporter);
                return;
            }
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
    let last_uses = block_last_uses(block, &program.locals);
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
                let typed = statement.ty.is_none().then(|| statement.value.as_ref().and_then(|value| typed_copy(value, program, state))).flatten();
                let copy = typed.unwrap_or_else(|| {
                    binding_ty.as_ref().map_or_else(
                        || statement.value.as_ref().is_some_and(|expr| expr_is_copy(expr, program, state)),
                        |ty| ty_is_copy(ty, &state.copy_names),
                    )
                });
                let mut escape_reported = false;
                if let Some(value) = &statement.value {
                    if copy {
                        check_expr(value, program, state, reporter);
                    } else {
                        consume_expr(value, program, state, reporter);
                    }
                    escape_reported = check_block_borrow_escape(value, program, reporter);
                }
                let mut borrows = borrow_bindings(
                    statement.value.as_ref(),
                    &statement.pattern,
                    &last_uses,
                    program,
                    state,
                );
                if escape_reported {
                    borrows.retain(|borrow| !borrow.local_escape && !borrow.owner.is_temporary());
                }
                define_let_pattern(
                    &statement.pattern,
                    matches!(statement.value, Some(Expr::Borrow { .. })),
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
    if state.reachable {
        check_scope_end_escapes(block.span, state, reporter);
        let drops = droppable_locals_from_scope(state.scopes.last().unwrap(), state.scope_order.last().unwrap());
        if !drops.is_empty() {
            program
                .drops
                .borrow_mut()
                .insert(ExitPoint::BlockEnd(block as *const Block), drops);
        }
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
    let scrutinee_origins = borrow_origins_from_expr(scrutinee, program, state);
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
        bind_pattern_origins(&arm.pattern, &scrutinee_origins, program, &mut arm_state);
        if let Some(guard) = &arm.guard {
            check_expr(guard, program, &mut arm_state, reporter);
        }
        consume_expr(&arm.body, program, &mut arm_state, reporter);
        if arm_state.reachable {
            check_scope_end_escapes(expr_span(&arm.body), &arm_state, reporter);
        }
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

fn check_ident_use(expr: &Expr, state: &OwnershipState, reporter: &mut Reporter) {
    if let Some(binding) = state.get(expr) {
        check_local_use(binding, expr_span(expr), reporter);
    }
}

fn check_local_use(binding: &BindingState, span: Span, reporter: &mut Reporter) {
    if binding.moved_at.is_some() {
        reporter.push(Diagnostic::error(
            "use-after-move",
            span,
            format!("use of moved value `{}`", binding.name),
        ));
    }
}

/// Moves the value out of local `id` at `span`.
fn consume_local(id: LocalId, span: Span, state: &mut OwnershipState, reporter: &mut Reporter) {
    let Some(binding) = state.binding_by_id(Some(id)) else {
        return;
    };
    check_local_use(binding, span, reporter);
    let owner = Owner { id: Some(id), name: binding.name.clone() };
    if active_borrows_of(&owner, state).next().is_some() {
        reporter.push(Diagnostic::error(
            "borrow-conflict",
            span,
            format!("cannot move `{}` while it is borrowed", owner.name),
        ));
    }
    if let Some(binding) = state.binding_by_id_mut(Some(id))
        && !binding.copy
    {
        binding.moved_at = Some(span);
        binding.move_count += 1;
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
        for (name, before_binding) in &before.scopes[scope_index] {
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
                    format!("inconsistent move state for `{}` across branches", before_binding.name),
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
        Pat::Ident(..) => state.define(pattern, copy, ty),
        Pat::Binding { pattern: inner, .. } => {
            state.define(pattern, copy, ty.clone());
            bind_pattern(inner, copy, ty, program, state);
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
                    field_ty.as_ref().map_or(copy, |ty| ty_is_copy(ty, &state.copy_names)),
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
                    field_ty.as_ref().map_or(copy, |ty| ty_is_copy(ty, &state.copy_names)),
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
                    field_ty.as_ref().map_or(copy, |ty| ty_is_copy(ty, &state.copy_names)),
                    field_ty,
                    program,
                    state,
                );
            }
        }
        Pat::Range { .. } | Pat::Wildcard(_) | Pat::Literal(_, _) => {}
    }
}

#[allow(clippy::too_many_arguments)]
fn define_let_pattern(
    pattern: &Pat,
    borrow_checked_at_value: bool,
    copy: bool,
    ty: Option<Ty>,
    borrows: Vec<BorrowBinding>,
    program: &Program,
    state: &mut OwnershipState,
    reporter: &mut Reporter,
) {
    if let Pat::Ident(_, span) = pattern {
        for borrow in &borrows {
            if borrow.owner.is_temporary() || borrow.local_escape {
                reporter.push(Diagnostic::error(
                    "lifetime-error",
                    *span,
                    borrow_outlives_owner_message(&borrow.owner.name),
                ));
            } else if !borrow_checked_at_value {
                check_borrow_conflict(*span, borrow, state, reporter);
            }
        }
        state.define_with_borrows(pattern, copy, ty, borrows);
    } else {
        bind_destructuring_pattern(pattern, copy, ty, borrows, program, state);
    }
}

fn bind_destructuring_pattern(
    pattern: &Pat,
    copy: bool,
    ty: Option<Ty>,
    borrows: Vec<BorrowBinding>,
    program: &Program,
    state: &mut OwnershipState,
) {
    let origins: Vec<BorrowOrigin> = borrows
        .into_iter()
        .map(|borrow| BorrowOrigin {
            owner: borrow.owner,
            mutable: borrow.mutable,
            local_escape: borrow.local_escape,
            field_path: None,
        })
        .collect();
    bind_pattern(pattern, copy, ty, program, state);
    bind_pattern_origins(pattern, &origins, program, state);
}

fn borrow_bindings(
    value: Option<&Expr>,
    pattern: &Pat,
    last_uses: &HashMap<LocalId, usize>,
    program: &Program,
    state: &OwnershipState,
) -> Vec<BorrowBinding> {
    let Some(value) = value else {
        return Vec::new();
    };
    let last_use = match pattern {
        Pat::Ident(..) => program.locals.pat(pattern).and_then(|id| last_uses.get(&id).copied()).unwrap_or(0),
        _ => 0,
    };
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
    owner: Owner,
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
        Expr::Borrow { mutable, expr, .. } => borrow_owners_or_temporary(expr, *mutable, program, state)
            .into_iter()
            .map(|(owner, mutable)| BorrowOrigin {
                owner,
                mutable,
                local_escape: false,
                field_path: None,
            })
            .collect(),
        Expr::Ident(..) => state
            .get(expr)
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
            if is_variant_constructor(function_name, program) {
                return args.iter().flat_map(|arg| borrow_origins_from_expr(arg, program, state)).collect();
            }
            let Some(signature) = program.functions.get(function_name) else {
                return Vec::new();
            };
            let Some(Ty::Borrow {
                mutable,
                lifetime: return_lifetime,
                ..
            }) = &signature.return_ty
            else {
                return held_borrow_origins(expr, None, &signature.params, args, program, state);
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
                return held_borrow_origins(expr, Some(receiver), &signature.params, args, program, state);
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
        Expr::Tuple(items, _) => items.iter().flat_map(|item| borrow_origins_from_expr(item, program, state)).collect(),
        Expr::Closure { body, .. } => closure_captures(body, state)
            .into_iter()
            .filter_map(|id| state.binding_by_id(Some(id)))
            .flat_map(|binding| binding.borrows.iter())
            .map(|borrow| BorrowOrigin {
                owner: borrow.owner.clone(),
                mutable: borrow.mutable,
                local_escape: borrow.local_escape,
                field_path: borrow.field_path.clone(),
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
        Expr::AssociatedCall { ty, function, args, .. } => {
            let Some(name) = type_name(ty) else {
                return Vec::new();
            };
            if program.variants.contains_key(&(name.clone(), function.clone())) {
                return args.iter().flat_map(|arg| borrow_origins_from_expr(arg, program, state)).collect();
            }
            match program.methods.get(&(name, function.clone())) {
                Some(signature) if !matches!(signature.return_ty, Some(Ty::Borrow { .. })) => {
                    held_borrow_origins(expr, None, &signature.params, args, program, state)
                }
                _ => Vec::new(),
            }
        }
        Expr::Match { scrutinee, arms, .. } => {
            let scrutinee_origins = borrow_origins_from_expr(scrutinee, program, state);
            let scrutinee_ty = expr_ty(scrutinee, program, state);
            arms.iter()
                .flat_map(|arm| {
                    let mut arm_state = state.clone();
                    arm_state.push_scope();
                    bind_pattern(&arm.pattern, false, scrutinee_ty.clone(), program, &mut arm_state);
                    bind_pattern_origins(&arm.pattern, &scrutinee_origins, program, &mut arm_state);
                    borrow_origins_from_expr(&arm.body, program, &arm_state)
                })
                .collect()
        }
        _ => Vec::new(),
    }
}

fn held_borrow_origins(
    call: &Expr,
    receiver: Option<&Expr>,
    params: &[Ty],
    args: &[Expr],
    program: &Program,
    state: &OwnershipState,
) -> Vec<BorrowOrigin> {
    if !expr_ty(call, program, state).is_some_and(|ty| ty_may_hold_borrow(&ty, program)) {
        return Vec::new();
    }
    let mut origins = receiver.map_or_else(Vec::new, |receiver| borrow_origins_from_expr(receiver, program, state));
    let arg_params = if receiver.is_some() { params.get(1..).unwrap_or_default() } else { params };
    for (param, arg) in arg_params.iter().zip(args) {
        if receiver.is_some() && matches!(param, Ty::Borrow { .. }) {
            continue;
        }
        origins.extend(borrow_origins_from_expr(arg, program, state));
    }
    origins
}

fn borrow_origins_from_block(
    block: &Block,
    program: &Program,
    state: &OwnershipState,
) -> Vec<BorrowOrigin> {
    let mut local_state = state.clone();
    local_state.push_scope();
    let last_uses = block_last_uses(block, &program.locals);
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
            |ty| ty_is_copy(ty, &local_state.copy_names),
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
        if !origin.owner.is_temporary()
            && state.binding_by_id(origin.owner.id).is_none()
            && local_state.binding_by_id(origin.owner.id).is_some()
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
    if let Pat::Ident(..) = pattern {
        state.define_with_borrows(pattern, copy, ty, borrows);
    } else {
        bind_destructuring_pattern(pattern, copy, ty, borrows, program, state);
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
                    borrow_owners_or_temporary(receiver, returned_mutable, program, state)
                        .into_iter()
                        .map(|(owner, mutable)| BorrowOrigin {
                            owner,
                            mutable,
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
    borrow_owners(expr, mutable, state)
        .into_iter()
        .map(|(owner, mutable)| BorrowBinding {
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
    mutable: bool,
    program: &Program,
    state: &OwnershipState,
) -> Vec<(Owner, bool)> {
    let owners = borrow_owners(expr, mutable, state);
    if !owners.is_empty() {
        owners
    } else if !matches!(expr, Expr::Literal(..)) && expr_ty(expr, program, state).is_some() {
        vec![(Owner::temporary(), mutable)]
    } else {
        Vec::new()
    }
}

fn current_last_use(expr: &Expr, state: &OwnershipState) -> usize {
    let Some(id) = state.id(expr) else {
        return 0;
    };
    state
        .last_uses
        .iter()
        .rev()
        .find_map(|uses| uses.get(&id).copied())
        .unwrap_or(0)
}

fn target_identifier(expr: &Expr) -> Option<&Expr> {
    match expr {
        Expr::Ident(..) => Some(expr),
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

/// Reborrowing through a binding that holds borrows reaches their owners, but
/// never more mutably than the held borrow itself.
fn borrow_owners(expr: &Expr, mutable: bool, state: &OwnershipState) -> Vec<(Owner, bool)> {
    let Some(root) = place_root(expr) else {
        return Vec::new();
    };
    match state.get(root) {
        Some(binding) if !binding.borrows.is_empty() => binding
            .borrows
            .iter()
            .map(|borrow| (borrow.owner.clone(), mutable && borrow.mutable))
            .collect(),
        _ => vec![(state.owner(root), mutable)],
    }
}

fn check_borrow_conflict(span: Span, new_borrow: &BorrowBinding, state: &OwnershipState, reporter: &mut Reporter) {
    if let Some((holder, _)) =
        active_borrows_of(&new_borrow.owner, state).find(|(_, existing)| new_borrow.mutable || existing.mutable)
    {
        reporter.push(Diagnostic::error(
            "borrow-conflict",
            span,
            format!(
                "cannot borrow `{}` as {} because it is already borrowed by `{holder}`",
                new_borrow.owner.name,
                if new_borrow.mutable { "mutable" } else { "shared" },
            ),
        ));
    }
}

fn check_assignment_while_borrowed(
    place: &Expr,
    span: Span,
    state: &OwnershipState,
    reporter: &mut Reporter,
) {
    let owner = state.owner(place);
    if active_borrows_of(&owner, state).next().is_some() {
        reporter.push(Diagnostic::error(
            "borrow-conflict",
            span,
            format!("cannot assign to `{}` while it is borrowed", owner.name),
        ));
    }
}

fn root_place_name(expr: &Expr) -> Option<&Expr> {
    match expr {
        Expr::Ident(..) => Some(expr),
        Expr::Field { base, .. } => root_place_name(base),
        _ => None,
    }
}

/// Live borrows of `owner`, each with the name of the binding holding it.
fn active_borrows_of<'a>(
    owner: &'a Owner,
    state: &'a OwnershipState,
) -> impl Iterator<Item = (&'a str, &'a BorrowBinding)> {
    state
        .scopes
        .iter()
        .flat_map(|scope| scope.values())
        .flat_map(|binding| binding.borrows.iter().map(|borrow| (binding.name.as_str(), borrow)))
        .chain(state.temporary_borrows.iter().map(|borrow| (TEMPORARY_BORROW_OWNER, borrow)))
        .filter(move |(_, borrow)| borrow.owner == *owner && borrow.last_use >= state.current_statement)
}

fn block_last_uses(block: &Block, locals: &Locals) -> HashMap<LocalId, usize> {
    let mut uses = HashMap::new();
    for statement in &block.stmts {
        collect_statement_uses(statement, locals, &mut uses);
    }
    if let Some(tail) = &block.tail {
        collect_expr_uses(tail, locals, &mut uses);
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

fn collect_statement_uses(statement: &Stmt, locals: &Locals, uses: &mut HashMap<LocalId, usize>) {
    match statement {
        Stmt::Let(statement) => {
            if let Some(value) = &statement.value {
                collect_expr_uses(value, locals, uses);
            }
        }
        Stmt::Expr(expr) => collect_expr_uses(expr, locals, uses),
        Stmt::Item(_) => {}
    }
}

/// ADR: a bare `&T` cannot safely cross into a spawned task — its lifetime is
/// tied to the spawning stack frame, which the task may outlive or run
/// alongside. `Arc<T>` (an owned value, not a borrow) is the sanctioned
/// escape hatch, so it never trips this check.
fn check_shared_without_sync_capture(
    expr: &Expr,
    span: Span,
    state: &OwnershipState,
    reporter: &mut Reporter,
) {
    let mut uses = HashMap::new();
    collect_expr_uses(expr, &state.locals, &mut uses);
    let mut captured: Vec<&BindingState> = uses.keys().filter_map(|id| state.binding_by_id(Some(*id))).collect();
    captured.sort_by(|left, right| left.name.cmp(&right.name));
    for binding in captured {
        if matches!(binding.ty, Some(Ty::Borrow { .. })) {
            reporter.push(Diagnostic::error(
                "shared-without-sync",
                span,
                format!(
                    "`{}` is a bare reference; wrap it in `Arc` to share it with a spawned task",
                    binding.name
                ),
            ));
        }
    }
}

fn collect_expr_uses(expr: &Expr, locals: &Locals, uses: &mut HashMap<LocalId, usize>) {
    match expr {
        Expr::Ident(_, span) => {
            if let Some(id) = locals.expr(expr) {
                uses.insert(id, span.start());
            }
        }
        Expr::Block(block) | Expr::Unsafe(block, _) => {
            for statement in &block.stmts {
                collect_statement_uses(statement, locals, uses);
            }
            if let Some(tail) = &block.tail {
                collect_expr_uses(tail, locals, uses);
            }
        }
        Expr::Loop { body, .. } => {
            for statement in &body.stmts {
                collect_statement_uses(statement, locals, uses);
            }
            if let Some(tail) = &body.tail {
                collect_expr_uses(tail, locals, uses);
            }
        }
        Expr::If {
            condition,
            then_branch,
            else_branch,
            ..
        } => {
            collect_expr_uses(condition, locals, uses);
            for statement in &then_branch.stmts {
                collect_statement_uses(statement, locals, uses);
            }
            if let Some(tail) = &then_branch.tail {
                collect_expr_uses(tail, locals, uses);
            }
            if let Some(else_branch) = else_branch {
                collect_expr_uses(else_branch, locals, uses);
            }
        }
        Expr::While {
            condition, body, ..
        } => {
            collect_expr_uses(condition, locals, uses);
            for statement in &body.stmts {
                collect_statement_uses(statement, locals, uses);
            }
            if let Some(tail) = &body.tail {
                collect_expr_uses(tail, locals, uses);
            }
        }
        Expr::Match {
            scrutinee, arms, ..
        } => {
            collect_expr_uses(scrutinee, locals, uses);
            for arm in arms {
                if let Some(guard) = &arm.guard {
                    collect_expr_uses(guard, locals, uses);
                }
                collect_expr_uses(&arm.body, locals, uses);
            }
        }
        Expr::Call { callee, args, .. } => {
            collect_expr_uses(callee, locals, uses);
            for arg in args {
                collect_expr_uses(arg, locals, uses);
            }
        }
        Expr::MethodCall { receiver, args, .. } => {
            collect_expr_uses(receiver, locals, uses);
            for arg in args {
                collect_expr_uses(arg, locals, uses);
            }
        }
        Expr::AssociatedCall { args, .. } => {
            for arg in args {
                collect_expr_uses(arg, locals, uses);
            }
        }
        Expr::StructLiteral { fields, .. } => {
            for (_, value) in fields {
                collect_expr_uses(value, locals, uses);
            }
        }
        Expr::Tuple(items, _) => {
            for item in items {
                collect_expr_uses(item, locals, uses);
            }
        }
        Expr::Binary { left, right, .. } => {
            collect_expr_uses(left, locals, uses);
            collect_expr_uses(right, locals, uses);
        }
        Expr::Unary { expr, .. }
        | Expr::Return(Some(expr), _)
        | Expr::Break(Some(expr), _)
        | Expr::Spawn { expr, .. }
        | Expr::Closure { body: expr, .. }
        | Expr::Comptime { expr, .. }
        | Expr::Yield(expr, _)
        | Expr::Borrow { expr, .. }
        | Expr::Try { expr, .. }
        | Expr::Cast { expr, .. } => collect_expr_uses(expr, locals, uses),
        Expr::Assign { target, value, .. } => {
            collect_expr_uses(target, locals, uses);
            collect_expr_uses(value, locals, uses);
        }
        Expr::Field { base, .. } => collect_expr_uses(base, locals, uses),
        Expr::Index { base, index, .. } => {
            collect_expr_uses(base, locals, uses);
            for index_expr in index {
                collect_expr_uses(index_expr, locals, uses);
            }
        }
        Expr::Select { arms, default, .. } => {
            for arm in arms {
                collect_expr_uses(&arm.operation, locals, uses);
                for statement in &arm.body.stmts {
                    collect_statement_uses(statement, locals, uses);
                }
            }
            if let Some(default) = default {
                for statement in &default.stmts {
                    collect_statement_uses(statement, locals, uses);
                }
            }
        }
        Expr::Literal(_, _)
        | Expr::Return(None, _)
        | Expr::Break(None, _)
        | Expr::Continue(_)
        // `quote { .. }`/`#(expr)` (`phase-9-comptime` Decision 7): not
        // relevant to this function's own spawn-capture-shape analysis.
        | Expr::Quote(..)
        | Expr::Splice(..) => {}
    }
}

/// Whether a value of the resolved type `ty` is copied rather than moved;
/// `None` when the type is not known.
fn type_is_copy(ty: &paco_types::Type, copy_names: &HashSet<String>) -> Option<bool> {
    use paco_types::Type;
    Some(match ty {
        Type::Unknown | Type::Error => return None,
        Type::Int(_)
        | Type::Float(_)
        | Type::Bool
        | Type::Char
        | Type::Unit
        | Type::Never
        | Type::Borrow { .. }
        | Type::RawPointer { .. }
        | Type::TypeValue(_)
        | Type::Code
        | Type::Dim(_)
        | Type::Pack(_)
        | Type::Spread(_) => true,
        Type::Tuple(items) => items.iter().all(|item| type_is_copy(item, copy_names) == Some(true)),
        Type::Struct(name, args) | Type::Enum(name, args) => {
            copy_names.contains(name.rsplit("::").next().unwrap_or(name))
                && args.iter().all(|arg| type_is_copy(arg, copy_names) == Some(true))
        }
        Type::Generic(name) => copy_names.contains(name),
        Type::String | Type::Slice(_) | Type::Fn(..) => false,
    })
}

fn typed_copy(expr: &Expr, program: &Program, state: &OwnershipState) -> Option<bool> {
    type_is_copy(program.expr_types.get(&(expr as *const Expr))?, &state.copy_names)
}

fn expr_is_copy(expr: &Expr, program: &Program, state: &OwnershipState) -> bool {
    if let Some(copy) = typed_copy(expr, program, state) {
        return copy;
    }
    match expr {
        Expr::Literal(literal, _) => !matches!(literal, Literal::String(_)),
        Expr::Ident(..) => state.get(expr).is_some_and(|binding| binding.copy),
        Expr::Call { callee, .. } => {
            let Expr::Ident(name, _) = callee.as_ref() else {
                return false;
            };
            program
                .functions
                .get(name)
                .and_then(|signature| signature.return_ty.as_ref())
                .is_some_and(|ty| ty_is_copy(ty, &state.copy_names))
        }
        Expr::MethodCall { receiver, method, .. }
            if paco_types::float_intrinsic_arity(method).is_some() && method_signature(receiver, method, program, state).is_none() =>
        {
            true
        }
        Expr::MethodCall { .. } | Expr::AssociatedCall { .. } | Expr::Field { .. } => {
            expr_ty(expr, program, state)
                .as_ref()
                .is_some_and(|ty| ty_is_copy(ty, &state.copy_names))
        }
        Expr::Binary { .. } | Expr::Unary { .. } => true,
        _ => false,
    }
}

fn expr_ty(expr: &Expr, program: &Program, state: &OwnershipState) -> Option<Ty> {
    match expr {
        Expr::Binary { op, left, right, .. }
            if let Some(method) = operator_method(*op)
                && let Some(signature) = method_signature(left, method, program, state) =>
        {
            signature.return_ty.clone().map(|ty| {
                let mut substitutions =
                    method_call_substitutions(signature, std::slice::from_ref(right.as_ref()), program, state);
                if let Some(receiver_ty) = expr_ty(left, program, state) {
                    let receiver_ty = match receiver_ty {
                        Ty::Borrow { ty, .. } => *ty,
                        other => other,
                    };
                    substitutions.insert("Self".to_string(), receiver_ty);
                }
                substitute_ty(ty, &substitutions)
            })
        }
        Expr::Literal(literal, span) => Some(literal_ty(literal, *span)),
        Expr::Ident(..) => state.get(expr).and_then(|binding| binding.ty.clone()),
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
        Expr::Tuple(items, span) if !items.is_empty() => Some(Ty::Tuple(
            items.iter().map(|item| expr_ty(item, program, state)).collect::<Option<Vec<_>>>()?,
            *span,
        )),
        Expr::AssociatedCall {
            ty, function, args, span,
        } => {
            if let Ty::Path(path, _) = ty
                && path.len() == 1
                && let Some(signature) = program.functions.get(&format!("{}::{function}", path[0]))
            {
                if program.grads.contains(&format!("{}::{function}", path[0])) {
                    return program.grad_result_ty(args, *span);
                }
                return signature.return_ty.clone();
            }
            type_name(ty)
                .and_then(|name| {
                    program
                        .methods
                        .get(&(name, function.clone()))
                        .and_then(|signature| {
                            signature.return_ty.clone().map(|return_ty| match return_ty {
                                Ty::Path(path, _) if path == ["Self"] => ty.clone(),
                                return_ty => {
                                    let mut substitutions = call_substitutions(signature, args, program, state);
                                    substitutions.insert("Self".to_string(), ty.clone());
                                    substitute_ty(return_ty, &substitutions)
                                }
                            })
                        })
                })
                .or_else(|| Some(ty.clone()))
        }
        Expr::MethodCall {
            receiver,
            method,
            args,
            ..
        } => method_signature(receiver, method, program, state).and_then(|signature| {
            signature.return_ty.clone().map(|ty| {
                let mut substitutions = method_call_substitutions(signature, args, program, state);
                if let Some(receiver_ty) = expr_ty(receiver, program, state) {
                    let receiver_ty = match receiver_ty {
                        Ty::Borrow { ty, .. } => *ty,
                        other => other,
                    };
                    substitutions.insert("Self".to_string(), receiver_ty);
                }
                substitute_ty(ty, &substitutions)
            })
        }),
        Expr::Field { base, field, .. } => field_ty(base, field, program, state),
        Expr::Borrow { mutable, expr, span } => Some(Ty::Borrow {
            mutable: *mutable,
            lifetime: None,
            ty: Box::new(expr_ty(expr, program, state)?),
            span: *span,
        }),
        Expr::Call { callee, args, .. } => {
            let Expr::Ident(name, _) = callee.as_ref() else {
                return None;
            };
            if program.grads.contains(name) {
                return program.grad_result_ty(args, expr_span(expr));
            }
            let item_ty = program.functions.get(name).and_then(|signature| {
                signature.return_ty.clone().map(|ty| {
                    substitute_ty(ty, &call_substitutions(signature, args, program, state))
                })
            })?;
            if program.iter_functions.contains(name) {
                Some(Ty::Generic {
                    path: vec!["Generator".to_string()],
                    args: vec![item_ty],
                    span: Span::new_root(0, 0),
                })
            } else {
                Some(item_ty)
            }
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
                |ty| ty_is_copy(ty, &local_state.copy_names),
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
    let scrutinee_copy = scrutinee_ty.as_ref().is_some_and(|ty| ty_is_copy(ty, &state.copy_names));
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

fn ordering_method(op: paco_syntax::ast::BinaryOp) -> Option<&'static str> {
    use paco_syntax::ast::BinaryOp;
    matches!(op, BinaryOp::Lt | BinaryOp::Le | BinaryOp::Gt | BinaryOp::Ge).then_some("cmp")
}

fn operator_method(op: paco_syntax::ast::BinaryOp) -> Option<&'static str> {
    use paco_syntax::ast::BinaryOp;
    match op {
        BinaryOp::Add => Some("add"),
        BinaryOp::Sub => Some("sub"),
        BinaryOp::Mul => Some("mul"),
        BinaryOp::Div => Some("div"),
        BinaryOp::Rem => Some("rem"),
        _ => None,
    }
}

fn literal_ty(literal: &Literal, span: Span) -> Ty {
    let name = match literal {
        Literal::Int(_) => "i64",
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
        .or_else(|| program.intrinsics.get(method))
}

fn associated_function_params<'a>(ty: &Ty, function: &str, program: &'a Program) -> Option<&'a [Ty]> {
    if let Ty::Path(path, _) = ty
        && path.len() == 1
        && let Some(signature) = program.functions.get(&format!("{}::{function}", path[0]))
    {
        return Some(&signature.params);
    }
    let name = type_name(ty)?;
    program
        .methods
        .get(&(name, function.to_string()))
        .map(|signature| signature.params.as_slice())
}

fn type_name(ty: &Ty) -> Option<String> {
    match ty {
        Ty::Path(path, _) | Ty::Generic { path, .. } => Some(path.join("::")),
        Ty::Slice(..) => Some(paco_types::SLICE_TYPE_NAME.to_string()),
        Ty::Borrow { ty, .. } => type_name(ty),
        _ => None,
    }
}

fn resolve_self_ty(ty: &Ty, self_ty: Option<&str>) -> Ty {
    let Some(self_ty) = self_ty else { return ty.clone() };
    match ty {
        Ty::Path(path, span) if path.first().is_some_and(|name| name == "Self") => {
            Ty::Path(vec![self_ty.to_string()], *span)
        }
        Ty::Borrow { mutable, lifetime, ty: inner, span } => Ty::Borrow {
            mutable: *mutable,
            lifetime: lifetime.clone(),
            ty: Box::new(resolve_self_ty(inner, Some(self_ty))),
            span: *span,
        },
        other => other.clone(),
    }
}

fn ty_is_copy(ty: &Ty, copy_names: &HashSet<String>) -> bool {
    match ty {
        Ty::Path(path, _) if path.last().is_some_and(|name| copy_names.contains(name)) => true,
        Ty::Generic { path, args, .. } if path.last().is_some_and(|name| copy_names.contains(name)) => {
            args.iter().all(|arg| ty_is_copy(arg, copy_names))
        }
        Ty::Path(path, _) => path.first().is_some_and(|name| {
            matches!(
                name.as_str(),
                "i8" | "i16"
                    | "i32"
                    | "i64"
                    | "u8"
                    | "u16"
                    | "u32"
                    | "u64"
                    | "float"
                    | "f64"
                    | "f32"
                    | "f16"
                    | "bf16"
                    | "f8e4m3"
                    | "f8e5m2"
                    | "bool"
                    | "char"
                    | "byte"
                    // `phase-9-comptime` Decisions 5/7: `type`/`Code`
                    // values are compile-time-only handles with no
                    // backing store to move out of or double-free —
                    // copying one is as cheap and harmless as copying a
                    // scalar, and a derive fn's own `t: type` parameter
                    // is routinely read more than once (`fields_of(t)`,
                    // then `#(t)` in the fn's own final `quote { .. }`).
                    | "type"
                    | "Code"
            )
        }),
        Ty::Borrow { .. } | Ty::RawPointer { .. } => true,
        Ty::Tuple(items, _) => items.iter().all(|item| ty_is_copy(item, copy_names)),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn primitive_copy_classification_matches_ownership_rules() {
        for name in [
            "i8", "i16", "i32", "i64", "u8", "u16", "u32", "u64", "float", "bool", "char", "byte",
        ] {
            assert!(ty_is_copy(&path_ty(name), &HashSet::new()), "{name} should be Copy");
        }
        assert!(!ty_is_copy(&path_ty("int"), &HashSet::new()), "int is no longer a valid type");
        assert!(!ty_is_copy(&path_ty("string"), &HashSet::new()));
    }

    #[test]
    fn borrow_types_are_copy_for_move_analysis() {
        let borrowed = Ty::Borrow {
            mutable: false,
            lifetime: None,
            ty: Box::new(path_ty("Box")),
            span: Span::new_root(0, 0),
        };

        assert!(ty_is_copy(&borrowed, &HashSet::new()));
    }

    fn path_ty(name: &str) -> Ty {
        Ty::Path(vec![name.to_string()], Span::new_root(0, 0))
    }
}
