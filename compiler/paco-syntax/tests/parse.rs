use paco_diag::Reporter;
use paco_span::SourceMap;
use paco_syntax::{
    ast::{BinaryOp, Expr, Item, Literal, Pat, Stmt, Ty, UsePathKind, VariantFields},
    lex::lex,
    parse::parse_module,
};

#[test]
fn parser_preserves_multiplication_precedence_over_addition() {
    let mut sources = SourceMap::new();
    let file = sources.add_file("main.paco", "fn main() -> i64 { 1 + 2 * 3 }");
    let mut reporter = Reporter::new();
    let tokens = lex(sources.source(file).unwrap(), file, &mut reporter);

    let module = parse_module(&tokens, &mut reporter).unwrap();

    let Item::Fn(function) = &module.items[0] else {
        panic!("expected function item");
    };
    let Some(tail) = &function.body.tail else {
        panic!("expected function tail expression");
    };
    let Expr::Binary {
        op: BinaryOp::Add,
        left,
        right,
        ..
    } = tail.as_ref()
    else {
        panic!("expected addition at expression root");
    };
    assert!(matches!(left.as_ref(), Expr::Literal(Literal::Int(1), _)));
    assert!(matches!(
        right.as_ref(),
        Expr::Binary {
            op: BinaryOp::Mul,
            ..
        }
    ));
    assert!(!reporter.has_errors());
}

#[test]
fn parser_parses_struct_with_fields() {
    let module = parse_source("struct Point { x: i64, y: i64 }");

    let Item::Struct(point) = &module.items[0] else {
        panic!("expected struct item");
    };
    assert_eq!(point.name, "Point");
    assert_eq!(point.fields.len(), 2);
    assert_eq!(point.fields[0].name, "x");
    assert_eq!(point.fields[1].name, "y");
}

#[test]
fn parser_parses_module_level_const() {
    let module = parse_source("const EPS: f32 = 0.000001;");

    let Item::Const(decl) = &module.items[0] else {
        panic!("expected const item");
    };
    assert_eq!(decl.name, "EPS");
    assert!(matches!(&decl.ty, Ty::Path(path, _) if path == &["f32".to_string()]));
}

#[test]
fn parser_rejects_const_without_a_type_annotation() {
    let mut sources = SourceMap::new();
    let file = sources.add_file("main.paco", "const TILE = 64");
    let mut reporter = Reporter::new();
    let tokens = lex(sources.source(file).unwrap(), file, &mut reporter);

    let result = parse_module(&tokens, &mut reporter);

    assert!(result.is_err());
    assert!(reporter.has_errors());
}

#[test]
fn parser_parses_associated_consts() {
    let module = parse_source("struct Tensor<T> { data: T, const RANK: i64 = 2; }");
    let Item::Struct(decl) = &module.items[0] else {
        panic!("expected struct item");
    };
    assert_eq!(decl.consts.len(), 1);
    assert_eq!(decl.consts[0].name, "RANK");

    let module = parse_source("enum Shape { Circle(i64), const DEFAULT_RADIUS: i64 = 1; }");
    let Item::Enum(decl) = &module.items[0] else {
        panic!("expected enum item");
    };
    assert_eq!(decl.consts.len(), 1);
    assert_eq!(decl.consts[0].name, "DEFAULT_RADIUS");

    let module = parse_source("methods Point { const ORIGIN_X: i64 = 0; fn dummy() {} }");
    let Item::Methods(decl) = &module.items[0] else {
        panic!("expected methods item");
    };
    assert_eq!(decl.consts.len(), 1);
    assert_eq!(decl.consts[0].name, "ORIGIN_X");
}

#[test]
fn parser_parses_question_mark_after_a_call() {
    let module = parse_source("fn main() { read_file(path)? }");
    let Item::Fn(function) = &module.items[0] else {
        panic!("expected function item");
    };
    let tail = function.body.tail.as_deref().unwrap();
    let Expr::Try { expr, .. } = tail else {
        panic!("expected Try expression");
    };
    assert!(matches!(expr.as_ref(), Expr::Call { .. }));
}

#[test]
fn parser_parses_question_mark_after_a_plain_identifier() {
    let module = parse_source("fn main() { result? }");
    let Item::Fn(function) = &module.items[0] else {
        panic!("expected function item");
    };
    let tail = function.body.tail.as_deref().unwrap();
    let Expr::Try { expr, .. } = tail else {
        panic!("expected Try expression");
    };
    assert!(matches!(expr.as_ref(), Expr::Ident(name, _) if name == "result"));
}

#[test]
fn parser_parses_question_mark_chained_after_a_method_call() {
    let module = parse_source("fn main() { x.foo()? }");
    let Item::Fn(function) = &module.items[0] else {
        panic!("expected function item");
    };
    let tail = function.body.tail.as_deref().unwrap();
    let Expr::Try { expr, .. } = tail else {
        panic!("expected Try expression");
    };
    assert!(matches!(expr.as_ref(), Expr::MethodCall { .. }));
}

#[test]
fn parser_parses_a_leading_module_declaration() {
    let module = parse_source("module nn; fn main() {}");
    assert_eq!(module.name.as_ref().map(|decl| decl.name.as_str()), Some("nn"));
    assert_eq!(module.items.len(), 1);
}

#[test]
fn parser_parses_a_file_with_no_module_declaration_unchanged() {
    let module = parse_source("fn main() {}");
    assert!(module.name.is_none());
    assert_eq!(module.items.len(), 1);
}

#[test]
fn parser_rejects_a_module_declaration_after_an_item() {
    let mut sources = SourceMap::new();
    let file = sources.add_file("main.paco", "fn helper() {} module nn");
    let mut reporter = Reporter::new();
    let tokens = lex(sources.source(file).unwrap(), file, &mut reporter);

    parse_module(&tokens, &mut reporter).ok();

    assert!(reporter.has_errors());
}

#[test]
fn parser_parses_pub_on_fn_struct_and_enum() {
    let module = parse_source("pub fn forward() {} pub struct Linear { w: i64 } pub enum Shape { Circle }");
    let Item::Fn(f) = &module.items[0] else { panic!("expected fn") };
    assert!(f.is_pub);
    let Item::Struct(s) = &module.items[1] else { panic!("expected struct") };
    assert!(s.is_pub);
    let Item::Enum(e) = &module.items[2] else { panic!("expected enum") };
    assert!(e.is_pub);
}

