use paco_diag::Reporter;
use paco_span::SourceMap;
use paco_syntax::{
    ast::{BinaryOp, Expr, GenericParamKind, Item, Literal, Stmt, Ty},
    fmt::format_module,
    lex::lex,
    parse::parse_module,
};

fn parse(source: &str) -> (Option<paco_syntax::ast::Module>, Reporter) {
    let mut sources = SourceMap::new();
    let file = sources.add_file("main.paco", source);
    let mut reporter = Reporter::new();
    let tokens = lex(sources.source(file).unwrap(), file, &mut reporter);
    let module = parse_module(&tokens, &mut reporter).ok();
    (module, reporter)
}

fn parse_ok(source: &str) -> paco_syntax::ast::Module {
    let (module, reporter) = parse(source);
    assert!(!reporter.has_errors(), "{:?}", reporter.diagnostics());
    module.unwrap()
}

fn let_ty(module: &paco_syntax::ast::Module, fn_index: usize) -> Ty {
    let Item::Fn(function) = &module.items[fn_index] else { panic!("expected fn") };
    let Stmt::Let(statement) = &function.body.stmts[0] else { panic!("expected let") };
    statement.ty.clone().unwrap()
}

#[test]
fn a_trailing_const_pack_parses_with_its_kind() {
    let module = parse_ok("struct Tensor<T, const D: int...> { data: []T }");
    let Item::Struct(decl) = &module.items[0] else { panic!("expected struct") };
    assert_eq!(decl.generics.len(), 2);
    assert_eq!(decl.generics[0].name, "T");
    assert_eq!(decl.generics[0].kind, GenericParamKind::Type);
    assert_eq!(decl.generics[1].name, "D");
    let GenericParamKind::ConstPack(Ty::Path(path, _)) = &decl.generics[1].kind else {
        panic!("expected a const pack, got {:?}", decl.generics[1].kind);
    };
    assert_eq!(path, &vec!["int".to_string()]);
}

#[test]
fn fixed_arity_const_params_parse_alongside_type_and_lifetime_params() {
    let module = parse_ok("fn matmul<'a, T, const M: int, const K: int>(x: &'a T) -> i64 { 0 }");
    let Item::Fn(function) = &module.items[0] else { panic!("expected fn") };
    let kinds: Vec<_> = function.generics.iter().map(|param| (param.name.as_str(), &param.kind)).collect();
    assert!(matches!(kinds[0], ("a", GenericParamKind::Lifetime)));
    assert!(matches!(kinds[1], ("T", GenericParamKind::Type)));
    assert!(matches!(kinds[2], ("M", GenericParamKind::Const(_))));
    assert!(matches!(kinds[3], ("K", GenericParamKind::Const(_))));
    assert_eq!(paco_syntax::ast::generic_names(&function.generics), vec!["T", "M", "K"]);
}

#[test]
fn a_const_pack_in_a_non_trailing_position_is_rejected() {
    let (_, reporter) = parse("struct Bad<const D: int..., T> { x: T }");
    let diagnostic = reporter.diagnostics().iter().find(|d| d.code() == "PACO-E0114").expect("pack position error");
    assert!(diagnostic.primary().message.contains("must be the last generic parameter"));
}

#[test]
fn two_const_packs_are_rejected() {
    let (_, reporter) = parse("struct Bad<const A: int..., const B: int...> { }");
    let diagnostic = reporter.diagnostics().iter().find(|d| d.code() == "PACO-E0114").expect("pack count error");
    assert!(diagnostic.primary().message.contains("only one const parameter pack"));
}

#[test]
fn dyn_is_a_distinct_const_argument() {
    let module = parse_ok("fn main() { let t: Tensor<f32, Dyn, 768> = x; }");
    let Ty::Generic { args, .. } = let_ty(&module, 0) else { panic!("expected generic type") };
    assert!(matches!(&args[0], Ty::Path(path, _) if path == &vec!["f32".to_string()]));
    assert!(matches!(&args[1], Ty::DynDim(_)));
    assert!(matches!(&args[2], Ty::Const(expr, _) if matches!(expr.as_ref(), Expr::Literal(Literal::Int(768), _))));
}

#[test]
fn a_dyn_path_that_names_a_type_is_not_the_marker() {
    let module = parse_ok("fn main() { let t: Box<Dyn::Thing> = x; }");
    let Ty::Generic { args, .. } = let_ty(&module, 0) else { panic!("expected generic type") };
    assert!(matches!(&args[0], Ty::Path(path, _) if path.len() == 2));
}

