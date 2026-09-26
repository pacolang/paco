use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use paco_diag::{Diagnostic, Reporter};
use paco_mir::{Body, ComptimeKey, ComptimeSite, ComptimeValue, InstantiationRegistry, Profile};
use paco_span::{SourceMap, Span};
use paco_syntax::ast::{BinaryOp, Expr, FnDecl, Item, Module, Visit};

use paco_types::Type;

use super::{ModuleContext, function_is_generic, module_owns_method, qualified_module_functions, resolve_key};

/// Lowers a checked program's functions to MIR, eagerly for what the
/// program runs and on demand for what compile-time evaluation calls.
pub(crate) struct Session<'s, 'm> {
    pub(crate) contexts: &'s [ModuleContext<'m>],
    profile: Profile,
    pub(crate) instantiations: InstantiationRegistry,
    pub(crate) done: RefCell<HashSet<(String, Vec<Type>)>>,
    /// Raw source text for a `stdlib::test` assertion call's own argument
    /// sub-expressions (`unit-testing`'s design.md); empty wherever a
    /// caller has no `SourceMap` in hand (task 3.2's `Lowerer::source_text`
    /// itself defaults the same way).
    source_text: HashMap<Span, String>,
}

/// A generic item's declaration, its module, its bindings and its hidden
/// dimension names, in argument order.
type Instance<'s, 'm> = (&'m FnDecl, &'s ModuleContext<'m>, HashMap<String, Type>, Vec<String>);

pub(crate) struct Lowered {
    pub(crate) bodies: Vec<(String, Body)>,
    /// Whether gradients were expanded, so the tape runtime is linked in.
    pub(crate) differentiated: bool,
}

impl<'s, 'm> Session<'s, 'm> {
    pub(crate) fn new(contexts: &'s [ModuleContext<'m>], profile: Profile) -> Self {
        Self::with_source_text(contexts, profile, HashMap::new())
    }

    pub(crate) fn with_source_text(contexts: &'s [ModuleContext<'m>], profile: Profile, source_text: HashMap<Span, String>) -> Self {
        Self { contexts, profile, instantiations: InstantiationRegistry::new(), done: RefCell::new(HashSet::new()), source_text }
    }

    fn lower(
        &self,
        function: &FnDecl,
        context: &ModuleContext<'m>,
        substitutions: &HashMap<String, Type>,
        hidden: &[String],
    ) -> (Body, Vec<(String, Body)>) {
        if function.is_iter {
            paco_mir::lower_iter_fn_with_substitutions(
                function, &context.typed, &context.registry, &context.drops,
                self.profile, substitutions, &self.instantiations, &self.source_text,
            )
        } else {
            paco_mir::lower_instance(
                function, &context.typed, &context.registry, &context.drops,
                self.profile, substitutions, hidden, &self.instantiations, &self.source_text,
            )
        }
    }

    /// Every function the compiled program contains: each module's
    /// non-generic functions, then each generic instance they use.
    pub(crate) fn lower_program(&self) -> Lowered {
        let empty = HashMap::new();
        let mut bodies = Vec::new();
        let mut names = HashSet::new();
        for context in self.contexts {
            for (qualified_name, function) in qualified_module_functions(context.module, &context.qualifier) {
                if !names.insert(qualified_name.clone()) {
                    continue;
                }
                if paco_mir::is_comptime_only(function) || function_is_generic(&context.typed, function) {
                    continue;
                }
                let (body, outlined) = self.lower(function, context, &empty, &[]);
                let name = if qualified_name == "main" { paco_mir::ENTRY_SYMBOL.to_string() } else { qualified_name };
                bodies.push((name, body));
                bodies.extend(outlined);
            }
        }
        bodies.extend(self.drain());
        Lowered { bodies, differentiated: false }
    }

    /// Lowers every generic instance recorded so far, and the instances
    /// those record, until none remain.
    pub(crate) fn drain(&self) -> Vec<(String, Body)> {
        let mut bodies = Vec::new();
        loop {
            let pending = self.instantiations.drain_pending();
            if pending.is_empty() {
                return bodies;
            }
            for (method_name, type_args) in pending {
                if !self.done.borrow_mut().insert((method_name.clone(), type_args.clone())) {
                    continue;
                }
                let Some((function, context, substitutions, hidden)) = self.instance(&method_name, &type_args) else { continue };
                let (body, outlined) = self.lower(function, context, &substitutions, &hidden);
                bodies.push((paco_mir::mangled_name(&method_name, &type_args), body));
                bodies.extend(outlined);
            }
        }
    }

    fn instance(
        &self,
        method_name: &str,
        type_args: &[Type],
    ) -> Option<Instance<'s, 'm>> {
        let free_function = self.contexts.iter().find_map(|context| {
            context.module.items.iter().find_map(|item| match item {
                Item::Fn(function)
                    if resolve_key(&context.qualifier, &function.name) == method_name
                        || context.module.name.is_none() && function.name == method_name =>
                {
                    Some((function, context))
                }
                _ => None,
            })
        });
        if let Some((function, context)) = free_function {
            let (type_args, hidden) = symbolic_args(&[], function, type_args);
            let substitutions =
                paco_syntax::ast::generic_names(&function.generics).into_iter().zip(type_args).collect();
            return Some((function, context, substitutions, hidden));
        }
        let (type_name, method) = method_name.rsplit_once("::")?;
        let local_type_name = |context: &ModuleContext<'_>| {
            if context.qualifier.is_empty() { Some(type_name) } else { type_name.strip_prefix(&format!("{}::", context.qualifier)) }
        };
        let Some((function, generics, context)) = self.contexts.iter().find_map(|context| {
            module_owns_method(context.module, local_type_name(context)?, method).map(|(function, generics)| (function, generics, context))
        }) else {
            panic!("no declaration found for generic method `{method_name}` (instantiated with {type_args:?})");
        };
        let owner_params = local_type_name(context).map_or(&[][..], |name| context.registry.owner_generics(name));
        let (type_args, hidden) = symbolic_args(owner_params, function, type_args);
        let type_args = type_args.as_slice();
        let own = paco_syntax::ast::generic_names(&function.generics);
        let split = type_args.len().saturating_sub(own.len());
        let (owner_args, own_args) = type_args.split_at(split);
        let mut substitutions: HashMap<String, Type> = match context.typed.self_type_of(function) {
            Some(template @ (Type::Struct(name, _) | Type::Enum(name, _))) => {
                let concrete = match template {
                    Type::Enum(..) => Type::Enum(name.clone(), owner_args.to_vec()),
                    _ => Type::Struct(name.clone(), owner_args.to_vec()),
                };
                paco_types::bind_generics(template, &concrete)
            }
            _ => generics.into_iter().zip(owner_args.iter().cloned()).collect(),
        };
        substitutions.extend(own.into_iter().zip(own_args.iter().cloned()));
        Some((function, context, substitutions, hidden))
    }

    /// The non-generic function named `name` in any module (a `comptime fn`
    /// included), lowered with the bodies it outlines.
    fn lower_named(&self, name: &str) -> Vec<(String, Body)> {
        for context in self.contexts {
            for (qualified_name, function) in qualified_module_functions(context.module, &context.qualifier) {
                if qualified_name == name && !function_is_generic(&context.typed, function) {
                    let (body, mut outlined) = self.lower(function, context, &HashMap::new(), &[]);
                    outlined.push((qualified_name, body));
                    return outlined;
                }
            }
        }
        Vec::new()
    }
}