#[test]
fn parser_parses_the_adr_0015_linear_example() {
    let module =
        parse_source("pub struct Linear { w: i64, pub b: i64, pub fn forward(&self) {} fn init_weights(&mut self) {} }");
    let Item::Struct(decl) = &module.items[0] else {
        panic!("expected struct")
    };
    assert!(decl.is_pub);
    assert!(!decl.fields[0].is_pub);
    assert!(decl.fields[1].is_pub);
    assert!(decl.methods[0].is_pub);
    assert!(!decl.methods[1].is_pub);
}

#[test]
fn parser_rejects_pub_directly_before_an_enum_variant() {
    let mut sources = SourceMap::new();
    let file = sources.add_file("main.paco", "enum E { pub Variant, }");
    let mut reporter = Reporter::new();
    let tokens = lex(sources.source(file).unwrap(), file, &mut reporter);

    parse_module(&tokens, &mut reporter).ok();

    assert!(reporter.has_errors());
}

#[test]
fn parser_parses_std_style_and_domain_style_module_paths() {
    let module = parse_source("use stdlib::io;");
    let Item::Use(decl) = &module.items[0] else {
        panic!("expected use")
    };
    assert_eq!(decl.path, vec!["stdlib".to_string(), "io".to_string()]);
    assert!(decl.alias.is_none());

    let module = parse_source("use example.com/team/tensor;");
    let Item::Use(decl) = &module.items[0] else {
        panic!("expected use")
    };
    assert_eq!(
        decl.path,
        vec![
            "example".to_string(),
            "com".to_string(),
            "team".to_string(),
            "tensor".to_string(),
        ]
    );
}

#[test]
fn parser_records_plain_vs_domain_use_path_kind() {
    let module = parse_source("use a::b::c;");
    let Item::Use(decl) = &module.items[0] else {
        panic!("expected use")
    };
    assert_eq!(decl.kind, UsePathKind::Plain);

    let module = parse_source("use a.b/c;");
    let Item::Use(decl) = &module.items[0] else {
        panic!("expected use")
    };
    assert_eq!(decl.kind, UsePathKind::Domain);
}

#[test]
fn parser_rejects_malformed_domain_paths() {
    for source in ["use example.com//tensor", "use example.com/"] {
        let mut sources = SourceMap::new();
        let file = sources.add_file("main.paco", source);
        let mut reporter = Reporter::new();
        let tokens = lex(sources.source(file).unwrap(), file, &mut reporter);

        let result = parse_module(&tokens, &mut reporter);

        assert!(result.is_err() || reporter.has_errors(), "{source} should fail to parse");
    }
}

#[test]
fn parser_parses_an_aliased_use_import() {
    let module = parse_source("use example.com/team/json as parser;");
    let Item::Use(decl) = &module.items[0] else {
        panic!("expected use")
    };
    assert_eq!(decl.alias.as_deref(), Some("parser"));
}

#[test]
fn parser_parses_assoc_type_with_and_without_default() {
    let module = parse_source("trait A { type Output; }");
    let Item::Trait(decl) = &module.items[0] else {
        panic!("expected trait item");
    };
    assert_eq!(decl.assoc_types.len(), 1);
    assert_eq!(decl.assoc_types[0].name, "Output");
    assert!(decl.assoc_types[0].default.is_none());

    let module = parse_source("trait A { type Output = i64; }");
    let Item::Trait(decl) = &module.items[0] else {
        panic!("expected trait item");
    };
    assert!(matches!(
        &decl.assoc_types[0].default,
        Some(Ty::Path(path, _)) if path == &["i64".to_string()]
    ));
}

#[test]
fn parser_parses_trait_methods_with_and_without_a_body() {
    let module = parse_source("trait Shape { fn area(&self) -> float; }");
    let Item::Trait(decl) = &module.items[0] else {
        panic!("expected trait item");
    };
    assert_eq!(decl.methods[0].name, "area");
    assert!(decl.methods[0].body.is_none());

    let module = parse_source("trait Shape { fn describe(&self) -> string { \"x\" } }");
    let Item::Trait(decl) = &module.items[0] else {
        panic!("expected trait item");
    };
    assert_eq!(decl.methods[0].name, "describe");
    assert!(decl.methods[0].body.is_some());
}

#[test]
fn parser_parses_a_full_trait_declaration() {
    let module = parse_source(
        "trait Index<Idx> { type Output; fn index(&self, i: Idx) -> &Self::Output; }",
    );
    let Item::Trait(decl) = &module.items[0] else {
        panic!("expected trait item");
    };
    assert_eq!(decl.name, "Index");
    assert_eq!(decl.generics, vec!["Idx".to_string()]);
    assert_eq!(decl.assoc_types.len(), 1);
    assert_eq!(decl.assoc_types[0].name, "Output");
    assert_eq!(decl.methods.len(), 1);
    assert_eq!(decl.methods[0].name, "index");
    assert!(decl.methods[0].body.is_none());
}

#[test]
fn parser_wires_trait_into_module_item_dispatch() {
    let module = parse_source("trait Greet { fn hello(&self) -> string; } fn main() {}");
    assert!(matches!(module.items[0], Item::Trait(_)));
    assert!(matches!(module.items[1], Item::Fn(_)));
}

#[test]
fn parser_parses_enum_with_tuple_and_unit_variants() {
    let module = parse_source("enum Maybe { Some(i64), None }");

    let Item::Enum(maybe) = &module.items[0] else {
        panic!("expected enum item");
    };
    assert_eq!(maybe.name, "Maybe");
    assert_eq!(maybe.variants.len(), 2);
    assert_eq!(maybe.variants[0].name, "Some");
    assert!(matches!(maybe.variants[0].fields, VariantFields::Tuple(_)));
    assert!(matches!(maybe.variants[1].fields, VariantFields::Unit));
}