#[test]
fn const_expressions_parse_in_argument_position() {
    let module = parse_ok("fn main() { let t: Tensor<f32, M * 2, (N + 1) * 3, 2 * M> = x; }");
    let Ty::Generic { args, .. } = let_ty(&module, 0) else { panic!("expected generic type") };
    assert!(matches!(&args[1], Ty::Const(expr, _) if matches!(expr.as_ref(), Expr::Binary { op: BinaryOp::Mul, .. })));
    assert!(matches!(&args[2], Ty::Const(..)));
    assert!(matches!(&args[3], Ty::Const(..)));
}

#[test]
fn const_arguments_parse_in_expression_position_type_applications() {
    let module = parse_ok("fn main() { let t = Tensor<f32, 2, Dyn>::zeros(); }");
    let Item::Fn(function) = &module.items[0] else { panic!("expected fn") };
    let Stmt::Let(statement) = &function.body.stmts[0] else { panic!("expected let") };
    let Some(Expr::AssociatedCall { ty: Ty::Generic { args, .. }, .. }) = &statement.value else {
        panic!("expected associated call, got {:?}", statement.value);
    };
    assert!(matches!(&args[1], Ty::Const(..)));
    assert!(matches!(&args[2], Ty::DynDim(_)));
}

#[test]
fn const_generics_round_trip_through_the_formatter() {
    let source = "struct Tensor<T, const D: int...> {\n    data: []T,\n}\n\nfn f(a: &Tensor<f32, Dyn, M * 2>) {\n}\n";
    let module = parse_ok(source);
    let formatted = format_module(&module, Some(source));
    assert!(formatted.contains("struct Tensor<T, const D: int...>"), "{formatted}");
    assert!(formatted.contains("Tensor<f32, Dyn, M * 2>"), "{formatted}");
    let reparsed = parse_ok(&formatted);
    assert_eq!(format_module(&reparsed, Some(&formatted)), formatted);
}

#[test]
fn a_pack_expansion_parses_as_its_own_argument() {
    let module = parse_ok("methods<T, const D: int...> Tensor<T, D...> { fn f(&self) {} }");
    let Item::Methods(block) = &module.items[0] else { panic!("expected methods block") };
    let Ty::Generic { args, .. } = &block.target else { panic!("expected generic target") };
    assert!(matches!(&args[1], Ty::Expand(name, _) if name == "D"));
    let formatted = format_module(&module, None);
    assert!(formatted.contains("Tensor<T, D...>"), "{formatted}");
}

#[test]
fn a_dim_parameter_parses_with_its_kind_and_round_trips() {
    let source = "fn rows<T, dim B, const C: int>(g: &Grid<T, B, C>) -> i64 {\n    B\n}\n";
    let module = parse_ok(source);
    let Item::Fn(function) = &module.items[0] else { panic!("expected fn") };
    assert_eq!(function.generics[1].name, "B");
    assert_eq!(function.generics[1].kind, GenericParamKind::Dim);
    assert!(function.generics[1].is_dim());
    let formatted = format_module(&module, Some(source));
    assert!(formatted.contains("fn rows<T, dim B, const C: int>"), "{formatted}");
    assert_eq!(format_module(&parse_ok(&formatted), Some(&formatted)), formatted);
}

#[test]
fn dim_stays_an_ordinary_identifier_elsewhere() {
    let module = parse_ok("fn f<dim>(dim: i64) -> i64 { let n = x.dim(0); dim }");
    let Item::Fn(function) = &module.items[0] else { panic!("expected fn") };
    assert_eq!(function.generics[0].kind, GenericParamKind::Type);
}

#[test]
fn an_existential_dimension_parses_and_round_trips() {
    let source = "fn nonzero(v: Grid<f32, Dyn>) -> Grid<f32, ?n> {\n    v\n}\n";
    let module = parse_ok(source);
    let Item::Fn(function) = &module.items[0] else { panic!("expected fn") };
    let Some(Ty::Generic { args, .. }) = &function.return_ty else { panic!("expected generic return type") };
    assert!(matches!(&args[1], Ty::Existential(name, _) if name == "n"));
    let formatted = format_module(&module, Some(source));
    assert!(formatted.contains("-> Grid<f32, ?n>"), "{formatted}");
}

#[test]
fn an_existential_outside_a_return_or_field_type_is_rejected() {
    let (_, reporter) = parse("fn f(g: Grid<f32, ?n>) {}");
    assert!(reporter.diagnostics().iter().any(|d| d.primary().message.contains("only in a return type or a struct field type")));
    let (_, reporter) = parse("fn f() { let g: Grid<f32, ?n> = x; }");
    assert!(reporter.has_errors());
    parse_ok("struct Batch { x: Grid<f32, ?b, 784>, y: Grid<i64, ?b> }");
}
