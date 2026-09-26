use paco_diag::Reporter;
use paco_span::SourceMap;
use paco_syntax::{ast::Item, lex::lex, parse::parse_module};
use paco_types::{IntWidth, Type, check_module, infer_module};

#[test]
fn comptime_block_type_checks_as_its_inner_expressions_type() {
    let mut sources = SourceMap::new();
    let file = sources.add_file("main.paco", "fn main() -> i64 { comptime { 1 + 2 } }");
    let mut reporter = Reporter::new();
    let tokens = lex(sources.source(file).unwrap(), file, &mut reporter);
    let module = parse_module(&tokens, &mut reporter).expect("source should parse");
    let typed = match infer_module(&module, &mut reporter) {
        Ok(typed) => typed,
        Err(_) => panic!("module should type-check: {}", reporter.emit_to_string(&sources)),
    };

    let Item::Fn(function) = &module.items[0] else {
        panic!("expected fn item");
    };
    let tail = function.body.tail.as_deref().expect("expected a tail expression");
    assert_eq!(typed.type_of(tail), Some(&Type::Int(IntWidth::I64)));
}

#[test]
fn a_type_typed_parameter_type_checks_as_a_type_value_of_unknown_content() {
    let mut sources = SourceMap::new();
    let file = sources.add_file("main.paco", "fn f(t: type) {}\nfn main() {}");
    let mut reporter = Reporter::new();
    let tokens = lex(sources.source(file).unwrap(), file, &mut reporter);
    let module = parse_module(&tokens, &mut reporter).expect("source should parse");
    let typed = match infer_module(&module, &mut reporter) {
        Ok(typed) => typed,
        Err(_) => panic!("module should type-check: {}", reporter.emit_to_string(&sources)),
    };

    let Item::Fn(function) = &module.items[0] else {
        panic!("expected fn item");
    };
    let param = &function.params[0];
    assert_eq!(typed.type_of_param(param), Some(&Type::TypeValue(Box::new(Type::Unknown))));
}

#[test]
fn an_ordinary_program_with_no_type_valued_parameters_is_unaffected() {
    let mut sources = SourceMap::new();
    let file = sources.add_file("main.paco", "fn add(a: i64, b: i64) -> i64 { a + b }\nfn main() { print(add(1, 2)) }");
    let mut reporter = Reporter::new();
    let tokens = lex(sources.source(file).unwrap(), file, &mut reporter);
    let module = parse_module(&tokens, &mut reporter).expect("source should parse");

    assert!(check_module(&module, &mut reporter).is_ok(), "{}", reporter.emit_to_string(&sources));
}

#[test]
fn a_bare_struct_name_resolves_as_a_type_value_call_argument() {
    let mut sources = SourceMap::new();
    let file = sources.add_file(
        "main.paco",
        "struct Foo { x: i64 }\nfn f(t: type) {}\nfn main() { f(Foo) }",
    );
    let mut reporter = Reporter::new();
    let tokens = lex(sources.source(file).unwrap(), file, &mut reporter);
    let module = parse_module(&tokens, &mut reporter).expect("source should parse");
    let typed = match infer_module(&module, &mut reporter) {
        Ok(typed) => typed,
        Err(_) => panic!("module should type-check: {}", reporter.emit_to_string(&sources)),
    };

    let Item::Fn(main_fn) = &module.items[2] else {
        panic!("expected fn item");
    };
    let call_stmt = main_fn.body.tail.as_deref().expect("expected a tail expression");
    let paco_syntax::ast::Expr::Call { args, .. } = call_stmt else {
        panic!("expected a call expression, found {call_stmt:?}");
    };
    assert_eq!(
        typed.type_of(&args[0]),
        Some(&Type::TypeValue(Box::new(Type::Struct("Foo".to_string(), Vec::new()))))
    );
}

#[test]
fn a_bare_identifier_used_as_an_ordinary_value_is_unaffected() {
    let mut sources = SourceMap::new();
    let file = sources.add_file("main.paco", "fn main() { let x: i64 = 1;\n print(x) }");
    let mut reporter = Reporter::new();
    let tokens = lex(sources.source(file).unwrap(), file, &mut reporter);
    let module = parse_module(&tokens, &mut reporter).expect("source should parse");

    assert!(check_module(&module, &mut reporter).is_ok(), "{}", reporter.emit_to_string(&sources));
}

#[test]
fn a_generic_struct_name_as_a_type_argument_is_a_type_error() {
    let mut sources = SourceMap::new();
    let file = sources.add_file(
        "main.paco",
        "struct Box<T> { value: T }\nfn f(t: type) {}\nfn main() { f(Box) }",
    );
    let mut reporter = Reporter::new();
    let tokens = lex(sources.source(file).unwrap(), file, &mut reporter);
    let module = parse_module(&tokens, &mut reporter).expect("source should parse");

    let result = infer_module(&module, &mut reporter);
    assert!(result.is_err());
    assert!(reporter.diagnostics().iter().any(|d| d.code() == "PACO-E0351"));
}