#[test]
fn parser_parses_struct_literal_and_field_access() {
    let module = parse_source("fn main() { let p = Point { x: 1, y: 2 }; p.x }");
    let Item::Fn(function) = &module.items[0] else {
        panic!("expected function item");
    };
    let Some(tail) = &function.body.tail else {
        panic!("expected tail expression");
    };

    assert!(matches!(
        tail.as_ref(),
        Expr::Field { field, .. } if field == "x"
    ));
    let Expr::Field { base, .. } = tail.as_ref() else {
        panic!("expected field access");
    };
    assert!(matches!(base.as_ref(), Expr::Ident(name, _) if name == "p"));
    let Some(Expr::StructLiteral { fields, .. }) =
        function
            .body
            .stmts
            .iter()
            .find_map(|statement| match statement {
                paco_syntax::ast::Stmt::Let(statement) => statement.value.as_ref(),
                _ => None,
            })
    else {
        panic!("expected struct literal in let initializer");
    };
    assert_eq!(fields.len(), 2);
}

#[test]
fn parser_parses_associated_function_call() {
    let module = parse_source("fn main() { Point::origin() }");
    let Item::Fn(function) = &module.items[0] else {
        panic!("expected function item");
    };
    let Some(tail) = &function.body.tail else {
        panic!("expected tail expression");
    };

    assert!(matches!(
        tail.as_ref(),
        Expr::AssociatedCall { function, .. } if function == "origin"
    ));
}

#[test]
fn parser_parses_method_with_self_receiver_inside_struct() {
    let module = parse_source("struct Point { x: i64, fn value(&self) -> i64 { self.x } }");
    let Item::Struct(point) = &module.items[0] else {
        panic!("expected struct item");
    };

    assert_eq!(point.methods.len(), 1);
    assert_eq!(point.methods[0].name, "value");
    assert_eq!(point.methods[0].params.len(), 1);
    assert!(matches!(
        &point.methods[0].params[0].ty,
        Ty::Borrow {
            mutable: false,
            ty,
            ..
        } if matches!(ty.as_ref(), Ty::Path(path, _) if path == &vec!["Self".to_string()])
    ));
}

#[test]
fn parser_parses_slice_borrow_and_mut_borrow_types() {
    let mut sources = SourceMap::new();
    let file = sources.add_file(
        "main.paco",
        "struct Weights { data: []f32 } fn sum(values: &[]f32) -> f32 { 0.0 } fn fill(values: &mut []f32) {}",
    );
    let mut reporter = Reporter::new();
    let tokens = lex(sources.source(file).unwrap(), file, &mut reporter);
    let module = parse_module(&tokens, &mut reporter).unwrap();
    assert!(!reporter.has_errors());

    let Item::Struct(weights) = &module.items[0] else {
        panic!("expected struct item");
    };
    assert!(matches!(
        &weights.fields[0].ty,
        Ty::Slice(elem, _) if matches!(elem.as_ref(), Ty::Path(path, _) if path == &vec!["f32".to_string()])
    ));

    let Item::Fn(sum) = &module.items[1] else {
        panic!("expected fn item");
    };
    assert!(matches!(
        &sum.params[0].ty,
        Ty::Borrow { mutable: false, ty, .. } if matches!(ty.as_ref(), Ty::Slice(elem, _) if matches!(elem.as_ref(), Ty::Path(path, _) if path == &vec!["f32".to_string()]))
    ));

    let Item::Fn(fill) = &module.items[2] else {
        panic!("expected fn item");
    };
    assert!(matches!(
        &fill.params[0].ty,
        Ty::Borrow { mutable: true, ty, .. } if matches!(ty.as_ref(), Ty::Slice(elem, _) if matches!(elem.as_ref(), Ty::Path(path, _) if path == &vec!["f32".to_string()]))
    ));
}

#[test]
fn parser_parses_single_and_multi_dimensional_indexing() {
    let mut sources = SourceMap::new();
    let file = sources.add_file(
        "main.paco",
        r#"
        fn main() {
            a[i];
            a[i, j];
            a[i, j, k]
        }
        "#,
    );
    let mut reporter = Reporter::new();
    let tokens = lex(sources.source(file).unwrap(), file, &mut reporter);
    let module = parse_module(&tokens, &mut reporter).unwrap();
    assert!(!reporter.has_errors());

    let Item::Fn(main) = &module.items[0] else {
        panic!("expected fn item");
    };
    let index_lens: Vec<usize> = main
        .body
        .stmts
        .iter()
        .map(|stmt| {
            let Stmt::Expr(Expr::Index { index, .. }) = stmt else {
                panic!("expected an index expression statement, found {stmt:?}");
            };
            index.len()
        })
        .collect();
    assert_eq!(index_lens, vec![1, 2]);

    // The third statement is the tail expression, not counted in `stmts`.
    let Some(tail) = &main.body.tail else {
        panic!("expected a tail expression");
    };
    let Expr::Index { index, .. } = tail.as_ref() else {
        panic!("expected an index expression tail");
    };
    assert_eq!(index.len(), 3);
}

#[test]
fn parser_parses_a_qualified_struct_literal_from_a_used_module() {
    let mut sources = SourceMap::new();
    let file = sources.add_file("main.paco", "fn main() { a::Point { x: 1, y: 2 } }");
    let mut reporter = Reporter::new();
    let tokens = lex(sources.source(file).unwrap(), file, &mut reporter);
    let module = parse_module(&tokens, &mut reporter).unwrap();
    assert!(!reporter.has_errors());

    let Item::Fn(main) = &module.items[0] else {
        panic!("expected fn item");
    };
    let Some(tail) = &main.body.tail else {
        panic!("expected tail expression");
    };
    let Expr::StructLiteral { ty, fields, .. } = tail.as_ref() else {
        panic!("expected a struct literal, found {tail:?}");
    };
    assert!(matches!(ty, Ty::Path(path, _) if path == &vec!["a".to_string(), "Point".to_string()]));
    assert_eq!(fields.len(), 2);
}

#[test]
fn parser_still_parses_an_unqualified_associated_call_and_unit_variant() {
    let mut sources = SourceMap::new();
    let file = sources.add_file(
        "main.paco",
        "fn main() { let p = Point::new(1, 2); let o = Option::None; }",
    );
    let mut reporter = Reporter::new();
    let tokens = lex(sources.source(file).unwrap(), file, &mut reporter);
    let module = parse_module(&tokens, &mut reporter).unwrap();
    assert!(!reporter.has_errors());

    let Item::Fn(main) = &module.items[0] else {
        panic!("expected fn item");
    };
    let Stmt::Let(let_p) = &main.body.stmts[0] else {
        panic!("expected let statement");
    };
    assert!(matches!(
        let_p.value.as_ref().unwrap(),
        Expr::AssociatedCall { function, args, .. } if function == "new" && args.len() == 2
    ));
    let Stmt::Let(let_o) = &main.body.stmts[1] else {
        panic!("expected let statement");
    };
    assert!(matches!(
        let_o.value.as_ref().unwrap(),
        Expr::AssociatedCall { function, args, .. } if function == "None" && args.is_empty()
    ));
}

