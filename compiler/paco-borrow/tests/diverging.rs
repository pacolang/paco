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
fn a_branch_that_panics_does_not_disagree_about_moves() {
    let source = r#"
    fn consume(value: string) { }
    fn pick(flag: bool, message: string) {
        if flag {
            consume(message);
        } else {
            panic(message);
        }
    }
    "#;
    assert_eq!(check_source(source), None);
}

#[test]
fn a_value_moved_before_a_panic_is_still_moved_after_a_branch_that_does_not_panic() {
    let source = r#"
    fn consume(value: string) { }
    fn pick(flag: bool, message: string) {
        if flag {
            consume(message);
        }
        consume(message);
    }
    "#;
    assert!(check_source(source).is_some_and(|errors| errors.contains("use-after-move")));
}
