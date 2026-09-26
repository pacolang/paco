use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use paco_comptime::{Limits, Program, evaluate};
use paco_diag::Reporter;
use paco_mir::{Body, ComptimeValue, Constant, Profile, TypeLayouts, TypeRegistry};
use paco_span::SourceMap;
use paco_syntax::ast::Item;
use paco_syntax::{lex::lex, parse::parse_module};
use paco_types::{IntWidth, infer_module};

fn eval(source: &str) -> Result<ComptimeValue, String> {
    let mut sources = SourceMap::new();
    let file = sources.add_file("main.paco", source);
    let mut reporter = Reporter::new();
    let tokens = lex(sources.source(file).unwrap(), file, &mut reporter);
    let module = parse_module(&tokens, &mut reporter).unwrap();
    let typed = infer_module(&module, &mut reporter).unwrap_or_else(|_| panic!("{}", reporter.emit_to_string(&sources)));
    let drops = paco_borrow::analyze_module(&module, &mut reporter).unwrap();
    let registry = TypeRegistry::from_module(&module);
    let layouts = TypeLayouts::from_module(&module);
    let mut bodies: HashMap<String, Rc<Body>> = HashMap::new();
    for item in &module.items {
        if let Item::Fn(function) = item {
            let (body, outlined) = paco_mir::lower_function(function, &typed, &registry, &drops, Profile::Debug);
            bodies.insert(function.name.clone(), Rc::new(body));
            bodies.extend(outlined.into_iter().map(|(name, body)| (name, Rc::new(body))));
        }
    }
    let program = Program { layouts: &layouts, externs: HashSet::new(), structs: HashMap::new(), enums: HashSet::new() };
    evaluate(&program, &mut bodies, "run", &[], Limits::default()).map(|evaluation| evaluation.value).map_err(|error| error.message)
}

fn int(value: i64) -> ComptimeValue {
    ComptimeValue::Scalar(Constant::Int(value, IntWidth::I64))
}

#[test]
fn factorial_recurses_with_checked_arithmetic() {
    let source = "fn factorial(n: i64) -> i64 {\n    if n <= 1 { 1 } else { n * factorial(n - 1) }\n}\n\nfn run() -> i64 {\n    factorial(10)\n}\n";
    assert_eq!(eval(source), Ok(int(3628800)));
}

#[test]
fn a_mut_borrow_swap_writes_through_both_pointers() {
    let source = "fn swap(a: &mut i64, b: &mut i64) {\n    let t = *a;\n    *a = *b;\n    *b = t;\n}\n\nfn run() -> i64 {\n    let mut x: i64 = 1;\n    let mut y: i64 = 2;\n    swap(&mut x, &mut y);\n    x * 10 + y\n}\n";
    assert_eq!(eval(source), Ok(int(21)));
}

#[test]
fn a_struct_field_is_updated_through_a_mut_borrow() {
    let source = "struct Point {\n    x: i64,\n    label: string,\n}\n\nfn bump(p: &mut Point) {\n    p.x = p.x + 5;\n    p.label = \"moved\";\n}\n\nfn run() -> string {\n    let mut p = Point { x: 1, label: \"start\" };\n    bump(&mut p);\n    if p.x == 6 { p.label } else { \"wrong\" }\n}\n";
    assert_eq!(eval(source), Ok(ComptimeValue::Scalar(Constant::Str("moved".to_string()))));
}

#[test]
fn rc_strong_counts_follow_clones_and_drops() {
    let source = "struct Counter {\n    value: i64,\n}\n\nfn run() -> i64 {\n    let a = Rc::new(Counter { value: 5 });\n    let b = a.clone();\n    let mut total = b.strong_count() * 100;\n    {\n        let c = a.clone();\n        total = total + c.strong_count() * 10;\n    }\n    total + a.strong_count()\n}\n";
    assert_eq!(eval(source), Ok(int(232)));
}

#[test]
fn overflow_is_an_error_at_the_failing_operation() {
    let source = "fn run() -> i64 {\n    let big: i64 = 9223372036854775807;\n    big + 1\n}\n";
    assert_eq!(eval(source), Err("attempt to add with overflow".to_string()));
}