#[test]
fn parser_parses_generic_struct_type_application() {
    let module = parse_source(
        "struct Box<T> { value: T } fn main() { let b: Box<i64> = Box<i64> { value: 1 }; b.value }",
    );

    let Item::Struct(container) = &module.items[0] else {
        panic!("expected struct item");
    };
    assert_eq!(container.generics, vec!["T"]);
    let Item::Fn(function) = &module.items[1] else {
        panic!("expected function item");
    };
    let ty = match &function.body.stmts[0] {
        paco_syntax::ast::Stmt::Let(statement) => {
            statement.ty.as_ref().expect("expected annotated let")
        }
        _ => panic!("expected let statement"),
    };
    assert!(
        matches!(ty, Ty::Generic { path, args, .. } if path == &vec!["Box".to_string()] && args.len() == 1)
    );
}

#[test]
fn parser_parses_match_with_literal_and_wildcard_arms() {
    let module = parse_source("fn main() -> i64 { match value { 0 => 1, _ => 2 } }");
    let Item::Fn(function) = &module.items[0] else {
        panic!("expected function item");
    };
    let Some(tail) = &function.body.tail else {
        panic!("expected tail expression");
    };
    let Expr::Match { arms, .. } = tail.as_ref() else {
        panic!("expected match expression");
    };

    assert_eq!(arms.len(), 2);
    assert!(matches!(arms[0].pattern, Pat::Literal(Literal::Int(0), _)));
    assert!(matches!(arms[1].pattern, Pat::Wildcard(_)));
}

#[test]
fn parser_parses_guarded_enum_variant_pattern() {
    let module = parse_source(
        "fn main() -> i64 { match value { Maybe::Some(x) if x > 0 => x, Maybe::None => 0 } }",
    );
    let Item::Fn(function) = &module.items[0] else {
        panic!("expected function item");
    };
    let Some(tail) = &function.body.tail else {
        panic!("expected tail expression");
    };
    let Expr::Match { arms, .. } = tail.as_ref() else {
        panic!("expected match expression");
    };

    assert_eq!(arms.len(), 2);
    assert!(arms[0].guard.is_some());
    assert!(
        matches!(&arms[0].pattern, Pat::Enum { path, fields, .. } if path == &vec!["Maybe".to_string(), "Some".to_string()] && fields.len() == 1)
    );
    assert!(
        matches!(&arms[1].pattern, Pat::Enum { path, fields, .. } if path == &vec!["Maybe".to_string(), "None".to_string()] && fields.is_empty())
    );
}

#[test]
fn parser_parses_at_binding_with_range_pattern() {
    let module = parse_source("fn main() -> i64 { match n { digit @ 1..=9 => digit, _ => 0 } }");
    let Item::Fn(function) = &module.items[0] else {
        panic!("expected function item");
    };
    let Some(tail) = &function.body.tail else {
        panic!("expected tail expression");
    };
    let Expr::Match { arms, .. } = tail.as_ref() else {
        panic!("expected match expression");
    };

    assert!(matches!(
        &arms[0].pattern,
        Pat::Binding { name, pattern, .. }
            if name == "digit" && matches!(pattern.as_ref(), Pat::Range { inclusive: true, .. })
    ));
}

#[test]
fn parser_desugars_if_let_to_match_expression() {
    let module =
        parse_source("fn main() -> i64 { if let Maybe::Some(x) = value { x } else { 0 } }");
    let Item::Fn(function) = &module.items[0] else {
        panic!("expected function item");
    };
    let Some(tail) = &function.body.tail else {
        panic!("expected tail expression");
    };
    let Expr::Match { arms, .. } = tail.as_ref() else {
        panic!("expected if let to parse as match expression");
    };

    assert_eq!(arms.len(), 2);
    assert!(
        matches!(&arms[0].pattern, Pat::Enum { path, fields, .. } if path == &vec!["Maybe".to_string(), "Some".to_string()] && fields.len() == 1)
    );
    assert!(matches!(arms[1].pattern, Pat::Wildcard(_)));
}

#[test]
fn parser_desugars_while_let_to_loop_with_match() {
    let module = parse_source("fn main() { while let Maybe::Some(x) = next() { print(x) } }");
    let Item::Fn(function) = &module.items[0] else {
        panic!("expected function item");
    };
    let Some(tail) = &function.body.tail else {
        panic!("expected tail expression");
    };
    let Expr::Loop { body, .. } = tail.as_ref() else {
        panic!("expected while let to parse as loop expression");
    };
    let paco_syntax::ast::Stmt::Expr(Expr::Match { arms, .. }) = &body.stmts[0] else {
        panic!("expected loop body to contain match expression");
    };

    assert_eq!(arms.len(), 2);
    assert!(
        matches!(&arms[0].pattern, Pat::Enum { path, fields, .. } if path == &vec!["Maybe".to_string(), "Some".to_string()] && fields.len() == 1)
    );
    assert!(matches!(arms[1].pattern, Pat::Wildcard(_)));
}

#[test]
fn parser_desugars_for_range_to_block_expression() {
    let module = parse_source("fn main() { for n in 1..=3 { print(n) } }");
    let Item::Fn(function) = &module.items[0] else {
        panic!("expected function item");
    };
    let Some(tail) = &function.body.tail else {
        panic!("expected tail expression");
    };
    let Expr::Block(block) = tail.as_ref() else {
        panic!("expected for range to parse as block expression");
    };

    assert_eq!(block.stmts.len(), 2);
    let paco_syntax::ast::Stmt::Let(cursor) = &block.stmts[0] else {
        panic!("expected generated cursor binding");
    };
    assert!(
        matches!(&cursor.pattern, Pat::Ident(name, _) if name.starts_with("$paco_for_cursor_"))
    );
    assert!(matches!(
        block.stmts[1],
        paco_syntax::ast::Stmt::Expr(Expr::Loop { .. })
    ));
}

