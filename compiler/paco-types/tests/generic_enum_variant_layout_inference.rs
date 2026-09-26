use paco_diag::Reporter;
use paco_span::SourceMap;
use paco_syntax::ast::{Item, Stmt};
use paco_syntax::{lex::lex, parse::parse_module};
use paco_types::{IntWidth, Type, infer_module};

const OPTION_DECL: &str = "enum Option<T> { Some(T), None }\n";

fn typed_module(source: &str) -> (&'static paco_syntax::ast::Module, paco_types::TypedModule<'static>) {
    let mut sources = SourceMap::new();
    let file = sources.add_file("main.paco", format!("{OPTION_DECL}{source}"));
    let mut reporter = Reporter::new();
    let tokens = lex(sources.source(file).unwrap(), file, &mut reporter);
    let module = parse_module(&tokens, &mut reporter).unwrap();
    assert!(!reporter.has_errors(), "{}", reporter.emit_to_string(&sources));
    let module: &'static paco_syntax::ast::Module = Box::leak(Box::new(module));
    let typed = infer_module(module, &mut reporter).unwrap_or_else(|_| {
        panic!("module should type-check: {}", reporter.emit_to_string(&sources))
    });
    (module, typed)
}

fn option_i64() -> Type {
    Type::Enum("Option".to_string(), vec![Type::Int(IntWidth::I64)])
}

#[test]
fn a_let_annotated_none_caches_the_substituted_type() {
    let (module, typed) = typed_module("fn main() { let x: Option<i64> = Option::None; }");
    let Item::Fn(function) = &module.items[1] else {
        panic!("expected fn item");
    };
    let Stmt::Let(let_stmt) = &function.body.stmts[0] else {
        panic!("expected let statement");
    };
    let value = let_stmt.value.as_ref().unwrap();
    assert_eq!(typed.type_of(value), Some(&option_i64()));
}

#[test]
fn a_returned_none_caches_the_substituted_type() {
    let (module, typed) = typed_module(
        r#"
        fn maybe(flag: bool) -> Option<i64> {
            if flag {
                return Option::None
            }
            Option::Some(5)
        }
        "#,
    );
    let Item::Fn(function) = &module.items[1] else {
        panic!("expected fn item");
    };
    let Stmt::Expr(paco_syntax::ast::Expr::If { then_branch, .. }) = &function.body.stmts[0] else {
        panic!("expected if statement");
    };
    let Some(paco_syntax::ast::Expr::Return(value, _)) = then_branch.tail.as_deref() else {
        panic!("expected return tail expression");
    };
    let value = value.as_ref().unwrap();
    assert_eq!(typed.type_of(value), Some(&option_i64()));
}

#[test]
fn a_tail_expression_none_caches_the_substituted_type() {
    let (module, typed) = typed_module("fn maybe() -> Option<i64> { Option::None }");
    let Item::Fn(function) = &module.items[1] else {
        panic!("expected fn item");
    };
    let tail = function.body.tail.as_ref().unwrap();
    assert_eq!(typed.type_of(tail), Some(&option_i64()));
}
