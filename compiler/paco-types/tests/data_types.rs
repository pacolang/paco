use paco_diag::Reporter;
use paco_span::SourceMap;
use paco_syntax::{lex::lex, parse::parse_module};
use paco_types::check_module;

#[test]
fn type_checker_accepts_struct_literal_field_access() {
    let error = check_source(
        "struct Point { x: int, y: int } fn main() { let p = Point { x: 1, y: 2 }; print(p.x) }",
    );

    assert!(error.is_none(), "{error:?}");
}

#[test]
fn type_checker_rejects_wrong_struct_field_type() {
    let error = check_source(
        "struct Point { x: int, y: int } fn main() { let p = Point { x: true, y: 2 } }",
    )
    .expect("expected type error");

    assert!(error.contains("PACO-E0302"));
    assert!(error.contains("type mismatch"));
}

#[test]
fn type_checker_rejects_missing_and_unknown_struct_fields() {
    let missing =
        check_source("struct Point { x: int, y: int } fn main() { let p = Point { x: 1 } }")
            .expect("expected missing field error");
    let unknown =
        check_source("struct Point { x: int } fn main() { let p = Point { x: 1, y: 2 } }")
            .expect("expected unknown field error");
    let duplicate =
        check_source("struct Point { x: int } fn main() { let p = Point { x: 1, x: 2 } }")
            .expect("expected duplicate field error");

    assert!(missing.contains("PACO-E0311"));
    assert!(missing.contains("missing field"));
    assert!(unknown.contains("PACO-E0311"));
    assert!(unknown.contains("unknown field"));
    assert!(duplicate.contains("PACO-E0311"));
    assert!(duplicate.contains("duplicate field"));
}

#[test]
fn type_checker_accepts_method_and_associated_function_calls() {
    let source = r#"
struct Point {
    x: int,
    y: int,

    fn origin() -> Point { Point { x: 0, y: 0 } }
    fn sum(self&) -> int { self.x + self.y }
}

fn main() {
    let p = Point::origin()
    print(p.sum())
}
"#;

    let error = check_source(source);

    assert!(error.is_none(), "{error:?}");
}

#[test]
fn type_checker_accepts_enum_variant_construction() {
    let error =
        check_source("enum Maybe { Some(int), None } fn main() { let x: Maybe = Maybe::Some(1) }");

    assert!(error.is_none(), "{error:?}");
}

#[test]
fn type_checker_instantiates_generic_struct_field() {
    let error = check_source(
        "struct Box<T> { value: T } fn main() { let b = Box<int> { value: 1 }; print(b.value) }",
    );

    assert!(error.is_none(), "{error:?}");
}

#[test]
fn type_checker_instantiates_generic_function_calls() {
    let valid = check_source("fn id<T>(value: T) -> T { value } fn main() { let x: int = id(1) }");
    let invalid =
        check_source("fn id<T>(value: T) -> T { value } fn main() { let x: int = id(true) }")
            .expect("expected generic function instantiation error");
    let invalid_body =
        check_source("fn bad<T>(value: T) -> T { 1 } fn main() { let ignored = bad(true); }")
            .expect("expected generic function body error");

    assert!(valid.is_none(), "{valid:?}");
    assert!(invalid.contains("PACO-E0302"));
    assert!(invalid.contains("type mismatch"));
    assert!(invalid_body.contains("PACO-E0302"));
    assert!(invalid_body.contains("return type mismatch"));
}

#[test]
fn type_checker_rejects_generic_arity_mismatches_in_declarations() {
    let missing = check_source("struct Box<T> { value: T } fn bad(value: Box) {} fn main() {}")
        .expect("expected missing generic argument error");
    let extra =
        check_source("struct Box<T> { value: T } fn bad(value: Box<int, bool>) {} fn main() {}")
            .expect("expected extra generic argument error");
    let field =
        check_source("struct Box<T> { value: T } struct Holder { value: Box } fn main() {}")
            .expect("expected field generic argument error");

    assert!(missing.contains("PACO-E0316"));
    assert!(missing.contains("generic arity mismatch"));
    assert!(extra.contains("PACO-E0316"));
    assert!(extra.contains("generic arity mismatch"));
    assert!(field.contains("PACO-E0316"));
    assert!(field.contains("generic arity mismatch"));
}

#[test]
fn type_checker_rejects_unknown_methods_and_associated_calls() {
    let method =
        check_source("struct Point { x: int } fn main() { let p = Point { x: 1 }; p.y() }")
            .expect("expected unknown method error");
    let associated = check_source("struct Point { x: int } fn main() { let p = Point::origin() }")
        .expect("expected unknown associated function error");

    assert!(method.contains("PACO-E0314"));
    assert!(method.contains("method `y` not found"));
    assert!(associated.contains("PACO-E0314"));
    assert!(associated.contains("associated function `origin` not found"));
}

#[test]
fn type_checker_rejects_immutable_field_assignment() {
    let error =
        check_source("struct Point { x: int } fn main() { let p = Point { x: 1 }; p.x = 2 }")
            .expect("expected immutable field assignment error");

    assert!(error.contains("PACO-E0307"));
    assert!(error.contains("immutable binding"));
}

#[test]
fn type_checker_rejects_indirect_recursive_value_layout() {
    let error = check_source("struct A { b: B } struct B { a: A } fn main() {}")
        .expect("expected recursive value layout error");

    assert!(error.contains("PACO-E0315"));
    assert!(error.contains("recursive by-value field"));
}

#[test]
fn type_checker_rejects_generic_mediated_recursive_value_layout() {
    let error =
        check_source("struct Box<T> { value: T } struct Node { child: Box<Node> } fn main() {}")
            .expect("expected recursive value layout error");

    assert!(error.contains("PACO-E0315"));
    assert!(error.contains("recursive by-value field"));
}

fn check_source(source: &str) -> Option<String> {
    let mut sources = SourceMap::new();
    let file = sources.add_file("main.paco", source);
    let mut reporter = Reporter::new();
    let tokens = lex(sources.source(file).unwrap(), file, &mut reporter);
    let module = match parse_module(&tokens, &mut reporter) {
        Ok(module) if !reporter.has_errors() => module,
        _ => return Some(reporter.emit_to_string(&sources)),
    };
    match check_module(&module, &mut reporter) {
        Ok(()) if !reporter.has_errors() => None,
        _ => Some(reporter.emit_to_string(&sources)),
    }
}