#[test]
fn parser_desugars_for_over_a_non_range_expression_to_a_next_dispatched_loop() {
    let module = parse_source("fn main() { for x in items { print(x) } }");
    let Item::Fn(function) = &module.items[0] else {
        panic!("expected function item");
    };
    let Some(tail) = &function.body.tail else {
        panic!("expected tail expression");
    };
    let Expr::Block(block) = tail.as_ref() else {
        panic!("expected for-over-iter to parse as block expression");
    };

    assert_eq!(block.stmts.len(), 2);
    let paco_syntax::ast::Stmt::Let(cursor) = &block.stmts[0] else {
        panic!("expected generated iterator binding");
    };
    assert!(matches!(&cursor.pattern, Pat::Ident(name, _) if name.starts_with("$paco_for_iter_")));
    assert!(matches!(&cursor.value, Some(Expr::Ident(name, _)) if name == "items"));

    let paco_syntax::ast::Stmt::Expr(Expr::Loop { body, .. }) = &block.stmts[1] else {
        panic!("expected loop in for-over-iter block");
    };
    let paco_syntax::ast::Stmt::Expr(Expr::Match { scrutinee, arms, .. }) = &body.stmts[0] else {
        panic!("expected match on `.next()` in loop body");
    };
    let Expr::MethodCall { method, args, .. } = scrutinee.as_ref() else {
        panic!("expected `.next()` method call as match scrutinee");
    };
    assert_eq!(method, "next");
    assert!(args.is_empty());

    assert_eq!(arms.len(), 2);
    assert!(
        matches!(&arms[0].pattern, Pat::Enum { path, fields, .. } if path == &vec!["Option".to_string(), "Some".to_string()] && fields.len() == 1)
    );
    assert!(
        matches!(&arms[1].pattern, Pat::Enum { path, fields, .. } if path == &vec!["Option".to_string(), "None".to_string()] && fields.is_empty())
    );
    assert!(matches!(&arms[1].body, Expr::Break(None, _)));
}

#[test]
fn parser_rewrites_for_range_continue_to_increment_first() {
    let module = parse_source("fn main() { for n in 1..=3 { if n == 2 { continue } print(n) } }");
    let Item::Fn(function) = &module.items[0] else {
        panic!("expected function item");
    };
    let Some(Expr::Block(block)) = function.body.tail.as_deref() else {
        panic!("expected for range block");
    };
    let paco_syntax::ast::Stmt::Expr(Expr::Loop { body, .. }) = &block.stmts[1] else {
        panic!("expected loop in for range block");
    };
    let paco_syntax::ast::Stmt::Expr(Expr::If { then_branch, .. }) = &body.stmts[0] else {
        panic!("expected loop body guard");
    };
    let paco_syntax::ast::Stmt::Expr(Expr::If {
        then_branch: continue_branch,
        ..
    }) = &then_branch.stmts[1]
    else {
        panic!("expected user continue guard");
    };
    let Some(Expr::Block(continue_block)) = continue_branch.tail.as_deref() else {
        panic!("expected continue to be rewritten to a block");
    };

    assert!(matches!(
        continue_block.stmts[0],
        paco_syntax::ast::Stmt::Expr(Expr::Assign { .. })
    ));
    assert!(matches!(
        continue_block.stmts[1],
        paco_syntax::ast::Stmt::Expr(Expr::Continue(_))
    ));
}

#[test]
fn parser_parses_extern_block_with_one_function() {
    let module = parse_source("extern \"C\" { fn cblas_sgemm(a: *const f32, n: i64); }");

    let Item::Extern(block) = &module.items[0] else {
        panic!("expected extern item");
    };
    assert_eq!(block.abi, "C");
    assert_eq!(block.functions.len(), 1);
    assert_eq!(block.functions[0].name, "cblas_sgemm");
    assert!(block.functions[0].body.is_none());
}

#[test]
fn parser_rejects_a_body_on_an_extern_function() {
    let mut sources = SourceMap::new();
    let file = sources.add_file("main.paco", "extern \"C\" { fn cblas_sgemm(a: i64) { } }");
    let mut reporter = Reporter::new();
    let tokens = lex(sources.source(file).unwrap(), file, &mut reporter);

    let result = parse_module(&tokens, &mut reporter);

    assert!(result.is_err());
    assert!(reporter.has_errors());
}

#[test]
fn parser_parses_raw_pointer_types() {
    let module = parse_source("fn f(a: *const i64, b: *mut i64) { }");

    let Item::Fn(function) = &module.items[0] else {
        panic!("expected function item");
    };
    assert!(matches!(
        &function.params[0].ty,
        Ty::RawPointer { mutable: false, .. }
    ));
    assert!(matches!(
        &function.params[1].ty,
        Ty::RawPointer { mutable: true, .. }
    ));
}

#[test]
fn parser_rejects_a_raw_pointer_without_const_or_mut() {
    let mut sources = SourceMap::new();
    let file = sources.add_file("main.paco", "fn f(a: *i64) { }");
    let mut reporter = Reporter::new();
    let tokens = lex(sources.source(file).unwrap(), file, &mut reporter);

    let result = parse_module(&tokens, &mut reporter);

    assert!(result.is_err());
    assert!(reporter.has_errors());
}

#[test]
fn parser_parses_prefix_deref_and_keeps_multiplication_working() {
    let module = parse_source("fn f(p: *const i64, a: i64, b: i64) -> i64 { *p + a * b }");

    let Item::Fn(function) = &module.items[0] else {
        panic!("expected function item");
    };
    let Some(Expr::Binary { left, right, .. }) = function.body.tail.as_deref() else {
        panic!("expected addition at tail");
    };
    assert!(matches!(
        left.as_ref(),
        Expr::Unary {
            op: paco_syntax::ast::UnaryOp::Deref,
            ..
        }
    ));
    assert!(matches!(
        right.as_ref(),
        Expr::Binary {
            op: BinaryOp::Mul,
            ..
        }
    ));
}

#[test]
fn parser_parses_unsafe_block_as_let_initializer_and_tail_expr() {
    let module = parse_source("fn f(p: *const i64) -> i64 { let x = unsafe { *p }; unsafe { x } }");

    let Item::Fn(function) = &module.items[0] else {
        panic!("expected function item");
    };
    assert!(matches!(
        &function.body.stmts[0],
        paco_syntax::ast::Stmt::Let(let_stmt) if matches!(let_stmt.value.as_ref(), Some(Expr::Unsafe(_, _)))
    ));
    assert!(matches!(
        function.body.tail.as_deref(),
        Some(Expr::Unsafe(_, _))
    ));
}