/// An instance's type arguments with each hidden dimension replaced by a
/// fresh name, and those names in argument order.
fn symbolic_args(owner: &[paco_syntax::ast::GenericParam], function: &FnDecl, type_args: &[Type]) -> (Vec<Type>, Vec<String>) {
    let mut args = type_args.to_vec();
    let mut hidden = Vec::new();
    for slot in paco_mir::dims::hidden_slots(owner, Some(function), type_args) {
        let name = paco_types::fresh_atom(&paco_mir::dims::slot_name(owner, Some(function), slot));
        paco_mir::dims::with_slot(&mut args, slot, Type::Generic(name.clone()));
        hidden.push(name);
    }
    (args, hidden)
}

/// MIR bodies for compile-time evaluation, lowered as they are needed.
pub(crate) struct Provider<'a, 's, 'm> {
    session: &'a Session<'s, 'm>,
    bodies: HashMap<String, Rc<Body>>,
}

impl<'a, 's, 'm> Provider<'a, 's, 'm> {
    pub(crate) fn new(session: &'a Session<'s, 'm>, bodies: Vec<(String, Body)>) -> Self {
        let mut provider = Self { session, bodies: HashMap::new() };
        provider.add(bodies);
        provider
    }

    fn add(&mut self, bodies: Vec<(String, Body)>) {
        self.bodies.extend(bodies.into_iter().map(|(name, body)| (name, Rc::new(body))));
    }
}