#[test]
fn calling_a_comptime_fn_outside_comptime_is_rejected() {
    let mut sources = SourceMap::new();
    let file = sources.add_file(
        "main.paco",
        "comptime fn helper() -> i64 { 1 }\nfn main() { helper() }",
    );
    let mut reporter = Reporter::new();
    let tokens = lex(sources.source(file).unwrap(), file, &mut reporter);
    let module = parse_module(&tokens, &mut reporter).expect("source should parse");

    let result = infer_module(&module, &mut reporter);
    assert!(result.is_err());
    assert!(reporter.diagnostics().iter().any(|d| d.code() == "PACO-E0352"));
}

#[test]
fn calling_a_comptime_fn_inside_a_comptime_block_is_accepted() {
    let mut sources = SourceMap::new();
    let file = sources.add_file(
        "main.paco",
        "comptime fn helper() -> i64 { 1 }\nfn main() -> i64 { comptime { helper() } }",
    );
    let mut reporter = Reporter::new();
    let tokens = lex(sources.source(file).unwrap(), file, &mut reporter);
    let module = parse_module(&tokens, &mut reporter).expect("source should parse");

    assert!(infer_module(&module, &mut reporter).is_ok(), "{}", reporter.emit_to_string(&sources));
}

#[test]
fn a_quote_expression_type_checks_as_code_inside_comptime() {
    let mut sources = SourceMap::new();
    let file = sources.add_file(
        "main.paco",
        "fn main() { let c = comptime { quote { 1 + 2 } }; }",
    );
    let mut reporter = Reporter::new();
    let tokens = lex(sources.source(file).unwrap(), file, &mut reporter);
    let module = parse_module(&tokens, &mut reporter).expect("source should parse");
    let typed = match infer_module(&module, &mut reporter) {
        Ok(typed) => typed,
        Err(_) => panic!("module should type-check: {}", reporter.emit_to_string(&sources)),
    };

    let Item::Fn(function) = &module.items[0] else {
        panic!("expected fn item");
    };
    let paco_syntax::ast::Stmt::Let(let_stmt) = &function.body.stmts[0] else {
        panic!("expected a let statement");
    };
    let value = let_stmt.value.as_ref().expect("expected a let value");
    let paco_syntax::ast::Expr::Comptime { expr: quote_expr, .. } = value else {
        panic!("expected a comptime block, found {value:?}");
    };
    assert_eq!(typed.type_of(quote_expr), Some(&Type::Code));
}

#[test]
fn a_quote_expression_outside_comptime_is_rejected() {
    let mut sources = SourceMap::new();
    let file = sources.add_file("main.paco", "fn main() { quote { 1 } }");
    let mut reporter = Reporter::new();
    let tokens = lex(sources.source(file).unwrap(), file, &mut reporter);
    let module = parse_module(&tokens, &mut reporter).expect("source should parse");

    let result = infer_module(&module, &mut reporter);
    assert!(result.is_err());
    assert!(reporter.diagnostics().iter().any(|d| d.code() == "PACO-E0352"));
}

#[test]
fn a_splice_inside_a_quote_template_is_type_checked_against_the_enclosing_scope() {
    // The template's own literal structure (`self`, `hello`, ...) is not
    // resolved/type-checked as ordinary code (`phase-9-comptime` Decision
    // 7) — only `#(missing)`'s own inner expression is, and it references
    // a name the enclosing scope never defines.
    let mut sources = SourceMap::new();
    let file = sources.add_file(
        "main.paco",
        "struct User { age: i64 }\nfn main() { comptime { quote { methods #(missing) { fn hello(&self) -> i64 { 1 } } } } }",
    );
    let mut reporter = Reporter::new();
    let tokens = lex(sources.source(file).unwrap(), file, &mut reporter);
    let module = parse_module(&tokens, &mut reporter).expect("source should parse");

    let result = infer_module(&module, &mut reporter);
    assert!(result.is_err());
    assert!(reporter.diagnostics().iter().any(|d| d.code() == "PACO-E0319"), "{}", reporter.emit_to_string(&sources));
}

#[test]
fn a_type_typed_parameter_passed_to_another_type_typed_parameter_type_checks() {
    // Bug found implementing `phase-9-comptime` task 7.1 (`derive_display`
    // calling `fields_of(t)`/`type_name(t)` on its own `t: type`
    // parameter): `infer_type_value_arg` resolved every bare-identifier
    // `type` argument as a literal struct/enum name, even when the
    // identifier was already a bound `type`-typed parameter — reporting
    // "unresolved identifier" for the parameter's own name instead of
    // reusing its already-known `Type::TypeValue`.
    let mut sources = SourceMap::new();
    let file = sources.add_file(
        "main.paco",
        "struct Foo { x: i64 }\nfn inner(t: type) {}\nfn outer(t: type) { inner(t) }\nfn main() { outer(Foo) }",
    );
    let mut reporter = Reporter::new();
    let tokens = lex(sources.source(file).unwrap(), file, &mut reporter);
    let module = parse_module(&tokens, &mut reporter).expect("source should parse");

    assert!(infer_module(&module, &mut reporter).is_ok(), "{}", reporter.emit_to_string(&sources));
}
