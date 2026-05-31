use paco_diag::Reporter;
use paco_span::SourceMap;
use paco_syntax::{lex::lex, parse::parse_module};
use paco_types::check_module;

#[test]
fn type_checker_accepts_exhaustive_enum_match() {
    let error = check_source(
        r#"
enum Maybe { Some(int), None }

fn main() {
    let value: Maybe = Maybe::Some(1)
    let result: int = match value {
        Maybe::Some(x) => x,
        Maybe::None => 0,
    }
}
"#,
    );

    assert!(error.is_none(), "{error:?}");
}

#[test]
fn type_checker_rejects_non_exhaustive_enum_match_with_witness() {
    let error = check_source(
        r#"
enum Shape { Circle, Rectangle, Triangle }

fn main() {
    let shape: Shape = Shape::Circle()
    let sides: int = match shape {
        Shape::Circle => 0,
        Shape::Rectangle => 4,
    }
}
"#,
    )
    .expect("expected non-exhaustive match error");

    assert!(error.contains("PACO-E0401"));
    assert!(error.contains("non-exhaustive match"));
    assert!(error.contains("Shape::Triangle"));
}

#[test]
fn type_checker_rejects_unreachable_arm_after_wildcard() {
    let error = check_source(
        r#"
fn main() {
    let value: int = match 1 {
        _ => 0,
        1 => 1,
    }
}
"#,
    )
    .expect("expected unreachable arm error");

    assert!(error.contains("PACO-E0402"));
    assert!(error.contains("unreachable match arm"));
    assert!(error.contains("_"));
}

#[test]
fn type_checker_rejects_literal_arm_covered_by_range() {
    let error = check_source(
        r#"
fn main() {
    let value: int = match 3 {
        1..=9 => 1,
        3 => 2,
        _ => 0,
    }
}
"#,
    )
    .expect("expected unreachable range-covered arm error");

    assert!(error.contains("PACO-E0402"));
    assert!(error.contains("unreachable match arm"));
    assert!(error.contains("3"));
}

#[test]
fn type_checker_rejects_dynamic_range_pattern_bounds() {
    let error = check_source(
        r#"
fn main() {
    let value: int = match 3 {
        lo..=hi => 1,
        _ => 0,
    }
}
"#,
    )
    .expect("expected dynamic range pattern bound error");

    assert!(error.contains("PACO-E0306"));
    assert!(error.contains("range pattern bounds"));
}

#[test]
fn type_checker_rejects_wildcard_after_exhaustive_bool_match() {
    let error = check_source(
        r#"
fn main() {
    let value: int = match true {
        true => 1,
        false => 0,
        _ => 2,
    }
}
"#,
    )
    .expect("expected unreachable wildcard error");

    assert!(error.contains("PACO-E0402"));
    assert!(error.contains("unreachable match arm"));
    assert!(error.contains("all constructors"));
}

#[test]
fn type_checker_rejects_wildcard_after_exhaustive_enum_match() {
    let error = check_source(
        r#"
enum Maybe { Some(int), None }

fn main() {
    let value: Maybe = Maybe::Some(1)
    let result: int = match value {
        Maybe::Some(x) => x,
        Maybe::None => 0,
        _ => 1,
    }
}
"#,
    )
    .expect("expected unreachable wildcard error");

    assert!(error.contains("PACO-E0402"));
    assert!(error.contains("unreachable match arm"));
    assert!(error.contains("all constructors"));
}

#[test]
fn type_checker_requires_bool_match_guards() {
    let error = check_source(
        r#"
enum Maybe { Some(int), None }

fn main() {
    let value: Maybe = Maybe::Some(1)
    let result: int = match value {
        Maybe::Some(x) if x => x,
        Maybe::None => 0,
        _ => 0,
    }
}
"#,
    )
    .expect("expected guard type error");

    assert!(error.contains("PACO-E0303"));
    assert!(error.contains("guard"));
}

#[test]
fn type_checker_treats_guarded_arms_as_non_exhaustive() {
    let error = check_source(
        r#"
enum Maybe { Some(int), None }

fn main() {
    let value: Maybe = Maybe::Some(1)
    let result: int = match value {
        Maybe::Some(x) if x > 0 => x,
        Maybe::None => 0,
    }
}
"#,
    )
    .expect("expected guarded non-exhaustive match error");

    assert!(error.contains("PACO-E0401"));
    assert!(error.contains("Maybe::Some"));
}

#[test]
fn type_checker_treats_payload_literal_patterns_as_non_exhaustive() {
    let error = check_source(
        r#"
enum Maybe { Some(int), None }

fn main() {
    let value: Maybe = Maybe::Some(1)
    let result: int = match value {
        Maybe::Some(0) => 0,
        Maybe::None => 0,
    }
}
"#,
    )
    .expect("expected payload-specific non-exhaustive match error");

    assert!(error.contains("PACO-E0401"));
    assert!(error.contains("Maybe::Some"));
}

#[test]
fn type_checker_rejects_or_pattern_with_inconsistent_bindings() {
    let error = check_source(
        r#"
enum Maybe { Some(int), None }

fn main() {
    let value: Maybe = Maybe::Some(1)
    let result: int = match value {
        Maybe::Some(x) | Maybe::None => x,
    }
}
"#,
    )
    .expect("expected inconsistent or-pattern binding error");

    assert!(error.contains("PACO-E0306"));
    assert!(error.contains("or-pattern alternatives"));
}

#[test]
fn type_checker_uses_pattern_bound_variable_types() {
    let error = check_source(
        r#"
enum Maybe { Some(int), None }

fn main() {
    let value: Maybe = Maybe::Some(1)
    let result: int = match value {
        Maybe::Some(x) => x + 1,
        Maybe::None => 0,
    }
}
"#,
    );

    assert!(error.is_none(), "{error:?}");
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
