use paco_diag::Reporter;
use paco_span::SourceMap;
use paco_syntax::{lex::lex, parse::parse_module};
use paco_types::check_module;

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
fn a_struct_implementing_add_makes_plus_type_check() {
    let error = check_source(
        r#"
        struct Point {
            x: i64,
            y: i64,

            fn add(&self, other: Self) -> Self {
                Point { x: self.x + other.x, y: self.y + other.y }
            }
        }

        fn main() {
            let a = Point { x: 1, y: 2 };
            let b = Point { x: 3, y: 4 };
            let c: Point = a + b;
        }
        "#,
    );
    assert_eq!(error, None);
}

#[test]
fn a_struct_without_add_reports_a_dedicated_diagnostic_not_a_numeric_mismatch() {
    let error = check_source(
        r#"
        struct Point { x: i64, y: i64 }

        fn main() {
            let a = Point { x: 1, y: 2 };
            let b = Point { x: 3, y: 4 };
            let c = a + b;
        }
        "#,
    )
    .expect("expected an error");
    assert!(error.contains("PACO-E0334"), "expected PACO-E0334, got: {error}");
    assert!(!error.contains("PACO-E0301"), "should not fall back to the generic numeric-mismatch code");
}

#[test]
fn numeric_arithmetic_type_checking_is_unchanged() {
    let error = check_source("fn main() { let x: i64 = 1 + 2; let y: float = 1.0 + 2.0; }");
    assert_eq!(error, None);
}

#[test]
fn a_struct_implementing_neg_makes_unary_minus_type_check() {
    let error = check_source(
        r#"
        struct Point {
            x: i64,
            y: i64,

            fn neg(&self) -> Self {
                Point { x: 0 - self.x, y: 0 - self.y }
            }
        }

        fn main() {
            let a = Point { x: 1, y: 2 };
            let b: Point = -a;
        }
        "#,
    );
    assert_eq!(error, None);
}

#[test]
fn a_generic_struct_implementing_add_resolves_the_concrete_type_argument() {
    let error = check_source(
        r#"
        struct Vector2<T> {
            x: T,
            y: T,

            fn add(&self, other: Self) -> Self {
                other
            }
        }

        fn main() {
            let a = Vector2<i64> { x: 1, y: 2 };
            let b = Vector2<i64> { x: 3, y: 4 };
            let c: Vector2<i64> = a + b;
        }
        "#,
    );
    assert_eq!(error, None);
}