#[test]
fn parser_parses_the_four_unsafe_extern_modifier_combinations() {
    let module = parse_source(
        "fn a() { } unsafe fn b() { } extern \"C\" fn c() { } unsafe extern \"C\" fn d() { }",
    );

    let Item::Fn(a) = &module.items[0] else {
        panic!("expected fn a");
    };
    assert!(!a.is_unsafe && a.extern_abi.is_none());

    let Item::Fn(b) = &module.items[1] else {
        panic!("expected fn b");
    };
    assert!(b.is_unsafe && b.extern_abi.is_none());

    let Item::Fn(c) = &module.items[2] else {
        panic!("expected fn c");
    };
    assert!(!c.is_unsafe && c.extern_abi.as_deref() == Some("C"));

    let Item::Fn(d) = &module.items[3] else {
        panic!("expected fn d");
    };
    assert!(d.is_unsafe && d.extern_abi.as_deref() == Some("C"));
}

#[test]
fn parser_parses_named_arguments_as_plain_positional_args() {
    let module = parse_source("fn main() { f(capacity: 8, 2) }");
    let Item::Fn(function) = &module.items[0] else {
        panic!("expected fn item");
    };
    let Some(Expr::Call { args, .. }) = function.body.tail.as_deref() else {
        panic!("expected a call tail expression");
    };
    assert!(matches!(&args[0], Expr::Literal(Literal::Int(8), _)));
    assert!(matches!(&args[1], Expr::Literal(Literal::Int(2), _)));
}

#[test]
fn parser_accepts_every_fixed_width_numeric_type_name_and_round_trips_through_fmt() {
    for name in ["i8", "i16", "i32", "i64", "u8", "u16", "u32", "u64", "char", "byte"] {
        let source = format!("fn f(x: {name}) -> {name} {{ x }}");
        let module = parse_source(&source);
        let Item::Fn(function) = &module.items[0] else {
            panic!("expected function item for `{name}`");
        };
        assert!(
            matches!(&function.params[0].ty, Ty::Path(path, _) if path == &[name.to_string()]),
            "expected param type `{name}`, found {:?}",
            function.params[0].ty
        );

        let formatted = paco_syntax::fmt::format_module(&module, Some(&source));
        assert!(
            formatted.contains(&format!("x: {name}")),
            "formatted output for `{name}` did not round-trip: {formatted}"
        );
    }
}

#[test]
fn parser_parses_a_char_literal_into_expr_literal_char() {
    let module = parse_source("fn main() { let c = 'a'; }");
    let Item::Fn(function) = &module.items[0] else {
        panic!("expected fn item");
    };
    let Some(Stmt::Let(let_stmt)) = function.body.stmts.first() else {
        panic!("expected a let statement");
    };
    let value = let_stmt.value.as_ref().expect("expected an initializer");
    assert!(matches!(value, Expr::Literal(Literal::Char('a'), _)));
}

#[test]
fn parser_parses_a_byte_typed_let_as_a_plain_integer_literal() {
    let module = parse_source("fn main() { let b: byte = 65; }");
    let Item::Fn(function) = &module.items[0] else {
        panic!("expected fn item");
    };
    let Some(Stmt::Let(let_stmt)) = function.body.stmts.first() else {
        panic!("expected a let statement");
    };
    assert!(matches!(&let_stmt.ty, Some(Ty::Path(path, _)) if path == &["byte".to_string()]));
    let value = let_stmt.value.as_ref().expect("expected an initializer");
    assert!(matches!(value, Expr::Literal(Literal::Int(65), _)));
}

fn parse_source(source: &str) -> paco_syntax::ast::Module {
    let mut sources = SourceMap::new();
    let file = sources.add_file("main.paco", source);
    let mut reporter = Reporter::new();
    let tokens = lex(sources.source(file).unwrap(), file, &mut reporter);
    let module = parse_module(&tokens, &mut reporter).unwrap();
    assert!(
        !reporter.has_errors(),
        "{}",
        reporter.emit_to_string(&sources)
    );
    module
}

#[test]
fn parser_parses_as_cast_with_correct_precedence() {
    let module = parse_source("fn main() { let x = 300 as u8; }");
    let Item::Fn(function) = &module.items[0] else {
        panic!("expected fn item");
    };
    let Some(Stmt::Let(let_stmt)) = function.body.stmts.first() else {
        panic!("expected a let statement");
    };
    let value = let_stmt.value.as_ref().expect("expected an initializer");
    let Expr::Cast { expr, ty, .. } = value else {
        panic!("expected a cast expression, found {value:?}");
    };
    assert!(matches!(expr.as_ref(), Expr::Literal(Literal::Int(300), _)));
    assert!(matches!(ty, Ty::Path(path, _) if path == &["u8".to_string()]));
}

#[test]
fn parser_attaches_a_derive_attribute_to_a_struct() {
    let module = parse_source("#[derive(Display)]\nstruct Point { x: i64, y: i64 }");
    let Item::Struct(decl) = &module.items[0] else {
        panic!("expected struct item");
    };
    assert_eq!(decl.attrs.len(), 1);
    assert_eq!(decl.attrs[0].name, "derive");
    let paco_syntax::ast::AttributeArg::Path(path, _) = &decl.attrs[0].args[0] else {
        panic!("expected a path attribute arg");
    };
    assert_eq!(path, &["Display".to_string()]);
}

#[test]
fn parser_attaches_an_attribute_to_a_function() {
    let module = parse_source("#[test]\nfn check() { 1 }");
    let Item::Fn(function) = &module.items[0] else {
        panic!("expected fn item");
    };
    assert_eq!(function.attrs.len(), 1);
    assert_eq!(function.attrs[0].name, "test");
    assert!(function.attrs[0].args.is_empty());
}

