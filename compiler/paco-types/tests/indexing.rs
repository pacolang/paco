use paco_diag::Reporter;
use paco_span::SourceMap;
use paco_syntax::{lex::lex, parse::parse_module};
use paco_types::{Type, check_module, infer_module};

fn check_source(source: &str) -> Option<String> {
    let mut sources = SourceMap::new();
    let file = sources.add_file("main.paco", source);
    let mut reporter = Reporter::new();
    let tokens = lex(sources.source(file).unwrap(), file, &mut reporter);
    let module = parse_module(&tokens, &mut reporter).expect("source should parse");

    if check_module(&module, &mut reporter).is_err() {
        Some(reporter.emit_to_string(&sources))
    } else {
        None
    }
}

#[test]
fn indexing_a_slice_with_an_integer_yields_its_element_type() {
    let mut sources = SourceMap::new();
    let file = sources.add_file(
        "main.paco",
        "fn first(weights: &[]float) -> float { let x: float = weights[0]; x }",
    );
    let mut reporter = Reporter::new();
    let tokens = lex(sources.source(file).unwrap(), file, &mut reporter);
    let module = parse_module(&tokens, &mut reporter).unwrap();
    let typed = match infer_module(&module, &mut reporter) {
        Ok(typed) => typed,
        Err(_) => panic!("module should type-check: {}", reporter.emit_to_string(&sources)),
    };

    let paco_syntax::ast::Item::Fn(function) = &module.items[0] else {
        panic!("expected fn item");
    };
    let paco_syntax::ast::Stmt::Let(let_stmt) = &function.body.stmts[0] else {
        panic!("expected let statement");
    };
    let index_expr = let_stmt.value.as_ref().unwrap();
    assert_eq!(typed.type_of(index_expr), Some(&Type::Float(paco_types::FloatWidth::F64)));
}

#[test]
fn two_dimensional_indexing_dispatches_to_a_structurally_matching_index_method() {
    let error = check_source(
        r#"
        struct Matrix {
            rows: i64,
            data: []float,

            fn index(&self, i: (i64, i64)) -> &float {
                let (row, col) = i;
                &self.data[row * self.rows + col]
            }
        }

        fn main() {
            let m = Matrix { rows: 2, data: slice_of_zeros<float>(4) };
            let x: float = m[0, 1];
        }
        "#,
    );
    assert_eq!(error, None);
}

#[test]
fn the_placeholder_slice_of_zeros_builtin_constructs_a_slice_of_the_requested_type() {
    let error = check_source("fn main() { let buf: []i64 = slice_of_zeros<i64>(4); }");
    assert_eq!(error, None);
}

#[test]
fn a_missing_index_method_is_a_dedicated_diagnostic_not_a_generic_method_not_found() {
    let error = check_source(
        r#"
        struct Point { x: i64, y: i64 }

        fn main() {
            let p = Point { x: 1, y: 2 };
            let v = p[0];
        }
        "#,
    )
    .expect("expected an error");
    assert!(error.contains("PACO-E0332"), "expected PACO-E0332, got: {error}");
    assert!(!error.contains("PACO-E0314"), "should not fall back to the generic method-not-found code");
}
