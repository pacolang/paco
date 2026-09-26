use paco_borrow::check_module;
use paco_diag::Reporter;
use paco_span::SourceMap;
use paco_syntax::{lex::lex, parse::parse_module};

fn parse_source(source: &str) -> paco_syntax::ast::Module {
    let mut sources = SourceMap::new();
    let file = sources.add_file("main.paco", source);
    let mut reporter = Reporter::new();
    let tokens = lex(sources.source(file).unwrap(), file, &mut reporter);
    let module = parse_module(&tokens, &mut reporter).unwrap();
    assert!(!reporter.has_errors());
    module
}

// Bug found implementing `phase-9-comptime` tasks 7.1/7.2: `fields_of`
// (Decision 6) is a builtin, never a declared `Item::Fn`, so
// `paco-borrow`'s own `Program` had no way to know its return type —
// `expr_ty` on `fields_of(t)` fell back to unknown, so the desugared
// `for field in fields_of(t) { .. }` loop's `.next()` call couldn't find
// `FieldIter::next`'s own `&mut self` receiver and defaulted to treating
// it as a move, spuriously failing every such loop with "loop body may
// move `$paco_for_iter_N` on more than one iteration".
#[test]
fn a_for_loop_over_fields_of_borrow_checks_cleanly() {
    let module = parse_source(
        r#"
enum Option<T> { Some(T), None }
struct FieldInfo { name: string, ty: type }
struct FieldIter {
    items: []FieldInfo,
    pos: i64,

    fn next(&mut self) -> Option<FieldInfo> {
        Option::None
    }
}

struct User { name: string, age: i64 }

fn describe(t: type) {
    comptime {
        for field in fields_of(t) {
            let name = field.name;
        }
    }
}

fn main() {
    comptime { describe(User) }
}
"#,
    );

    let mut reporter = Reporter::new();
    let result = check_module(&module, &mut reporter);
    assert!(result.is_ok(), "{:?}", reporter.diagnostics());
}

// Bug found implementing task 7.1: `ty_is_copy` didn't include `type`/
// `Code` (`phase-9-comptime` Decisions 5/7) — both are compile-time-only
// handles with no backing store, so using one more than once (e.g. a
// derive fn's own `t: type` parameter, read by `fields_of(t)` and again
// by `#(t)` in its own final `quote { .. }`) spuriously reported
// "use of moved value".
#[test]
fn a_type_typed_parameter_can_be_used_more_than_once() {
    let module = parse_source(
        r#"
struct User { name: string }

fn f(t: type) -> type {
    let a = t;
    let b = t;
    b
}

fn main() {
    comptime { f(User) }
}
"#,
    );

    let mut reporter = Reporter::new();
    let result = check_module(&module, &mut reporter);
    assert!(result.is_ok(), "{:?}", reporter.diagnostics());
}