#[test]
fn parser_attaches_an_attribute_to_a_struct_field() {
    let module = parse_source("struct Point { #[serde(\"X\")] x: i64, y: i64 }");
    let Item::Struct(decl) = &module.items[0] else {
        panic!("expected struct item");
    };
    assert_eq!(decl.fields[0].attrs.len(), 1);
    assert_eq!(decl.fields[0].attrs[0].name, "serde");
    let paco_syntax::ast::AttributeArg::Literal(Literal::String(value), _) =
        &decl.fields[0].attrs[0].args[0]
    else {
        panic!("expected a string literal attribute arg");
    };
    assert_eq!(value, "X");
    assert!(decl.fields[1].attrs.is_empty());
}

#[test]
fn parser_attaches_an_attribute_to_an_enum_variant() {
    let module = parse_source("enum Status { #[default] Idle, Running }");
    let Item::Enum(decl) = &module.items[0] else {
        panic!("expected enum item");
    };
    assert_eq!(decl.variants[0].attrs.len(), 1);
    assert_eq!(decl.variants[0].attrs[0].name, "default");
    assert!(decl.variants[1].attrs.is_empty());
}

#[test]
fn parser_stacks_multiple_attributes_on_one_item() {
    let module = parse_source("#[derive(Display)]\n#[test]\nstruct Marker {}");
    let Item::Struct(decl) = &module.items[0] else {
        panic!("expected struct item");
    };
    assert_eq!(decl.attrs.len(), 2);
    assert_eq!(decl.attrs[0].name, "derive");
    assert_eq!(decl.attrs[1].name, "test");
}

#[test]
fn parser_parses_a_comptime_block_expression() {
    let module = parse_source("fn main() { comptime { 1 + 2 } }");
    let Item::Fn(function) = &module.items[0] else {
        panic!("expected fn item");
    };
    let tail = function.body.tail.as_ref().expect("expected a tail expression");
    let Expr::Comptime { expr, .. } = tail.as_ref() else {
        panic!("expected a comptime expression, found {tail:?}");
    };
    assert!(matches!(expr.as_ref(), Expr::Block(_)));
}

#[test]
fn parser_parses_a_comptime_fn_modifier() {
    let module = parse_source("comptime fn f() -> i64 { 1 }");
    let Item::Fn(function) = &module.items[0] else {
        panic!("expected fn item");
    };
    assert!(function.is_comptime);
    assert!(!function.is_iter);
    assert!(!function.is_unsafe);
}

#[test]
fn parser_rejects_comptime_and_iter_combined() {
    let mut sources = SourceMap::new();
    let file = sources.add_file("main.paco", "comptime iter fn f() {}");
    let mut reporter = Reporter::new();
    let tokens = lex(sources.source(file).unwrap(), file, &mut reporter);

    parse_module(&tokens, &mut reporter).ok();

    assert!(reporter.has_errors());
}

#[test]
fn parser_parses_a_quote_template_with_all_three_splice_positions() {
    let module = parse_source(
        "comptime fn f(t: type) -> i64 { quote { methods #(t) { fn g(&self) -> string { self.#(name).to_string() } } }; 0 }",
    );
    let Item::Fn(function) = &module.items[0] else {
        panic!("expected fn item");
    };
    assert!(function.is_comptime);
    let Stmt::Expr(quote_stmt) = &function.body.stmts[0] else {
        panic!("expected an expression statement");
    };
    let Expr::Quote(body, _) = quote_stmt else {
        panic!("expected a quote expression, found {quote_stmt:?}");
    };
    let paco_syntax::ast::QuoteBody::Item(Item::Methods(block)) = body.as_ref() else {
        panic!("expected a methods-block quote body, found {body:?}");
    };
    // Type-position splice: the methods block's own target type.
    assert!(matches!(&block.target, Ty::Splice(_, _)));
    let method = &block.methods[0];
    let Some(tail) = &method.body.tail else {
        panic!("expected a tail expression in the generated method body");
    };
    // Identifier-position splice desugars to a `splice_field` call.
    let Expr::MethodCall { receiver, method: method_name, .. } = tail.as_ref() else {
        panic!("expected a method call, found {tail:?}");
    };
    assert_eq!(method_name, "to_string");
    let Expr::Call { callee, args, .. } = receiver.as_ref() else {
        panic!("expected the splice_field desugaring, found {receiver:?}");
    };
    assert!(matches!(callee.as_ref(), Expr::Ident(name, _) if name == "splice_field"));
    assert_eq!(args.len(), 2);
}

#[test]
fn parser_parses_a_quote_expression_body_with_a_splice() {
    let module = parse_source("comptime fn f() -> i64 { quote { #(1) }; 0 }");
    let Item::Fn(function) = &module.items[0] else {
        panic!("expected fn item");
    };
    let Stmt::Expr(quote_stmt) = &function.body.stmts[0] else {
        panic!("expected an expression statement");
    };
    let Expr::Quote(body, _) = quote_stmt else {
        panic!("expected a quote expression, found {quote_stmt:?}");
    };
    let paco_syntax::ast::QuoteBody::Expr(inner) = body.as_ref() else {
        panic!("expected an expression quote body, found {body:?}");
    };
    assert!(matches!(inner, Expr::Splice(_, _)));
}

#[test]
fn parser_distinguishes_tuples_unit_and_grouping_by_element_count() {
    let tail = |source: &str| {
        let module = parse_source(source);
        let Item::Fn(function) = &module.items[0] else {
            panic!("expected function item");
        };
        function.body.tail.as_deref().unwrap().clone()
    };
    let Expr::Tuple(items, _) = tail("fn main() { (1, (2, 3),) }") else {
        panic!("expected a tuple");
    };
    assert_eq!(items.len(), 2);
    assert!(matches!(&items[1], Expr::Tuple(inner, _) if inner.len() == 2));
    assert!(matches!(tail("fn main() { () }"), Expr::Tuple(items, _) if items.is_empty()));
    assert!(matches!(tail("fn main() { (1 + 2) }"), Expr::Binary { .. }));
}