impl paco_comptime::Bodies for Provider<'_, '_, '_> {
    fn body(&mut self, name: &str) -> Option<Rc<Body>> {
        if let Some(body) = self.bodies.get(name) {
            return Some(body.clone());
        }
        let instances = self.session.drain();
        self.add(instances);
        if !self.bodies.contains_key(name) {
            let named = self.session.lower_named(name);
            self.add(named);
            let instances = self.session.drain();
            self.add(instances);
        }
        self.bodies.get(name).cloned()
    }
}

/// Output a compilation's compile-time code printed.
#[derive(Default)]
pub(crate) struct ComptimeOutput {
    pub(crate) stdout: String,
    pub(crate) stderr: String,
}

/// Evaluates every outlined `comptime` block, reporting failures as
/// diagnostics, and returns the values to embed.
pub(crate) fn evaluate_sites(
    session: &Session<'_, '_>,
    bodies: Vec<(String, Body)>,
    program: &paco_comptime::Program<'_>,
    reporter: &mut Reporter,
    output: &mut ComptimeOutput,
) -> Result<HashMap<ComptimeKey, ComptimeValue>, ()> {
    let sites: Vec<ComptimeSite> = session.instantiations.take_comptime_sites();
    let mut provider = Provider::new(session, bodies);
    let mut values = HashMap::new();
    let mut failed = false;
    for site in sites {
        match paco_comptime::evaluate(program, &mut provider, &site.name, &[], paco_comptime::Limits::default()) {
            Ok(evaluation) => {
                output.stdout.push_str(&evaluation.output);
                output.stderr.push_str(&evaluation.stderr);
                if matches!(evaluation.value, ComptimeValue::Type(_) | ComptimeValue::Code(_)) {
                    reporter.push(Diagnostic::error(
                        "PACO-E0350",
                        site.span,
                        "comptime evaluation failed: a `type` or `Code` value exists only during compilation and cannot be used by the compiled program",
                    ));
                    failed = true;
                } else {
                    values.insert(site.key, evaluation.value);
                }
            }
            Err(error) => {
                reporter.push(Diagnostic::error(
                    "PACO-E0350",
                    error.span.unwrap_or(site.span),
                    format!("comptime evaluation failed: {}", error.message),
                ));
                failed = true;
            }
        }
    }
    if failed { Err(()) } else { Ok(values) }
}

