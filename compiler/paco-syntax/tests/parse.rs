use paco_diag::Reporter;
use paco_span::SourceMap;
use paco_syntax::{
    ast::{BinaryOp, Expr, Item, Literal},
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