#[test]
fn parser_parses_struct_patterns_with_shorthand_nesting_and_rest() {
    let module = parse_source(
        "fn main() -> i64 { match value { Point { x: 0, y } => y, Line { from: Point { x, .. }, .. } => x, _ => 0 } }",
    );
    let Item::Fn(function) = &module.items[0] else {
        panic!("expected function item");
    };
    let Some(tail) = &function.body.tail else {
        panic!("expected tail expression");
    };
    let Expr::Match { arms, .. } = tail.as_ref() else {
        panic!("expected match expression");
    };

    let Pat::Struct { path, fields, rest, .. } = &arms[0].pattern else {
        panic!("expected struct pattern");
    };
    assert_eq!(path, &vec!["Point".to_string()]);
    assert!(!rest);
    assert!(matches!(&fields[0], (name, Pat::Literal(Literal::Int(0), _)) if name == "x"));
    assert!(matches!(&fields[1], (name, Pat::Ident(binding, _)) if name == "y" && binding == "y"));

    let Pat::Struct { fields, rest: true, .. } = &arms[1].pattern else {
        panic!("expected struct pattern with rest");
    };
    assert!(matches!(&fields[0].1, Pat::Struct { rest: true, .. }));
}

fn parse_with_reporter(source: &str) -> (Option<paco_syntax::ast::Module>, Reporter) {
    let mut sources = SourceMap::new();
    let file = sources.add_file("main.paco", source);
    let mut reporter = Reporter::new();
    let tokens = lex(sources.source(file).unwrap(), file, &mut reporter);
    let module = parse_module(&tokens, &mut reporter).ok();
    (module, reporter)
}

#[test]
fn a_missing_semicolon_is_reported_at_the_end_of_the_previous_statement() {
    let source = "fn main() {\n    let r = &mut y\n    print(r);\n}\n";
    let (_, reporter) = parse_with_reporter(source);
    let diagnostic = &reporter.diagnostics()[0];
    assert_eq!(diagnostic.code(), "PACO-E0115");
    let end = source.find("&mut y").unwrap() + "&mut y".len();
    assert_eq!((diagnostic.primary().span.start(), diagnostic.primary().span.end()), (end, end));
    let suggestion = diagnostic.suggestion().expect("a `;` suggestion");
    assert_eq!(suggestion.replacement, ";");
    assert_eq!(suggestion.span.start(), end);
}

#[test]
fn a_semicolon_separates_a_mut_borrow_from_a_deref_assignment() {
    let (module, reporter) = parse_with_reporter("fn main() { let r = &mut y; *r = 5; }");
    assert!(!reporter.has_errors());
    let Item::Fn(function) = &module.unwrap().items[0] else { panic!("expected fn") };
    assert_eq!(function.body.stmts.len(), 2);
    assert!(matches!(
        &function.body.stmts[1],
        Stmt::Expr(Expr::Assign { target, .. }) if matches!(target.as_ref(), Expr::Unary { .. })
    ));
}

#[test]
fn block_like_statements_need_no_semicolon_and_end_the_statement() {
    let source = "fn main() -> i64 {\n    if c { 1 } else { 2 }\n    -1\n}\n";
    let (module, reporter) = parse_with_reporter(source);
    assert!(!reporter.has_errors());
    let Item::Fn(function) = &module.unwrap().items[0] else { panic!("expected fn") };
    assert!(matches!(&function.body.stmts[0], Stmt::Expr(Expr::If { .. })));
    assert!(matches!(function.body.tail.as_deref(), Some(Expr::Unary { .. })));
}

#[test]
fn a_block_like_statement_may_continue_with_a_method_call() {
    let (module, reporter) = parse_with_reporter("fn main() { match x { _ => y }.unwrap(); }");
    assert!(!reporter.has_errors());
    let Item::Fn(function) = &module.unwrap().items[0] else { panic!("expected fn") };
    assert!(matches!(&function.body.stmts[0], Stmt::Expr(Expr::MethodCall { .. })));
}

#[test]
fn items_that_are_not_blocks_require_a_semicolon() {
    for source in ["module m\nfn f() {}", "use stdlib::io\nfn f() {}", "const A: i64 = 1\nfn f() {}"] {
        let (_, reporter) = parse_with_reporter(source);
        assert_eq!(reporter.diagnostics()[0].code(), "PACO-E0115", "{source}");
    }
}

#[test]
fn a_non_block_match_arm_needs_a_comma_before_the_next_arm() {
    let (_, reporter) = parse_with_reporter("fn f() { match x { 1 => a 2 => b } }");
    assert_eq!(reporter.diagnostics()[0].code(), "PACO-E0112");
    let (_, reporter) = parse_with_reporter("fn f() { match x { 1 => { a } 2 => b } }");
    assert!(!reporter.has_errors());
}

fn shape(expr: &Expr) -> String {
    match expr {
        Expr::Binary { op, left, right, .. } => format!("({} {op:?} {})", shape(left), shape(right)),
        Expr::Unary { op, expr, .. } => format!("({op:?} {})", shape(expr)),
        Expr::Borrow { expr, .. } => format!("(& {})", shape(expr)),
        Expr::Literal(Literal::Int(value), _) => value.to_string(),
        Expr::Ident(name, _) => name.clone(),
        other => format!("{other:?}"),
    }
}

fn tail_shape(body: &str) -> String {
    let module = parse_source(&format!("fn main() {{ {body} }}"));
    let Item::Fn(function) = &module.items[0] else { panic!("expected function item") };
    shape(function.body.tail.as_ref().expect("tail expression"))
}

#[test]
fn bitwise_operators_bind_between_comparison_and_addition() {
    assert_eq!(tail_shape("x & 1 == 0"), "((x BitAnd 1) Eq 0)");
    assert_eq!(tail_shape("1 | 2 ^ 3 & 4 << 1"), "(1 BitOr (2 BitXor (3 BitAnd (4 Shl 1))))");
    assert_eq!(tail_shape("a >> 1 + 2"), "(a Shr (1 Add 2))");
    assert_eq!(tail_shape("~a & b"), "((BitNot a) BitAnd b)");
    assert_eq!(tail_shape("a | b || c && d ^ e"), "((a BitOr b) Or (c And (d BitXor e)))");
    assert_eq!(tail_shape("&a & b"), "((& a) BitAnd b)");
    assert_eq!(tail_shape("a > b"), "(a Gt b)");
    assert_eq!(tail_shape("a < b"), "(a Lt b)");
}

#[test]
fn nested_generic_arguments_still_close_with_adjacent_greater_thans() {
    let module = parse_source("fn main() { let v: Vec<Vec<i64>> = Vec::new(); let (tx, rx) = channel<Vec<i64>>(capacity: 1); let s = v.len() >> 1; }");
    let Item::Fn(function) = &module.items[0] else { panic!("expected function item") };
    assert_eq!(function.body.stmts.len(), 3);
}