/// Lowers the program with `profile`, first evaluating its `comptime`
/// blocks (lowered for debug, as compile-time code always runs) when it has
/// any, then expanding its `grad` calls.
#[allow(clippy::too_many_arguments)]
pub(crate) fn lower_evaluating_comptime(
    contexts: &[ModuleContext<'_>],
    layouts: &paco_mir::TypeLayouts<'_>,
    externs: &[(String, Vec<Type>, Type)],
    modules: &[&Module],
    sources: &SourceMap,
    profile: Profile,
    reporter: &mut Reporter,
    output: &mut ComptimeOutput,
) -> Result<Lowered, ()> {
    let source_text = test_assert_source_text(modules, sources);
    let evaluation = Session::with_source_text(contexts, Profile::Debug, source_text.clone());
    let first = evaluation.lower_program();
    if !evaluation.instantiations.has_comptime_sites() {
        return if profile == Profile::Debug {
            differentiate(&evaluation, first, layouts, externs, reporter)
        } else {
            let session = Session::with_source_text(contexts, profile, source_text);
            let lowered = session.lower_program();
            differentiate(&session, lowered, layouts, externs, reporter)
        };
    }
    let (structs, enums) = type_decls(modules);
    let names = externs.iter().map(|(name, ..)| name.clone()).collect();
    let program = paco_comptime::Program { layouts, externs: names, structs, enums };
    let values = evaluate_sites(&evaluation, first.bodies, &program, reporter, output)?;
    let session = Session::with_source_text(contexts, profile, source_text);
    session.instantiations.set_comptime_values(values);
    let lowered = session.lower_program();
    differentiate(&session, lowered, layouts, externs, reporter)
}

/// Expands every `grad` call in `lowered` (ADR 0026), reporting what cannot
/// be differentiated.
fn differentiate(
    session: &Session<'_, '_>,
    mut lowered: Lowered,
    layouts: &paco_mir::TypeLayouts<'_>,
    externs: &[(String, Vec<Type>, Type)],
    reporter: &mut Reporter,
) -> Result<Lowered, ()> {
    if !paco_mir::autodiff::has_grad_sites(&lowered.bodies) {
        return Ok(lowered);
    }
    let mut host = SessionHost { session, derivatives: derivatives(session.contexts), lowered: HashSet::new(), pending: Vec::new() };
    host.lowered.extend(lowered.bodies.iter().map(|(name, _)| name.clone()));
    let errors = paco_mir::autodiff::differentiate(&mut lowered.bodies, layouts, externs, &mut host);
    let failed = !errors.is_empty();
    for error in errors {
        reporter.push(error);
    }
    lowered.bodies.extend(host.take_remaining());
    lowered.differentiated = true;
    if failed { Err(()) } else { Ok(lowered) }
}

/// `#[derivative(of = f)]` registrations: `f`'s symbol to the derivative's.
fn derivatives(contexts: &[ModuleContext<'_>]) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for context in contexts {
        for item in &context.module.items {
            let Item::Fn(function) = item else { continue };
            let Some((path, _)) = paco_types::derivative_of(function) else { continue };
            let of = match path.as_slice() {
                [name] => resolve_key(&context.qualifier, name),
                _ => path.join("::"),
            };
            map.insert(of, resolve_key(&context.qualifier, &function.name));
        }
    }
    map
}

struct SessionHost<'x, 's, 'm> {
    session: &'x Session<'s, 'm>,
    derivatives: HashMap<String, String>,
    lowered: HashSet<String>,
    pending: Vec<(String, Body)>,
}

impl SessionHost<'_, '_, '_> {
    /// Makes sure `symbol` (instantiated from `base` with `args`) has a body.
    fn ensure(&mut self, base: &str, args: &[Type]) -> String {
        let symbol = self.session.instantiations.request(base, args);
        if args.is_empty() && !self.lowered.contains(&symbol) {
            for (name, body) in self.session.lower_named(&symbol) {
                self.pending_insert(name, body);
            }
        }
        symbol
    }

    fn pending_insert(&mut self, name: String, body: Body) {
        if self.lowered.insert(name.clone()) {
            self.pending.push((name, body));
        }
    }

    fn take_remaining(&mut self) -> Vec<(String, Body)> {
        paco_mir::autodiff::Host::take_bodies(self)
    }
}

impl paco_mir::autodiff::Host for SessionHost<'_, '_, '_> {
    fn method(&mut self, owner: &Type, method: &str) -> Option<String> {
        let (name, args) = match owner {
            Type::Struct(name, args) | Type::Enum(name, args) => (name.clone(), args.clone()),
            _ => return None,
        };
        let declared = self.session.contexts.iter().find_map(|context| {
            let local = if context.qualifier.is_empty() { Some(name.as_str()) } else { name.strip_prefix(&format!("{}::", context.qualifier)) };
            let local = local.filter(|local| module_owns_method(context.module, local, method).is_some()).or_else(|| {
                (!context.qualifier.is_empty() && !name.contains("::") && module_owns_method(context.module, &name, method).is_some())
                    .then_some(name.as_str())
            })?;
            Some(resolve_key(&context.qualifier, &format!("{local}::{method}")))
        })?;
        Some(self.ensure(&declared, &args))
    }

    fn derivative(&mut self, function: &str) -> Option<String> {
        if let Some(derivative) = self.derivatives.get(function).cloned() {
            return Some(self.ensure(&derivative, &[]));
        }
        let instance = self
            .session
            .done
            .borrow()
            .iter()
            .find(|(name, args)| !args.is_empty() && paco_mir::mangled_name(name, args) == function && self.derivatives.contains_key(name))
            .cloned();
        let (name, args) = instance?;
        let derivative = self.derivatives[&name].clone();
        Some(self.ensure(&derivative, &args))
    }

    fn take_bodies(&mut self) -> Vec<(String, Body)> {
        for (name, body) in self.session.drain() {
            self.pending_insert(name, body);
        }
        std::mem::take(&mut self.pending)
    }
}

