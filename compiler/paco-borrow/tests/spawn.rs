use paco_borrow::check_module;
use paco_diag::Reporter;
use paco_span::SourceMap;
use paco_syntax::{lex::lex, parse::parse_module};

fn check_source(source: &str) -> Option<String> {
    let mut sources = SourceMap::new();
    let file = sources.add_file("main.paco", source);
    let mut reporter = Reporter::new();
    let tokens = lex(sources.source(file).unwrap(), file, &mut reporter);
    let module = parse_module(&tokens, &mut reporter).expect("source should parse");
    check_module(&module, &mut reporter).is_err().then(|| reporter.emit_to_string(&sources))
}

#[test]
fn spawning_a_call_whose_argument_only_borrows_a_local_does_not_move_it() {
    let source = r#"
    struct Counter {
        value: i64,

        fn clone(&self) -> Self { Counter { value: self.value } }
    }
    fn bump(c: Counter) { }
    fn main() {
        let c = Counter { value: 0 };
        spawn bump(c.clone());
        print(c.value);
    }
    "#;
    assert_eq!(check_source(source), None);
}

#[test]
fn spawning_a_call_passing_a_local_by_value_still_moves_it() {
    let source = r#"
    fn show(s: string) { print(s); }
    fn main() {
        let s = "x";
        spawn show(s);
        print(s);
    }
    "#;
    let error = check_source(source).expect("expected a use-after-move error");
    assert!(error.contains("use-after-move"), "{error}");
    assert!(error.contains("`s`"), "{error}");
}

#[test]
fn spawning_a_closure_still_moves_its_owned_captures() {
    let source = r#"
    fn main() {
        let s = "x";
        spawn (|| print(s))();
        print(s);
    }
    "#;
    let error = check_source(source).expect("expected a use-after-move error");
    assert!(error.contains("use-after-move"), "{error}");
    assert!(error.contains("`s`"), "{error}");
}

#[test]
fn spawning_a_local_closure_by_name_still_moves_its_captures() {
    let source = r#"
    fn main() {
        let s = "x";
        let shout = || print(s);
        spawn shout();
        print(s);
    }
    "#;
    let error = check_source(source).expect("expected a use-after-move error");
    assert!(error.contains("use-after-move"), "{error}");
    assert!(error.contains("`s`"), "{error}");
}
