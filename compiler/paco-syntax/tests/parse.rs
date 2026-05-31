use paco_diag::Reporter;
use paco_span::SourceMap;
use paco_syntax::{
    ast::{BinaryOp, Expr, Item, Literal, Pat, Ty, VariantFields},
    lex::lex,
    parse::parse_module,
};

#[test]
fn parser_preserves_multiplication_precedence_over_addition() {
    let mut sources = SourceMap::new();
    let file = sources.add_file("main.paco", "fn main() -> int { 1 + 2 * 3 }");
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
    let module = parse_source("struct Point { x: int, y: int }");

    let Item::Struct(point) = &module.items[0] else {
        panic!("expected struct item");
    };
    assert_eq!(point.name, "Point");
    assert_eq!(point.fields.len(), 2);
    assert_eq!(point.fields[0].name, "x");
    assert_eq!(point.fields[1].name, "y");
}

#[test]
fn parser_parses_enum_with_tuple_and_unit_variants() {
    let module = parse_source("enum Maybe { Some(int), None }");

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
    let module = parse_source("struct Point { x: int, fn value(self&) -> int { self.x } }");
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
fn parser_parses_generic_struct_type_application() {
    let module = parse_source(
        "struct Box<T> { value: T } fn main() { let b: Box<int> = Box<int> { value: 1 }; b.value }",
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
    let module = parse_source("fn main() -> int { match value { 0 => 1, _ => 2 } }");
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
        "fn main() -> int { match value { Maybe::Some(x) if x > 0 => x, Maybe::None => 0 } }",
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
    let module = parse_source("fn main() -> int { match n { digit @ 1..=9 => digit, _ => 0 } }");
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
        parse_source("fn main() -> int { if let Maybe::Some(x) = value { x } else { 0 } }");
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