/// Whether any of `modules` runs code at compile time: a `comptime { .. }`
/// expression, or a call to a compile-time-only function.
pub(crate) fn has_comptime(modules: &[&Module]) -> bool {
    struct Finder(bool, HashSet<String>);
    impl Visit for Finder {
        fn visit_expr(&mut self, expr: &Expr) {
            match expr {
                Expr::Comptime { .. } => self.0 = true,
                Expr::Call { callee, .. } if matches!(callee.as_ref(), Expr::Ident(name, _) if self.1.contains(name)) => self.0 = true,
                _ => {}
            }
            if !self.0 {
                paco_syntax::ast::walk_expr(self, expr);
            }
        }

        fn visit_fn_decl(&mut self, function: &FnDecl) {
            if !paco_mir::is_comptime_only(function) {
                paco_syntax::ast::walk_fn_decl(self, function);
            }
        }
    }
    let compile_time_only = modules
        .iter()
        .flat_map(|module| &module.items)
        .filter_map(|item| match item {
            Item::Fn(function) if paco_mir::is_comptime_only(function) => Some(function.name.clone()),
            _ => None,
        })
        .collect();
    let mut finder = Finder(false, compile_time_only);
    for module in modules {
        finder.visit_module(module);
        for item in &module.items {
            if let Item::Const(decl) = item {
                finder.visit_expr(&decl.value);
            }
        }
    }
    finder.0
}

/// `stdlib::test`'s nine `#[builtin(name)]`-tagged assertion functions
/// (`compiler/paco-mir`'s own copy of this same closed set, per
/// `#[builtin(grad)]`'s own precedent — see `unit-testing`'s design.md).
const TEST_ASSERT_BUILTINS: &[&str] =
    &["assert", "assert_eq", "assert_ne", "assert_true", "assert_false", "assert_some", "assert_none", "assert_ok", "assert_err"];

/// `span`'s raw source text, or `None` for a span this `SourceMap` cannot
/// resolve (an unknown file, or a byte range outside the file's text).
fn span_text(sources: &SourceMap, span: Span) -> Option<String> {
    sources.source(span.file_id())?.get(span.start()..span.end()).map(str::to_string)
}

/// Raw source text for every `stdlib::test` assertion call's own argument
/// sub-expressions in `modules` (`unit-testing`'s design.md): the whole
/// first argument always, and, when it (or, for `assert_eq`/`assert_ne`,
/// either separate argument) is itself a top-level `==`/`!=`, both operand
/// sub-expressions too — `paco-mir`'s `Lowerer::source_text` reads this
/// back by span to name a failing assertion's expression(s) in its panic
/// message. Matches a call by its own bare name against the fixed,
/// reserved set above, the same way `find_grad_call` already matches
/// `grad` by name rather than re-resolving `#[builtin]` at every call site.
pub(crate) fn test_assert_source_text(modules: &[&Module], sources: &SourceMap) -> HashMap<Span, String> {
    struct Finder<'a> {
        sources: &'a SourceMap,
        out: HashMap<Span, String>,
    }
    impl Finder<'_> {
        fn note(&mut self, expr: &Expr) {
            if let Some(text) = span_text(self.sources, paco_syntax::parse::expr_span(expr)) {
                self.out.insert(paco_syntax::parse::expr_span(expr), text);
            }
        }
        fn note_if_comparison(&mut self, expr: &Expr) {
            if let Expr::Binary { op: BinaryOp::Eq | BinaryOp::Ne, left, right, .. } = expr {
                self.note(left);
                self.note(right);
            }
        }
        /// `kind` (already the bare, reserved builtin name) was called with
        /// `args`, however the call was written — a plain `assert_eq(a, b)`
        /// or a qualified `test::assert_eq(a, b)` (parsed as a distinct
        /// `Expr::AssociatedCall`, not a colon-qualified `Expr::Call`).
        fn record(&mut self, kind: &str, args: &[Expr]) {
            match (kind, args) {
                ("assert_eq" | "assert_ne", [left, right, ..]) => {
                    self.note(left);
                    self.note(right);
                }
                (_, [first, ..]) => {
                    self.note(first);
                    self.note_if_comparison(first);
                }
                _ => {}
            }
        }
    }
    impl Visit for Finder<'_> {
        fn visit_expr(&mut self, expr: &Expr) {
            match expr {
                Expr::Call { callee, args, .. } => {
                    if let Expr::Ident(name, _) = callee.as_ref()
                        && let Some(kind) = name.rsplit("::").next()
                        && TEST_ASSERT_BUILTINS.contains(&kind)
                    {
                        self.record(kind, args);
                    }
                }
                Expr::AssociatedCall { function, args, .. } if TEST_ASSERT_BUILTINS.contains(&function.as_str()) => {
                    self.record(function, args);
                }
                _ => {}
            }
            paco_syntax::ast::walk_expr(self, expr);
        }
    }
    let mut finder = Finder { sources, out: HashMap::new() };
    for module in modules {
        finder.visit_module(module);
    }
    finder.out
}

/// The declared fields of every struct and the name of every enum in
/// `modules`, for `fields_of`.
pub(crate) fn type_decls(modules: &[&Module]) -> (paco_comptime::StructFields, HashSet<String>) {
    let mut structs = HashMap::new();
    let mut enums = HashSet::new();
    for module in modules {
        for item in &module.items {
            match item {
                Item::Struct(decl) => {
                    structs.insert(decl.name.clone(), decl.fields.iter().map(|field| (field.name.clone(), field.ty.clone())).collect());
                }
                Item::Enum(decl) => {
                    enums.insert(decl.name.clone());
                }
                _ => {}
            }
        }
    }
    (structs, enums)
}

/// The part of `module` compile-time code in it can reach from `roots`:
/// every type, trait, const, import and extern declaration, the functions
/// `roots` reach by name, and only the methods they call — so code that
/// uses what a derive is about to generate does not have to check yet.
pub(crate) fn comptime_slice(module: &Module, roots: &HashSet<String>) -> Module {
    #[derive(Default)]
    struct Names(HashSet<String>);
    impl Visit for Names {
        fn visit_expr(&mut self, expr: &Expr) {
            match expr {
                Expr::Ident(name, _) => {
                    self.0.insert(name.clone());
                }
                Expr::MethodCall { method, .. } => {
                    self.0.insert(method.clone());
                }
                Expr::AssociatedCall { function, .. } => {
                    self.0.insert(function.clone());
                }
                _ => {}
            }
            paco_syntax::ast::walk_expr(self, expr);
        }
    }
    let functions: Vec<&FnDecl> = module
        .items
        .iter()
        .flat_map(|item| match item {
            Item::Fn(function) => vec![function],
            Item::Struct(decl) => decl.methods.iter().collect(),
            Item::Enum(decl) => decl.methods.iter().collect(),
            Item::Methods(block) => block.methods.iter().collect(),
            _ => Vec::new(),
        })
        .collect();
    let mut reached = roots.clone();
    loop {
        let mut names = Names::default();
        for function in functions.iter().filter(|function| reached.contains(&function.name)) {
            names.visit_fn_decl(function);
        }
        let before = reached.len();
        reached.extend(names.0);
        if reached.len() == before {
            break;
        }
    }
    let keep = |methods: &[FnDecl]| -> Vec<FnDecl> {
        methods.iter().filter(|method| reached.contains(&method.name)).cloned().collect()
    };
    let items = module
        .items
        .iter()
        .filter_map(|item| match item {
            Item::Fn(function) => reached.contains(&function.name).then(|| item.clone()),
            Item::Struct(decl) => {
                let mut decl = decl.clone();
                decl.methods = keep(&decl.methods);
                Some(Item::Struct(decl))
            }
            Item::Enum(decl) => {
                let mut decl = decl.clone();
                decl.methods = keep(&decl.methods);
                Some(Item::Enum(decl))
            }
            Item::Methods(block) => {
                let mut block = block.clone();
                block.methods = keep(&block.methods);
                (!block.methods.is_empty()).then_some(Item::Methods(block))
            }
            other => Some(other.clone()),
        })
        .collect();
    Module { name: module.name.clone(), items, span: module.span }
}
